//! The TV's external inputs and switching between them.

use serde_json::{Value, json};

use super::{Report, missing};
use crate::client::Client;
use crate::error::Error;

/// An HDMI or other input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// Like `HDMI_3`.
    pub id: String,
    /// The name set on the TV, like `HDMI 3` or `PS5 Game Console`.
    pub label: String,
    /// The app that shows it, like `com.webos.app.hdmi3`: it is the foreground app while the
    /// input is on screen.
    pub app_id: String,
    /// A cable with power on the other end.
    pub plugged: bool,
    /// The TV currently sees a signal on it.
    pub connected: bool,
    /// Names of the devices on it that announce themselves over HDMI-CEC, like `PlayStation 5`.
    pub devices: Vec<String>,
}

/// The inputs, in the TV's order: `tv/getExternalInputList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs(pub Vec<Input>);

impl Report for Inputs {
    const URI: &'static str = "tv/getExternalInputList";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let devices = payload["devices"]
            .as_array()
            .ok_or_else(|| missing("devices"))?;
        let inputs = devices
            .iter()
            .filter_map(|device| {
                let id = device["id"].as_str()?.to_owned();
                let flag = |key: &str| device[key].as_bool().unwrap_or(false);
                let cec = device["subList"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|sub| sub["osdName"].as_str())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect();
                Some(Input {
                    label: device["label"].as_str().unwrap_or(&id).to_owned(),
                    app_id: device["appId"].as_str().unwrap_or_default().to_owned(),
                    plugged: flag("hdmiPlugIn"),
                    connected: flag("connected"),
                    devices: cec,
                    id,
                })
            })
            .collect();
        Ok(Self(inputs))
    }
}

impl Client {
    /// Switches to the input `id`, like `HDMI_3`.
    pub async fn switch_input(&self, id: &str) -> Result<(), Error> {
        self.request("tv/switchInput", json!({"inputId": id}))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_carry_their_cec_devices() {
        let inputs = Inputs::read(&json!({"returnValue": true, "devices": [
            {"id": "HDMI_1", "label": "HDMI 1", "port": 1, "connected": false, "appId": "com.webos.app.hdmi1",
             "hdmiPlugIn": false, "subList": [], "subCount": 0},
            {"id": "HDMI_3", "label": "Game Console", "port": 3, "connected": false, "appId": "com.webos.app.hdmi3",
             "hdmiPlugIn": true, "subCount": 2, "subList": [
                {"id": "SIMPLINK", "osdName": "Game Console X", "portId": 3},
                {"id": "URCU", "brandName": "X", "labelName": "X"}]}]}))
        .unwrap();
        assert_eq!(inputs.0.len(), 2);
        let console = &inputs.0[1];
        assert_eq!(console.label, "Game Console");
        assert_eq!(console.app_id, "com.webos.app.hdmi3");
        assert!(console.plugged && !console.connected);
        assert_eq!(console.devices, ["Game Console X"]);
        assert_eq!(inputs.0[0].devices, Vec::<String>::new());
    }
}
