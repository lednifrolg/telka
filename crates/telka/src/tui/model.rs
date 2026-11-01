//! The TUI's state and what it does: messages in, effects out. No I/O happens here, so tests
//! drive it with plain values.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use webos::{App, Button, Input, Keyboard, Picture, PowerMode, PowerState, Volume};

use super::keymap::{self, Action};
use crate::names::fold;
use crate::theme;

/// How long a change may take to show: the volume, mute and screen within moments, an app or an
/// input within seconds, the TV going off within seconds and coming on after a wake within two
/// minutes.
const CHANGE: SignedDuration = SignedDuration::from_secs(5);
const IN_FRONT: SignedDuration = SignedDuration::from_secs(10);
const GOING_OFF: SignedDuration = SignedDuration::from_secs(15);
const COMING_ON: SignedDuration = SignedDuration::from_secs(120);
/// How often a TV that is off is looked for: it may come on by its own remote.
pub const RETRY: Duration = Duration::from_secs(5);
/// How long the second `p` that turns the TV off may take.
const ARMED: SignedDuration = SignedDuration::from_secs(3);
/// Volume steps this close together change one activity line.
const FOLD: SignedDuration = SignedDuration::from_secs(1);
const NOTICE: SignedDuration = SignedDuration::from_secs(3);
/// How long a pressed button lights up, how long a change from elsewhere flashes, and how long
/// a bar takes to run to its new value.
const PRESSED: SignedDuration = SignedDuration::from_millis(450);
const FLASH: SignedDuration = SignedDuration::from_millis(1400);
const GLIDE: SignedDuration = SignedDuration::from_millis(300);
const LIGHT_STEP: i16 = 5;
const ACTIVITY: usize = 50;
const SENT: usize = 10;

/// What the TV is and does, as read when the connection starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub model: String,
    pub address: IpAddr,
    pub release: Option<String>,
    pub secure: bool,
    pub signed: bool,
    pub power: PowerState,
    pub app: String,
    pub volume: Option<Volume>,
    pub output: Option<String>,
    pub sound_mode: Option<String>,
    pub picture: Option<Picture>,
    pub apps: Vec<App>,
    pub inputs: Vec<Input>,
}

/// A change the TV reports on its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Power(PowerState),
    App(String),
    Volume(Volume),
    Output(String),
    /// A text field on the TV got or lost focus.
    Keyboard(Keyboard),
    /// The picture settings, read back after telka changed them.
    Picture(Picture),
}

/// What the mouse does with the TV's pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    /// By pixels on the TV.
    Move {
        dx: i32,
        dy: i32,
    },
    Click,
    Scroll {
        dy: i32,
    },
}

/// Why there is no connection.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    NotPaired,
    /// The TV refused: another TV, a reset TV, an unknown certificate.
    PairAgain(String),
    /// Nothing answers: the TV is off, asleep or elsewhere.
    Off {
        model: Option<String>,
        address: Option<IpAddr>,
    },
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PairingMsg {
    /// The prompt is on the TV.
    Prompt {
        address: IpAddr,
        signed: bool,
    },
    Done,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Key(KeyEvent),
    Tick(Timestamp),
    /// A click on something that stands for a key.
    Act(Action),
    /// The wheel over the volume.
    Wheel {
        up: bool,
    },
    /// The mouse in the remote pane, as the TV's pointer.
    Pointer(Pointer),
    /// Something sent without an activity line failed.
    Failed(String),
    Tab(Tab),
    Connected(Result<Box<Snapshot>, Failure>),
    Changed(Change),
    /// The connection ended.
    Lost,
    /// A request for an activity entry failed or went through.
    Done {
        entry: u64,
        result: Result<(), String>,
    },
    /// The alert that carried a picture change stayed on the TV, and telka pressed OK.
    Stuck {
        entry: u64,
    },
    Woke(Result<(), String>),
    Pairing(PairingMsg),
}

/// Work for the I/O side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Connect {
        after: Duration,
    },
    Press(Button),
    Pointer(Pointer),
    /// Text into the field with focus, as it is typed.
    Type(String),
    /// Backspace in the field with focus.
    Delete,
    /// Enter in the field with focus.
    Enter,
    /// The latest wanted volume: older ones still on their way are dropped.
    Volume(u8),
    /// A picture setting through the alert, like `backlight`; a newer value for the same entry
    /// replaces one still on its way.
    Picture {
        key: &'static str,
        value: String,
        entry: u64,
    },
    Mute {
        muted: bool,
        entry: u64,
    },
    Launch {
        app: String,
        entry: u64,
    },
    Input {
        input: String,
        entry: u64,
    },
    Screen {
        on: bool,
        entry: u64,
    },
    TurnOff {
        entry: u64,
    },
    Wake {
        entry: u64,
    },
    Pair,
    CancelPairing,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Connecting,
    Live,
    /// Nothing answers; looked for again at `retry_at`.
    Off {
        retry_at: Timestamp,
        reason: Option<String>,
    },
    Waking {
        since: Timestamp,
    },
    Pairing(Dialog),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    Searching {
        reason: Option<String>,
    },
    Prompt {
        address: IpAddr,
        signed: bool,
        until: Timestamp,
    },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Tv,
    Activity,
}

impl Tab {
    pub const ALL: [Self; 2] = [Self::Tv, Self::Activity];

    pub fn name(self) -> &'static str {
        match self {
            Self::Tv => "tv",
            Self::Activity => "activity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Apps,
    Inputs,
    /// Every button of the remote.
    Buttons,
    Modes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub kind: PickerKind,
    pub filter: String,
    pub selected: usize,
}

/// One row of a picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The app id, or the input id.
    pub id: String,
    pub title: String,
    pub detail: String,
    pub current: bool,
}

/// The line that types into the TV's text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// What was typed since it opened.
    pub text: String,
    /// What kind of field has focus, like `url`, when the TV said.
    pub field: Option<String>,
    /// A password field: shown as dots.
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    Help,
    Picker(Picker),
    Text(TextLine),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Telka,
    /// Anything else: the remote, a phone, the TV itself.
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Done(Option<SignedDuration>),
    Warn(String),
    Failed(String),
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub at: Timestamp,
    pub what: &'static str,
    pub text: String,
    pub origin: Origin,
    pub status: Status,
}

/// A bar that runs to its new value instead of jumping there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glide {
    from: u8,
    to: u8,
    since: Timestamp,
}

impl Glide {
    fn still(level: u8, now: Timestamp) -> Self {
        Self {
            from: level,
            to: level,
            since: now,
        }
    }

    /// Heads for `to` from where the bar is now; jumps there without animations.
    fn head(&mut self, to: u8, now: Timestamp, animations: bool) {
        if to != self.to {
            *self = Self {
                from: if animations { self.at(now) } else { to },
                to,
                since: now,
            };
        }
    }

    /// Where the bar is at `now`: fast at first, slow at the end.
    pub fn at(&self, now: Timestamp) -> u8 {
        let done = now.duration_since(self.since).as_secs_f64() / GLIDE.as_secs_f64();
        if done >= 1.0 {
            return self.to;
        }
        let eased = 1.0 - (1.0 - done.max(0.0)).powi(2);
        let at = f64::from(self.from) + (f64::from(self.to) - f64::from(self.from)) * eased;
        // Between two u8 values.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let at = at.round() as u8;
        at
    }

    fn moving(&self, now: Timestamp) -> bool {
        self.from != self.to && now < self.since + GLIDE
    }

    /// Follows `level` from `bar`; a bar that appears does not run.
    fn follow(bar: &mut Option<Self>, level: Option<u8>, now: Timestamp, animations: bool) {
        match (bar.as_mut(), level) {
            (_, None) => *bar = None,
            (None, Some(level)) => *bar = Some(Self::still(level, now)),
            (Some(glide), Some(level)) => glide.head(level, now, animations),
        }
    }
}

