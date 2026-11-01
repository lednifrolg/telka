//! Client for the local SSAP API of LG webOS TVs.
//!
//! SSAP (Simple Service Access Protocol) is the WebSocket API that LG's own remote apps use on the
//! home network. LG does not document it; this crate follows what the community worked out from
//! that traffic, checked against an LG C1 on webOS 6.5.3. Every command acts on a real TV that
//! someone may be watching. The crate has no UI dependencies; the `telka` binary builds the CLI
//! and TUI on top of it.

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
