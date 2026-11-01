//! The commands that change something. Each waits until the TV reports the change, at most a few
//! seconds, unless `--no-wait`; a change the TV does not report in time exits with 5.

use std::str::FromStr;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::time::{Instant, sleep, timeout_at};
use webos::{
    Button, Endpoint, ForegroundApp, Inputs, LaunchPoints, Manifest, Media, Picture, PowerState,
    Report, Volume, Watch,
};

use crate::config::Config;
use crate::connect::{self, Tv};
use crate::error::AppError;
use crate::names;
use crate::output::{self, BOLD, DIM, paint};
use crate::paths::Paths;
use crate::store::Paired;
use crate::theme;

/// How long a change may take to show: the volume within milliseconds, an app or an input within
/// seconds, the TV going off within a few seconds and on after a wake within half a minute.
const CHANGE: Duration = Duration::from_secs(5);
const IN_FRONT: Duration = Duration::from_secs(10);
const GOING_OFF: Duration = Duration::from_secs(15);
const COMING_ON: Duration = Duration::from_secs(30);

/// Points Wake-on-LAN at a test double, e.g. `127.0.0.1:9009`, instead of the broadcast. For
/// tests only, with [`connect::TEST_TV`].
pub const TEST_WAKE: &str = "TELKA_WOL_TARGET";

/// How a wait for the TV's report ended.
enum Seen<T> {
    Confirmed,
    /// The connection ended: for turning off, that is the answer.
    Closed,
    /// Not in time; the last report, if any.
    Not(Option<T>),
}

/// Waits until `watch` reports a value that is `done`. The value before the change counts too:
/// when it already was what was asked for, no report comes.
async fn seen<T: Report>(
    watch: &mut Watch<T>,
    done: impl Fn(&T) -> bool,
    wait: Duration,
) -> Seen<T> {
    if done(watch.current()) {
        return Seen::Confirmed;
    }
    let deadline = Instant::now() + wait;
    let mut last = None;
    loop {
        match timeout_at(deadline, watch.next()).await {
            Ok(Some(Ok(value))) if done(&value) => return Seen::Confirmed,
            Ok(Some(Ok(value))) => last = Some(value),
            Ok(Some(Err(_))) => {}
            Ok(None) => return Seen::Closed,
            Err(_) => return Seen::Not(last),
        }
    }
}

/// "0.1 s" or "12 s".
fn took(started: Instant) -> String {
    let seconds = started.elapsed().as_secs_f64();
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{seconds:.0} s")
    }
}

fn done(what: impl std::fmt::Display, started: Instant) {
    output::success(format!("{what}  {}", paint(DIM, took(started))));
}

/// A volume: `15`, or relative to now: `+2`, `-2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Set(u8),
    Up(u8),
    Down(u8),
}

impl FromStr for Level {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let wrong = || format!("{text} is not a volume: 0 to 100, or +N or -N");
        let number = |digits: &str| digits.parse::<u8>().map_err(|_| wrong());
        match text.as_bytes().first() {
            Some(b'+') => number(&text[1..]).map(Self::Up),
            Some(b'-') => number(&text[1..]).map(Self::Down),
            _ => number(text).and_then(|level| {
                if level <= 100 {
                    Ok(Self::Set(level))
                } else {
                    Err(wrong())
                }
            }),
        }
    }
}

pub async fn volume(tv: &Tv, level: Level, no_wait: bool) -> Result<(), AppError> {
    let mut watch = tv.client.watch::<Volume>().await?;
    let now = watch.current().clone();
    if !now.adjustable {
        return Err(AppError::Usage(format!(
            "the TV does not set the volume itself: the sound goes to {}",
            now.output
                .as_deref()
                .map_or_else(|| "another device".to_owned(), theme::sound_output)
        )));
    }
    let target = match level {
        Level::Set(level) => level,
        Level::Up(step) => now.level.saturating_add(step),
        Level::Down(step) => now.level.saturating_sub(step),
    }
    .min(now.max);
    let started = Instant::now();
    tv.client.set_volume(target).await?;
    let text = format!("volume {} → {target}", now.level);
    if no_wait {
        output::success(text);
        return Ok(());
    }
    match seen(&mut watch, |volume| volume.level == target, CHANGE).await {
        Seen::Confirmed => {
            done(text, started);
            Ok(())
        }
        Seen::Closed | Seen::Not(None) => Err(not_reported(&format!("volume {target}"))),
        Seen::Not(Some(volume)) => Err(AppError::NotConfirmed(format!(
            "sent volume {target}, but the TV reports {}",
            volume.level
        ))),
    }
}

