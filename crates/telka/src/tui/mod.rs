//! The interactive TUI: `telka` without a command.
//!
//! [`model`] holds the state and decides what to do, [`view`] draws it. This module does the
//! I/O: terminal input, the clock, and the background work the model asks for: the connection
//! and the TV's changes, buttons, requests, waking, and pairing.

mod keymap;
mod model;
mod view;

use std::io::{self, IsTerminal};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Position;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use webos::{
    Button, Client, ForegroundApp, Inputs, Keyboard, LaunchPoints, Manifest, Picture, PowerState,
    SoundOutput, SoundSettings, Volume,
};

use crate::config::{self, Config};
use crate::connect::{self, Tv};
use crate::error::AppError;
use crate::output;
use crate::pairing;
use crate::paths::Paths;
use crate::store::Paired;
use model::{Change, Effect, Failure, Model, Msg, PairingMsg, Pointer, Snapshot, Start};
use view::{Hotspot, Target};

/// How often the clock and the countdowns are updated, and how often while a spinner turns or
/// something flashes.
const TICK: Duration = Duration::from_secs(1);
const SPINNER: Duration = Duration::from_millis(100);
/// How long, and how often, picture settings are read back after a change: the TV shows them
/// in its answers a moment later.
const READ_BACK: Duration = Duration::from_secs(3);
const READ_EVERY: Duration = Duration::from_millis(300);

/// What reaches the event loop.
enum Incoming {
    Msg(Box<Msg>),
    Mouse(MouseEvent),
    /// A new connection, for the requests to come.
    Session(Client),
}

pub async fn run(paths: Paths) -> Result<(), AppError> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(AppError::Usage(
            "`telka` without a command opens the TUI, which needs a terminal; see `telka --help`"
                .into(),
        ));
    }
    let config = config::load(&paths)?;
    let (mut model, effects) = Model::start(Start {
        now: Timestamp::now(),
        tz: TimeZone::system(),
        animations: config.animations(),
    });
    let mut terminal = ratatui::init();
    let result = match execute!(io::stdout(), EnableMouseCapture) {
        Ok(()) => {
            // Leave the terminal's mouse alone even after a panic.
            let restore = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let _ = execute!(io::stdout(), DisableMouseCapture);
                restore(info);
            }));
            let result = event_loop(&paths, &config, &mut terminal, &mut model, effects).await;
            let _ = execute!(io::stdout(), DisableMouseCapture);
            result
        }
        Err(e) => Err(AppError::io("terminal", e)),
    };
    ratatui::restore();
    result
}

/// What goes over the input socket.
#[derive(Debug, Clone, Copy)]
enum RemoteInput {
    Button(Button),
    Pointer(Pointer),
}

/// A picture setting on its way, for one activity entry.
#[derive(Debug)]
struct Setting {
    key: &'static str,
    value: String,
    entry: u64,
}

/// The connection and what keeps working on it.
struct Session {
    client: Client,
    remote: mpsc::UnboundedSender<RemoteInput>,
    volume: watch::Sender<Option<u8>>,
    picture: mpsc::UnboundedSender<Setting>,
    workers: Vec<JoinHandle<()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.abort();
        }
    }
}

/// Background work that a newer request of the same kind replaces.
#[derive(Default)]
struct Tasks {
    connection: Option<JoinHandle<()>>,
    pairing: Option<JoinHandle<()>>,
}

impl Tasks {
    fn replace(slot: &mut Option<JoinHandle<()>>, task: JoinHandle<()>) {
        if let Some(old) = slot.replace(task) {
            old.abort();
        }
    }
}

/// A press of the left button: where, on what, and whether it moved since.
struct Press {
    target: Option<Target>,
    last: Position,
    moved: bool,
}

struct Runner<'a> {
    paths: &'a Paths,
    fixed: Option<IpAddr>,
    tx: mpsc::UnboundedSender<Incoming>,
    tasks: Tasks,
    session: Option<Session>,
}