/// What a change telka sent should look like when the TV reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    Volume(u8),
    Muted(bool),
    App(String),
    Screen(bool),
    Off,
    On,
    Light(u8),
    Energy(String),
}

#[derive(Debug, Clone)]
struct Pending {
    entry: u64,
    expect: Expect,
    since: Timestamp,
    until: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub until: Timestamp,
}

pub struct Start {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub animations: bool,
}

pub struct Model {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub phase: Phase,
    /// The TV as last known; kept while it is off, so the panes say which TV.
    pub tv: Option<Snapshot>,
    /// Model and address from the pairing, before the first connection.
    pub known: Option<(String, IpAddr)>,
    pub overlay: Overlay,
    /// Newest first.
    pub activity: VecDeque<Entry>,
    next_entry: u64,
    pending: Vec<Pending>,
    /// The buttons sent last, oldest first.
    pub sent: VecDeque<&'static str>,
    /// What the pointer did last.
    pub pointer: Option<String>,
    pub tab: Tab,
    pub notice: Option<Notice>,
    off_armed: Option<Timestamp>,
    next_play: Button,
    /// The picture mode telka set: the TV does not tell it.
    pub picture_mode: Option<String>,
    /// The action whose button lights up, until when.
    pressed: Option<(Action, Timestamp)>,
    pub volume_bar: Option<Glide>,
    pub light_bar: Option<Glide>,
    animations: bool,
    pub quit: bool,
}

impl Model {
    pub fn start(start: Start) -> (Self, Vec<Effect>) {
        let model = Self {
            now: start.now,
            tz: start.tz,
            phase: Phase::Connecting,
            tv: None,
            known: None,
            overlay: Overlay::None,
            activity: VecDeque::new(),
            next_entry: 1,
            pending: Vec::new(),
            sent: VecDeque::new(),
            pointer: None,
            tab: Tab::Tv,
            notice: None,
            off_armed: None,
            next_play: Button::PAUSE,
            picture_mode: None,
            pressed: None,
            volume_bar: None,
            light_bar: None,
            animations: start.animations,
            quit: false,
        };
        (
            model,
            vec![Effect::Connect {
                after: Duration::ZERO,
            }],
        )
    }

    /// Whether something waits, so a spinner turns.
    pub fn spinning(&self) -> bool {
        !self.pending.is_empty()
            || matches!(
                self.phase,
                Phase::Connecting
                    | Phase::Waking { .. }
                    | Phase::Pairing(Dialog::Searching { .. } | Dialog::Prompt { .. })
            )
    }

    /// Whether something moves, so frames come faster.
    pub fn animating(&self) -> bool {
        let moving = |bar: Option<Glide>| bar.is_some_and(|bar| bar.moving(self.now));
        self.animations
            && (self.pressed.is_some_and(|(_, until)| until > self.now)
                || self.activity.iter().any(|entry| self.flashing(entry))
                || moving(self.volume_bar)
                || moving(self.light_bar))
    }

    /// Whether a change from elsewhere still flashes.
    pub fn flashing(&self, entry: &Entry) -> bool {
        self.animations && entry.origin == Origin::Remote && entry.at + FLASH > self.now
    }

    /// Whether the button for `action` lights up.
    pub fn lit(&self, action: Action) -> bool {
        self.animations
            && self
                .pressed
                .is_some_and(|(pressed, until)| pressed == action && until > self.now)
    }

    /// The OLED light as shown: what telka asked for while the TV has not reported it yet.
    pub fn shown_light(&self) -> Option<u8> {
        self.pending
            .iter()
            .find_map(|pending| match pending.expect {
                Expect::Light(level) => Some(level),
                _ => None,
            })
            .or_else(|| self.tv.as_ref()?.picture.as_ref()?.backlight)
    }

    /// Energy saving as shown, the same way.
    pub fn shown_energy(&self) -> Option<String> {
        self.pending
            .iter()
            .find_map(|pending| match &pending.expect {
                Expect::Energy(level) => Some(level.clone()),
                _ => None,
            })
            .or_else(|| self.tv.as_ref()?.picture.as_ref()?.energy_saving.clone())
    }

    /// The volume as shown: what telka asked for while the TV has not reported it yet.
    pub fn shown_volume(&self) -> Option<(u8, bool)> {
        let volume = self.tv.as_ref()?.volume.as_ref()?;
        let level = self
            .pending
            .iter()
            .find_map(|pending| match pending.expect {
                Expect::Volume(level) => Some(level),
                _ => None,
            })
            .unwrap_or(volume.level);
        let muted = self
            .pending
            .iter()
            .find_map(|pending| match pending.expect {
                Expect::Muted(muted) => Some(muted),
                _ => None,
            })
            .unwrap_or(volume.muted);
        Some((level, muted))
    }

    /// What is in front, in words.
    pub fn in_front(&self) -> String {
        let Some(tv) = &self.tv else {
            return String::new();
        };
        if let Some(input) = tv.inputs.iter().find(|input| input.app_id == tv.app) {
            return theme::input_name(&input.id, &input.label);
        }
        if let Some(app) = tv.apps.iter().find(|app| app.id == tv.app) {
            return app.title.clone();
        }
        theme::in_front(&tv.app, None, None)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let effects = self.handle(msg);
        let (now, animations) = (self.now, self.animations);
        let volume = self.shown_volume().map(|(level, _)| level);
        Glide::follow(&mut self.volume_bar, volume, now, animations);
        let light = self.shown_light();
        Glide::follow(&mut self.light_bar, light, now, animations);
        effects
    }