fn not_reported(what: &str) -> AppError {
    AppError::NotConfirmed(format!("sent {what}, but the TV did not report it in time"))
}

pub async fn mute(tv: &Tv, muted: bool, no_wait: bool) -> Result<(), AppError> {
    let mut watch = tv.client.watch::<Volume>().await?;
    let started = Instant::now();
    tv.client.set_mute(muted).await?;
    let text = if muted { "muted" } else { "unmuted" };
    if no_wait {
        output::success(text);
        return Ok(());
    }
    match seen(&mut watch, |volume| volume.muted == muted, CHANGE).await {
        Seen::Confirmed => {
            done(text, started);
            Ok(())
        }
        _ => Err(not_reported(text)),
    }
}

/// Waits until `app_id` is in front.
async fn in_front(
    watch: &mut Watch<ForegroundApp>,
    app_id: &str,
    title: &str,
    started: Instant,
    no_wait: bool,
) -> Result<(), AppError> {
    if no_wait {
        output::success(title);
        return Ok(());
    }
    match seen(watch, |front| front.0 == app_id, IN_FRONT).await {
        Seen::Confirmed => {
            output::success(format!(
                "{}  {}",
                paint(BOLD, title),
                paint(DIM, format!("in front in {}", took(started)))
            ));
            Ok(())
        }
        Seen::Not(Some(front)) => Err(AppError::NotConfirmed(format!(
            "asked for {title}, but {} is in front",
            front.0
        ))),
        _ => Err(not_reported(title)),
    }
}

pub async fn app(tv: &Tv, name: &str, url: Option<&str>, no_wait: bool) -> Result<(), AppError> {
    let apps: LaunchPoints = tv.client.get().await?;
    let app = names::app(&apps.0, name)?;
    let mut watch = tv.client.watch::<ForegroundApp>().await?;
    let started = Instant::now();
    match url {
        Some(url) if app.id == "com.webos.app.browser" => tv.client.open_url(url).await?,
        content => tv.client.launch(&app.id, content).await?,
    }
    in_front(&mut watch, &app.id, &app.title, started, no_wait).await
}

pub async fn input(tv: &Tv, name: &str, no_wait: bool) -> Result<(), AppError> {
    let inputs: Inputs = tv.client.get().await?;
    let input = names::input(&inputs.0, name)?;
    let title = theme::input_name(&input.id, &input.label);
    let mut watch = tv.client.watch::<ForegroundApp>().await?;
    let started = Instant::now();
    tv.client.switch_input(&input.id).await?;
    in_front(&mut watch, &input.app_id, &title, started, no_wait).await
}

pub async fn screen(tv: &Tv, on: bool, no_wait: bool) -> Result<(), AppError> {
    let mut watch = tv.client.watch::<PowerState>().await?;
    let started = Instant::now();
    tv.client.set_screen(on).await?;
    let text = if on { "screen on" } else { "screen off" };
    if no_wait {
        output::success(text);
        return Ok(());
    }
    let wanted =
        |power: &PowerState| power.is_on() && (power.mode == webos::PowerMode::ScreenOff) != on;
    match seen(&mut watch, wanted, CHANGE).await {
        Seen::Confirmed => {
            done(text, started);
            Ok(())
        }
        _ => Err(not_reported(text)),
    }
}

pub async fn off(tv: &Tv, no_wait: bool) -> Result<(), AppError> {
    let mut watch = tv.client.watch::<PowerState>().await?;
    if !watch.current().is_on() {
        output::line("the TV is already off");
        return Ok(());
    }
    let started = Instant::now();
    tv.client.turn_off().await?;
    if no_wait {
        output::success("power button pressed");
        return Ok(());
    }
    match seen(&mut watch, |power| !power.is_on(), GOING_OFF).await {
        Seen::Confirmed | Seen::Closed => {
            done("TV off", started);
            Ok(())
        }
        Seen::Not(_) => Err(not_reported("the power button")),
    }
}

