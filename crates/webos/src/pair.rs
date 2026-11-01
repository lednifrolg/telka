//! Greeting the TV, pairing with it, and registering with a client key.
//!
//! Pairing sends a manifest of permissions without a client key; the TV shows a prompt, and once
//! someone accepts it with the remote, answers with a new client key. The grants are fixed then:
//! registering later with the key and a longer permission list gets nothing more. Only a manifest
//! LG signed unlocks permissions such as `WRITE_SETTINGS` or `READ_UPDATE_INFO`; webOS 26 refuses
//! it, so pairing falls back to the unsigned one there.

use std::fmt;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::{Instant, timeout, timeout_at};
use tokio_tungstenite::tungstenite::{self, Message};

use crate::client::Client;
use crate::error::Error;
use crate::frame::{self, Incoming, Kind};
use crate::tls::Tls;
use crate::transport::{self, Endpoint, Socket};

/// How long a prompt waits for someone with the remote, unless the caller says otherwise.
pub const PROMPT_WINDOW: Duration = Duration::from_secs(60);

/// The key the TV issues at pairing. It lets anyone on the network control the TV, so it is not
/// printed by `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientKey(String);

impl ClientKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientKey(..)")
    }
}

/// The manifest a client pairs and registers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Manifest {
    /// LG's own remote app manifest, signed by LG: every permission, refused by webOS 26.
    Signed,
    /// The same permissions without LG's signature; the TV grants only part of them.
    Unsigned,
}

impl Manifest {
    pub(crate) fn json(self) -> &'static str {
        match self {
            Self::Signed => include_str!("manifest/signed.json"),
            Self::Unsigned => include_str!("manifest/unsigned.json"),
        }
    }
}

/// How long each step may take. The defaults suit a TV on the home network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// TCP connect, TLS and WebSocket handshakes, and the TV's `hello`, each.
    pub connect: Duration,
    /// An answer to a request. Starting an app can take a few seconds.
    pub request: Duration,
    /// How often the connection pings the TV.
    pub ping_every: Duration,
    /// How long the TV may stay silent, pongs included, before the connection counts as lost:
    /// a TV going to sleep often just stops answering.
    pub silence: Duration,
    /// How long registering with a key waits for the TV to confirm it after showing a prompt.
    pub key_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(10),
            ping_every: Duration::from_secs(5),
            silence: Duration::from_secs(12),
            key_grace: Duration::from_secs(2),
        }
    }
}

/// How to reach a TV: which certificates to accept, and timing.
#[derive(Debug, Clone)]
pub struct Options {
    pub tls: Tls,
    pub timing: Timing,
}

impl Options {
    /// LG's TV certificate and the default timing.
    pub fn lg() -> Result<Self, Error> {
        Ok(Self {
            tls: Tls::lg()?,
            timing: Timing::default(),
        })
    }
}

/// What the TV says about itself before anything else.
#[derive(Debug, Clone, PartialEq)]
pub struct Hello {
    /// `deviceUUID`: the identity of the TV, stable across addresses and firmware updates.
    pub uuid: String,
    /// `deviceOSReleaseVersion`, the webOS version users see, e.g. `6.5.3`.
    pub release: Option<String>,
    /// `deviceOSVersion`, the platform version, e.g. `4.1.0` on webOS 6.
    pub os_version: Option<String>,
    /// How the TV can pair: `PROMPT`, `PIN`, `COMBINED`.
    pub pairing_types: Vec<String>,
    /// The whole `hello` frame.
    pub raw: Value,
}