    fn handle(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Tick(now) => {
                self.tick(now);
                Vec::new()
            }
            Msg::Key(key) => self.key(key),
            Msg::Act(action) => self.act(action),
            Msg::Wheel { up } => self.act(if up {
                Action::VolumeUp
            } else {
                Action::VolumeDown
            }),
            Msg::Tab(tab) => {
                self.tab = tab;
                Vec::new()
            }
            Msg::Pointer(pointer) => {
                if self.phase != Phase::Live {
                    return Vec::new();
                }
                self.pointer = Some(match pointer {
                    Pointer::Move { dx, dy } => format!("move  dx {dx:+}  dy {dy:+}"),
                    Pointer::Click => "click".into(),
                    Pointer::Scroll { dy } if dy < 0 => "scroll up".into(),
                    Pointer::Scroll { .. } => "scroll down".into(),
                });
                vec![Effect::Pointer(pointer)]
            }
            Msg::Failed(message) => {
                self.notice(message);
                Vec::new()
            }
            Msg::Connected(Ok(snapshot)) => self.connected(*snapshot),
            Msg::Connected(Err(failure)) => self.failed(failure),
            Msg::Changed(change) => {
                self.changed(change);
                Vec::new()
            }
            Msg::Lost => self.lost(),
            Msg::Stuck { entry } => {
                // OK makes the change, so it is still waited for, with time for the TV to show it.
                let until = self.now + CHANGE;
                for pending in self
                    .pending
                    .iter_mut()
                    .filter(|pending| pending.entry == entry)
                {
                    pending.until = until;
                }
                self.settle_entry(
                    entry,
                    Status::Warn("the alert stayed on the TV; telka pressed OK".into()),
                );
                Vec::new()
            }
            Msg::Done { entry, result } => {
                match result {
                    Err(message) => self.settle(entry, Status::Failed(message)),
                    // Nothing to wait for, like a picture mode: done once the TV took it.
                    Ok(()) if !self.pending.iter().any(|pending| pending.entry == entry) => {
                        let at = self
                            .activity
                            .iter()
                            .find(|found| found.id == entry)
                            .map(|found| found.at);
                        let took = at.map(|at| self.now.duration_since(at));
                        self.settle_entry(entry, Status::Done(took));
                    }
                    Ok(()) => {}
                }
                Vec::new()
            }
            Msg::Woke(Ok(())) => vec![Effect::Connect {
                after: Duration::ZERO,
            }],
            Msg::Woke(Err(message)) => {
                if let Some(pending) = self.take(|expect| *expect == Expect::On) {
                    self.settle_entry(pending.entry, Status::Failed(message));
                }
                self.off(None)
            }
            Msg::Pairing(msg) => self.pairing(msg),
        }
    }

    fn tick(&mut self, now: Timestamp) {
        self.now = now;
        let (expired, waiting): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|pending| pending.until <= now);
        self.pending = waiting;
        for pending in expired {
            if !self.warned(pending.entry) {
                self.settle_entry(
                    pending.entry,
                    Status::Warn("the TV did not report it".into()),
                );
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.until <= now)
        {
            self.notice = None;
        }
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            until: self.now + NOTICE,
        });
    }

    fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match &mut self.overlay {
            Overlay::Help => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('?') => self.overlay = Overlay::None,
                    KeyCode::Char('q') => self.quit = true,
                    _ => {}
                }
                return Vec::new();
            }
            Overlay::Picker(_) => return self.picker_key(key),
            Overlay::Text(_) => return self.text_key(key),
            Overlay::None => {}
        }
        if let Phase::Pairing(dialog) = &self.phase {
            return match (key.code, dialog) {
                (KeyCode::Char('q'), _) => {
                    self.quit = true;
                    vec![Effect::CancelPairing]
                }
                (KeyCode::Enter, Dialog::Failed(_)) => {
                    self.phase = Phase::Pairing(Dialog::Searching { reason: None });
                    vec![Effect::Pair]
                }
                (KeyCode::Esc, Dialog::Searching { .. } | Dialog::Prompt { .. }) => {
                    self.phase = Phase::Pairing(Dialog::Failed("pairing cancelled".into()));
                    vec![Effect::CancelPairing]
                }
                _ => Vec::new(),
            };
        }
        keymap::action(key).map_or_else(Vec::new, |action| self.act(action))
    }

    fn act(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Quit => {
                self.quit = true;
                return Vec::new();
            }
            Action::Help => {
                self.overlay = Overlay::Help;
                return Vec::new();
            }
            Action::NextTab => {
                self.tab = match self.tab {
                    Tab::Tv => Tab::Activity,
                    Tab::Activity => Tab::Tv,
                };
                return Vec::new();
            }
            Action::Tab(index) => {
                if let Some(tab) = Tab::ALL.get(index) {
                    self.tab = *tab;
                }
                return Vec::new();
            }
            _ => {}
        }
        match self.phase {
            Phase::Live => self.live(action),
            Phase::Off { .. } => match action {
                Action::Power => self.wake(),
                Action::Resync => {
                    self.phase = Phase::Connecting;
                    vec![Effect::Connect {
                        after: Duration::ZERO,
                    }]
                }
                Action::Back => Vec::new(),
                _ => {
                    self.notice("the TV is off · p wakes it");
                    Vec::new()
                }
            },
            _ => Vec::new(),
        }
    }

    fn live(&mut self, action: Action) -> Vec<Effect> {
        self.pressed = Some((action, self.now + PRESSED));
        match action {
            Action::Press(button) => self.press(button),
            Action::Back => self.press(Button::BACK),
            Action::PlayPause => {
                let button = self.next_play;
                self.next_play = if button == Button::PAUSE {
                    Button::PLAY
                } else {
                    Button::PAUSE
                };
                self.press(button)
            }
            Action::VolumeUp => self.volume(1),
            Action::VolumeDown => self.volume(-1),
            Action::Mute => {
                let Some((_, muted)) = self.shown_volume() else {
                    return Vec::new();
                };
                let text = if muted { "unmuted" } else { "muted" };
                let entry = self.expect("volume", text.into(), Expect::Muted(!muted), CHANGE);
                vec![Effect::Mute {
                    muted: !muted,
                    entry,
                }]
            }
            Action::LightUp => self.light(LIGHT_STEP),
            Action::LightDown => self.light(-LIGHT_STEP),
            Action::Energy => self.energy(),
            Action::Modes => {
                if self.signed() {
                    self.overlay = Overlay::Picker(Picker {
                        kind: PickerKind::Modes,
                        filter: String::new(),
                        selected: 0,
                    });
                }
                Vec::new()
            }
            Action::Type => {
                self.overlay = Overlay::Text(TextLine {
                    text: String::new(),
                    field: None,
                    hidden: false,
                });
                Vec::new()
            }
            Action::Apps | Action::Inputs | Action::Buttons => {
                let kind = match action {
                    Action::Apps => PickerKind::Apps,
                    Action::Inputs => PickerKind::Inputs,
                    _ => PickerKind::Buttons,
                };
                self.overlay = Overlay::Picker(Picker {
                    kind,
                    filter: String::new(),
                    selected: 0,
                });
                Vec::new()
            }
            Action::Screen => {
                let Some(tv) = &self.tv else {
                    return Vec::new();
                };
                let on = tv.power.mode == PowerMode::ScreenOff;
                let text = if on {
                    "screen on"
                } else {
                    "screen off, the sound stays"
                };
                let entry = self.expect("screen", text.into(), Expect::Screen(on), CHANGE);
                vec![Effect::Screen { on, entry }]
            }
            Action::Power => {
                if self.off_armed.is_some_and(|until| until > self.now) {
                    self.off_armed = None;
                    self.notice = None;
                    let entry = self.expect("power", "turn off".into(), Expect::Off, GOING_OFF);
                    return vec![Effect::TurnOff { entry }];
                }
                self.off_armed = Some(self.now + ARMED);
                self.notice("press p again to turn the TV off");
                Vec::new()
            }
            Action::Resync => {
                self.log("read", "everything from the TV again".into(), Status::Info);
                vec![Effect::Connect {
                    after: Duration::ZERO,
                }]
            }
            Action::Quit | Action::Help | Action::NextTab | Action::Tab(_) => Vec::new(),
        }
    }

    /// Whether picture settings can change; tells why not.
    fn signed(&mut self) -> bool {
        let signed = self.tv.as_ref().is_some_and(|tv| tv.signed);
        if !signed {
            self.notice("picture settings need a pairing with LG's signed manifest");
        }
        signed
    }

    /// Energy saving, the next level after the one the TV reports.
    fn energy(&mut self) -> Vec<Effect> {
        if !self.signed() {
            return Vec::new();
        }
        let levels = crate::control::ENERGY_SAVING;
        let next = self
            .shown_energy()
            .and_then(|now| levels.iter().position(|level| *level == now))
            .map_or(levels[0], |index| levels[(index + 1) % levels.len()]);
        let text = format!("energy saving {next}");
        let folded = self.fold(
            |expect| matches!(expect, Expect::Energy(_)),
            Expect::Energy(next.to_owned()),
            |_| text.clone(),
        );
        let entry = match folded {
            Some(entry) => entry,
            None => self.expect("picture", text, Expect::Energy(next.to_owned()), CHANGE),
        };
        vec![Effect::Picture {
            key: "energySaving",
            value: next.to_owned(),
            entry,
        }]
    }

    fn light(&mut self, step: i16) -> Vec<Effect> {
        if !self.signed() {
            return Vec::new();
        }
        let Some(level) = self.shown_light() else {
            return Vec::new();
        };
        let target = u8::try_from((i16::from(level) + step).clamp(0, 100)).unwrap_or(level);
        let folded = self.fold(
            |expect| matches!(expect, Expect::Light(_)),
            Expect::Light(target),
            |text| {
                let from = text.split(" → ").next().unwrap_or_default();
                format!("{from} → {target}")
            },
        );
        let entry = match folded {
            Some(entry) => entry,
            None => self.expect(
                "picture",
                format!("OLED light {level} → {target}"),
                Expect::Light(target),
                CHANGE,
            ),
        };
        vec![Effect::Picture {
            key: "backlight",
            value: target.to_string(),
            entry,
        }]
    }

    /// Folds a change into the one of its kind still on its way, unless that one went wrong:
    /// it now heads for `expect`, and its line reads `text` of the old one. Its entry.
    fn fold(
        &mut self,
        kind: impl Fn(&Expect) -> bool,
        expect: Expect,
        text: impl FnOnce(&str) -> String,
    ) -> Option<u64> {
        let warned: Vec<u64> = self
            .activity
            .iter()
            .filter(|entry| matches!(entry.status, Status::Warn(_)))
            .map(|entry| entry.id)
            .collect();
        let until = self.now + CHANGE;
        let pending = self
            .pending
            .iter_mut()
            .find(|pending| kind(&pending.expect) && !warned.contains(&pending.entry))?;
        pending.expect = expect;
        pending.until = until;
        let id = pending.entry;
        if let Some(entry) = self.activity.iter_mut().find(|entry| entry.id == id) {
            entry.text = text(&entry.text);
        }
        Some(id)
    }

    fn press(&mut self, button: Button) -> Vec<Effect> {
        self.sent.push_back(glyph(button));
        while self.sent.len() > SENT {
            self.sent.pop_front();
        }
        vec![Effect::Press(button)]
    }

    fn volume(&mut self, step: i16) -> Vec<Effect> {
        let Some((level, _)) = self.shown_volume() else {
            return Vec::new();
        };
        let max = self
            .tv
            .as_ref()
            .and_then(|tv| tv.volume.as_ref())
            .map_or(100, |volume| volume.max);
        let target =
            u8::try_from((i16::from(level) + step).clamp(0, i16::from(max))).unwrap_or(level);
        // Steps close together change the line of the steps before.
        let recent = self.activity.front().is_some_and(|entry| {
            entry.what == "volume"
                && entry.origin == Origin::Telka
                && entry.status == Status::Waiting
                && self.now.duration_since(entry.at) < FOLD
                && self.pending.iter().any(|pending| {
                    pending.entry == entry.id && matches!(pending.expect, Expect::Volume(_))
                })
        });
        if recent {
            let entry = self.activity.front_mut().expect("checked above");
            let from = entry
                .text
                .split(" → ")
                .next()
                .unwrap_or_default()
                .to_owned();
            entry.text = format!("{from} → {target}");
            entry.at = self.now;
            let id = entry.id;
            for pending in &mut self.pending {
                if pending.entry == id {
                    pending.expect = Expect::Volume(target);
                    pending.until = self.now + CHANGE;
                }
            }
        } else {
            self.pending
                .retain(|pending| !matches!(pending.expect, Expect::Volume(_)));
            self.expect(
                "volume",
                format!("{level} → {target}"),
                Expect::Volume(target),
                CHANGE,
            );
        }
        vec![Effect::Volume(target)]
    }

    fn picker_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let choices = self.choices();
        let Overlay::Picker(picker) = &mut self.overlay else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(choices.len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.selected = 0;
            }
            KeyCode::Enter => {
                let kind = picker.kind;
                let chosen = choices.get(picker.selected).cloned();
                self.overlay = Overlay::None;
                if let Some(choice) = chosen {
                    return self.choose(kind, choice);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.filter.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        Vec::new()
    }

    fn text_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Overlay::Text(line) = &mut self.overlay else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.close_text(),
            KeyCode::Enter => {
                self.close_text();
                vec![Effect::Enter]
            }
            KeyCode::Backspace => {
                line.text.pop();
                vec![Effect::Delete]
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                line.text.push(c);
                vec![Effect::Type(c.to_string())]
            }
            _ => Vec::new(),
        }
    }

    /// Closes the text line, telling what went into the TV.
    fn close_text(&mut self) -> Vec<Effect> {
        if let Overlay::Text(line) = std::mem::replace(&mut self.overlay, Overlay::None)
            && !line.text.is_empty()
        {
            let shown = if line.hidden {
                "•".repeat(line.text.chars().count())
            } else {
                line.text
            };
            self.log("type", format!("\"{shown}\""), Status::Info);
        }
        Vec::new()
    }

    /// The rows of the open picker, filtered.
    pub fn choices(&self) -> Vec<Choice> {
        let (Overlay::Picker(picker), Some(tv)) = (&self.overlay, &self.tv) else {
            return Vec::new();
        };
        let wanted = fold(&picker.filter);
        let all: Vec<Choice> = match picker.kind {
            PickerKind::Modes => crate::control::PICTURE_MODES
                .iter()
                .map(|mode| Choice {
                    id: (*mode).to_owned(),
                    title: (*mode).to_owned(),
                    detail: String::new(),
                    current: self.picture_mode.as_deref() == Some(*mode),
                })
                .collect(),
            PickerKind::Buttons => Button::all()
                .map(|button| Choice {
                    id: button.name().to_owned(),
                    title: button.name().to_owned(),
                    detail: String::new(),
                    current: false,
                })
                .collect(),
            PickerKind::Apps => tv
                .apps
                .iter()
                .map(|app| Choice {
                    id: app.id.clone(),
                    title: app.title.clone(),
                    detail: app.id.clone(),
                    current: app.id == tv.app,
                })
                .collect(),
            PickerKind::Inputs => tv
                .inputs
                .iter()
                .map(|input| Choice {
                    id: input.id.clone(),
                    title: theme::input_name(&input.id, &input.label),
                    detail: input.devices.join(", "),
                    current: input.app_id == tv.app,
                })
                .collect(),
        };
        // Names that start with the filter first, then the ones that contain it.
        let (starting, containing): (Vec<Choice>, Vec<Choice>) = all
            .into_iter()
            .filter(|choice| {
                wanted.is_empty()
                    || fold(&choice.title).contains(&wanted)
                    || fold(&choice.detail).contains(&wanted)
            })
            .partition(|choice| fold(&choice.title).starts_with(&wanted));
        starting.into_iter().chain(containing).collect()
    }

    fn choose(&mut self, kind: PickerKind, choice: Choice) -> Vec<Effect> {
        match kind {
            PickerKind::Modes => {
                self.picture_mode = Some(choice.id.clone());
                let entry = self.log(
                    "picture",
                    format!("picture mode {}", choice.id),
                    Status::Waiting,
                );
                vec![Effect::Picture {
                    key: "pictureMode",
                    value: choice.id,
                    entry,
                }]
            }
            PickerKind::Buttons => Button::from_name(&choice.id)
                .map(|button| self.press(button))
                .unwrap_or_default(),
            PickerKind::Apps => {
                let entry = self.expect(
                    "app",
                    choice.title,
                    Expect::App(choice.id.clone()),
                    IN_FRONT,
                );
                vec![Effect::Launch {
                    app: choice.id,
                    entry,
                }]
            }
            PickerKind::Inputs => {
                let app_id = self.tv.as_ref().and_then(|tv| {
                    tv.inputs
                        .iter()
                        .find(|input| input.id == choice.id)
                        .map(|input| input.app_id.clone())
                });
                let Some(app_id) = app_id else {
                    return Vec::new();
                };
                let entry = self.expect("input", choice.title, Expect::App(app_id), IN_FRONT);
                vec![Effect::Input {
                    input: choice.id,
                    entry,
                }]
            }
        }
    }

    fn wake(&mut self) -> Vec<Effect> {
        self.phase = Phase::Waking { since: self.now };
        let entry = self.expect("power", "Wake-on-LAN".into(), Expect::On, COMING_ON);
        vec![Effect::Wake { entry }]
    }

    fn connected(&mut self, snapshot: Snapshot) -> Vec<Effect> {
        let first = self.tv.is_none();
        let was_off = matches!(self.phase, Phase::Off { .. } | Phase::Waking { .. });
        let on = snapshot.power.is_on();
        let count = (snapshot.apps.len(), snapshot.inputs.len());
        self.tv = Some(snapshot);
        self.phase = Phase::Live;
        if on && let Some(pending) = self.take(|expect| *expect == Expect::On) {
            let took = self.now.duration_since(pending.since);
            self.settle_entry(pending.entry, Status::Done(Some(took)));
        } else if first {
            self.log(
                "tv",
                format!("connected · {} apps · {} inputs", count.0, count.1),
                Status::Info,
            );
        } else if was_off && on {
            self.log_change("power", "off → on".into());
        }
        Vec::new()
    }

    fn failed(&mut self, failure: Failure) -> Vec<Effect> {
        match failure {
            Failure::NotPaired => {
                self.phase = Phase::Pairing(Dialog::Searching { reason: None });
                vec![Effect::Pair]
            }
            Failure::PairAgain(reason) => {
                self.phase = Phase::Pairing(Dialog::Searching {
                    reason: Some(reason),
                });
                vec![Effect::Pair]
            }
            Failure::Off { model, address } => {
                if let (Some(model), Some(address)) = (model, address) {
                    self.known = Some((model, address));
                }
                if matches!(self.phase, Phase::Waking { .. }) {
                    // Woken, but not answering yet: keep waiting.
                    return vec![Effect::Connect { after: RETRY }];
                }
                self.off(None)
            }
            Failure::Other(message) => self.off(Some(message)),
        }
    }

    fn off(&mut self, reason: Option<String>) -> Vec<Effect> {
        self.phase = Phase::Off {
            retry_at: self.now + RETRY.try_into().unwrap_or(SignedDuration::ZERO),
            reason,
        };
        self.off_armed = None;
        vec![Effect::Connect { after: RETRY }]
    }

    fn lost(&mut self) -> Vec<Effect> {
        if let Some(pending) = self.take(|expect| *expect == Expect::Off) {
            let took = self.now.duration_since(pending.since);
            self.settle_entry(pending.entry, Status::Done(Some(took)));
        } else if matches!(self.phase, Phase::Live) {
            self.log_change("power", "on → off".into());
        }
        if let Some(tv) = &mut self.tv {
            tv.power = PowerState {
                mode: PowerMode::PowerOff,
                processing: None,
                power_on_reason: None,
            };
        }
        // Whatever waited for the TV will not come now.
        for pending in std::mem::take(&mut self.pending) {
            if pending.expect == Expect::On {
                self.pending.push(pending);
            } else {
                self.settle_entry(pending.entry, Status::Warn("the TV went away".into()));
            }
        }
        self.overlay = match self.overlay {
            Overlay::Help => Overlay::Help,
            _ => Overlay::None,
        };
        self.off(None)
    }

    fn changed(&mut self, change: Change) {
        let Some(tv) = &mut self.tv else {
            return;
        };
        match change {
            Change::Volume(volume) => {
                let before = tv.volume.replace(volume.clone());
                self.volume_changed(before, &volume);
            }
            Change::App(app) => {
                let before = std::mem::replace(&mut tv.app, app.clone());
                if let Some(pending) = self.take(|expect| *expect == Expect::App(app.clone())) {
                    self.done(&pending);
                    return;
                }
                if before != app {
                    let tv = self.tv.as_ref().expect("checked above");
                    let name = |id: &str| {
                        let apps = webos::LaunchPoints(tv.apps.clone());
                        let inputs = webos::Inputs(tv.inputs.clone());
                        theme::in_front(id, Some(&apps), Some(&inputs))
                    };
                    let text = format!("{} → {}", name(&before), name(&app));
                    self.log_change("app", text);
                }
            }
            Change::Power(power) => {
                let before = theme::power(&tv.power);
                tv.power = power.clone();
                let screen_off = power.mode == PowerMode::ScreenOff;
                let settled = self
                    .take(|expect| match expect {
                        Expect::Screen(on) => power.is_on() && screen_off != *on,
                        Expect::Off => !power.is_on(),
                        Expect::On => power.is_on(),
                        _ => false,
                    })
                    .inspect(|pending| self.done(pending));
                let after = theme::power(&power);
                if settled.is_none() && before != after {
                    self.log_change("power", format!("{before} → {after}"));
                }
            }
            Change::Picture(picture) => {
                let (light, energy) = (picture.backlight, picture.energy_saving.clone());
                tv.picture = Some(picture);
                let seen = |expect: &Expect| match expect {
                    Expect::Light(level) => light == Some(*level),
                    Expect::Energy(level) => energy.as_ref() == Some(level),
                    _ => false,
                };
                while let Some(pending) = self.take(seen) {
                    self.done(&pending);
                }
            }
            Change::Keyboard(keyboard) => {
                if keyboard.focused && self.overlay == Overlay::None {
                    self.overlay = Overlay::Text(TextLine {
                        text: String::new(),
                        field: keyboard.content_type,
                        hidden: keyboard.hidden,
                    });
                } else if !keyboard.focused && matches!(self.overlay, Overlay::Text(_)) {
                    self.close_text();
                }
            }
            Change::Output(output) => {
                let before = tv.output.replace(output.clone());
                if before.as_ref() != Some(&output) {
                    let before = before.as_deref().map(theme::sound_output);
                    let text = match before {
                        Some(before) => format!("{before} → {}", theme::sound_output(&output)),
                        None => theme::sound_output(&output),
                    };
                    self.log_change("sound", text);
                }
            }
        }
    }

    /// Settles what telka waited for, or tells about a change from elsewhere.
    fn volume_changed(&mut self, before: Option<webos::Volume>, volume: &webos::Volume) {
        let mut told = false;
        if let Some(pending) = self.take(|expect| *expect == Expect::Volume(volume.level)) {
            self.done(&pending);
            told = true;
        }
        if let Some(pending) = self.take(|expect| *expect == Expect::Muted(volume.muted)) {
            self.done(&pending);
            told = true;
        }
        let ours = self
            .pending
            .iter()
            .any(|pending| matches!(pending.expect, Expect::Volume(_) | Expect::Muted(_)));
        if told || ours {
            return;
        }
        if let Some(before) = before {
            if before.level != volume.level {
                self.log_change("volume", format!("{} → {}", before.level, volume.level));
            }
            if before.muted != volume.muted {
                let text = if volume.muted { "muted" } else { "unmuted" };
                self.log_change("volume", text.into());
            }
        }
    }

    fn pairing(&mut self, msg: PairingMsg) -> Vec<Effect> {
        match msg {
            PairingMsg::Prompt { address, signed } => {
                self.phase = Phase::Pairing(Dialog::Prompt {
                    address,
                    signed,
                    until: self.now + SignedDuration::from_secs(60),
                });
                Vec::new()
            }
            PairingMsg::Done => {
                self.log("pair", "paired with the TV".into(), Status::Info);
                self.phase = Phase::Connecting;
                vec![Effect::Connect {
                    after: Duration::ZERO,
                }]
            }
            PairingMsg::Failed(message) => {
                self.phase = Phase::Pairing(Dialog::Failed(message));
                Vec::new()
            }
        }
    }

    /// Adds an entry that waits for the TV to report `expect`.
    fn expect(
        &mut self,
        what: &'static str,
        text: String,
        expect: Expect,
        wait: SignedDuration,
    ) -> u64 {
        let id = self.log(what, text, Status::Waiting);
        self.pending.push(Pending {
            entry: id,
            expect,
            since: self.now,
            until: self.now + wait,
        });
        id
    }

    fn log(&mut self, what: &'static str, text: String, status: Status) -> u64 {
        let id = self.next_entry;
        self.next_entry += 1;
        self.activity.push_front(Entry {
            id,
            at: self.now,
            what,
            text,
            origin: Origin::Telka,
            status,
        });
        self.activity.truncate(ACTIVITY);
        id
    }

    /// A change that came from anywhere but telka.
    fn log_change(&mut self, what: &'static str, text: String) {
        let id = self.log(what, text, Status::Info);
        if let Some(entry) = self.activity.iter_mut().find(|entry| entry.id == id) {
            entry.origin = Origin::Remote;
        }
    }

    fn take(&mut self, matches: impl Fn(&Expect) -> bool) -> Option<Pending> {
        let index = self
            .pending
            .iter()
            .position(|pending| matches(&pending.expect))?;
        Some(self.pending.remove(index))
    }

    /// Settles what the TV showed; a warning stays, since something went wrong on the way.
    fn done(&mut self, pending: &Pending) {
        if self.warned(pending.entry) {
            return;
        }
        let took = self.now.duration_since(pending.since);
        self.settle_entry(pending.entry, Status::Done(Some(took)));
    }

    fn warned(&self, entry: u64) -> bool {
        self.activity
            .iter()
            .any(|found| found.id == entry && matches!(found.status, Status::Warn(_)))
    }

    /// Settles an entry and forgets what it waited for.
    fn settle(&mut self, entry: u64, status: Status) {
        self.pending.retain(|pending| pending.entry != entry);
        self.settle_entry(entry, status);
    }

    fn settle_entry(&mut self, entry: u64, status: Status) {
        if let Some(found) = self.activity.iter_mut().find(|found| found.id == entry) {
            found.status = status;
        }
    }
}

