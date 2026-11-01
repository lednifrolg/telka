//! Reaching the paired TV: at the address that worked last, else wherever mDNS finds it.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use webos::{Client, Endpoint, Options};

use crate::config::Config;
use crate::error::AppError;
use crate::paths::Paths;
use crate::store::Paired;

/// A TV on the home network answers within milliseconds; one that takes longer is off.
const CONNECT_WAIT: Duration = Duration::from_millis(1500);
const DISCOVERY_WAIT: Duration = Duration::from_secs(2);

/// Points telka at a plain WebSocket test double, e.g. `ws://127.0.0.1:3000`, instead of the
/// paired TV's address. For tests only.
pub const TEST_TV: &str = "TELKA_TV_URL";

/// A connected TV and its pairing.
pub struct Tv {
    pub client: Client,
    pub paired: Paired,
}

/// Connection options: LG's certificate, and short waits for a TV that may be off.
pub fn options() -> Result<Options, AppError> {
    let mut options = Options::lg()?;
    options.timing.connect = CONNECT_WAIT;
    Ok(options)
}

impl Tv {
    pub async fn connect(paths: &Paths, config: &Config) -> Result<Self, AppError> {
        let paired = Paired::load(paths)?.ok_or(AppError::NotPaired)?;
        let options = options()?;
        let credentials = paired.credentials();
        if let Some(address) = test_tv()? {
            let endpoint = Endpoint {
                address,
                secure: false,
            };
            return match Client::connect(endpoint, &credentials, &options).await {
                Ok(client) => Ok(Self { client, paired }),
                Err(error) if unreachable(&error) => Err(off(address.ip())),
                Err(error) => Err(refused(error, &address.ip())),
            };
        }
        let address = config.tv.unwrap_or(paired.address);
        match reach(address, &paired, &options).await? {
            Some(client) => return Ok(Self { client, paired }),
            None if config.tv.is_some() => return Err(off(address)),
            None => {}
        }
        // The TV may have a new address: look for its serial number on the network.
        let Some(serial) = paired.serial.as_deref() else {
            return Err(off(address));
        };
        let found = webos::find(serial, DISCOVERY_WAIT)
            .await
            .ok()
            .flatten()
            .map(|tv| tv.addresses)
            .unwrap_or_default();
        for candidate in found.into_iter().filter(|candidate| *candidate != address) {
            if let Some(client) = reach(candidate, &paired, &options).await? {
                let mut paired = paired;
                paired.address = candidate;
                paired.save(paths)?;
                return Ok(Self { client, paired });
            }
        }
        Err(off(address))
    }
}

/// The connection, or `None` when nothing answers at `address`.
async fn reach(
    address: IpAddr,
    paired: &Paired,
    options: &Options,
) -> Result<Option<Client>, AppError> {
    match Client::connect_to(address, &paired.credentials(), options).await {
        Ok(client) => Ok(Some(client)),
        Err(error) if unreachable(&error) => Ok(None),
        Err(error) => Err(refused(error, &address)),
    }
}

/// Nothing usable answered: the TV is off, asleep, or not at the address.
fn unreachable(error: &webos::Error) -> bool {
    matches!(
        error,
        webos::Error::Connect(_)
            | webos::Error::TimedOut
            | webos::Error::WebSocket(_)
            | webos::Error::Closed
    )
}

/// A TV that answered and refused: the reason, with the address it answered at.
pub fn refused(error: webos::Error, address: &IpAddr) -> AppError {
    match error {
        webos::Error::WrongTv { expected, found } => AppError::WrongTv {
            address: address.to_string(),
            expected,
            found,
        },
        webos::Error::UnknownCertificate { found } => AppError::UnknownCertificate {
            address: address.to_string(),
            found: found.to_string(),
        },
        other => other.into(),
    }
}

pub fn off(address: IpAddr) -> AppError {
    AppError::Unreachable(format!(
        "the TV does not answer at {address}: it is off, asleep or elsewhere; `telka on` wakes it"
    ))
}

/// The test double from [`TEST_TV`], if it is set.
pub fn test_tv() -> Result<Option<SocketAddr>, AppError> {
    let Some(url) = std::env::var_os(TEST_TV) else {
        return Ok(None);
    };
    let url = url.to_string_lossy();
    url.strip_prefix("ws://")
        .and_then(|rest| rest.trim_end_matches('/').parse().ok())
        .map(Some)
        .ok_or_else(|| AppError::Usage(format!("{TEST_TV} must be ws://<ip>:<port>, not {url}")))
}