pub async fn on(paths: &Paths, config: &Config, no_wait: bool) -> Result<(), AppError> {
    match Tv::connect(paths, config).await {
        Ok(tv) => {
            let mut watch = tv.client.watch::<PowerState>().await?;
            if watch.current().is_on() {
                output::line("the TV is already on");
                return Ok(());
            }
            // Awake but off, like in Active Standby: its power button turns it on.
            let started = Instant::now();
            press(&tv, Button::POWER).await?;
            coming_on(&mut watch, started, no_wait).await
        }
        Err(AppError::Unreachable(_)) => wake(paths, config, no_wait).await,
        Err(error) => Err(error),
    }
}

pub async fn toggle(paths: &Paths, config: &Config, no_wait: bool) -> Result<(), AppError> {
    match Tv::connect(paths, config).await {
        Ok(tv) => {
            let power: PowerState = tv.client.get().await?;
            if power.is_on() {
                off(&tv, no_wait).await
            } else {
                drop(tv);
                on(paths, config, no_wait).await
            }
        }
        Err(AppError::Unreachable(_)) => wake(paths, config, no_wait).await,
        Err(error) => Err(error),
    }
}

async fn press(tv: &Tv, button: Button) -> Result<(), AppError> {
    let remote = tv.client.remote().await?;
    remote.button(button).await?;
    remote.close().await;
    Ok(())
}

async fn coming_on(
    watch: &mut Watch<PowerState>,
    started: Instant,
    no_wait: bool,
) -> Result<(), AppError> {
    if no_wait {
        output::success("the TV answers");
        return Ok(());
    }
    match seen(watch, PowerState::is_on, COMING_ON).await {
        Seen::Confirmed => {
            done("TV on", started);
            Ok(())
        }
        _ => Err(not_reported("turning on")),
    }
}

/// Wakes a TV that does not answer, and waits until it is on.
async fn wake(paths: &Paths, config: &Config, no_wait: bool) -> Result<(), AppError> {
    let paired = Paired::load(paths)?.ok_or(AppError::NotPaired)?;
    let mac = paired.mac.ok_or_else(|| {
        AppError::Usage(
            "telka knows no address to wake the TV at; pair again: `telka unpair`, then \
             `telka pair`"
                .into(),
        )
    })?;
    output::note("waking the TV over the network; this takes up to a minute");
    let options = connect::options()?;
    let started = Instant::now();
    let woken = if let Some(address) = connect::test_tv()? {
        let target = std::env::var(TEST_WAKE)
            .ok()
            .and_then(|target| target.parse().ok())
            .ok_or_else(|| AppError::Usage(format!("{TEST_WAKE} must be <ip>:<port>")))?;
        let endpoint = Endpoint {
            address,
            secure: false,
        };
        webos::wake_until(mac, &[target], endpoint, &options, webos::WAKE_WINDOW).await
    } else {
        let address = config.tv.unwrap_or(paired.address);
        webos::wake_and_wait(mac, address, &options, webos::WAKE_WINDOW).await
    };
    match woken {
        Ok(_) => {}
        Err(webos::Error::TimedOut) => {
            return Err(AppError::Unreachable(format!(
                "the TV did not wake within {} s; it wakes only with \"Turn on via Wi-Fi\" on, \
                 after it was turned off by its power button (as `telka off` does)",
                webos::WAKE_WINDOW.as_secs()
            )));
        }
        Err(error) => return Err(error.into()),
    }
    // It answers some seconds before it is on.
    let tv = Tv::connect(paths, config).await?;
    let mut watch = tv.client.watch::<PowerState>().await?;
    coming_on(&mut watch, started, no_wait).await
}