/// How a sent button reads in the remote pane.
pub fn glyph(button: Button) -> &'static str {
    match button.name() {
        "UP" => "↑",
        "DOWN" => "↓",
        "LEFT" => "←",
        "RIGHT" => "→",
        "ENTER" => "OK",
        "BACK" => "back",
        "HOME" => "home",
        "MENU" => "settings",
        "REWIND" => "◀◀",
        "FASTFORWARD" => "▶▶",
        "PAUSE" => "pause",
        "PLAY" => "play",
        other => other,
    }
}

#[cfg(test)]
pub mod tests {
    use serde_json::json;
    use webos::Report;

    use super::*;

    pub fn now() -> Timestamp {
        "2026-11-01T20:04:00Z".parse().unwrap()
    }

    pub fn start() -> Start {
        Start {
            now: now(),
            tz: TimeZone::UTC,
            animations: true,
        }
    }

    pub fn snapshot() -> Snapshot {
        Snapshot {
            model: "OLED55C16LA".into(),
            address: "192.168.1.30".parse().unwrap(),
            release: Some("6.5.3".into()),
            secure: true,
            signed: true,
            power: PowerState::read(&json!({"state": "Active"})).unwrap(),
            app: "youtube.leanback.v4".into(),
            volume: Some(
                Volume::read(
                    &json!({"volumeStatus": {"volume": 12, "muteStatus": false, "maxVolume": 100}}),
                )
                .unwrap(),
            ),
            output: Some("tv_speaker".into()),
            sound_mode: Some("movie".into()),
            picture: Some(
                Picture::read(&json!({"settings": {"backlight": "80", "contrast": "100",
                    "brightness": "50", "color": "50", "energySaving": "auto"}}))
                .unwrap(),
            ),
            apps: webos::LaunchPoints::read(&json!({"launchPoints": [
                {"id": "youtube.leanback.v4", "title": "YouTube"},
                {"id": "netflix", "title": "Netflix"},
                {"id": "com.wbd.stream", "title": "HBO Max"},
                {"id": "tv.solocoo.slsk", "title": "Skylink Live TV Application"},
                {"id": "voyo.cz", "title": "Oneplay"},
                {"id": "com.webos.app.browser", "title": "Web Browser"}]}))
            .unwrap()
            .0,
            inputs: webos::Inputs::read(&json!({"devices": [
                {"id": "HDMI_1", "label": "HDMI 1", "appId": "com.webos.app.hdmi1"},
                {"id": "HDMI_2", "label": "Game Console", "appId": "com.webos.app.hdmi2",
                 "hdmiPlugIn": true, "subList": [{"osdName": "Game Console X"}]}]}))
            .unwrap()
            .0,
        }
    }

