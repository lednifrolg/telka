//! A fake TV: SSAP over plain WebSocket, with enough state for telka's commands. It answers
//! requests, pushes every subscription when something changes, takes buttons on the input socket
//! like the real one, applies settings through alerts, and wakes on a magic packet.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::broadcast;
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one thing a test can set on the TV"
)]
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
    pub picture_mode: String,
    pub wake_on_lan: bool,
    /// Takes changes without doing them, to test unconfirmed changes.
    pub ignores_changes: bool,
    /// Never closes alerts, like one stuck on screen.
    pub alerts_stick: bool,
    /// Every frame telka sent, in order.
    pub frames: Vec<Value>,
    /// Buttons pressed on the input socket.
    pub buttons: Vec<String>,
    pub toasts: Vec<String>,
    pub typed: String,
    pub magic_packets: usize,
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
            picture_mode: "normal".into(),
            wake_on_lan: true,
            ignores_changes: false,
            alerts_stick: false,
            frames: Vec::new(),
            buttons: Vec::new(),
            toasts: Vec::new(),
            typed: String::new(),
            magic_packets: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Event {
    Changed,
    /// The TV goes off: every connection closes.
    Off,
}

#[derive(Clone)]
pub struct FakeTv {
    pub address: SocketAddr,
    /// Where it listens for magic packets.
    pub wake_address: SocketAddr,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<Event>,
}

