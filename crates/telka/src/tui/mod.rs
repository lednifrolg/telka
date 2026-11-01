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
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Position;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use webos::{
    Button, Client, ForegroundApp, Inputs, LaunchPoints, Manifest, Picture, PowerState,
    SoundOutput, SoundSettings, Volume,
};

use crate::config::{self, Config};
use crate::connect::{self, Tv};
use crate::error::AppError;
use crate::output;
use crate::pairing;
use crate::paths::Paths;
use crate::store::Paired;
use model::{Change, Effect, Failure, Model, Msg, PairingMsg, Snapshot, Start};
use view::{Hotspot, Target};

/// How often the clock and the countdowns are updated, and how often while a spinner turns.
const TICK: Duration = Duration::from_secs(1);
const SPINNER: Duration = Duration::from_millis(100);

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

/// The connection and what keeps working on it.
struct Session {
    client: Client,
    buttons: mpsc::UnboundedSender<Button>,
    volume: watch::Sender<Option<u8>>,
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
    while !model.quit {
        terminal
            .draw(|frame| hotspots = view::render(model, frame))
            .map_err(|e| AppError::io("terminal", e))?;
        let wait = if model.spinning() { SPINNER } else { TICK };
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
                Incoming::Mouse(event) => messages.extend(place(event, &hotspots)),
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

/// The message a click or the wheel stands for, by what is under the pointer.
fn place(event: MouseEvent, hotspots: &[Hotspot]) -> Option<Msg> {
    let pointer = Position::new(event.column, event.row);
    let spot = hotspots
        .iter()
        .rev()
        .find(|spot| spot.area.contains(pointer))?;
    let up = match event.kind {
        MouseEventKind::ScrollUp => Some(true),
        MouseEventKind::ScrollDown => Some(false),
        _ => None,
    };
    match (&spot.target, up) {
        (Target::Volume, Some(up)) => Some(Msg::Wheel { up }),
        (Target::Act(action), None) => Some(Msg::Act(*action)),
        (Target::Tab(tab), None) => Some(Msg::Tab(*tab)),
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
    fn report(&self) -> impl Fn(Msg) + Send + 'static {
        let tx = self.tx.clone();
        // The receiver is gone only when the TUI has quit.
        move |msg| {
            let _ = tx.send(Incoming::Msg(Box::new(msg)));
        }
    }

    fn start_session(&mut self, client: Client) {
        let (buttons, presses) = mpsc::unbounded_channel();
        let (volume, levels) = watch::channel(None);
        let workers = vec![
            tokio::spawn(press_buttons(client.clone(), presses)),
            tokio::spawn(set_volume(client.clone(), levels)),
        ];
        self.session = Some(Session {
            client,
            buttons,
            volume,
            workers,
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
                    let _ = session.buttons.send(button);
                }
            }
            Effect::Volume(level) => {
                if let Some(session) = &self.session {
                    session.volume.send_replace(Some(level));
                }
            }
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

/// Sends buttons in order over one input socket, opened when the first one comes and again when
/// it broke.
async fn press_buttons(client: Client, mut presses: mpsc::UnboundedReceiver<Button>) {
    let mut remote = None;
    while let Some(button) = presses.recv().await {
        for _ in 0..2 {
            if remote.is_none() {
                remote = client.remote().await.ok();
            }
            let Some(open) = &remote else { break };
            if open.button(button).await.is_ok() {
                break;
            }
            remote = None;
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
    let config = Config { tv: fixed };
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
    loop {
        let change = tokio::select! {
            change = power.next() => change.map(|change| change.map(Change::Power)),
            change = front.next() => change.map(|change| change.map(|front| Change::App(front.0))),
            change = volume.next() => change.map(|change| change.map(Change::Volume)),
            change = output.next() => change.map(|change| change.map(|output| Change::Output(output.0))),
        };
        match change {
            Some(Ok(change)) => report(Msg::Changed(change)),
            Some(Err(_)) => {}
            None => return report(Msg::Lost),
        }
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