async fn event_loop(
    paths: &Paths,
    config: &Config,
    terminal: &mut DefaultTerminal,
    model: &mut Model,
    effects: Vec<Effect>,
) -> Result<(), AppError> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    read_input(tx.clone());
    let mut runner = Runner {
        paths,
        fixed: config.tv,
        tx,
        tasks: Tasks::default(),
        session: None,
    };
    for effect in effects {
        runner.spawn(effect);
    }
    let mut hotspots: Vec<Hotspot> = Vec::new();
    let mut press: Option<Press> = None;
    let speed = config.pointer_speed();
    while !model.quit {
        terminal
            .draw(|frame| hotspots = view::render(model, frame))
            .map_err(|e| AppError::io("terminal", e))?;
        let wait = if model.spinning() || model.animating() {
            SPINNER
        } else {
            TICK
        };
        let first = tokio::select! {
            Some(incoming) = rx.recv() => incoming,
            () = tokio::time::sleep(wait) => Incoming::Msg(Box::new(Msg::Tick(Timestamp::now()))),
        };
        let mut incoming = vec![first];
        while let Ok(more) = rx.try_recv() {
            incoming.push(more);
        }
        // The clock first: a key is taken at the time it came.
        let mut messages = vec![Msg::Tick(Timestamp::now())];
        for item in incoming {
            match item {
                Incoming::Msg(msg) => messages.push(*msg),
                Incoming::Mouse(event) => {
                    messages.extend(place(event, &hotspots, &mut press, speed));
                }
                Incoming::Session(client) => runner.start_session(client),
            }
        }
        for msg in messages {
            if msg == Msg::Lost {
                runner.session = None;
            }
            for effect in model.update(msg) {
                runner.spawn(effect);
            }
        }
    }
    for task in [runner.tasks.connection.take(), runner.tasks.pairing.take()]
        .into_iter()
        .flatten()
    {
        task.abort();
    }
    if let Some(session) = runner.session.take() {
        session.client.close().await;
    }
    Ok(())
}

/// What the mouse stands for, by what is under it. A click acts when the button comes up
/// without a drag; a drag that starts in the remote pane moves the TV's pointer.
fn place(
    event: MouseEvent,
    hotspots: &[Hotspot],
    press: &mut Option<Press>,
    speed: i32,
) -> Option<Msg> {
    let pointer = Position::new(event.column, event.row);
    let under = hotspots
        .iter()
        .rev()
        .find(|spot| spot.area.contains(pointer))
        .map(|spot| spot.target.clone());
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            *press = Some(Press {
                target: under,
                last: pointer,
                moved: false,
            });
            None
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let held = press.as_mut()?;
            if !matches!(held.target, Some(Target::Pad | Target::Act(_))) {
                return None;
            }
            let dx = i32::from(pointer.x) - i32::from(held.last.x);
            let dy = i32::from(pointer.y) - i32::from(held.last.y);
            if dx == 0 && dy == 0 {
                return None;
            }
            held.last = pointer;
            held.moved = true;
            Some(Msg::Pointer(Pointer::Move {
                dx: dx * speed,
                dy: dy * speed * 2,
            }))
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let held = press.take()?;
            if held.moved {
                return None;
            }
            match held.target? {
                Target::Act(action) => Some(Msg::Act(action)),
                Target::Tab(tab) => Some(Msg::Tab(tab)),
                Target::Pad => Some(Msg::Pointer(Pointer::Click)),
                Target::Volume => None,
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = event.kind == MouseEventKind::ScrollUp;
            match under? {
                Target::Volume => Some(Msg::Wheel { up }),
                Target::Pad | Target::Act(_) => Some(Msg::Pointer(Pointer::Scroll {
                    dy: if up { -3 } else { 3 },
                })),
                Target::Tab(_) => None,
            }
        }
        _ => None,
    }
}

