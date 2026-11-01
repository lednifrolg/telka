//! Apps: what is installed, what is in front, starting and closing them, and playback.

use serde_json::{Value, json};

use super::{Report, missing};
use crate::client::Client;
use crate::error::Error;

/// An app as the home screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub id: String,
    pub title: String,
    /// Path of a medium-sized icon on the TV's file system.
    pub icon: Option<String>,
    /// Part of webOS rather than installed from the store.
    pub system: bool,
    pub removable: bool,
}

/// The apps on the home screen, in the TV's order: `listLaunchPoints`. Hidden system apps are
/// not among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPoints(pub Vec<App>);

impl Report for LaunchPoints {
    const URI: &'static str = "com.webos.applicationManager/listLaunchPoints";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let points = payload["launchPoints"]
            .as_array()
            .ok_or_else(|| missing("launchPoints"))?;
        let apps = points
            .iter()
            .filter_map(|point| {
                let text = |key: &str| point[key].as_str().map(str::to_owned);
                Some(App {
                    id: text("id")?,
                    title: text("title").unwrap_or_default(),
                    icon: text("mediumLargeIcon").or_else(|| text("icon")),
                    system: point["systemApp"].as_bool().unwrap_or(false),
                    removable: point["removable"].as_bool().unwrap_or(false),
                })
            })
            .collect();
        Ok(Self(apps))
    }
}

/// The app in front: a streaming app, `com.webos.app.hdmi3` for an HDMI input, or
/// `com.webos.app.home` and the like for the TV's own screens. Empty while nothing is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApp(pub String);

impl Report for ForegroundApp {
    const URI: &'static str = "com.webos.applicationManager/getForegroundAppInfo";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        payload["appId"]
            .as_str()
            .map(|id| Self(id.to_owned()))
            .ok_or_else(|| missing("appId"))
    }
}

/// Playback in the app in front. The TV takes these whether anything plays or not, and does not
/// say what plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Play,
    Pause,
    Stop,
    Rewind,
    FastForward,
}

impl Client {
    /// Starts or brings forward the app `id`. `content` is what the app should open, like a
    /// YouTube video URL; apps that understand it take it from both `contentId` and the
    /// `contentTarget` parameter.
    pub async fn launch(&self, id: &str, content: Option<&str>) -> Result<(), Error> {
        let payload = match content {
            Some(content) => {
                json!({"id": id, "contentId": content, "params": {"contentTarget": content}})
            }
            None => json!({"id": id}),
        };
        self.request("system.launcher/launch", payload).await?;
        Ok(())
    }

    /// Opens `url` in the TV's web browser.
    pub async fn open_url(&self, url: &str) -> Result<(), Error> {
        self.request("system.launcher/open", json!({"target": url}))
            .await?;
        Ok(())
    }

    pub async fn close_app(&self, id: &str) -> Result<(), Error> {
        self.request("system.launcher/close", json!({"id": id}))
            .await?;
        Ok(())
    }

    pub async fn media(&self, action: Media) -> Result<(), Error> {
        let uri = match action {
            Media::Play => "media.controls/play",
            Media::Pause => "media.controls/pause",
            Media::Stop => "media.controls/stop",
            Media::Rewind => "media.controls/rewind",
            Media::FastForward => "media.controls/fastForward",
        };
        self.request(uri, json!({})).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_points_keep_order_and_skip_broken_entries() {
        let points = LaunchPoints::read(&json!({"returnValue": true, "subscribed": false, "caseDetail": {"change": []},
            "launchPoints": [
                {"id": "netflix", "title": "Netflix", "systemApp": false, "removable": true,
                 "mediumLargeIcon": "/media/cryptofs/apps/usr/palm/applications/netflix/icon.png", "params": {}},
                {"title": "no id"},
                {"id": "com.webos.app.browser", "title": "Web Browser", "systemApp": true, "removable": false,
                 "icon": "/usr/palm/applications/com.webos.app.browser/icon.png"}]}))
        .unwrap();
        let ids: Vec<&str> = points.0.iter().map(|app| app.id.as_str()).collect();
        assert_eq!(ids, ["netflix", "com.webos.app.browser"]);
        assert!(points.0[1].system && !points.0[1].removable);
        assert!(points.0[1].icon.as_deref().unwrap().ends_with("icon.png"));
        assert!(LaunchPoints::read(&json!({})).is_err());
        assert_eq!(
            ForegroundApp::read(&json!({"appId": "com.webos.app.hdmi3", "windowId": ""})).unwrap(),
            ForegroundApp("com.webos.app.hdmi3".into())
        );
    }
}
