//! A fake TV: SSAP over plain WebSocket, with enough state for telka's commands.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, accept_async};

pub const UUID: &str = "4f1c2a7e-0b3d-4e6a-9c58-2d7f1e0a6b93";
pub const KEY: &str = "5a8e3c1f9d2b4a6e7c0f1b3d5e7a9c2f";
pub const MODEL: &str = "OLED55C16LA";
pub const SERIAL: &str = "101MAXX00001";
pub const MAC: &str = "02:00:5e:10:00:01";

/// What the prompt on the TV gets from the person with the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    Accept,
    Decline,
}

#[derive(Debug)]
pub struct State {
    /// When off, every connection closes at once.
    pub on: bool,
    pub uuid: String,
    pub key: String,
    pub prompt: Prompt,
    pub power: String,
    pub app: String,
    pub volume: u8,
    pub muted: bool,
    pub output: String,
    pub sound_mode: String,
    pub backlight: u8,
    pub energy: String,
    pub wake_on_lan: bool,
    /// Every frame telka sent, in order.
    pub frames: Vec<Value>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            on: true,
            uuid: UUID.into(),
            key: KEY.into(),
            prompt: Prompt::Accept,
            power: "Active".into(),
            app: "netflix".into(),
            volume: 12,
            muted: false,
            output: "tv_speaker".into(),
            sound_mode: "movie".into(),
            backlight: 80,
            energy: "auto".into(),
            wake_on_lan: true,
            frames: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct FakeTv {
    pub address: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl FakeTv {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tv = Self {
            address: listener.local_addr().unwrap(),
            state: Arc::default(),
        };
        let serving = tv.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                tokio::spawn(serving.clone().serve(tcp));
            }
        });
        tv
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// The frames telka sent of `kind`, like `request`.
    pub fn frames(&self, kind: &str) -> Vec<Value> {
        self.state()
            .frames
            .iter()
            .filter(|frame| frame["type"] == kind)
            .cloned()
            .collect()
    }

    /// The pairing `telka pair` leaves, under `home`.
    pub fn write_pairing(&self, home: &Path) {
        let dir = home.join("state");
        std::fs::create_dir_all(&dir).unwrap();
        let paired = json!({
            "uuid": UUID, "model": MODEL, "address": self.address.ip(), "serial": SERIAL, "mac": MAC,
            "manifest": "signed", "key": KEY, "since": "2026-11-01T12:00:00Z",
        });
        std::fs::write(dir.join("tv.json"), paired.to_string()).unwrap();
    }

    async fn serve(self, tcp: TcpStream) {
        if !self.state().on {
            return;
        }
        let Ok(mut socket) = accept_async(tcp).await else {
            return;
        };
        while let Some(Ok(message)) = socket.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let frame: Value = serde_json::from_str(text.as_str()).unwrap();
            self.state().frames.push(frame.clone());
            let id = frame["id"].clone();
            match frame["type"].as_str().unwrap_or_default() {
                "hello" => {
                    let uuid = self.state().uuid.clone();
                    send(
                        &mut socket,
                        json!({"type": "hello", "payload": {
                        "protocolVersion": 1, "deviceType": "tv", "deviceOS": "webOS",
                        "deviceOSVersion": "4.1.0", "deviceOSReleaseVersion": "6.5.3",
                        "deviceUUID": uuid, "pairingTypes": ["PIN", "PROMPT", "COMBINED"]}}),
                    )
                    .await;
                }
                "register" => {
                    if !self.register(&mut socket, &frame).await {
                        return;
                    }
                }
                "request" | "subscribe" => {
                    let uri = frame["uri"]
                        .as_str()
                        .unwrap_or_default()
                        .trim_start_matches("ssap://")
                        .to_owned();
                    let answer = match self.answer(&uri, &frame["payload"]) {
                        Ok(payload) => json!({"type": "response", "id": id, "payload": payload}),
                        Err(error) => {
                            json!({"type": "error", "id": id, "error": error, "payload": {}})
                        }
                    };
                    send(&mut socket, answer).await;
                }
                _ => {}
            }
        }
    }

    /// Answers a register frame; `false` when the connection should end.
    async fn register(&self, socket: &mut WebSocketStream<TcpStream>, frame: &Value) -> bool {
        let id = frame["id"].clone();
        let (key, prompt) = {
            let state = self.state();
            (state.key.clone(), state.prompt)
        };
        match frame["payload"]["client-key"].as_str() {
            Some(given) if given == key => {
                send(
                    socket,
                    json!({"type": "registered", "id": id, "payload": {"client-key": key}}),
                )
                .await;
                true
            }
            given => {
                send(socket, json!({"type": "response", "id": id, "payload": {"pairingType": "PROMPT", "returnValue": true}})).await;
                if given.is_some() {
                    // An unknown key: the prompt stays until telka gives up.
                    return true;
                }
                match prompt {
                    Prompt::Accept => {
                        send(
                            socket,
                            json!({"type": "registered", "id": id, "payload": {"client-key": key}}),
                        )
                        .await;
                        true
                    }
                    Prompt::Decline => {
                        send(socket, json!({"type": "error", "id": id, "error": "403 cancelled", "payload": {}})).await;
                        false
                    }
                }
            }
        }
    }

    fn answer(&self, uri: &str, payload: &Value) -> Result<Value, String> {
        let state = self.state();
        Ok(match uri {
            "system/getSystemInfo" => json!({"returnValue": true, "modelName": MODEL,
                "serialNumber": SERIAL, "receiverType": "DVB", "features": {"dvr": false}}),
            "com.webos.service.tvpower/power/getPowerState" => {
                json!({"returnValue": true, "state": state.power})
            }
            "com.webos.applicationManager/getForegroundAppInfo" => {
                json!({"returnValue": true, "appId": state.app, "windowId": "", "processId": ""})
            }
            "audio/getVolume" => json!({"returnValue": true, "volumeStatus": {
                "volume": state.volume, "muteStatus": state.muted, "maxVolume": 100,
                "adjustVolume": true, "activeStatus": true, "soundOutput": state.output}}),
            "com.webos.service.apiadapter/audio/getSoundOutput" => {
                json!({"returnValue": true, "soundOutput": state.output})
            }
            "settings/getSystemSettings" => {
                let settings = match payload["category"].as_str().unwrap_or_default() {
                    "picture" => {
                        json!({"backlight": state.backlight.to_string(), "contrast": "100",
                        "brightness": "50", "color": "50", "energySaving": state.energy})
                    }
                    "sound" => json!({"soundMode": state.sound_mode, "soundOutput": state.output}),
                    "network" => json!({"wolwowlOnOff": state.wake_on_lan.to_string()}),
                    _ => return Err("500 Application error".into()),
                };
                json!({"returnValue": true, "category": payload["category"], "settings": settings})
            }
            "com.webos.applicationManager/listLaunchPoints" => json!({"returnValue": true,
                "subscribed": false, "caseDetail": {"change": []}, "launchPoints": [
                    {"id": "youtube.leanback.v4", "title": "YouTube", "systemApp": false, "removable": true},
                    {"id": "netflix", "title": "Netflix", "systemApp": false, "removable": true},
                    {"id": "com.wbd.stream", "title": "HBO Max", "systemApp": false, "removable": true},
                    {"id": "com.hbo.hbomax", "title": "HBO Max", "systemApp": false, "removable": true},
                    {"id": "com.webos.app.browser", "title": "Web Browser", "systemApp": true, "removable": false}]}),
            "tv/getExternalInputList" => json!({"returnValue": true, "devices": [
                {"id": "HDMI_1", "label": "HDMI 1", "appId": "com.webos.app.hdmi1", "connected": false,
                 "hdmiPlugIn": false, "subList": []},
                {"id": "HDMI_2", "label": "Game Console", "appId": "com.webos.app.hdmi2", "connected": false,
                 "hdmiPlugIn": true, "subList": [{"id": "SIMPLINK", "osdName": "Game Console X"}]}]}),
            "com.webos.service.connectionmanager/getinfo" => json!({"returnValue": true,
                "wifiInfo": {"macAddress": MAC}, "wiredInfo": {"macAddress": "02:00:5e:10:00:02"}}),
            "com.webos.service.connectionmanager/getstatus" => json!({"returnValue": true,
                "wifi": {"state": "connected"}, "wired": {"state": "disconnected"}}),
            _ => return Err("404 no such service or method".into()),
        })
    }
}

async fn send(socket: &mut WebSocketStream<TcpStream>, frame: Value) {
    let _ = socket.send(Message::text(frame.to_string())).await;
}
