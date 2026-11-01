//! Power: what state the TV is in, turning it off, and the screen alone.
//!
//! An SSAP request cannot turn a TV on, since it closes its sockets when it sleeps; that takes
//! Wake-on-LAN, see [`crate::wake_and_wait`]. And Wake-on-LAN works only after the TV went off
//! the way its remote turns it off: on a C1, after `system/turnOff` no magic packet woke it, after
//! the power button every time.

use serde_json::{Value, json};

use super::{Report, missing};
use crate::client::Client;
use crate::error::Error;
use crate::pointer::Button;

/// The TV's power state, as `com.webos.service.tvpower/power/getPowerState` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerState {
    pub mode: PowerMode,
    /// A change under way, like `Request Power Off` or `Screen On`.
    pub processing: Option<String>,
    /// Why the TV last turned on, when it says.
    pub power_on_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerMode {
    Active,
    /// On, with the panel dark: sound keeps playing.
    ScreenOff,
    ScreenSaver,
    /// Off to the user, but awake, e.g. while it refreshes the OLED panel.
    ActiveStandby,
    Suspend,
    PowerOff,
    Other(String),
}

impl PowerState {
    /// Whether the TV is on for someone in front of it, also with the screen off. A TV on its
    /// way off counts as off.
    pub fn is_on(&self) -> bool {
        let leaving = self
            .processing
            .as_deref()
            .is_some_and(|step| step.contains("Power Off") || step.contains("Suspend"));
        !leaving
            && matches!(
                self.mode,
                PowerMode::Active | PowerMode::ScreenOff | PowerMode::ScreenSaver
            )
    }

    /// Whether the TV is on and shows a picture.
    pub fn is_screen_on(&self) -> bool {
        self.is_on() && self.mode != PowerMode::ScreenOff
    }
}

impl Report for PowerState {
    const URI: &'static str = "com.webos.service.tvpower/power/getPowerState";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let mode = match payload["state"].as_str().ok_or_else(|| missing("state"))? {
            "Active" => PowerMode::Active,
            "Screen Off" => PowerMode::ScreenOff,
            "Screen Saver" => PowerMode::ScreenSaver,
            "Active Standby" => PowerMode::ActiveStandby,
            "Suspend" => PowerMode::Suspend,
            "Power Off" => PowerMode::PowerOff,
            other => PowerMode::Other(other.to_owned()),
        };
        let text = |key: &str| payload[key].as_str().map(str::to_owned);
        Ok(Self {
            mode,
            processing: text("processing"),
            power_on_reason: text("powerOnReason"),
        })
    }
}

impl Client {
    /// Turns the TV off with the power button, so that Wake-on-LAN can turn it on again. Sends
    /// nothing and returns `false` when the TV is not on: the button toggles, and would turn a TV
    /// in Active Standby on.
    pub async fn turn_off(&self) -> Result<bool, Error> {
        if !self.get::<PowerState>().await?.is_on() {
            return Ok(false);
        }
        let remote = self.remote().await?;
        remote.button(Button::POWER).await?;
        remote.close().await;
        Ok(true)
    }

    /// Turns the panel off or back on; the TV stays on and sound keeps playing.
    pub async fn set_screen(&self, on: bool) -> Result<(), Error> {
        let uri = if on {
            "com.webos.service.tvpower/power/turnOnScreen"
        } else {
            "com.webos.service.tvpower/power/turnOffScreen"
        };
        // "passive" is never sent: the screen cannot be turned back on after it.
        self.request(uri, json!({"standbyMode": "active"})).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(payload: &Value) -> PowerState {
        PowerState::read(payload).unwrap()
    }

    #[test]
    fn power_states_read_and_tell_on_from_off() {
        let on = state(&json!({"returnValue": true, "state": "Active"}));
        assert_eq!(on.mode, PowerMode::Active);
        assert!(on.is_on() && on.is_screen_on());
        let dark = state(&json!({"state": "Screen Off"}));
        assert!(dark.is_on() && !dark.is_screen_on());
        let leaving = state(&json!({"state": "Active", "processing": "Request Power Off"}));
        assert!(!leaving.is_on());
        for off in ["Active Standby", "Suspend", "Power Off"] {
            assert!(!state(&json!({"state": off})).is_on(), "{off}");
        }
        let odd = state(&json!({"state": "Warm Standby", "powerOnReason": "remote"}));
        assert_eq!(odd.mode, PowerMode::Other("Warm Standby".into()));
        assert_eq!(odd.power_on_reason.as_deref(), Some("remote"));
        assert!(PowerState::read(&json!({"returnValue": true})).is_err());
    }
}
