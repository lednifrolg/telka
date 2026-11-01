//! Finding LG TVs on the local network over mDNS.
//!
//! The TV announces no SSAP service, but its AirPlay receiver does: `_airplay._tcp` with TXT
//! records for the manufacturer, model, serial number and firmware. The SSAP ports are then at
//! the same addresses. What identifies the TV for SSAP, its `deviceUUID`, takes a `hello`.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use tokio::time::{Instant, timeout_at};

use crate::error::Error;
use crate::{MDNS_MANUFACTURER, MDNS_SERVICE};

/// An LG TV that announced itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The name in the announcement, like `[LG] webOS TV OLED65C11LB`.
    pub name: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    /// Like `03.53.45`.
    pub firmware: Option<String>,
    /// IPv4 first, then IPv6, without link-local addresses.
    pub addresses: Vec<IpAddr>,
}

/// Every LG TV that answers within `wait`.
pub async fn discover(wait: Duration) -> Result<Vec<Discovered>, Error> {
    browse(wait, |_| false).await
}

/// The TV with this serial number, as soon as it answers, or `None` after `wait`.
pub async fn find(serial: &str, wait: Duration) -> Result<Option<Discovered>, Error> {
    let matches = |tv: &Discovered| tv.serial.as_deref() == Some(serial);
    let found = browse(wait, matches).await?;
    Ok(found.into_iter().find(matches))
}

async fn browse(
    wait: Duration,
    done: impl Fn(&Discovered) -> bool,
) -> Result<Vec<Discovered>, Error> {
    let failed = |error: mdns_sd::Error| Error::Discovery(error.to_string());
    let daemon = ServiceDaemon::new().map_err(failed)?;
    let events = daemon.browse(MDNS_SERVICE).map_err(failed)?;
    let deadline = Instant::now() + wait;
    let mut tvs: Vec<Discovered> = Vec::new();
    while let Ok(Ok(event)) = timeout_at(deadline, events.recv_async()).await {
        let ServiceEvent::ServiceResolved(service) = event else {
            continue;
        };
        let ips = service
            .get_addresses()
            .iter()
            .map(mdns_sd::ScopedIp::to_ip_addr);
        let txt = |key: &str| {
            service
                .txt_properties
                .get_property_val_str(key)
                .map(str::to_owned)
        };
        let Some(tv) = announced(service.get_fullname(), ips, txt) else {
            continue;
        };
        let finished = done(&tv);
        match tvs.iter_mut().find(|known| known.name == tv.name) {
            Some(known) => {
                let ips: BTreeSet<IpAddr> = known
                    .addresses
                    .iter()
                    .chain(&tv.addresses)
                    .copied()
                    .collect();
                *known = Discovered {
                    addresses: ordered(ips),
                    ..tv
                };
            }
            None => tvs.push(tv),
        }
        if finished {
            break;
        }
    }
    // The daemon runs on its own thread; it stops on its own when shutdown fails.
    let _ = daemon.shutdown();
    Ok(tvs)
}

/// The TV in one AirPlay announcement, if an LG TV made it. `txt` reads a TXT record field.
fn announced(
    fullname: &str,
    ips: impl IntoIterator<Item = IpAddr>,
    txt: impl Fn(&str) -> Option<String>,
) -> Option<Discovered> {
    if txt("manufacturer").as_deref() != Some(MDNS_MANUFACTURER) {
        return None;
    }
    let name = fullname
        .strip_suffix(MDNS_SERVICE)
        .map_or(fullname, |name| name.trim_end_matches('.'))
        .to_owned();
    // `serialNumber` comes with the MAC address and a newline after an underscore.
    let serial = txt("serialNumber")
        .and_then(|text| {
            text.split('_')
                .next()
                .map(|serial| serial.trim().to_owned())
        })
        .filter(|serial| !serial.is_empty());
    // `fv` is like `p20.03.53.45`: a prefix, then the version the TV's menu shows.
    let firmware = txt("fv").map(|fv| match fv.split_once('.') {
        Some((prefix, version)) if prefix.starts_with('p') => version.to_owned(),
        _ => fv,
    });
    Some(Discovered {
        name,
        model: txt("model"),
        serial,
        firmware,
        addresses: ordered(ips.into_iter().collect()),
    })
}

fn ordered(ips: BTreeSet<IpAddr>) -> Vec<IpAddr> {
    let mut usable: Vec<IpAddr> = ips
        .into_iter()
        .filter(|ip| match ip {
            IpAddr::V4(_) => true,
            IpAddr::V6(v6) => !v6.is_unicast_link_local(),
        })
        .collect();
    usable.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    usable
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn txt(fields: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let fields: HashMap<String, String> = fields
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| fields.get(key).cloned()
    }

    #[test]
    fn lg_announcements_become_tvs() {
        let tv = announced(
            "[LG] webOS TV OLED55C16LA._airplay._tcp.local.",
            [
                "fd00::30".parse().unwrap(),
                "fe80::30".parse().unwrap(),
                "192.168.1.30".parse().unwrap(),
            ],
            txt(&[
                ("manufacturer", "LG Electronics"),
                ("model", "OLED55C16LA"),
                ("serialNumber", "101MAXX00001_02:00:5e:10:00:02\n"),
                ("fv", "p20.03.53.45"),
            ]),
        )
        .unwrap();
        assert_eq!(tv.name, "[LG] webOS TV OLED55C16LA");
        assert_eq!(tv.model.as_deref(), Some("OLED55C16LA"));
        assert_eq!(tv.serial.as_deref(), Some("101MAXX00001"));
        assert_eq!(tv.firmware.as_deref(), Some("03.53.45"));
        let addresses: Vec<String> = tv.addresses.iter().map(ToString::to_string).collect();
        assert_eq!(addresses, ["192.168.1.30", "fd00::30"]);
    }

    #[test]
    fn other_airplay_receivers_are_skipped() {
        assert_eq!(
            announced(
                "Speaker._airplay._tcp.local.",
                [],
                txt(&[("manufacturer", "Other")])
            ),
            None
        );
        assert_eq!(
            announced("Speaker._airplay._tcp.local.", [], txt(&[])),
            None
        );
        let bare = announced(
            "x",
            [],
            txt(&[("manufacturer", "LG Electronics"), ("fv", "1.0")]),
        )
        .unwrap();
        assert_eq!(bare.name, "x");
        assert_eq!(bare.firmware.as_deref(), Some("1.0"));
        assert_eq!(bare.serial, None);
    }
}
