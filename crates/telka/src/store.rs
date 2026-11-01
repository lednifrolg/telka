//! The paired TV, kept in `tv.json`.

use std::fmt;
use std::net::IpAddr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use webos::{ClientKey, MacAddress, Manifest};

use crate::error::AppError;
use crate::files;
use crate::paths::Paths;

/// What pairing produced. The file holds the client key, so it is readable only by the owner,
/// and `Debug` leaves the key, serial number and MAC address out.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Paired {
    /// The TV's `deviceUUID`; another TV at the address is refused.
    pub uuid: String,
    /// The model, like `OLED65C11LB`.
    pub model: String,
    /// Where the TV answered last.
    pub address: IpAddr,
    /// To find the TV again over mDNS when its address changes.
    pub serial: Option<String>,
    /// The interface to wake with Wake-on-LAN.
    pub mac: Option<MacAddress>,
    /// Which manifest the TV granted the key for: only the signed one allows picture settings.
    pub manifest: Manifest,
    pub key: ClientKey,
    /// When the pairing was made.
    pub since: Timestamp,
}

impl fmt::Debug for Paired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Paired")
            .field("uuid", &self.uuid)
            .field("model", &self.model)
            .field("address", &self.address)
            .field("manifest", &self.manifest)
            .field("since", &self.since)
            .finish_non_exhaustive()
    }
}

impl Paired {
    pub fn load(paths: &Paths) -> Result<Option<Self>, AppError> {
        files::read_json(&paths.tv())
    }

    pub fn save(&self, paths: &Paths) -> Result<(), AppError> {
        files::write_json(&paths.tv(), self)
    }

    /// Forgets the pairing; `false` when there was none.
    pub fn forget(paths: &Paths) -> Result<bool, AppError> {
        files::remove(&paths.tv())
    }

    /// What the library needs to connect.
    pub fn credentials(&self) -> webos::Paired {
        webos::Paired {
            uuid: self.uuid.clone(),
            key: self.key.clone(),
            manifest: self.manifest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired() -> Paired {
        Paired {
            uuid: "4f1c2a7e-0b3d-4e6a-9c58-2d7f1e0a6b93".into(),
            model: "OLED55C16LA".into(),
            address: "192.168.1.30".parse().unwrap(),
            serial: Some("101MAXX00001".into()),
            mac: Some("02:00:5e:10:00:01".parse().unwrap()),
            manifest: Manifest::Signed,
            key: ClientKey::new("5a8e3c1f9d2b4a6e7c0f1b3d5e7a9c2f"),
            since: "2026-11-01T12:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn the_pairing_round_trips_and_debug_hides_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert_eq!(Paired::load(&paths).unwrap(), None);
        paired().save(&paths).unwrap();
        assert_eq!(Paired::load(&paths).unwrap(), Some(paired()));
        let shown = format!("{:?}", paired());
        for secret in ["5a8e3c1f", "101MAXX", "02:00:5e"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(Paired::forget(&paths).unwrap());
        assert!(!Paired::forget(&paths).unwrap());
    }
}
