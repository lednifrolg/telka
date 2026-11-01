//! Turning the TV on with Wake-on-LAN.
//!
//! A sleeping TV closes its SSAP sockets, so no request can wake it. A magic packet can, when
//! "Turn on via Wi-Fi" is on in the TV's settings: six `0xFF` bytes and the MAC address of the
//! interface it sleeps on, sixteen times, as a UDP broadcast.

use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::net::UdpSocket;

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

/// Sends the magic packet for `mac` as a broadcast to ports 9 and 7, three times.
pub async fn wake(mac: MacAddress) -> io::Result<()> {
    let broadcast = Ipv4Addr::BROADCAST.into();
    wake_to(
        mac,
        &[SocketAddr::new(broadcast, 9), SocketAddr::new(broadcast, 7)],
    )
    .await
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
    fn magic_packets_repeat_the_address() {
        let mac: MacAddress = "02:00:5e:10:00:01".parse().unwrap();
        let packet = magic_packet(mac);
        assert_eq!(packet[..6], [0xFF; 6]);
        assert!(packet[6..].chunks(6).all(|copy| copy == mac.octets()));
        assert_eq!(packet[6..].chunks(6).count(), 16);
    }
}