    pub fn live() -> Model {
        let (mut model, _) = Model::start(start());
        model.update(Msg::Connected(Ok(Box::new(snapshot()))));
        model
    }

    pub fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    pub fn later(model: &mut Model, millis: i64) {
        let now = model.now + SignedDuration::from_millis(millis);
        model.update(Msg::Tick(now));
    }

    fn volume(level: u8, muted: bool) -> Change {
        Change::Volume(
            Volume::read(
                &json!({"volumeStatus": {"volume": level, "muteStatus": muted, "maxVolume": 100}}),
            )
            .unwrap(),
        )
    }

    fn power(state: &str) -> Change {
        Change::Power(PowerState::read(&json!({"state": state})).unwrap())
    }

    #[test]
    fn starting_connects_and_tells_what_it_found() {
        let (mut model, effects) = Model::start(start());
        assert_eq!(
            effects,
            [Effect::Connect {
                after: Duration::ZERO
            }]
        );
        assert_eq!(model.phase, Phase::Connecting);
        model.update(Msg::Connected(Ok(Box::new(snapshot()))));
        assert_eq!(model.phase, Phase::Live);
        assert_eq!(model.in_front(), "YouTube");
        assert_eq!(model.activity[0].text, "connected · 6 apps · 2 inputs");
    }

