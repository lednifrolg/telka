//! Volume, mute and where the sound goes.

use serde_json::{Value, json};

use super::{Report, missing};
use crate::client::Client;
use crate::error::Error;

/// The volume, as `audio/getVolume` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub level: u8,
    pub max: u8,
    pub muted: bool,
    /// Whether the TV sets the volume itself; not with some external speakers.
    pub adjustable: bool,
    /// Where the sound goes, like `tv_speaker`, when the answer says.
    pub output: Option<String>,
}

impl Report for Volume {
    const URI: &'static str = "audio/getVolume";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        // webOS 6 nests the values in `volumeStatus`; older and newer versions answer flat.
        let status = payload.get("volumeStatus").unwrap_or(payload);
        let level = status["volume"].as_u64().ok_or_else(|| missing("volume"))?;
        let muted = status["muteStatus"]
            .as_bool()
            .or_else(|| status["mute"].as_bool())
            .unwrap_or(false);
        let clamp = |value: u64| u8::try_from(value).unwrap_or(u8::MAX);
        Ok(Self {
            level: clamp(level),
            max: clamp(status["maxVolume"].as_u64().unwrap_or(100)),
            muted,
            adjustable: status["adjustVolume"].as_bool().unwrap_or(true),
            output: status["soundOutput"].as_str().map(str::to_owned),
        })
    }
}

/// Where the sound goes: `tv_speaker`, `external_arc`, `external_optical`, `bt_soundbar`,
/// `headphone`, `lineout`, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundOutput(pub String);

impl Report for SoundOutput {
    const URI: &'static str = "com.webos.service.apiadapter/audio/getSoundOutput";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        payload["soundOutput"]
            .as_str()
            .map(|output| Self(output.to_owned()))
            .ok_or_else(|| missing("soundOutput"))
    }
}

impl Client {
    pub async fn set_volume(&self, level: u8) -> Result<(), Error> {
        self.request("audio/setVolume", json!({"volume": level}))
            .await?;
        Ok(())
    }

    /// One step up, as the remote does. Steps sent too fast get lost over HDMI ARC.
    pub async fn volume_up(&self) -> Result<(), Error> {
        self.request("audio/volumeUp", json!({})).await?;
        Ok(())
    }

    pub async fn volume_down(&self) -> Result<(), Error> {
        self.request("audio/volumeDown", json!({})).await?;
        Ok(())
    }

    pub async fn set_mute(&self, muted: bool) -> Result<(), Error> {
        self.request("audio/setMute", json!({"mute": muted}))
            .await?;
        Ok(())
    }

    pub async fn set_sound_output(&self, output: &str) -> Result<(), Error> {
        self.request(
            "com.webos.service.apiadapter/audio/changeSoundOutput",
            json!({"output": output}),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_reads_nested_and_flat() {
        let nested = Volume::read(
            &json!({"returnValue": true, "callerId": "x", "volumeStatus": {
            "activeStatus": true, "adjustVolume": true, "maxVolume": 100, "muteStatus": false,
            "volume": 12, "mode": "normal", "soundOutput": "tv_speaker"}}),
        )
        .unwrap();
        assert_eq!(
            nested,
            Volume {
                level: 12,
                max: 100,
                muted: false,
                adjustable: true,
                output: Some("tv_speaker".into())
            }
        );
        let flat = Volume::read(&json!({"volume": 30, "mute": true})).unwrap();
        assert_eq!((flat.level, flat.muted, flat.max), (30, true, 100));
        assert!(Volume::read(&json!({"volumeStatus": {}})).is_err());
        assert_eq!(
            SoundOutput::read(&json!({"soundOutput": "external_arc"})).unwrap(),
            SoundOutput("external_arc".into())
        );
    }
}
