//! System settings: reading the few the TV shares over SSAP, and changing settings through an
//! alert.
//!
//! `settings/getSystemSettings` answers only for an allowlist of keys per category, and one key
//! outside it fails the whole request. On a C1 the picture's `backlight` (the OLED light),
//! `contrast`, `brightness`, `color` and `energySaving` are on it; the picture mode, eye comfort
//! and the sleep timer are not. Values come back as strings.
//!
//! Writing goes around SSAP: an alert whose close action is a call to the TV's internal Luna
//! bus, which the notification service makes with its own rights ([`Client::luna`]). That needs
//! the `WRITE_NOTIFICATION_ALERT` permission of LG's signed manifest and returns nothing. On a C1
//! the alert never showed, and picture settings changed as asked; the sleep timer in category
//! `time` did not, although the call went through.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::timeout;

use super::{Report, missing, text};
use crate::client::Client;
use crate::error::Error;

/// The readable picture settings, as numbers where they are numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// OLED light on an OLED TV, backlight on others: 0 to 100.
    pub backlight: Option<u8>,
    pub contrast: Option<u8>,
    pub brightness: Option<u8>,
    pub color: Option<u8>,
    /// `auto`, `off`, `min`, `med`, `max` or `screen_off`.
    pub energy_saving: Option<String>,
}

fn settings(payload: &Value) -> Result<&Value, serde_json::Error> {
    let settings = &payload["settings"];
    if settings.is_object() {
        Ok(settings)
    } else {
        Err(missing("settings"))
    }
}

fn number(value: &Value) -> Option<u8> {
    text(value)?.trim().parse().ok()
}

impl Report for Picture {
    const URI: &'static str = "settings/getSystemSettings";

    fn payload() -> Value {
        json!({"category": "picture", "keys": ["backlight", "contrast", "brightness", "color", "energySaving"]})
    }

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let settings = settings(payload)?;
        Ok(Self {
            backlight: number(&settings["backlight"]),
            contrast: number(&settings["contrast"]),
            brightness: number(&settings["brightness"]),
            color: number(&settings["color"]),
            energy_saving: text(&settings["energySaving"]),
        })
    }
}

/// The readable sound settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundSettings {
    /// Like `standard`, `movie`, `music`, `aiSoundPlus`.
    pub mode: Option<String>,
    pub output: Option<String>,
}

impl Report for SoundSettings {
    const URI: &'static str = "settings/getSystemSettings";

    fn payload() -> Value {
        json!({"category": "sound", "keys": ["soundMode", "soundOutput"]})
    }

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let settings = settings(payload)?;
        Ok(Self {
            mode: text(&settings["soundMode"]),
            output: text(&settings["soundOutput"]),
        })
    }
}

/// Whether the TV lets Wake-on-LAN turn it on: "Turn on via Wi-Fi" in its settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeOnLan(pub bool);

impl Report for WakeOnLan {
    const URI: &'static str = "settings/getSystemSettings";

    fn payload() -> Value {
        json!({"category": "network", "keys": ["wolwowlOnOff"]})
    }

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let flag =
            text(&settings(payload)?["wolwowlOnOff"]).ok_or_else(|| missing("wolwowlOnOff"))?;
        Ok(Self(flag == "true"))
    }
}

/// How long closing the alert may take before it counts as stuck on screen.
const ALERT_CLOSE: Duration = Duration::from_secs(3);

impl Client {
    /// Calls `uri` on the TV's internal Luna bus, like
    /// `com.webos.settingsservice/setSystemSettings`, through an alert that is closed at once.
    /// The call's answer does not come back. [`Error::TimedOut`] means the alert may still be on
    /// screen; OK on the remote closes it.
    pub async fn luna(&self, uri: &str, params: Value) -> Result<(), Error> {
        let target = if uri.contains("://") {
            uri.to_owned()
        } else {
            format!("luna://{uri}")
        };
        let alert = self
            .request(
                "system.notifications/createAlert",
                json!({
                    "message": " ",
                    "buttons": [{"label": "", "onClick": target, "params": params}],
                    "onclose": {"uri": target, "params": params},
                    "onfail": {"uri": target, "params": params},
                }),
            )
            .await?;
        let id = alert["alertId"]
            .as_str()
            .ok_or_else(|| Error::Protocol("the TV created an alert without an id".into()))?;
        timeout(
            ALERT_CLOSE.min(self.options().timing.request),
            self.request("system.notifications/closeAlert", json!({"alertId": id})),
        )
        .await
        .map_err(|_| Error::TimedOut)??;
        Ok(())
    }

    /// Changes settings in `category` through [`Client::luna`], e.g. `picture` with
    /// `{"backlight": "70"}`.
    pub async fn set_system_settings(&self, category: &str, settings: Value) -> Result<(), Error> {
        self.luna(
            "com.webos.settingsservice/setSystemSettings",
            json!({"category": category, "settings": settings}),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_from_strings() {
        let picture = Picture::read(&json!({"returnValue": true, "category": "picture", "settings": {
            "backlight": "80", "contrast": "100", "brightness": "50", "color": "50", "energySaving": "auto"}}))
        .unwrap();
        assert_eq!(picture.backlight, Some(80));
        assert_eq!(picture.contrast, Some(100));
        assert_eq!(picture.energy_saving.as_deref(), Some("auto"));
        let partial = Picture::read(&json!({"settings": {"backlight": 75}})).unwrap();
        assert_eq!((partial.backlight, partial.color), (Some(75), None));
        assert!(Picture::read(&json!({"returnValue": true})).is_err());
        let sound = SoundSettings::read(
            &json!({"settings": {"soundMode": "movie", "soundOutput": "tv_speaker"}}),
        )
        .unwrap();
        assert_eq!(sound.mode.as_deref(), Some("movie"));
        assert_eq!(
            WakeOnLan::read(&json!({"settings": {"wolwowlOnOff": "true"}})).unwrap(),
            WakeOnLan(true)
        );
        assert_eq!(
            WakeOnLan::read(&json!({"settings": {"wolwowlOnOff": "false"}})).unwrap(),
            WakeOnLan(false)
        );
        assert!(WakeOnLan::read(&json!({"settings": {}})).is_err());
    }
}