    #[test]
    fn keys_go_to_the_tv_as_buttons() {
        let mut model = live();
        assert_eq!(model.update(key(KeyCode::Up)), [Effect::Press(Button::UP)]);
        assert_eq!(
            model.update(key(KeyCode::Enter)),
            [Effect::Press(Button::ENTER)]
        );
        assert_eq!(
            model.update(key(KeyCode::Esc)),
            [Effect::Press(Button::BACK)]
        );
        assert_eq!(
            model.update(key(KeyCode::Char(' '))),
            [Effect::Press(Button::PAUSE)]
        );
        assert_eq!(
            model.update(key(KeyCode::Char(' '))),
            [Effect::Press(Button::PLAY)]
        );
        assert_eq!(
            Vec::from(model.sent.clone()),
            ["↑", "OK", "back", "pause", "play"]
        );
    }

    #[test]
    fn volume_steps_show_at_once_fold_into_one_line_and_wait_for_the_tv() {
        let mut model = live();
        assert_eq!(model.update(key(KeyCode::Char('+'))), [Effect::Volume(13)]);
        later(&mut model, 100);
        assert_eq!(model.update(key(KeyCode::Char('+'))), [Effect::Volume(14)]);
        assert_eq!(model.shown_volume(), Some((14, false)));
        assert_eq!(model.activity[0].text, "12 → 14");
        assert_eq!(model.activity[0].status, Status::Waiting);
        // A step on its way is not someone else's change.
        model.update(Msg::Changed(volume(13, false)));
        assert_eq!(model.activity.len(), 2);
        assert_eq!(model.shown_volume(), Some((14, false)));
        later(&mut model, 100);
        model.update(Msg::Changed(volume(14, false)));
        assert!(matches!(model.activity[0].status, Status::Done(Some(_))));
        assert_eq!(model.shown_volume(), Some((14, false)));
    }