/// Reads terminal events on their own thread, since reading blocks.
fn read_input(tx: mpsc::UnboundedSender<Incoming>) {
    std::thread::spawn(move || {
        loop {
            let incoming = match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    Incoming::Msg(Box::new(Msg::Key(key)))
                }
                // A tick redraws at the new size.
                Ok(Event::Resize(..)) => Incoming::Msg(Box::new(Msg::Tick(Timestamp::now()))),
                Ok(Event::Mouse(mouse))
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Down(MouseButton::Left)
                            | MouseEventKind::Drag(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                            | MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                    ) =>
                {
                    Incoming::Mouse(mouse)
                }
                Ok(_) => continue,
                Err(_) => return,
            };
            if tx.send(incoming).is_err() {
                return;
            }
        }
    });
}

impl Runner<'_> {
    fn report(&self) -> impl Fn(Msg) + Clone + Send + Sync + 'static {
        let tx = self.tx.clone();
        // The receiver is gone only when the TUI has quit.
        move |msg| {
            let _ = tx.send(Incoming::Msg(Box::new(msg)));
        }
    }

    fn start_session(&mut self, client: Client) {
        let (remote, inputs) = mpsc::unbounded_channel();
        let (volume, levels) = watch::channel(None);
        let (picture, settings) = mpsc::unbounded_channel();
        let workers = vec![
            tokio::spawn(use_remote(client.clone(), inputs, self.report())),
            tokio::spawn(set_volume(client.clone(), levels)),
            tokio::spawn(set_pictures(
                client.clone(),
                settings,
                remote.clone(),
                self.report(),
            )),
        ];
        self.session = Some(Session {
            client,
            remote,
            volume,
            picture,
            workers,
        });
    }

    /// Runs `request` on the connection, telling only when it fails.
    fn fire<F>(&self, request: impl FnOnce(Client) -> F + Send + 'static)
    where
        F: Future<Output = Result<(), webos::Error>> + Send + 'static,
    {
        let report = self.report();
        let Some(session) = &self.session else {
            return;
        };
        let client = session.client.clone();
        tokio::spawn(async move {
            if let Err(error) = request(client).await {
                report(Msg::Failed(error.to_string()));
            }
        });
    }

    /// Runs `request` on the connection and reports how it went for `entry`.
    fn request<F>(&self, entry: u64, request: impl FnOnce(Client) -> F + Send + 'static)
    where
        F: Future<Output = Result<(), webos::Error>> + Send + 'static,
    {
        let report = self.report();
        let Some(session) = &self.session else {
            return report(Msg::Done {
                entry,
                result: Err("not connected".into()),
            });
        };
        let client = session.client.clone();
        tokio::spawn(async move {
            let result = request(client).await.map_err(|error| error.to_string());
            report(Msg::Done { entry, result });
        });
    }

    fn spawn(&mut self, effect: Effect) {
        match effect {
            Effect::Connect { after } => {
                self.session = None;
                let task = tokio::spawn(follow(
                    self.paths.clone(),
                    self.fixed,
                    after,
                    self.tx.clone(),
                ));
                Tasks::replace(&mut self.tasks.connection, task);
            }
            Effect::Press(button) => {
                if let Some(session) = &self.session {
                    let _ = session.remote.send(RemoteInput::Button(button));
                }
            }
            Effect::Pointer(pointer) => {
                if let Some(session) = &self.session {
                    let _ = session.remote.send(RemoteInput::Pointer(pointer));
                }
            }
            Effect::Type(text) => {
                self.fire(move |client| async move { client.insert_text(&text, false).await });
            }
            Effect::Delete => self.fire(|client| async move { client.delete_characters(1).await }),
            Effect::Enter => self.fire(|client| async move { client.send_enter().await }),
            Effect::Volume(level) => {
                if let Some(session) = &self.session {
                    session.volume.send_replace(Some(level));
                }
            }
            Effect::Picture { key, value, entry } => match &self.session {
                Some(session) => {
                    let _ = session.picture.send(Setting { key, value, entry });
                }
                None => self.report()(Msg::Done {
                    entry,
                    result: Err("not connected".into()),
                }),
            },
            Effect::Mute { muted, entry } => {
                self.request(
                    entry,
                    move |client| async move { client.set_mute(muted).await },
                );
            }
            Effect::Launch { app, entry } => {
                self.request(entry, move |client| async move {
                    client.launch(&app, None).await
                });
            }
            Effect::Input { input, entry } => {
                self.request(entry, move |client| async move {
                    client.switch_input(&input).await
                });
            }
            Effect::Screen { on, entry } => {
                self.request(
                    entry,
                    move |client| async move { client.set_screen(on).await },
                );
            }
            Effect::TurnOff { entry } => {
                self.request(entry, move |client| async move {
                    client.turn_off().await.map(drop)
                });
            }
            Effect::Wake { entry } => {
                let paths = self.paths.clone();
                let fixed = self.fixed;
                let report = self.report();
                tokio::spawn(async move {
                    let result = wake(&paths, fixed).await;
                    if let Err(message) = &result {
                        report(Msg::Done {
                            entry,
                            result: Err(message.clone()),
                        });
                    }
                    report(Msg::Woke(result));
                });
            }
            Effect::Pair => {
                let paths = self.paths.clone();
                let report = self.report();
                let task = tokio::spawn(async move {
                    let result = pair(&paths, &report).await;
                    report(Msg::Pairing(match result {
                        Ok(()) => PairingMsg::Done,
                        Err(error) => PairingMsg::Failed(output::message(&error)),
                    }));
                });
                Tasks::replace(&mut self.tasks.pairing, task);
            }
            Effect::CancelPairing => {
                if let Some(task) = self.tasks.pairing.take() {
                    task.abort();
                }
            }
        }
    }
}

/// Sends buttons and pointer moves in order over one input socket, opened when the first one
/// comes and again when it broke.
async fn use_remote(
    client: Client,
    mut inputs: mpsc::UnboundedReceiver<RemoteInput>,
    report: impl Fn(Msg),
) {
    let mut remote = None;
    while let Some(input) = inputs.recv().await {
        let mut sent = false;
        for _ in 0..2 {
            if remote.is_none() {
                remote = client.remote().await.ok();
            }
            let Some(open) = &remote else { break };
            let result = match input {
                RemoteInput::Button(button) => open.button(button).await,
                RemoteInput::Pointer(Pointer::Move { dx, dy }) => open.move_by(dx, dy, false).await,
                RemoteInput::Pointer(Pointer::Click) => open.click().await,
                RemoteInput::Pointer(Pointer::Scroll { dy }) => open.scroll(0, dy).await,
            };
            if result.is_ok() {
                sent = true;
                break;
            }
            remote = None;
        }
        if !sent {
            report(Msg::Failed("the TV's input socket does not answer".into()));
        }
    }
}

/// Sends the latest wanted volume; levels asked for while one is on its way are skipped.
async fn set_volume(client: Client, mut levels: watch::Receiver<Option<u8>>) {
    while levels.changed().await.is_ok() {
        let level = *levels.borrow_and_update();
        if let Some(level) = level {
            let _ = client.set_volume(level).await;
        }
    }
}

/// Changes picture settings one at a time, in order, and reads each back until the next one
/// goes out. A newer value for the same entry replaces one still waiting, so quick steps of the
/// OLED light make one change.
async fn set_pictures(
    client: Client,
    mut settings: mpsc::UnboundedReceiver<Setting>,
    remote: mpsc::UnboundedSender<RemoteInput>,
    report: impl Fn(Msg) + Clone + Send + Sync + 'static,
) {
    let mut reading: Option<JoinHandle<()>> = None;
    while let Some(first) = settings.recv().await {
        let mut queue = vec![first];
        while let Ok(next) = settings.try_recv() {
            queue.retain(|queued| (queued.key, queued.entry) != (next.key, next.entry));
            queue.push(next);
        }
        for Setting { key, value, entry } in queue {
            if let Some(reading) = reading.take() {
                reading.abort();
            }
            let change = json!({ key: &value });
            if set_picture(&client, change, entry, &remote, &report).await {
                let client = client.clone();
                let report = report.clone();
                reading = Some(tokio::spawn(async move {
                    read_back(&client, &report, |picture| shows(picture, key, &value)).await;
                }));
            }
        }
    }
}

/// Whether the TV reports `key` at `value`. It does not tell its picture mode, so that is read
/// for the settings that follow the mode.
fn shows(picture: &Picture, key: &str, value: &str) -> bool {
    match key {
        "backlight" => picture.backlight.map(|level| level.to_string()).as_deref() == Some(value),
        "energySaving" => picture.energy_saving.as_deref() == Some(value),
        _ => false,
    }
}

/// Changes picture settings through the alert, and presses OK when the alert stays on the TV,
/// which closes it and makes the change. Whether the change is on its way.
async fn set_picture(
    client: &Client,
    settings: Value,
    entry: u64,
    remote: &mpsc::UnboundedSender<RemoteInput>,
    report: &impl Fn(Msg),
) -> bool {
    match client.set_system_settings("picture", settings).await {
        Ok(()) => {
            report(Msg::Done {
                entry,
                result: Ok(()),
            });
            true
        }
        Err(webos::Error::AlertOpen) => {
            let _ = remote.send(RemoteInput::Button(Button::ENTER));
            report(Msg::Stuck { entry });
            true
        }
        Err(error) => {
            report(Msg::Done {
                entry,
                result: Err(error.to_string()),
            });
            false
        }
    }
}

/// Reads the picture settings until `wanted` sees the change, or for [`READ_BACK`], and reports
/// each new reading.
async fn read_back(client: &Client, report: &impl Fn(Msg), wanted: impl Fn(&Picture) -> bool) {
    let deadline = Instant::now() + READ_BACK;
    let mut last = None;
    while Instant::now() < deadline {
        tokio::time::sleep(READ_EVERY).await;
        let Ok(picture) = client.get::<Picture>().await else {
            return;
        };
        let seen = wanted(&picture);
        if last.as_ref() != Some(&picture) {
            last = Some(picture.clone());
            report(Msg::Changed(Change::Picture(picture)));
        }
        if seen {
            return;
        }
    }
}

/// Connects after `after`, reads the TV, then reports its changes until the connection ends.
async fn follow(
    paths: Paths,
    fixed: Option<IpAddr>,
    after: Duration,
    tx: mpsc::UnboundedSender<Incoming>,
) {
    let report = |msg| {
        let _ = tx.send(Incoming::Msg(Box::new(msg)));
    };
    tokio::time::sleep(after).await;
    let config = Config {
        tv: fixed,
        ..Config::default()
    };
    let tv = match Tv::connect(&paths, &config).await {
        Ok(tv) => tv,
        Err(error) => return report(Msg::Connected(Err(failure(&paths, &error)))),
    };
    let snapshot = match snapshot(&tv).await {
        Ok(snapshot) => snapshot,
        Err(error) => return report(Msg::Connected(Err(failure(&paths, &error.into())))),
    };
    let client = tv.client;
    let _ = tx.send(Incoming::Session(client.clone()));
    report(Msg::Connected(Ok(Box::new(snapshot))));
    let watches = (
        client.watch::<PowerState>().await,
        client.watch::<ForegroundApp>().await,
        client.watch::<Volume>().await,
        client.watch::<SoundOutput>().await,
    );
    let (Ok(mut power), Ok(mut front), Ok(mut volume), Ok(mut output)) = watches else {
        return report(Msg::Lost);
    };
    // Not every TV tells about its text fields; without it, `t` still opens the line.
    let mut keyboard = client.watch::<Keyboard>().await.ok();
    loop {
        let change = tokio::select! {
            change = power.next() => change.map(|change| change.map(Change::Power)),
            change = front.next() => change.map(|change| change.map(|front| Change::App(front.0))),
            change = volume.next() => change.map(|change| change.map(Change::Volume)),
            change = output.next() => change.map(|change| change.map(|output| Change::Output(output.0))),
            Some(change) = next_or_never(keyboard.as_mut()) => Some(change.map(Change::Keyboard)),
        };
        match change {
            Some(Ok(change)) => report(Msg::Changed(change)),
            Some(Err(_)) => {}
            None => return report(Msg::Lost),
        }
    }
}

/// The next change of an optional watch; never for a watch that is not there.
async fn next_or_never<T: webos::Report>(
    watch: Option<&mut webos::Watch<T>>,
) -> Option<Result<T, webos::Error>> {
    match watch {
        Some(watch) => watch.next().await,
        None => std::future::pending().await,
    }
}

/// What the TV is and does now.
async fn snapshot(tv: &Tv) -> Result<Snapshot, webos::Error> {
    let client = &tv.client;
    let power: PowerState = client.get().await?;
    let app: Option<ForegroundApp> = client.get().await.ok();
    let sound: Option<SoundSettings> = client.get().await.ok();
    let apps: Option<LaunchPoints> = client.get().await.ok();
    let inputs: Option<Inputs> = client.get().await.ok();
    Ok(Snapshot {
        model: tv.paired.model.clone(),
        address: client.endpoint().address.ip(),
        release: client.hello().release.clone(),
        secure: client.endpoint().secure,
        signed: tv.paired.manifest == Manifest::Signed,
        power,
        app: app.map(|app| app.0).unwrap_or_default(),
        volume: client.get().await.ok(),
        output: sound.as_ref().and_then(|sound| sound.output.clone()),
        sound_mode: sound.and_then(|sound| sound.mode),
        picture: client.get::<Picture>().await.ok(),
        apps: apps.map(|apps| apps.0).unwrap_or_default(),
        inputs: inputs.map(|inputs| inputs.0).unwrap_or_default(),
    })
}

fn failure(paths: &Paths, error: &AppError) -> Failure {
    match error {
        AppError::NotPaired => Failure::NotPaired,
        AppError::KeyRejected | AppError::WrongTv { .. } | AppError::UnknownCertificate { .. } => {
            Failure::PairAgain(output::message(error))
        }
        AppError::Unreachable(_) => {
            let paired = Paired::load(paths).ok().flatten();
            Failure::Off {
                model: paired.as_ref().map(|paired| paired.model.clone()),
                address: paired.map(|paired| paired.address),
            }
        }
        other => Failure::Other(output::message(other)),
    }
}

/// Wakes the paired TV over Wi-Fi until it answers.
async fn wake(paths: &Paths, fixed: Option<IpAddr>) -> Result<(), String> {
    let paired = Paired::load(paths)
        .map_err(|error| output::message(&error))?
        .ok_or("not paired")?;
    let mac = paired
        .mac
        .ok_or("telka knows no address to wake the TV at; pair again")?;
    let options = connect::options().map_err(|error| output::message(&error))?;
    let address = fixed.unwrap_or(paired.address);
    match webos::wake_and_wait(mac, address, &options, webos::WAKE_WINDOW).await {
        Ok(_) => Ok(()),
        Err(webos::Error::TimedOut) => Err(format!(
            "no answer within {} s: is \"Turn on via Wi-Fi\" on?",
            webos::WAKE_WINDOW.as_secs()
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// Pairs the way `telka pair` does, telling the dialog when the prompt is on the TV.
async fn pair(paths: &Paths, report: &impl Fn(Msg)) -> Result<(), AppError> {
    let started = pairing::start(None).await?;
    report(Msg::Pairing(PairingMsg::Prompt {
        address: started.address,
        signed: started.manifest() == Manifest::Signed,
    }));
    let (client, _) = pairing::finish(paths, started).await?;
    client.close().await;
    Ok(())
}
