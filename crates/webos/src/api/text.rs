//! Typing into the TV's text fields, and messages on its screen.

use serde_json::{Value, json};

use super::Report;
use crate::client::Client;
use crate::error::Error;

/// Whether a text field on the TV has focus, from the remote keyboard subscription
/// `com.webos.service.ime/registerRemoteKeyboard`. Only a subscription makes sense: its first
/// answer says nothing about focus, later ones come whenever focus changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keyboard {
    pub focused: bool,
    /// Like `text`, `url` or `number`, when the field says.
    pub content_type: Option<String>,
    /// A password field.
    pub hidden: bool,
}

impl Report for Keyboard {
    const URI: &'static str = "com.webos.service.ime/registerRemoteKeyboard";

    fn read(payload: &Value) -> Result<Self, serde_json::Error> {
        let widget = &payload["currentWidget"];
        Ok(Self {
            focused: widget["focus"].as_bool().unwrap_or(false),
            content_type: widget["contentType"].as_str().map(str::to_owned),
            hidden: widget["hiddenText"].as_bool().unwrap_or(false),
        })
    }
}

impl Client {
    /// Types `text` into the field with focus; `replace` replaces what is there. Only fields that
    /// use the TV's own keyboard take it, like the web browser's; apps with their own on-screen
    /// keyboard, like YouTube, ignore it. The TV answers success either way.
    pub async fn insert_text(&self, text: &str, replace: bool) -> Result<(), Error> {
        self.request(
            "com.webos.service.ime/insertText",
            json!({"text": text, "replace": replace}),
        )
        .await?;
        Ok(())
    }

    /// Deletes `count` characters before the cursor.
    pub async fn delete_characters(&self, count: u32) -> Result<(), Error> {
        self.request(
            "com.webos.service.ime/deleteCharacters",
            json!({"count": count}),
        )
        .await?;
        Ok(())
    }

    /// Enter in the field with focus, which usually submits it.
    pub async fn send_enter(&self) -> Result<(), Error> {
        self.request("com.webos.service.ime/sendEnterKey", json!({}))
            .await?;
        Ok(())
    }

    /// Shows `message` in a toast at the edge of the screen; returns the toast's id when the TV
    /// gives one. Over an app on the full screen, like YouTube, the toast does not show.
    pub async fn toast(&self, message: &str) -> Result<Option<String>, Error> {
        let answer = self
            .request(
                "system.notifications/createToast",
                json!({"message": message}),
            )
            .await?;
        Ok(answer["toastId"].as_str().map(str::to_owned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_focus_comes_from_the_widget() {
        assert_eq!(
            Keyboard::read(&json!({"subscribed": true})).unwrap(),
            Keyboard::default()
        );
        let focused = Keyboard::read(&json!({"currentWidget": {"focus": true, "contentType": "text", "hiddenText": false}, "focusChanged": true})).unwrap();
        assert!(focused.focused && !focused.hidden);
        assert_eq!(focused.content_type.as_deref(), Some("text"));
    }
}
