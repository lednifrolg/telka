//! Turning the TV on with Wake-on-LAN.
//!
//! A sleeping TV closes its SSAP sockets, so no request can wake it. A magic packet can, when
//! "Turn on via Wi-Fi" is on in the TV's settings: six `0xFF` bytes and the MAC address of the
//! interface it sleeps on, sixteen times, as a UDP broadcast.
//!
//! On a C1 that works only after the TV went off by its power button (see
//! [`Client::turn_off`](crate::Client::turn_off)), and the TV ignores the packets for about 30
//! seconds after going off. [`wake_and_wait`] therefore sends one every second until the TV
//! answers.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::net::UdpSocket;
use tokio::time::{Instant, sleep_until};

use crate::error::Error;
use crate::pair::{Hello, Options, hello};
use crate::transport::Endpoint;

/// A MAC address, shown as six lowercase hex pairs separated by colons.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacAddress({self})")
    }
}

/// The text is not a MAC address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a MAC address: expected six hex pairs separated by colons or dashes")]
pub struct ParseMacError;

impl FromStr for MacAddress {
    type Err = ParseMacError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = text.trim().split([':', '-']).collect();
        if parts.len() != 6 {
            return Err(ParseMacError);
        }
        let mut bytes = [0; 6];
        for (byte, part) in bytes.iter_mut().zip(parts) {
            if part.len() != 2 {
                return Err(ParseMacError);
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| ParseMacError)?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for MacAddress {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MacAddress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The magic packet that wakes the interface with `mac`.
pub fn magic_packet(mac: MacAddress) -> [u8; 102] {
    let mut packet = [0xFF; 102];
    for copy in packet[6..].as_chunks_mut::<6>().0 {
        copy.copy_from_slice(&mac.0);
    }
    packet
}

/// How often each packet goes out: a sleeping Wi-Fi interface can miss one.
const REPEAT: usize = 3;
const REPEAT_GAP: Duration = Duration::from_millis(100);

/// The broadcast to ports 9 and 7, where TVs listen for magic packets.
fn broadcast() -> [SocketAddr; 2] {
    let broadcast = Ipv4Addr::BROADCAST.into();
    [SocketAddr::new(broadcast, 9), SocketAddr::new(broadcast, 7)]
}

/// Sends the magic packet for `mac` as a broadcast to ports 9 and 7, three times.
pub async fn wake(mac: MacAddress) -> io::Result<()> {
    wake_to(mac, &broadcast()).await
}

/// Sends the magic packet for `mac` to each of `targets`, three times.
pub async fn wake_to(mac: MacAddress, targets: &[SocketAddr]) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.set_broadcast(true)?;
    let packet = magic_packet(mac);
    for round in 0..REPEAT {
        if round > 0 {
            tokio::time::sleep(REPEAT_GAP).await;
        }
        for target in targets {
            socket.send_to(&packet, target).await?;
        }
    }
    Ok(())
}

/// How long waking may take: up to 40 seconds while the TV still ignores packets, then a few
/// seconds until it answers.
pub const WAKE_WINDOW: Duration = Duration::from_secs(90);

const WAKE_EVERY: Duration = Duration::from_secs(1);

/// Sends the magic packet for `mac` every second until the TV at `ip` answers on its TLS port, or
/// `wait` passed ([`Error::TimedOut`]). Returns the TV's `hello`. The packets go to the broadcast
/// to everyone and to the broadcast of the local network the TV is on, since networks may pass on
/// only one of them. A TV that was just turned off still answers for a few seconds: wait until it
/// stops before calling this.
pub async fn wake_and_wait(
    mac: MacAddress,
    ip: IpAddr,
    options: &Options,
    wait: Duration,
) -> Result<Hello, Error> {
    let targets = targets_for(ip, if_addrs::get_if_addrs().unwrap_or_default());
    wake_until(mac, &targets, Endpoint::secure(ip), options, wait).await
}

/// Ports 9 and 7 of the broadcast to everyone and of the broadcast of each local network that
/// `ip` belongs to.
fn targets_for(ip: IpAddr, interfaces: Vec<if_addrs::Interface>) -> Vec<SocketAddr> {
    let mut addresses = vec![Ipv4Addr::BROADCAST];
    if let IpAddr::V4(tv) = ip {
        for interface in interfaces {
            if let if_addrs::IfAddr::V4(local) = interface.addr
                && !local.is_loopback()
                && same_network(local.ip, tv, local.netmask)
            {
                addresses.push(
                    local
                        .broadcast
                        .unwrap_or_else(|| directed_broadcast(local.ip, local.netmask)),
                );
            }
        }
    }
    addresses.dedup();
    addresses
        .into_iter()
        .flat_map(|address| [9, 7].map(|port| SocketAddr::new(address.into(), port)))
        .collect()
}

fn same_network(a: Ipv4Addr, b: Ipv4Addr, netmask: Ipv4Addr) -> bool {
    let mask = u32::from(netmask);
    u32::from(a) & mask == u32::from(b) & mask
}

fn directed_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(ip) | !u32::from(netmask))
}