    #[test]
    fn changes_from_elsewhere_are_told_as_such() {
        let mut model = live();
        model.update(Msg::Changed(volume(20, false)));
        model.update(Msg::Changed(Change::App("netflix".into())));
        model.update(Msg::Changed(power("Screen Off")));
        let lines: Vec<(&str, &str, Origin)> = model
            .activity
            .iter()
            .take(3)
            .map(|entry| (entry.what, entry.text.as_str(), entry.origin))
            .collect();
        assert_eq!(
            lines,
            [
                ("power", "on → screen off", Origin::Remote),
                ("app", "YouTube → Netflix", Origin::Remote),
                ("volume", "12 → 20", Origin::Remote),
            ]
        );
    }

    #[test]
    fn an_unreported_change_turns_into_a_warning() {
        let mut model = live();
        model.update(key(KeyCode::Char('m')));
        assert_eq!(model.shown_volume(), Some((12, true)));
        later(&mut model, 6000);
        assert!(matches!(model.activity[0].status, Status::Warn(_)));
        assert_eq!(model.shown_volume(), Some((12, false)));
    }

    #[test]
    fn pickers_filter_and_start_what_was_chosen() {
        let mut model = live();
        model.update(key(KeyCode::Char('a')));
        for c in "net".chars() {
            model.update(key(KeyCode::Char(c)));
        }
        let titles: Vec<String> = model.choices().into_iter().map(|c| c.title).collect();
        assert_eq!(titles, ["Netflix"]);
        let effects = model.update(key(KeyCode::Enter));
        assert!(matches!(&effects[..], [Effect::Launch { app, .. }] if app == "netflix"));
        assert_eq!(model.overlay, Overlay::None);
        later(&mut model, 900);
        model.update(Msg::Changed(Change::App("netflix".into())));
        assert_eq!(model.activity[0].text, "Netflix");
        assert!(
            matches!(model.activity[0].status, Status::Done(Some(took)) if took.as_millis() == 900)
        );

        model.update(key(KeyCode::Char('i')));
        model.update(key(KeyCode::Char('x')));
        let effects = model.update(key(KeyCode::Enter));
        assert!(matches!(&effects[..], [Effect::Input { input, .. }] if input == "HDMI_2"));
        model.update(Msg::Changed(Change::App("com.webos.app.hdmi2".into())));
        assert_eq!(model.in_front(), "HDMI 2 · Game Console");
    }

    #[test]
    fn power_off_takes_two_presses_and_wake_brings_the_tv_back() {
        let mut model = live();
        assert_eq!(model.update(key(KeyCode::Char('p'))), []);
        assert!(model.notice.is_some());
        let effects = model.update(key(KeyCode::Char('p')));
        assert!(matches!(&effects[..], [Effect::TurnOff { .. }]));
        later(&mut model, 2000);
        assert_eq!(model.update(Msg::Lost), [Effect::Connect { after: RETRY }]);
        assert!(matches!(model.phase, Phase::Off { .. }));
        assert!(matches!(model.activity[0].status, Status::Done(Some(_))));

        let effects = model.update(key(KeyCode::Char('p')));
        assert!(matches!(&effects[..], [Effect::Wake { .. }]));
        assert!(matches!(model.phase, Phase::Waking { .. }));
        assert_eq!(
            model.update(Msg::Woke(Ok(()))),
            [Effect::Connect {
                after: Duration::ZERO
            }]
        );
        later(&mut model, 40_000);
        model.update(Msg::Connected(Ok(Box::new(snapshot()))));
        assert_eq!(model.phase, Phase::Live);
        assert_eq!(model.activity[0].text, "Wake-on-LAN");
        assert!(
            matches!(model.activity[0].status, Status::Done(Some(took)) if took.as_secs() == 40)
        );
    }

    #[test]
    fn a_tv_that_goes_away_is_looked_for_again() {
        let mut model = live();
        model.update(Msg::Lost);
        assert_eq!(model.activity[0].text, "on → off");
        assert_eq!(model.activity[0].origin, Origin::Remote);
        assert_eq!(model.update(key(KeyCode::Up)), []);
        assert_eq!(
            model.notice.as_ref().unwrap().text,
            "the TV is off · p wakes it"
        );
        let effects = model.update(Msg::Connected(Err(Failure::Off {
            model: None,
            address: None,
        })));
        assert_eq!(effects, [Effect::Connect { after: RETRY }]);
        model.update(Msg::Connected(Ok(Box::new(snapshot()))));
        assert_eq!(model.activity[0].text, "off → on");
    }

    fn focus(field: Option<&str>, hidden: bool) -> Msg {
        Msg::Changed(Change::Keyboard(Keyboard {
            focused: field.is_some(),
            content_type: field.map(str::to_owned),
            hidden,
        }))
    }

    #[test]
    fn a_text_field_on_the_tv_opens_the_line_and_typing_goes_out_at_once() {
        let mut model = live();
        model.update(focus(Some("url"), false));
        assert!(
            matches!(&model.overlay, Overlay::Text(line) if line.field.as_deref() == Some("url"))
        );
        assert_eq!(
            model.update(key(KeyCode::Char('h'))),
            [Effect::Type("h".into())]
        );
        assert_eq!(
            model.update(key(KeyCode::Char('i'))),
            [Effect::Type("i".into())]
        );
        assert_eq!(model.update(key(KeyCode::Backspace)), [Effect::Delete]);
        model.update(key(KeyCode::Char('o')));
        // Letters go into the field, not to telka's keys.
        assert!(matches!(&model.overlay, Overlay::Text(line) if line.text == "ho"));
        assert_eq!(model.update(key(KeyCode::Enter)), [Effect::Enter]);
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.activity[0].text, "\"ho\"");

        model.update(focus(Some("text"), true));
        model.update(key(KeyCode::Char('x')));
        model.update(focus(None, false));
        assert_eq!(model.overlay, Overlay::None, "the field lost focus");
        assert_eq!(model.activity[0].text, "\"•\"", "a password stays hidden");

        model.update(key(KeyCode::Char('t')));
        assert!(matches!(&model.overlay, Overlay::Text(line) if line.field.is_none()));
        model.update(key(KeyCode::Esc));
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn the_mouse_moves_the_pointer_only_while_the_tv_is_on() {
        let mut model = live();
        let effects = model.update(Msg::Pointer(Pointer::Move { dx: 24, dy: -12 }));
        assert_eq!(
            effects,
            [Effect::Pointer(Pointer::Move { dx: 24, dy: -12 })]
        );
        assert_eq!(model.pointer.as_deref(), Some("move  dx +24  dy -12"));
        model.update(Msg::Pointer(Pointer::Click));
        assert_eq!(model.pointer.as_deref(), Some("click"));
        model.update(Msg::Lost);
        assert_eq!(model.update(Msg::Pointer(Pointer::Click)), []);
    }

    #[test]
    fn any_button_can_be_picked_by_name() {
        let mut model = live();
        model.update(key(KeyCode::Char('k')));
        for c in "info".chars() {
            model.update(key(KeyCode::Char(c)));
        }
        assert_eq!(
            model.update(key(KeyCode::Enter)),
            [Effect::Press(Button::INFO)]
        );
        assert_eq!(model.sent.back(), Some(&"INFO"));
    }

    #[test]
    fn a_missing_or_refused_pairing_opens_the_dialog() {
        let (mut model, _) = Model::start(start());
        assert_eq!(
            model.update(Msg::Connected(Err(Failure::NotPaired))),
            [Effect::Pair]
        );
        model.update(Msg::Pairing(PairingMsg::Prompt {
            address: "192.168.1.30".parse().unwrap(),
            signed: true,
        }));
        assert!(matches!(model.phase, Phase::Pairing(Dialog::Prompt { .. })));
        assert_eq!(model.update(key(KeyCode::Esc)), [Effect::CancelPairing]);
        assert_eq!(model.update(key(KeyCode::Enter)), [Effect::Pair]);
        model.update(Msg::Pairing(PairingMsg::Done));
        assert_eq!(model.phase, Phase::Connecting);

        let (mut model, _) = Model::start(start());
        model.update(Msg::Connected(Err(Failure::PairAgain(
            "the TV no longer accepts telka's key".into(),
        ))));
        assert!(matches!(
            &model.phase,
            Phase::Pairing(Dialog::Searching { reason: Some(_) })
        ));
    }

