//! Pairing, shared by `telka pair` and the TUI's pairing dialog.

use std::net::IpAddr;
use std::time::Duration;

use webos::{Client, Endpoint, Interfaces, LinkStatus, Manifest, Pairing, SystemInfo, WakeOnLan};

use crate::connect;
use crate::error::AppError;
use crate::paths::Paths;
use crate::store::Paired;

const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

/// Where "Turn on via Wi-Fi" is on a webOS 6 TV.
pub const WAKE_SETTING: &str =
    "Settings > General > Devices > External Devices > TV On With Mobile > Turn on via Wi-Fi";

/// A pairing whose prompt is on the TV.
pub struct Started {
    pub pairing: Pairing,
    pub address: IpAddr,
}

impl Started {
    pub fn manifest(&self) -> Manifest {
        self.pairing.manifest()
    }
}

/// Asks the TV at `address`, or the one LG TV on the network, to pair: the TLS port first, the
/// plain one when the TV refuses it. Returns once the prompt is on the TV.
pub async fn start(address: Option<IpAddr>) -> Result<Started, AppError> {
    let options = connect::options()?;
    if let Some(test) = connect::test_tv()? {
        let endpoint = Endpoint {
            address: test,
            secure: false,
        };
        let pairing = begin(endpoint, &options).await?;
        return Ok(Started {
            pairing,
            address: test.ip(),
        });
    }
    let address = match address {
        Some(address) => address,
        None => find_one().await?,
    };
    let pairing = match Pairing::start(Endpoint::secure(address), &options).await {
        Err(webos::Error::Connect(_) | webos::Error::WebSocket(_)) => {
            begin(Endpoint::plain(address), &options).await?
        }
        other => other.map_err(|error| connect::refused(error, &address))?,
    };
    Ok(Started { pairing, address })
}

async fn begin(endpoint: Endpoint, options: &webos::Options) -> Result<Pairing, AppError> {
    Pairing::start(endpoint, options)
        .await
        .map_err(|error| connect::refused(error, &endpoint.address.ip()))
}

/// The one LG TV on the network.
async fn find_one() -> Result<IpAddr, AppError> {
    let found = webos::discover(DISCOVERY_WAIT).await?;
    match found.as_slice() {
        [] => Err(AppError::Usage(
            "no LG TV answered on the network; give its address: `telka pair 192.168.1.30`".into(),
        )),
        [tv] => tv
            .addresses
            .first()
            .copied()
            .ok_or_else(|| AppError::Usage(format!("{} announced no address", tv.name))),
        several => {
            let list: Vec<String> = several
                .iter()
                .map(|tv| {
                    let address = tv
                        .addresses
                        .first()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    format!("{} at {address}", tv.name)
                })
                .collect();
            Err(AppError::Usage(format!(
                "several TVs answered: {}; give the address of one: `telka pair <address>`",
                list.join(", ")
            )))
        }
    }
}

/// Waits for someone to accept the prompt, and keeps the pairing: identity, model, and the
/// interface to wake.
pub async fn finish(paths: &Paths, started: Started) -> Result<(Client, Paired), AppError> {
    let manifest = started.manifest();
    let client = started.pairing.finish(webos::PROMPT_WINDOW).await?;
    let info: SystemInfo = client.get().await?;
    let interfaces: Option<Interfaces> = client.get().await.ok();
    let link: Option<LinkStatus> = client.get().await.ok();
    let credentials = client.paired();
    let paired = Paired {
        uuid: credentials.uuid.clone(),
        model: info.model,
        address: started.address,
        serial: info.serial,
        mac: interfaces.and_then(|interfaces| interfaces.wake_address(link.as_ref())),
        manifest,
        key: credentials.key.clone(),
        since: jiff::Timestamp::now(),
    };
    paired.save(paths)?;
    Ok((client, paired))
}

/// Why telka could not turn this TV on, if it could not.
pub async fn wake_problem(client: &Client, paired: &Paired) -> Option<String> {
    match client.get::<WakeOnLan>().await {
        Ok(WakeOnLan(true)) if paired.mac.is_some() => None,
        Ok(_) => Some(format!(
            "telka cannot turn this TV on: turn on \"Turn on via Wi-Fi\" ({WAKE_SETTING})"
        )),
        Err(_) => None,
    }
}