impl Hello {
    fn read(frame: &Incoming) -> Result<Self, Error> {
        let text = |key: &str| frame.payload[key].as_str().map(str::to_owned);
        let uuid = text("deviceUUID")
            .filter(|uuid| !uuid.is_empty())
            .ok_or_else(|| Error::Protocol("the TV's hello has no deviceUUID".into()))?;
        let pairing_types = frame.payload["pairingTypes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|kind| kind.as_str().map(str::to_owned))
            .collect();
        Ok(Self {
            uuid,
            release: text("deviceOSReleaseVersion"),
            os_version: text("deviceOSVersion"),
            pairing_types,
            raw: frame.raw.clone(),
        })
    }
}

/// Identifies the TV without pairing: opens a connection, reads its `hello` and closes again.
pub async fn hello(endpoint: Endpoint, options: &Options) -> Result<Hello, Error> {
    let (mut socket, hello) = greet(endpoint, options).await?;
    let _ = socket.close(None).await;
    Ok(hello)
}

/// Opens a WebSocket to the TV and reads its `hello`.
pub(crate) async fn greet(endpoint: Endpoint, options: &Options) -> Result<(Socket, Hello), Error> {
    let tls = endpoint.secure.then_some(&options.tls);
    let wait = options.timing.connect;
    let mut socket = transport::open(endpoint.address, &endpoint.url(), tls, wait).await?;
    socket
        .send(Message::text(frame::hello()))
        .await
        .map_err(Error::WebSocket)?;
    let answer = next_frame(&mut socket, Instant::now() + wait).await?;
    if answer.kind != Kind::Hello {
        return Err(Error::Protocol(format!(
            "the TV answered hello with {:?}",
            answer.kind
        )));
    }
    let hello = Hello::read(&answer)?;
    Ok((socket, hello))
}

/// The next frame from the TV, skipping pings and pongs.
pub(crate) async fn next_frame(socket: &mut Socket, deadline: Instant) -> Result<Incoming, Error> {
    loop {
        let message = timeout_at(deadline, socket.next())
            .await
            .map_err(|_| Error::TimedOut)?;
        match message {
            Some(Ok(Message::Text(text))) => {
                return Incoming::parse(text.as_str())
                    .map_err(|_| Error::Protocol("the TV sent something other than JSON".into()));
            }
            Some(Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed)) | None => {
                return Err(Error::Closed);
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(Error::WebSocket(error)),
        }
    }
}

const REGISTER_ID: &str = "register_0";

/// What an `error` answer to `register` means.
fn register_error(frame: &Incoming) -> Error {
    let error = frame.error.clone().unwrap_or_default();
    if error.contains("blacklist") {
        Error::ManifestRejected(error)
    } else if error.contains("cancel") {
        Error::PairingDeclined
    } else {
        Error::Rejected { message: error }
    }
}

/// The key in a `registered` frame.
fn issued_key(frame: &Incoming) -> Result<ClientKey, Error> {
    frame.payload["client-key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .map(ClientKey::new)
        .ok_or_else(|| Error::Protocol("the TV registered without a client key".into()))
}

/// Registers on an open connection with a known key. Returns the key the TV confirmed: usually
/// the same, but a new one when the TV asked again and someone accepted the prompt at once.
pub(crate) async fn register_with_key(
    socket: &mut Socket,
    manifest: Manifest,
    key: &ClientKey,
    timing: &Timing,
) -> Result<ClientKey, Error> {
    let register = frame::register(REGISTER_ID, manifest, Some(key));
    socket
        .send(Message::text(register))
        .await
        .map_err(Error::WebSocket)?;
    let mut deadline = Instant::now() + timing.request;
    let mut prompted = false;
    loop {
        let answer = match next_frame(socket, deadline).await {
            Err(Error::TimedOut) if prompted => return Err(Error::KeyRejected),
            other => other?,
        };
        if answer.id.as_deref() != Some(REGISTER_ID) {
            continue;
        }
        match answer.kind {
            Kind::Registered => {
                return answer
                    .payload
                    .get("client-key")
                    .map_or_else(|| Ok(key.clone()), |_| issued_key(&answer));
            }
            // A prompt on the TV: it does not know the key. Give it a moment in case it still
            // confirms, then give up, which also takes the prompt away.
            Kind::Response if answer.payload.get("pairingType").is_some() && !prompted => {
                prompted = true;
                deadline = Instant::now() + timing.key_grace;
            }
            Kind::Error => {
                return Err(match register_error(&answer) {
                    Error::PairingDeclined => Error::KeyRejected,
                    other => other,
                });
            }
            _ => {}
        }
    }
}

/// A pairing in progress: the TV shows a prompt to accept on the TV with the remote.
pub struct Pairing {
    socket: Socket,
    endpoint: Endpoint,
    hello: Hello,
    manifest: Manifest,
    options: Options,
    /// Set when the TV registered without asking.
    issued: Option<ClientKey>,
}

impl Pairing {
    /// Asks the TV to pair with LG's signed manifest, or the unsigned one when the TV refuses
    /// the signed one.
    pub async fn start(endpoint: Endpoint, options: &Options) -> Result<Self, Error> {
        match Self::start_with(endpoint, Manifest::Signed, options).await {
            Err(Error::ManifestRejected(_)) => {
                Self::start_with(endpoint, Manifest::Unsigned, options).await
            }
            other => other,
        }
    }

    /// Asks the TV to pair with `manifest`. Returns once the TV shows the prompt.
    pub async fn start_with(
        endpoint: Endpoint,
        manifest: Manifest,
        options: &Options,
    ) -> Result<Self, Error> {
        let (mut socket, hello) = greet(endpoint, options).await?;
        socket
            .send(Message::text(frame::register(REGISTER_ID, manifest, None)))
            .await
            .map_err(Error::WebSocket)?;
        let deadline = Instant::now() + options.timing.request;
        let issued = loop {
            let answer = next_frame(&mut socket, deadline).await?;
            if answer.id.as_deref() != Some(REGISTER_ID) {
                continue;
            }
            match answer.kind {
                Kind::Response if answer.payload.get("pairingType").is_some() => break None,
                Kind::Registered => break Some(issued_key(&answer)?),
                Kind::Error => return Err(register_error(&answer)),
                _ => {}
            }
        };
        Ok(Self {
            socket,
            endpoint,
            hello,
            manifest,
            options: options.clone(),
            issued,
        })
    }

    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub fn manifest(&self) -> Manifest {
        self.manifest
    }

    /// Waits up to `wait` for someone to accept the prompt; the connection then stays open as a
    /// [`Client`]. Store [`Client::paired`]: every later connection needs it.
    pub async fn finish(mut self, wait: Duration) -> Result<Client, Error> {
        let key = match self.issued.take() {
            Some(key) => key,
            None => self.wait_for_key(wait).await?,
        };
        Ok(Client::start(
            self.socket,
            self.endpoint,
            self.hello,
            key,
            self.manifest,
            &self.options,
        ))
    }

    async fn wait_for_key(&mut self, wait: Duration) -> Result<ClientKey, Error> {
        let deadline = Instant::now() + wait;
        loop {
            let answer = match next_frame(&mut self.socket, deadline).await {
                Err(Error::TimedOut) => {
                    // Closing takes the prompt off the screen.
                    let _ = timeout(self.options.timing.connect, self.socket.close(None)).await;
                    return Err(Error::PairingTimedOut);
                }
                other => other?,
            };
            if answer.id.as_deref() != Some(REGISTER_ID) {
                continue;
            }
            match answer.kind {
                Kind::Registered => return issued_key(&answer),
                Kind::Error => return Err(register_error(&answer)),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_stay_out_of_debug_output() {
        let key = ClientKey::new("0123456789abcdef");
        assert_eq!(format!("{key:?}"), "ClientKey(..)");
        assert_eq!(key.as_str(), "0123456789abcdef");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, r#""0123456789abcdef""#);
    }

    #[test]
    fn bundled_manifests_ask_for_the_same_permissions() {
        let signed: Value = serde_json::from_str(Manifest::Signed.json()).unwrap();
        let unsigned: Value = serde_json::from_str(Manifest::Unsigned.json()).unwrap();
        let list = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_owned())
                .collect()
        };
        let mut wanted = list(&signed["permissions"]);
        wanted.extend(list(&signed["signed"]["permissions"]));
        wanted.sort();
        wanted.dedup();
        assert_eq!(list(&unsigned["permissions"]), wanted);
        assert!(signed["signatures"][0]["signature"].as_str().unwrap().len() > 300);
        assert!(unsigned.get("signed").is_none());
        assert_eq!(
            serde_json::to_string(&Manifest::Unsigned).unwrap(),
            r#""unsigned""#
        );
    }

    #[test]
    fn hello_needs_an_identity() {
        let frame = Incoming::parse(
            r#"{"type":"hello","payload":{"deviceUUID":"u-1","deviceOSReleaseVersion":"6.5.3","deviceOSVersion":"4.1.0","pairingTypes":["PIN","PROMPT"]}}"#,
        )
        .unwrap();
        let hello = Hello::read(&frame).unwrap();
        assert_eq!(hello.uuid, "u-1");
        assert_eq!(hello.release.as_deref(), Some("6.5.3"));
        assert_eq!(hello.pairing_types, ["PIN", "PROMPT"]);
        let anonymous = Incoming::parse(r#"{"type":"hello","payload":{}}"#).unwrap();
        assert!(matches!(Hello::read(&anonymous), Err(Error::Protocol(_))));
    }

    #[test]
    fn register_errors_are_told_apart() {
        let error = |text: &str| {
            Incoming::parse(&format!(
                r#"{{"type":"error","id":"register_0","error":"{text}","payload":{{}}}}"#
            ))
            .unwrap()
        };
        assert!(matches!(
            register_error(&error(
                "403 Pairing rejected: blacklisted certificate detected"
            )),
            Error::ManifestRejected(_)
        ));
        assert!(matches!(
            register_error(&error("403 cancelled")),
            Error::PairingDeclined
        ));
        assert!(matches!(
            register_error(&error("500 something")),
            Error::Rejected { .. }
        ));
    }
}