    fn picture(light: u8, energy: &str) -> Msg {
        Msg::Changed(Change::Picture(
            Picture::read(&json!({"settings": {"backlight": light.to_string(),
                "energySaving": energy}}))
            .unwrap(),
        ))
    }

    #[test]
    fn light_steps_fold_into_one_line_and_are_read_back() {
        let mut model = live();
        let effects = model.update(key(KeyCode::Char(']')));
        let entry = model.activity[0].id;
        let light = |level: &str| Effect::Picture {
            key: "backlight",
            value: level.into(),
            entry,
        };
        assert_eq!(effects, [light("85")]);
        later(&mut model, 100);
        assert_eq!(model.update(key(KeyCode::Char(']'))), [light("90")]);
        assert_eq!(model.shown_light(), Some(90));
        assert_eq!(model.activity[0].text, "OLED light 80 → 90");
        // The alert took it; the TV shows it a moment later.
        model.update(Msg::Done {
            entry,
            result: Ok(()),
        });
        assert_eq!(model.activity[0].status, Status::Waiting);
        model.update(picture(85, "auto"));
        assert_eq!(model.activity[0].status, Status::Waiting);
        model.update(picture(90, "auto"));
        assert!(matches!(model.activity[0].status, Status::Done(Some(_))));

        // Not below 0.
        model
            .tv
            .as_mut()
            .unwrap()
            .picture
            .as_mut()
            .unwrap()
            .backlight = Some(3);
        let effects = model.update(key(KeyCode::Char('[')));
        assert!(matches!(&effects[..], [Effect::Picture { value, .. }] if value == "0"));
    }

    #[test]
    fn energy_saving_goes_round_and_waits_for_the_tv() {
        let mut model = live();
        let effects = model.update(key(KeyCode::Char('e')));
        let entry = model.activity[0].id;
        assert_eq!(
            effects,
            [Effect::Picture {
                key: "energySaving",
                value: "off".into(),
                entry
            }]
        );
        model.update(Msg::Done {
            entry,
            result: Ok(()),
        });
        assert_eq!(model.activity[0].status, Status::Waiting);
        model.update(picture(80, "off"));
        assert!(matches!(model.activity[0].status, Status::Done(Some(_))));
        model
            .tv
            .as_mut()
            .unwrap()
            .picture
            .as_mut()
            .unwrap()
            .energy_saving = Some("max".into());
        let effects = model.update(key(KeyCode::Char('e')));
        assert!(matches!(&effects[..], [Effect::Picture { value, .. }] if value == "auto"));

        // Quick presses go on from the level on its way, in one line.
        let mut model = live();
        model.update(key(KeyCode::Char('e')));
        let entry = model.activity[0].id;
        let effects = model.update(key(KeyCode::Char('e')));
        assert_eq!(
            effects,
            [Effect::Picture {
                key: "energySaving",
                value: "min".into(),
                entry
            }]
        );
        assert_eq!(model.activity[0].text, "energy saving min");
        assert_eq!(model.shown_energy().as_deref(), Some("min"));
    }

    #[test]
    fn the_picture_mode_is_picked_and_shown_as_set_by_telka() {
        let mut model = live();
        model.update(key(KeyCode::Char('v')));
        for c in "cin".chars() {
            model.update(key(KeyCode::Char(c)));
        }
        let effects = model.update(key(KeyCode::Enter));
        let entry = model.activity[0].id;
        assert_eq!(
            effects,
            [Effect::Picture {
                key: "pictureMode",
                value: "cinema".into(),
                entry
            }]
        );
        assert_eq!(model.picture_mode.as_deref(), Some("cinema"));
        assert_eq!(model.activity[0].text, "picture mode cinema");
        // Nothing to read back: done once the TV took it.
        later(&mut model, 400);
        model.update(Msg::Done {
            entry,
            result: Ok(()),
        });
        assert!(
            matches!(model.activity[0].status, Status::Done(Some(took)) if took.as_millis() == 400)
        );
        model.update(key(KeyCode::Char('v')));
        let current: Vec<String> = model
            .choices()
            .into_iter()
            .filter(|choice| choice.current)
            .map(|choice| choice.id)
            .collect();
        assert_eq!(current, ["cinema"]);
    }

    #[test]
    fn picture_keys_need_the_signed_pairing_and_a_stuck_alert_warns() {
        let (mut model, _) = Model::start(start());
        let mut unsigned = snapshot();
        unsigned.signed = false;
        model.update(Msg::Connected(Ok(Box::new(unsigned))));
        for c in ['[', ']', 'e', 'v'] {
            assert_eq!(model.update(key(KeyCode::Char(c))), []);
        }
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.notice.as_ref().unwrap().text.contains("signed"));

        let mut model = live();
        model.update(key(KeyCode::Char(']')));
        let entry = model.activity[0].id;
        later(&mut model, 3000);
        model.update(Msg::Stuck { entry });
        let stuck = |model: &Model| matches!(&model.activity[0].status, Status::Warn(why) if why.contains("pressed OK"));
        assert!(stuck(&model));
        // OK makes the change: the TV shows it a moment later, and the warning stays.
        later(&mut model, 2000);
        assert_eq!(model.shown_light(), Some(85));
        model.update(picture(85, "auto"));
        assert!(stuck(&model));
        assert!(!model.spinning());
    }

    #[test]
    fn a_pressed_button_lights_up_and_a_change_from_elsewhere_flashes() {
        let mut model = live();
        model.update(key(KeyCode::Char('h')));
        assert!(model.lit(Action::Press(Button::HOME)));
        assert!(!model.lit(Action::Press(Button::BACK)));
        assert!(model.animating());
        later(&mut model, 500);
        assert!(!model.lit(Action::Press(Button::HOME)));
        assert!(!model.animating());

        model.update(Msg::Changed(Change::App("netflix".into())));
        assert!(model.flashing(&model.activity[0]));
        assert!(model.animating());
        later(&mut model, 1500);
        assert!(!model.flashing(&model.activity[0]));
        assert!(!model.animating());
    }

    #[test]
    fn bars_run_to_a_new_value() {
        let mut model = live();
        assert_eq!(model.volume_bar.unwrap().at(model.now), 12);
        model.update(Msg::Changed(volume(32, false)));
        assert_eq!(model.volume_bar.unwrap().at(model.now), 12);
        assert!(model.animating());
        later(&mut model, 150);
        let halfway = model.volume_bar.unwrap().at(model.now);
        assert!((20..32).contains(&halfway), "{halfway}");
        later(&mut model, 200);
        assert_eq!(model.volume_bar.unwrap().at(model.now), 32);
        // Still the flash of the change from elsewhere.
        assert!(model.animating());
        later(&mut model, 1200);
        assert!(!model.animating());
    }

    #[test]
    fn without_animations_nothing_moves() {
        let (mut model, _) = Model::start(Start {
            animations: false,
            ..start()
        });
        model.update(Msg::Connected(Ok(Box::new(snapshot()))));
        model.update(key(KeyCode::Char('h')));
        model.update(Msg::Changed(volume(32, false)));
        assert!(!model.lit(Action::Press(Button::HOME)));
        assert!(!model.flashing(&model.activity[0]));
        assert_eq!(model.volume_bar.unwrap().at(model.now), 32);
        assert!(!model.animating());
    }
}
