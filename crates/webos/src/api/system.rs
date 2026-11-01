//! What the TV is, and its network interfaces.

use serde_json::Value;

use super::{Report, missing};
use crate::wol::MacAddress;

/// Model and serial number: `system/getSystemInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    /// Like `OLED65C11LB`.
    pub model: String,
    pub serial: Option<String>,
    /// The broadcast standard of the tuner, like `DVB` or `ATSC`.
    pub receiver: Option<String>,
}

impl Report for SystemInfo {
    const URI: &'static str = "system/getSystemInfo";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let text = |key: &str| payload[key].as_str().map(str::to_owned);
        Ok(Self {
            model: text("modelName").ok_or_else(|| missing("modelName"))?,
            serial: text("serialNumber"),
            receiver: text("receiverType"),
        })
    }
}

/// The MAC addresses of the TV's interfaces: `com.webos.service.connectionmanager/getinfo`. The
/// TV lists the wired one also when no cable is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interfaces {
    pub wifi: Option<MacAddress>,
    pub wired: Option<MacAddress>,
}

impl Report for Interfaces {
    const URI: &'static str = "com.webos.service.connectionmanager/getinfo";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let mac = |key: &str| payload[key]["macAddress"].as_str()?.parse().ok();
        Ok(Self {
            wifi: mac("wifiInfo"),
            wired: mac("wiredInfo"),
        })
    }
}

/// Which interfaces are up: `com.webos.service.connectionmanager/getstatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkStatus {
    pub wifi: bool,
    pub wired: bool,
    /// What the Wi-Fi driver reports about waking on LAN. A C1 with "Turn on via Wi-Fi" enabled
    /// still says `false` here while it is on.
    pub wake_on_wifi: Option<bool>,
}

impl Report for LinkStatus {
    const URI: &'static str = "com.webos.service.connectionmanager/getstatus";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let up = |key: &str| payload[key]["state"].as_str() == Some("connected");
        Ok(Self {
            wifi: up("wifi"),
            wired: up("wired"),
            wake_on_wifi: payload["wifi"]["isWakeOnWifiEnabled"].as_bool(),
        })
    }
}

impl Interfaces {
    /// The MAC address to wake: the interface that is up, Wi-Fi when neither says.
    pub fn wake_address(&self, status: Option<&LinkStatus>) -> Option<MacAddress> {
        match status {
            Some(status) if status.wired && !status.wifi => self.wired.or(self.wifi),
            _ => self.wifi.or(self.wired),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn interfaces_pick_the_one_that_is_up() {
        let interfaces = Interfaces::read(&json!({"returnValue": true, "subscribed": false,
            "wifiInfo": {"macAddress": "02:00:5E:10:00:01"}, "wiredInfo": {"macAddress": "02:00:5E:10:00:02"},
            "p2pInfo": {"macAddress": "02:00:5E:10:00:03"}}))
        .unwrap();
        let wifi: MacAddress = "02:00:5e:10:00:01".parse().unwrap();
        let wired: MacAddress = "02:00:5e:10:00:02".parse().unwrap();
        assert_eq!(interfaces.wifi, Some(wifi));
        let status = LinkStatus::read(
            &json!({"wifi": {"state": "connected", "isWakeOnWifiEnabled": false},
            "wired": {"state": "disconnected", "plugged": false}}),
        )
        .unwrap();
        assert!(status.wifi && !status.wired);
        assert_eq!(status.wake_on_wifi, Some(false));
        assert_eq!(interfaces.wake_address(Some(&status)), Some(wifi));
        let cabled = LinkStatus {
            wifi: false,
            wired: true,
            wake_on_wifi: None,
        };
        assert_eq!(interfaces.wake_address(Some(&cabled)), Some(wired));
        assert_eq!(interfaces.wake_address(None), Some(wifi));
        let info = SystemInfo::read(&json!({"modelName": "OLED55C16LA", "serialNumber": "101MAXX00001", "receiverType": "DVB"})).unwrap();
        assert_eq!(info.model, "OLED55C16LA");
        assert!(SystemInfo::read(&json!({})).is_err());
    }
}