pub async fn keys(tv: &Tv, names: &[String], gap: Duration) -> Result<(), AppError> {
    let buttons: Vec<Button> = names
        .iter()
        .map(|name| names::button(name))
        .collect::<Result<_, _>>()?;
    let remote = tv.client.remote().await?;
    for (i, button) in buttons.iter().enumerate() {
        if i > 0 {
            sleep(gap).await;
        }
        remote.button(*button).await?;
    }
    remote.close().await;
    let pressed: Vec<&str> = buttons.iter().map(|button| button.name()).collect();
    output::success(format!("pressed {}", pressed.join(" ")));
    Ok(())
}

/// Every button name, for `telka key --list`.
pub fn list_buttons() {
    let all: Vec<&str> = Button::all().map(Button::name).collect();
    for row in all.chunks(8) {
        output::line(row.join("  "));
    }
    output::note("also: ok, settings, ff, rew, vol+, vol-, ch+, ch-, zoom");
}

pub async fn type_text(tv: &Tv, text: &str, enter: bool, replace: bool) -> Result<(), AppError> {
    tv.client.insert_text(text, replace).await?;
    if enter {
        tv.client.send_enter().await?;
    }
    let count = text.chars().count();
    let and_enter = if enter { " and Enter" } else { "" };
    output::success(format!("typed {count} characters{and_enter}"));
    output::note("only text fields with the TV's own keyboard take it; YouTube has its own");
    Ok(())
}

pub async fn media(tv: &Tv, action: Media) -> Result<(), AppError> {
    tv.client.media(action).await?;
    let word = match action {
        Media::Play => "play",
        Media::Pause => "pause",
        Media::Stop => "stop",
        Media::Rewind => "rewind",
        Media::FastForward => "fast forward",
    };
    output::success(word);
    Ok(())
}

pub async fn notify(tv: &Tv, message: &str) -> Result<(), AppError> {
    tv.client.toast(message).await?;
    output::success("shown on the TV");
    output::note("a toast does not show over an app on the full screen");
    Ok(())
}

/// The picture modes of a 2021 LG OLED; the TV takes the HDR and Dolby Vision ones for such
/// content.
const PICTURE_MODES: &[&str] = &[
    "vivid",
    "normal",
    "eco",
    "cinema",
    "sports",
    "game",
    "photo",
    "filmMaker",
    "expert1",
    "expert2",
    "hdrVivid",
    "hdrStandard",
    "hdrCinemaBright",
    "hdrCinema",
    "hdrGame",
    "hdrFilmMaker",
    "dolbyHdrVivid",
    "dolbyHdrStandard",
    "dolbyHdrCinemaBright",
    "dolbyHdrCinema",
    "dolbyHdrGame",
];

const ENERGY_SAVING: &[&str] = &["auto", "off", "min", "med", "max"];

