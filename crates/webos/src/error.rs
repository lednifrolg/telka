//! Errors from talking to the TV.

use std::io;

use tokio_tungstenite::tungstenite;

use crate::tls::Fingerprint;

/// Talking to the TV failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No connection: the TV is off, asleep or not at this address.
    #[error("could not connect to the TV: {0}")]
    Connect(#[source] io::Error),
    /// The TV did not answer in time.
    #[error("the TV did not answer in time")]
    TimedOut,
    /// The TV presented a certificate that is not LG's TV certificate, so it may not be a TV.
    #[error("the TV presented an unknown certificate {found}")]
    UnknownCertificate { found: Fingerprint },
    /// TLS could not be set up.
    #[error("could not set up TLS: {0}")]
    Tls(#[from] rustls::Error),
    /// The WebSocket handshake or connection failed.
    #[error("the WebSocket connection failed: {0}")]
    WebSocket(#[source] tungstenite::Error),
    /// The connection closed while a request was waiting, or before it was sent.
    #[error("the connection to the TV closed")]
    Closed,
    /// The TV sent something the protocol does not allow at this point.
    #[error("unexpected message from the TV: {0}")]
    Protocol(String),
    /// An answer did not have the expected shape.
    #[error("unexpected answer to {uri}: {source}")]
    Decode {
        uri: String,
        #[source]
        source: serde_json::Error,
    },
    /// Another TV answers at the address: its `deviceUUID` is not the one paired with.
    #[error("another TV answers at this address ({found}, not {expected})")]
    WrongTv { expected: String, found: String },
    /// The TV does not know the client key and asks for pairing again, e.g. after a reset.
    #[error("the TV no longer accepts this client key; pair again")]
    KeyRejected,
    /// Someone declined the pairing prompt on the TV.
    #[error("pairing was declined on the TV")]
    PairingDeclined,
    /// Nobody answered the pairing prompt on the TV in time.
    #[error("nobody accepted the pairing prompt on the TV in time")]
    PairingTimedOut,
    /// The TV refuses to pair with this manifest, e.g. webOS 26 with LG's signed one.
    #[error("the TV refuses this manifest: {0}")]
    ManifestRejected(String),
    /// The pairing does not grant the permission the request needs (401), or the TV denies it
    /// to remote apps altogether (403).
    #[error("the TV does not allow this: {message}")]
    Denied { message: String },
    /// The TV has no such service or method (404).
    #[error("the TV does not support {uri}")]
    Unsupported { uri: String },
    /// Looking for TVs on the network failed.
    #[error("mDNS discovery failed: {0}")]
    Discovery(String),
    /// The TV took the request but could not do it, with its own explanation.
    #[error("the TV could not do it: {message}")]
    Rejected { message: String },
}