/// Like [`wake_and_wait`], with the packets going to `targets` and the TV answering at
/// `endpoint`.
pub async fn wake_until(
    mac: MacAddress,
    targets: &[SocketAddr],
    endpoint: Endpoint,
    options: &Options,
    wait: Duration,
) -> Result<Hello, Error> {
    let deadline = Instant::now() + wait;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(Error::Connect)?;
    socket.set_broadcast(true).map_err(Error::Connect)?;
    let packet = magic_packet(mac);
    // Each look at the TV may take a second at most, so a packet still goes out every second.
    let mut looking = options.clone();
    looking.timing.connect = looking.timing.connect.min(WAKE_EVERY);
    loop {
        let round = Instant::now();
        for target in targets {
            socket
                .send_to(&packet, target)
                .await
                .map_err(Error::Connect)?;
        }
        match hello(endpoint, &looking).await {
            Ok(hello) => return Ok(hello),
            // Still asleep, or only waking up.
            Err(Error::Connect(_) | Error::TimedOut | Error::WebSocket(_) | Error::Closed) => {}
            Err(other) => return Err(other),
        }
        if Instant::now() >= deadline {
            return Err(Error::TimedOut);
        }
        sleep_until((round + WAKE_EVERY).min(deadline)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_addresses_parse_and_print() {
        let mac: MacAddress = "02:00:5E:10:00:01".parse().unwrap();
        assert_eq!(mac.to_string(), "02:00:5e:10:00:01");
        assert_eq!("02-00-5e-10-00-01".parse(), Ok(mac));
        assert_eq!(mac.octets(), [0x02, 0x00, 0x5e, 0x10, 0x00, 0x01]);
        for bad in [
            "02:00:5e:10:00",
            "02:00:5e:10:00:0g",
            "0200:5e:10:00:01:02",
            "",
        ] {
            assert_eq!(bad.parse::<MacAddress>(), Err(ParseMacError), "{bad}");
        }
        let json = serde_json::to_string(&mac).unwrap();
        assert_eq!(json, r#""02:00:5e:10:00:01""#);
        assert_eq!(serde_json::from_str::<MacAddress>(&json).unwrap(), mac);
    }

    #[test]
    fn packets_also_go_to_the_tvs_own_network() {
        let interface = |ip: &str, netmask: &str, broadcast: Option<&str>| if_addrs::Interface {
            name: "eth0".into(),
            addr: if_addrs::IfAddr::V4(if_addrs::Ifv4Addr {
                ip: ip.parse().unwrap(),
                netmask: netmask.parse().unwrap(),
                prefixlen: 24,
                broadcast: broadcast.map(|b| b.parse().unwrap()),
            }),
            index: None,
            oper_status: if_addrs::IfOperStatus::Up,
            is_p2p: false,
        };
        let targets = targets_for(
            "192.168.1.30".parse().unwrap(),
            vec![
                interface("127.0.0.1", "255.0.0.0", None),
                interface("192.168.1.20", "255.255.255.0", None),
                interface("10.0.0.2", "255.255.255.0", Some("10.0.0.255")),
            ],
        );
        let shown: Vec<String> = targets.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "255.255.255.255:9",
                "255.255.255.255:7",
                "192.168.1.255:9",
                "192.168.1.255:7"
            ]
        );
        assert_eq!(
            targets_for("fd00::30".parse().unwrap(), Vec::new()).len(),
            2
        );
    }

    #[test]
    fn magic_packets_repeat_the_address() {
        let mac: MacAddress = "02:00:5e:10:00:01".parse().unwrap();
        let packet = magic_packet(mac);
        assert_eq!(packet[..6], [0xFF; 6]);
        assert!(packet[6..].chunks(6).all(|copy| copy == mac.octets()));
        assert_eq!(packet[6..].chunks(6).count(), 16);
    }
}
