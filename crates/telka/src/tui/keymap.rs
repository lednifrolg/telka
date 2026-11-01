//! The TUI's keys: one flat table that also drives the help overlay and the hint bar.
//!
//! The keyboard is the remote: arrows, Enter and Backspace go to the TV, letters are telka's
//! own actions. There is no focus to move and no mode, only the pickers, the help and the
//! pairing dialog, which take the keys while they are open.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use webos::Button;

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A button of the remote, sent to the TV as it is.
    Press(Button),
    /// Pause, then play, then pause: the TV does not say what plays.
    PlayPause,
    VolumeUp,
    VolumeDown,
    Mute,
    Apps,
    Inputs,
    /// Every other button of the remote, picked by name.
    Buttons,
    /// The line that types into the TV.
    Type,
    /// The panel off or on; the sound stays.
    Screen,
    /// Wakes a TV that is off; turns it off on the second press.
    Power,
    /// Reads everything from the TV again.
    Resync,
    Help,
    Quit,
    NextTab,
    /// Jump to the tab with this index.
    Tab(usize),
    /// Escape: closes what is open, else the remote's back button.
    Back,
}

/// When a binding is shown in the hint bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Always,
    /// While the TV is on: everything that goes to it.
    Live,
    /// Only in the narrow layout, where the panes are tabs.
    Tabs,
    Never,
}

pub struct Binding {
    /// The keys as the help shows them; empty for a key that shares the line above.
    pub keys: &'static str,
    /// The keys as the hint bar shows them.
    pub short: &'static str,
    pub help: &'static str,
    /// Short label for the hint bar.
    pub label: &'static str,
    pub hint: Hint,
    /// Lower stays longer in the hint bar when the window is narrow.
    pub rank: u8,
    codes: &'static [KeyCode],
    action: Action,
}

/// A second key for the action described on the line above.
const fn also(codes: &'static [KeyCode], action: Action) -> Binding {
    Binding {
        keys: "",
        short: "",
        help: "",
        label: "",
        hint: Hint::Never,
        rank: u8::MAX,
        codes,
        action,
    }
}

use KeyCode::Char;

pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: "←↑↓→",
        short: "←↑↓→",
        help: "the remote's arrows",
        label: "d-pad",
        hint: Hint::Live,
        rank: 2,
        codes: &[KeyCode::Up],
        action: Action::Press(Button::UP),
    },
    also(&[KeyCode::Down], Action::Press(Button::DOWN)),
    also(&[KeyCode::Left], Action::Press(Button::LEFT)),
    also(&[KeyCode::Right], Action::Press(Button::RIGHT)),
    Binding {
        keys: "⏎",
        short: "⏎",
        help: "OK",
        label: "ok",
        hint: Hint::Live,
        rank: 3,
        codes: &[KeyCode::Enter],
        action: Action::Press(Button::ENTER),
    },
    Binding {
        keys: "⌫ Esc",
        short: "⌫",
        help: "back (Esc closes what is open first)",
        label: "back",
        hint: Hint::Live,
        rank: 4,
        codes: &[KeyCode::Backspace],
        action: Action::Press(Button::BACK),
    },
    also(&[KeyCode::Esc], Action::Back),
    Binding {
        keys: "h  s",
        short: "h",
        help: "home, the TV's settings",
        label: "home",
        hint: Hint::Live,
        rank: 5,
        codes: &[Char('h')],
        action: Action::Press(Button::HOME),
    },
    also(&[Char('s')], Action::Press(Button::MENU)),
    Binding {
        keys: "␣  , .",
        short: "␣",
        help: "pause and play in turn, rewind, fast forward",
        label: "play/pause",
        hint: Hint::Live,
        rank: 10,
        codes: &[Char(' ')],
        action: Action::PlayPause,
    },
    also(&[Char(',')], Action::Press(Button::REWIND)),
    also(&[Char('.')], Action::Press(Button::FAST_FORWARD)),
    Binding {
        keys: "+ -  m",
        short: "+-",
        help: "volume, mute",
        label: "volume",
        hint: Hint::Live,
        rank: 7,
        codes: &[Char('+'), Char('=')],
        action: Action::VolumeUp,
    },
    also(&[Char('-')], Action::VolumeDown),
    Binding {
        keys: "",
        short: "m",
        help: "",
        label: "mute",
        hint: Hint::Live,
        rank: 9,
        codes: &[Char('m')],
        action: Action::Mute,
    },
    Binding {
        keys: "a  i",
        short: "a",
        help: "pick an app or an input",
        label: "apps",
        hint: Hint::Live,
        rank: 6,
        codes: &[Char('a')],
        action: Action::Apps,
    },
    Binding {
        keys: "",
        short: "i",
        help: "",
        label: "inputs",
        hint: Hint::Live,
        rank: 8,
        codes: &[Char('i')],
        action: Action::Inputs,
    },
    Binding {
        keys: "k",
        short: "k",
        help: "any other button of the remote: info, guide, colors, numbers, …",
        label: "buttons",
        hint: Hint::Never,
        rank: u8::MAX,
        codes: &[Char('k')],
        action: Action::Buttons,
    },
    Binding {
        keys: "t",
        short: "t",
        help: "type into the TV; opens on its own when a text field has focus",
        label: "type",
        hint: Hint::Never,
        rank: u8::MAX,
        codes: &[Char('t')],
        action: Action::Type,
    },
    Binding {
        keys: "d",
        short: "d",
        help: "screen off or on; the sound stays",
        label: "screen",
        hint: Hint::Never,
        rank: u8::MAX,
        codes: &[Char('d')],
        action: Action::Screen,
    },
    Binding {
        keys: "p",
        short: "p",
        help: "wake the TV; when it is on, twice to turn it off",
        label: "power",
        hint: Hint::Never,
        rank: u8::MAX,
        codes: &[Char('p')],
        action: Action::Power,
    },
    Binding {
        keys: "Tab 1 2",
        short: "Tab",
        help: "the TV or the activity, in a narrow window",
        label: "tab",
        hint: Hint::Tabs,
        rank: 11,
        codes: &[KeyCode::Tab],
        action: Action::NextTab,
    },
    also(&[Char('1')], Action::Tab(0)),
    also(&[Char('2')], Action::Tab(1)),
    Binding {
        keys: "R",
        short: "R",
        help: "read everything from the TV again",
        label: "reload",
        hint: Hint::Never,
        rank: u8::MAX,
        codes: &[Char('R')],
        action: Action::Resync,
    },
    Binding {
        keys: "?",
        short: "?",
        help: "these keys",
        label: "help",
        hint: Hint::Always,
        rank: 1,
        codes: &[Char('?')],
        action: Action::Help,
    },
    Binding {
        keys: "q",
        short: "q",
        help: "quit",
        label: "quit",
        hint: Hint::Always,
        rank: 0,
        codes: &[Char('q')],
        action: Action::Quit,
    },
];

/// What `key` does, if anything. Shift only matters for letters, which carry it in their case.
pub fn action(key: KeyEvent) -> Option<Action> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return (key.code == Char('c') && key.modifiers == KeyModifiers::CONTROL)
            .then_some(Action::Quit);
    }
    BINDINGS
        .iter()
        .find(|binding| binding.codes.contains(&key.code))
        .map(|binding| binding.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Option<Action> {
        action(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn keys_map_to_the_remote_and_to_telka() {
        assert_eq!(press(KeyCode::Up), Some(Action::Press(Button::UP)));
        assert_eq!(press(KeyCode::Enter), Some(Action::Press(Button::ENTER)));
        assert_eq!(press(KeyCode::Backspace), Some(Action::Press(Button::BACK)));
        assert_eq!(press(KeyCode::Esc), Some(Action::Back));
        assert_eq!(press(Char('h')), Some(Action::Press(Button::HOME)));
        assert_eq!(press(Char('=')), Some(Action::VolumeUp));
        assert_eq!(press(Char('R')), Some(Action::Resync));
        assert_eq!(press(Char('r')), None);
        assert_eq!(press(Char('j')), None, "no hjkl (D8)");
        let ctrl_c = KeyEvent::new(Char('c'), KeyModifiers::CONTROL);
        assert_eq!(action(ctrl_c), Some(Action::Quit));
        let alt_h = KeyEvent::new(Char('h'), KeyModifiers::ALT);
        assert_eq!(action(alt_h), None);
    }

    #[test]
    fn no_key_is_bound_twice() {
        let mut codes: Vec<String> = BINDINGS
            .iter()
            .flat_map(|binding| binding.codes.iter().map(|code| format!("{code:?}")))
            .collect();
        let all = codes.len();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all);
    }
}
