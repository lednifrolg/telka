//! Client for the local SSAP API of LG webOS TVs.
//!
//! SSAP (Simple Service Access Protocol) is the WebSocket API that LG's own remote apps use on the
//! home network. LG does not document it; this crate follows what the community worked out from
//! that traffic, checked against an LG C1 on webOS 6.5.3. Every command acts on a real TV that
//! someone may be watching, so nothing is retried behind the caller's back. The crate has no UI
//! dependencies; the `telka` binary builds the CLI and TUI on top of it.
//!
//! A typical session: pair once with [`Pairing`] while someone accepts the prompt on the TV, and
//! keep [`Client::paired`]. Later, [`Client::connect_to`] the TV with it, send requests with
//! [`Client::request`] and follow changes with [`Client::subscribe`]. The typed [`Report`]s,
//! like [`Volume`], are read with [`Client::get`] and followed with [`Client::watch`]; buttons and
//! the pointer go through a [`Remote`]. A sleeping TV wakes only by [`wake_and_wait`]; [`discover`] finds
//! TVs on the network.

mod api;
mod client;
mod discover;
mod error;
mod frame;
mod pair;
mod pointer;
mod tls;
mod transport;
mod wol;

pub use api::{
    App, ForegroundApp, Input, Inputs, Interfaces, Keyboard, LaunchPoints, LinkStatus, Media,
    Picture, PowerMode, PowerState, Report, SoundOutput, SoundSettings, SystemInfo, Volume,
    WakeOnLan, Watch,
};
pub use client::{Client, Paired, Subscription};
pub use discover::{Discovered, discover, find};
pub use error::Error;
pub use pair::{ClientKey, Hello, Manifest, Options, PROMPT_WINDOW, Pairing, Timing, hello};
pub use pointer::{Button, Remote};
pub use tls::{Fingerprint, LG_TV_CERTIFICATE, ParseFingerprintError, Tls};
pub use transport::Endpoint;
pub use wol::{
    MacAddress, ParseMacError, WAKE_WINDOW, magic_packet, wake, wake_and_wait, wake_to, wake_until,
};

/// TCP port of the plain WebSocket API. Still open on webOS 6; firmware from 2023 on closes it on
/// newer models, so [`WSS_PORT`] comes first.
pub const WS_PORT: u16 = 3000;

/// TCP port of the TLS WebSocket API. Every LG TV presents the same certificate here, so it proves
/// an LG TV but not which one; the `deviceUUID` of the `hello` answer identifies the TV.
pub const WSS_PORT: u16 = 3001;

/// Scheme of every SSAP request URI, as in `ssap://audio/getVolume`.
pub const SSAP_SCHEME: &str = "ssap://";

/// DNS-SD service type the TV announces over mDNS. It has no record of its own for SSAP, but its
/// AirPlay receiver carries the manufacturer, model and serial number in TXT records.
pub const MDNS_SERVICE: &str = "_airplay._tcp.local.";

/// Value of the `manufacturer` TXT record of an LG TV's AirPlay announcement.
pub const MDNS_MANUFACTURER: &str = "LG Electronics";
