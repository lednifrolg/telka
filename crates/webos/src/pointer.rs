//! The remote control: buttons and the pointer, over the TV's separate input socket.
//!
//! `com.webos.service.networkinput/getPointerInputSocket` names a second WebSocket. It takes short
//! text frames, `type:button\nname:HOME\n\n` and the like, and answers nothing: a button the TV
//! does not know is ignored without a word.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::SinkExt;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::client::Client;
use crate::error::Error;
use crate::transport::{self, Socket};

/// A button on the remote, by the name the TV knows it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Button(&'static str);

/// The buttons a remote may press. Left out on purpose: `IN_START`, `EZ_ADJUST` and
/// `ADVANCE_SETTING`, which open service menus.
const NAMES: &[&str] = &[
    "UP",
    "DOWN",
    "LEFT",
    "RIGHT",
    "ENTER",
    "BACK",
    "EXIT",
    "HOME",
    "MENU",
    "QMENU",
    "INFO",
    "GUIDE",
    "PROGRAM",
    "LIST",
    "DASH",
    "ASTERISK",
    "CC",
    "AD",
    "SAP",
    "TELETEXT",
    "TEXTOPTION",
    "ASPECT_RATIO",
    "MAGNIFIER_ZOOM",
    "LIVE_ZOOM",
    "SCREEN_REMOTE",
    "INPUT_HUB",
    "MYAPPS",
    "RECENT",
    "SEARCH",
    "PLAY",
    "PAUSE",
    "STOP",
    "REWIND",
    "FASTFORWARD",
    "RECORD",
    "VOLUMEUP",
    "VOLUMEDOWN",
    "MUTE",
    "CHANNELUP",
    "CHANNELDOWN",
    "RED",
    "GREEN",
    "YELLOW",
    "BLUE",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "NETFLIX",
    "AMAZON",
    "POWER",
];

impl Button {
    pub const UP: Self = Self("UP");
    pub const DOWN: Self = Self("DOWN");
    pub const LEFT: Self = Self("LEFT");
    pub const RIGHT: Self = Self("RIGHT");
    /// OK.
    pub const ENTER: Self = Self("ENTER");
    pub const BACK: Self = Self("BACK");
    pub const HOME: Self = Self("HOME");
    /// The quick settings, the gear on the remote.
    pub const MENU: Self = Self("MENU");
    pub const INFO: Self = Self("INFO");
    pub const PLAY: Self = Self("PLAY");
    pub const PAUSE: Self = Self("PAUSE");
    pub const REWIND: Self = Self("REWIND");
    pub const FAST_FORWARD: Self = Self("FASTFORWARD");
    /// Toggles: turns the TV off, and on from Active Standby.
    pub const POWER: Self = Self("POWER");

    /// The button called `name`, in any case.
    pub fn from_name(name: &str) -> Option<Self> {
        NAMES
            .iter()
            .find(|known| known.eq_ignore_ascii_case(name))
            .map(|&known| Self(known))
    }

    /// Every button, in a stable order.
    pub fn all() -> impl Iterator<Item = Self> {
        NAMES.iter().map(|&name| Self(name))
    }

    pub fn name(self) -> &'static str {
        self.0
    }
}

/// An open input socket. Frames go out in order; there is no answer to wait for.
pub struct Remote {
    socket: Mutex<Socket>,
}

/// How long the TV needs to read the last frame before the socket closes.
const LINGER: Duration = Duration::from_millis(120);

impl Remote {
    pub async fn button(&self, button: Button) -> Result<(), Error> {
        self.send(format!("type:button\nname:{}\n\n", button.name()))
            .await
    }

    /// Moves the pointer by `dx`, `dy` pixels; with `drag`, as with the button held.
    pub async fn move_by(&self, dx: i32, dy: i32, drag: bool) -> Result<(), Error> {
        let down = u8::from(drag);
        self.send(format!("type:move\ndx:{dx}\ndy:{dy}\ndown:{down}\n\n"))
            .await
    }

    pub async fn click(&self) -> Result<(), Error> {
        self.send("type:click\n\n".to_owned()).await
    }

    pub async fn scroll(&self, dx: i32, dy: i32) -> Result<(), Error> {
        self.send(format!("type:scroll\ndx:{dx}\ndy:{dy}\n\n"))
            .await
    }

    /// Closes the socket once the TV had time to read what went out last.
    pub async fn close(self) {
        tokio::time::sleep(LINGER).await;
        let _ = self.socket.into_inner().close(None).await;
    }

    async fn send(&self, frame: String) -> Result<(), Error> {
        self.socket
            .lock()
            .await
            .send(Message::text(frame))
            .await
            .map_err(Error::WebSocket)
    }
}

/// The port and whether TLS, from a socket path like
/// `wss://192.168.1.30:3001/resources/…/netinput.pointer.sock`.
fn socket_port(path: &str) -> Option<(u16, bool)> {
    let (secure, rest) = if let Some(rest) = path.strip_prefix("wss://") {
        (true, rest)
    } else {
        (false, path.strip_prefix("ws://")?)
    };
    let authority = rest.split('/').next()?;
    let port = authority.rsplit_once(':')?.1.parse().ok()?;
    Some((port, secure))
}

impl Client {
    /// Opens the input socket for buttons and the pointer. It connects to this TV's address
    /// whatever host the TV names.
    pub async fn remote(&self) -> Result<Remote, Error> {
        let uri = "com.webos.service.networkinput/getPointerInputSocket";
        let answer = self.request(uri, json!({})).await?;
        let path = answer["socketPath"]
            .as_str()
            .ok_or_else(|| Error::Protocol("the TV named no input socket".into()))?;
        let (port, secure) = socket_port(path)
            .ok_or_else(|| Error::Protocol(format!("unexpected input socket path {path}")))?;
        let address = SocketAddr::new(self.endpoint().address.ip(), port);
        let options = self.options();
        let tls = secure.then_some(&options.tls);
        let socket = transport::open(address, path, tls, options.timing.connect).await?;
        Ok(Remote {
            socket: Mutex::new(socket),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_are_found_by_name_but_not_the_service_ones() {
        assert_eq!(Button::from_name("home"), Some(Button::HOME));
        assert_eq!(Button::from_name("FastForward"), Some(Button::FAST_FORWARD));
        assert_eq!(Button::from_name("7").map(Button::name), Some("7"));
        for hidden in ["IN_START", "EZ_ADJUST", "ADVANCE_SETTING", "nope"] {
            assert_eq!(Button::from_name(hidden), None, "{hidden}");
        }
        assert_eq!(Button::all().count(), NAMES.len());
    }

    #[test]
    fn socket_paths_give_port_and_tls() {
        assert_eq!(
            socket_port("wss://192.168.1.30:3001/resources/abc/netinput.pointer.sock"),
            Some((3001, true))
        );
        assert_eq!(
            socket_port("ws://192.168.1.30:3000/resources/abc/netinput.pointer.sock"),
            Some((3000, false))
        );
        assert_eq!(
            socket_port("ws://[fd00::1]:3000/resources/abc"),
            Some((3000, false))
        );
        assert_eq!(socket_port("http://x:1/"), None);
        assert_eq!(socket_port("ws://no-port/"), None);
    }
}