impl FakeTv {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let wake = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let tv = Self {
            address: listener.local_addr().unwrap(),
            wake_address: wake.local_addr().unwrap(),
            state: Arc::default(),
            events: broadcast::channel(64).0,
        };
        let serving = tv.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                tokio::spawn(serving.clone().serve(tcp));
            }
        });
        let waking = tv.clone();
        tokio::spawn(async move { waking.listen_for_magic(wake).await });
        tv
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Changes the state as if someone used the remote, and tells every subscription.
    pub fn change(&self, edit: impl FnOnce(&mut State)) {
        edit(&mut self.state());
        let _ = self.events.send(Event::Changed);
    }

    /// Turns the TV off as its remote would: every connection closes.
    pub fn go_off(&self) {
        {
            let mut state = self.state();
            state.on = false;
            state.power = "Suspend".into();
        }
        let _ = self.events.send(Event::Off);
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

    /// The requests telka sent to `uri`, without the scheme.
    pub fn requests(&self, uri: &str) -> Vec<Value> {
        let uri = format!("ssap://{uri}");
        self.frames("request")
            .into_iter()
            .filter(|frame| frame["uri"] == uri)
            .collect()
    }

    /// The pairing `telka pair` leaves, under `home`.
    pub fn write_pairing(&self, home: &Path) {
        let dir = home.join("state");
        std::fs::create_dir_all(&dir).unwrap();
        let paired = json!({
            "uuid": UUID, "model": MODEL, "address": self.address.ip(), "serial": SERIAL,
            "mac": MAC, "manifest": "signed", "key": KEY, "since": "2026-11-01T12:00:00Z",
        });
        std::fs::write(dir.join("tv.json"), paired.to_string()).unwrap();
    }

    /// Wakes when the magic packet for [`MAC`] arrives: answering at once in `Suspend`, like the
    /// real one, and `Active` a moment later.
    async fn listen_for_magic(self, socket: UdpSocket) {
        let mac: Vec<u8> = MAC
            .split(':')
            .map(|part| u8::from_str_radix(part, 16).unwrap())
            .collect();
        let mut magic = vec![0xFF; 6];
        for _ in 0..16 {
            magic.extend(&mac);
        }
        let mut buffer = [0; 256];
        while let Ok((length, _)) = socket.recv_from(&mut buffer).await {
            if buffer[..length] != magic[..] {
                continue;
            }
            let asleep = {
                let mut state = self.state();
                state.magic_packets += 1;
                !state.on && state.wake_on_lan
            };
            if asleep {
                let tv = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    tv.change(|state| {
                        state.on = true;
                        state.power = "Suspend".into();
                    });
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    tv.change(|state| state.power = "Active".into());
                });
            }
        }
    }

    async fn serve(self, tcp: TcpStream) {
        if !self.state().on {
            return;
        }
        let Ok(mut socket) = accept_async(tcp).await else {
            return;
        };
        let mut events = self.events.subscribe();
        let mut subscriptions: Vec<(Value, String, Value)> = Vec::new();
        loop {
            tokio::select! {
                message = socket.next() => {
                    let Some(Ok(message)) = message else { return };
                    let Message::Text(text) = message else { continue };
                    let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else {
                        // The input socket: `type:button\nname:HOME\n\n` and the like.
                        self.input(text.as_str());
                        continue;
                    };
                    self.state().frames.push(frame.clone());
                    if !self.frame(&mut socket, &frame, &mut subscriptions).await {
                        return;
                    }
                }
                event = events.recv() => match event {
                    Ok(Event::Changed) => {
                        for (id, uri, payload) in &subscriptions {
                            if let Ok(answer) = self.answer(uri, payload) {
                                send(&mut socket, json!({"type": "response", "id": id, "payload": answer})).await;
                            }
                        }
                    }
                    Ok(Event::Off) | Err(_) => {
                        let _ = socket.close(None).await;
                        return;
                    }
                },
            }
        }
    }

    /// Answers one frame; `false` when the connection should end.
    async fn frame(
        &self,
        socket: &mut WebSocketStream<TcpStream>,
        frame: &Value,
        subscriptions: &mut Vec<(Value, String, Value)>,
    ) -> bool {
        let id = frame["id"].clone();
        let uri = frame["uri"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("ssap://")
            .to_owned();
        match frame["type"].as_str().unwrap_or_default() {
            "hello" => {
                let uuid = self.state().uuid.clone();
                send(
                    socket,
                    json!({"type": "hello", "payload": {
                    "protocolVersion": 1, "deviceType": "tv", "deviceOS": "webOS",
                    "deviceOSVersion": "4.1.0", "deviceOSReleaseVersion": "6.5.3",
                    "deviceUUID": uuid, "pairingTypes": ["PIN", "PROMPT", "COMBINED"]}}),
                )
                .await;
            }
            "register" => return self.register(socket, frame).await,
            "subscribe" => {
                subscriptions.push((id.clone(), uri.clone(), frame["payload"].clone()));
                let answer = self.answer(&uri, &frame["payload"]);
                send(socket, reply(&id, answer)).await;
            }
            "unsubscribe" => subscriptions.retain(|(sub, _, _)| *sub != id),
            "request" => {
                if uri == "system.notifications/closeAlert" && self.state().alerts_stick {
                    return true;
                }
                let answer = self.act(&uri, &frame["payload"]);
                send(socket, reply(&id, answer)).await;
            }
            _ => {}
        }
        true
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

    /// A frame on the input socket.
    fn input(&self, text: &str) {
        let Some(name) = text
            .lines()
            .find_map(|line| line.strip_prefix("name:"))
            .map(str::to_owned)
        else {
            return;
        };
        self.state().buttons.push(name.clone());
        if name != "POWER" {
            return;
        }
        let power = self.state().power.clone();
        if power == "Active Standby" {
            self.change(|state| state.power = "Active".into());
            return;
        }
        // Off the way the remote turns it off: a moment going down, then gone.
        self.change(|state| state.power = "Active Standby".into());
        let tv = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            {
                let mut state = tv.state();
                state.on = false;
                state.power = "Suspend".into();
            }
            let _ = tv.events.send(Event::Off);
        });
    }

    /// Does what a request asks, then answers it.
    fn act(&self, uri: &str, payload: &Value) -> Result<Value, String> {
        let ignores = self.state().ignores_changes;
        let done = json!({"returnValue": true});
        match uri {
            "audio/setVolume" | "audio/setMute" | "tv/switchInput" if ignores => Ok(done),
            "audio/setVolume" => {
                let volume = u8::try_from(payload["volume"].as_u64().unwrap()).unwrap();
                self.change(|state| state.volume = volume);
                Ok(done)
            }
            "audio/setMute" => {
                let muted = payload["mute"].as_bool().unwrap();
                self.change(|state| state.muted = muted);
                Ok(done)
            }
            "system.launcher/launch" | "system.launcher/open" | "tv/switchInput" => {
                let app = match uri {
                    "system.launcher/open" => "com.webos.app.browser".to_owned(),
                    "tv/switchInput" => format!(
                        "com.webos.app.{}",
                        payload["inputId"]
                            .as_str()
                            .unwrap()
                            .to_lowercase()
                            .replace('_', "")
                    ),
                    _ => payload["id"].as_str().unwrap().to_owned(),
                };
                // Apps take a moment to come to the front.
                let tv = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    tv.change(|state| state.app = app);
                });
                Ok(json!({"returnValue": true, "id": payload["id"]}))
            }
            "com.webos.service.tvpower/power/turnOffScreen" => {
                self.change(|state| state.power = "Screen Off".into());
                Ok(done)
            }
            "com.webos.service.tvpower/power/turnOnScreen" => {
                self.change(|state| state.power = "Active".into());
                Ok(done)
            }
            "system.notifications/createToast" => {
                let message = payload["message"].as_str().unwrap().to_owned();
                self.state().toasts.push(message);
                Ok(json!({"returnValue": true, "toastId": "toast-1"}))
            }
            "com.webos.service.ime/insertText" => {
                let text = payload["text"].as_str().unwrap().to_owned();
                self.state().typed.push_str(&text);
                Ok(done)
            }
            "com.webos.service.ime/sendEnterKey" => {
                self.state().typed.push('\n');
                Ok(done)
            }
            "media.controls/play" | "media.controls/pause" | "media.controls/stop" => Ok(done),
            "system.notifications/createAlert" => {
                let alert = json!({"type": "alert", "onclose": payload["onclose"]});
                self.state().frames.push(alert);
                Ok(json!({"returnValue": true, "alertId": "alert-1"}))
            }
            "system.notifications/closeAlert" => {
                let onclose = self
                    .state()
                    .frames
                    .iter()
                    .rev()
                    .find(|frame| frame["type"] == "alert")
                    .map(|frame| frame["onclose"].clone())
                    .unwrap_or_default();
                if onclose["uri"] == "luna://com.webos.settingsservice/setSystemSettings"
                    && onclose["params"]["category"] == "picture"
                {
                    let settings = onclose["params"]["settings"].clone();
                    self.change(|state| {
                        if let Some(light) = settings["backlight"].as_str() {
                            state.backlight = light.parse().unwrap();
                        }
                        if let Some(energy) = settings["energySaving"].as_str() {
                            energy.clone_into(&mut state.energy);
                        }
                        if let Some(mode) = settings["pictureMode"].as_str() {
                            mode.clone_into(&mut state.picture_mode);
                        }
                    });
                }
                Ok(done)
            }
            _ => self.answer(uri, payload),
        }
    }

    /// What the TV answers for state it reports.
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
            "com.webos.service.networkinput/getPointerInputSocket" => json!({"returnValue": true,
                "socketPath": format!("ws://{}/resources/0123/netinput.pointer.sock", self.address)}),
            _ => return Err("404 no such service or method".into()),
        })
    }
}

fn reply(id: &Value, answer: Result<Value, String>) -> Value {
    match answer {
        Ok(payload) => json!({"type": "response", "id": id, "payload": payload}),
        Err(error) => json!({"type": "error", "id": id, "error": error, "payload": {}}),
    }
}

async fn send(socket: &mut WebSocketStream<TcpStream>, frame: Value) {
    let _ = socket.send(Message::text(frame.to_string())).await;
}