/// The value of `list` that `text` names, in any case.
fn one_of(text: &str, list: &[&'static str], what: &str) -> Result<&'static str, AppError> {
    list.iter()
        .find(|known| known.eq_ignore_ascii_case(text))
        .copied()
        .ok_or_else(|| AppError::Usage(format!("{what} is one of {}", list.join(", "))))
}

/// Picture settings to change; none shows them.
#[derive(Debug, Default)]
pub struct PictureChange {
    pub mode: Option<String>,
    pub light: Option<u8>,
    pub energy: Option<String>,
}

pub async fn picture(tv: &Tv, change: PictureChange, json: bool) -> Result<(), AppError> {
    let before: Picture = tv.client.get().await?;
    if change.mode.is_none() && change.light.is_none() && change.energy.is_none() {
        return show_picture(&before, json);
    }
    if tv.paired.manifest != Manifest::Signed {
        return Err(AppError::Usage(
            "changing picture settings needs a pairing with LG's signed manifest, which this TV \
             refused"
                .into(),
        ));
    }
    let mut settings = Map::new();
    let mode = change
        .mode
        .as_deref()
        .map(|mode| one_of(mode, PICTURE_MODES, "the picture mode"))
        .transpose()?;
    if let Some(mode) = mode {
        settings.insert("pictureMode".into(), json!(mode));
    }
    if let Some(light) = change.light {
        if light > 100 {
            return Err(AppError::Usage("OLED light is 0 to 100".into()));
        }
        settings.insert("backlight".into(), json!(light.to_string()));
    }
    let energy = change
        .energy
        .as_deref()
        .map(|energy| one_of(energy, ENERGY_SAVING, "energy saving"))
        .transpose()?;
    if let Some(energy) = energy {
        settings.insert("energySaving".into(), json!(energy));
    }
    let started = Instant::now();
    match tv
        .client
        .set_system_settings("picture", Value::Object(settings))
        .await
    {
        Err(webos::Error::TimedOut) => {
            return Err(AppError::NotConfirmed(
                "the TV did not close the alert that carries the change; if it is on screen, \
                 press OK on the remote"
                    .into(),
            ));
        }
        other => other?,
    }
    // The settings show in the TV's answers a moment later.
    let deadline = Instant::now() + CHANGE;
    let after = loop {
        let after: Picture = tv.client.get().await?;
        let light_ok = change.light.is_none() || after.backlight == change.light;
        let energy_ok = energy.is_none() || after.energy_saving.as_deref() == energy;
        if (light_ok && energy_ok) || Instant::now() >= deadline {
            break after;
        }
        sleep(Duration::from_millis(250)).await;
    };
    if let Some(light) = change.light {
        if after.backlight != Some(light) {
            return Err(AppError::NotConfirmed(format!(
                "sent OLED light {light}, but the TV reports {}",
                after
                    .backlight
                    .map_or_else(|| "nothing".into(), |l| l.to_string())
            )));
        }
        let from = before
            .backlight
            .map_or_else(|| "?".into(), |l| l.to_string());
        done(format!("OLED light {from} → {light}"), started);
    }
    if let Some(energy) = energy {
        if after.energy_saving.as_deref() != Some(energy) {
            return Err(not_reported(&format!("energy saving {energy}")));
        }
        done(format!("energy saving {energy}"), started);
    }
    if let Some(mode) = mode {
        output::success(format!("picture mode {mode}"));
        output::note("the TV does not tell its picture mode, so this one is not confirmed");
    }
    Ok(())
}

fn show_picture(picture: &Picture, json: bool) -> Result<(), AppError> {
    if json {
        return output::json(
            &json!({"backlight": picture.backlight, "contrast": picture.contrast,
            "brightness": picture.brightness, "color": picture.color,
            "energy_saving": picture.energy_saving}),
        );
    }
    let label = |text: &str| paint(DIM, output::pad(text, 12));
    let number = |value: Option<u8>| value.map_or_else(|| "?".to_owned(), |v| v.to_string());
    if let Some(light) = picture.backlight {
        output::line(format!(
            "{}{}  {light}",
            label("OLED light"),
            output::bar(light, 100, 24)
        ));
    }
    output::line(format!("{}{}", label("contrast"), number(picture.contrast)));
    output::line(format!(
        "{}{}",
        label("brightness"),
        number(picture.brightness)
    ));
    output::line(format!("{}{}", label("color"), number(picture.color)));
    if let Some(energy) = &picture.energy_saving {
        output::line(format!("{}{energy}", label("energy")));
    }
    output::line(format!(
        "{}{}",
        label("mode"),
        paint(DIM, "the TV does not tell")
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_read_absolute_or_relative() {
        assert_eq!("15".parse(), Ok(Level::Set(15)));
        assert_eq!("+2".parse(), Ok(Level::Up(2)));
        assert_eq!("-10".parse(), Ok(Level::Down(10)));
        assert!("101".parse::<Level>().is_err());
        assert!("loud".parse::<Level>().is_err());
        assert!("+".parse::<Level>().is_err());
    }

    #[test]
    fn settings_take_known_values_in_any_case() {
        assert_eq!(
            one_of("FILMMAKER", PICTURE_MODES, "mode").unwrap(),
            "filmMaker"
        );
        assert_eq!(one_of("med", ENERGY_SAVING, "energy").unwrap(), "med");
        assert!(
            one_of("dazzling", PICTURE_MODES, "the picture mode")
                .unwrap_err()
                .to_string()
                .starts_with("the picture mode is one of vivid, normal")
        );
    }
}
