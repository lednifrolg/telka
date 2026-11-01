//! SSAP frames: one JSON object per WebSocket text message.
//!
//! The client sends `hello`, `register`, `request`, `subscribe` and `unsubscribe` frames, each
//! with an `id` of its choosing; the TV answers with `hello`, `response`, `registered` or `error`
//! frames that carry the same `id`. A subscription keeps its `id`: every later change arrives as
//! another `response` with it.

use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::SSAP_SCHEME;
use crate::error::Error;
use crate::pair::{ClientKey, Manifest};

/// What kind of frame the TV sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    Hello,
    Response,
    Registered,
    Error,
    Other(String),
}

/// One frame from the TV.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Incoming {
    pub(crate) kind: Kind,
    /// The `id` of the frame it answers. The TV echoes ours, which are strings; a number is
    /// read as its decimal text.
    pub(crate) id: Option<String>,
    /// For `error` frames, e.g. `401 insufficient permissions`.
    pub(crate) error: Option<String>,
    /// `null` when the frame has none.
    pub(crate) payload: Value,
    /// The whole frame as the TV sent it.
    pub(crate) raw: Value,
}

impl Incoming {
    pub(crate) fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let raw: Value = serde_json::from_str(text)?;
        let kind = match raw["type"].as_str().unwrap_or_default() {
            "hello" => Kind::Hello,
            "response" => Kind::Response,
            "registered" => Kind::Registered,
            "error" => Kind::Error,
            other => Kind::Other(other.to_owned()),
        };
        let id = match &raw["id"] {
            Value::String(id) => Some(id.clone()),
            Value::Number(id) => Some(id.to_string()),
            _ => None,
        };
        Ok(Self {
            kind,
            id,
            error: raw["error"].as_str().map(str::to_owned),
            payload: raw["payload"].clone(),
            raw,
        })
    }

    /// What the frame means for the request with `uri` it answers: the payload, or why the TV
    /// refused.
    pub(crate) fn outcome(&self, uri: &str) -> Result<Value, Error> {
        match self.kind {
            Kind::Error => Err(tv_error(
                self.error.as_deref().unwrap_or_default(),
                &self.payload,
                uri,
            )),
            Kind::Response if self.payload["returnValue"] == Value::Bool(false) => {
                Err(Error::Rejected {
                    message: error_text(&self.payload).unwrap_or_else(|| "no reason given".into()),
                })
            }
            Kind::Response => Ok(self.payload.clone()),
            _ => Err(Error::Protocol(format!(
                "a {:?} frame answered {uri}",
                self.kind
            ))),
        }
    }
}

/// The TV's own explanation in a payload, as `errorText` or `errorMessage`.
fn error_text(payload: &Value) -> Option<String> {
    ["errorText", "errorMessage"]
        .iter()
        .find_map(|key| payload[key].as_str())
        .map(str::to_owned)
}

/// Reads an `error` frame: a status code and a reason, like `404 no such service or method`,
/// and for status 500 the payload's `errorText`.
fn tv_error(error: &str, payload: &Value, uri: &str) -> Error {
    let (code, reason) = error.split_once(' ').unwrap_or((error, ""));
    match code.parse::<u16>() {
        Ok(401 | 403) => Error::Denied {
            message: reason.to_owned(),
        },
        Ok(404) => Error::Unsupported {
            uri: uri.to_owned(),
        },
        _ => Error::Rejected {
            message: error_text(payload).unwrap_or_else(|| error.to_owned()),
        },
    }
}

/// A request URI: `audio/getVolume` becomes `ssap://audio/getVolume`; one with its own scheme,
/// such as `luna://`, stays as it is.
pub(crate) fn uri(path: &str) -> String {
    if path.contains("://") {
        path.to_owned()
    } else {
        format!("{SSAP_SCHEME}{path}")
    }
}

pub(crate) fn hello() -> String {
    json!({"id": "hello", "type": "hello", "payload": {}}).to_string()
}

pub(crate) fn request(id: &str, uri: &str, payload: &Value) -> String {
    json!({"id": id, "type": "request", "uri": uri, "payload": payload}).to_string()
}

pub(crate) fn subscribe(id: &str, uri: &str, payload: &Value) -> String {
    json!({"id": id, "type": "subscribe", "uri": uri, "payload": payload}).to_string()
}

pub(crate) fn unsubscribe(id: &str, uri: &str) -> String {
    json!({"id": id, "type": "unsubscribe", "uri": uri}).to_string()
}

/// The register frame. The manifest goes in as LG wrote it, so its signature stays valid.
pub(crate) fn register(id: &str, manifest: Manifest, key: Option<&ClientKey>) -> String {
    #[derive(serde::Serialize)]
    struct Payload<'a> {
        #[serde(rename = "forcePairing")]
        force_pairing: bool,
        #[serde(rename = "pairingType")]
        pairing_type: &'static str,
        #[serde(rename = "client-key", skip_serializing_if = "Option::is_none")]
        client_key: Option<&'a str>,
        manifest: &'a RawValue,
    }
    let manifest: &RawValue =
        serde_json::from_str(manifest.json()).expect("the bundled manifests are valid JSON");
    let payload = Payload {
        force_pairing: false,
        pairing_type: "PROMPT",
        client_key: key.map(ClientKey::as_str),
        manifest,
    };
    json!({"id": id, "type": "register", "payload": payload}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(text: &str) -> Incoming {
        Incoming::parse(text).unwrap()
    }

    #[test]
    fn frames_keep_their_kind_id_and_payload() {
        let response = frame(r#"{"type":"response","id":"7","payload":{"returnValue":true}}"#);
        assert_eq!(response.kind, Kind::Response);
        assert_eq!(response.id.as_deref(), Some("7"));
        assert_eq!(response.outcome("x").unwrap(), json!({"returnValue": true}));
        let numbered = frame(r#"{"type":"response","id":7,"payload":{}}"#);
        assert_eq!(numbered.id.as_deref(), Some("7"));
        let odd = frame(r#"{"type":"surprise"}"#);
        assert_eq!(odd.kind, Kind::Other("surprise".into()));
        assert_eq!(odd.payload, Value::Null);
        assert!(Incoming::parse("not json").is_err());
    }

    #[test]
    fn errors_say_why() {
        let denied = frame(
            r#"{"type":"error","id":"1","error":"401 insufficient permissions","payload":{}}"#,
        );
        assert!(
            matches!(denied.outcome("u"), Err(Error::Denied { message }) if message == "insufficient permissions")
        );
        let forbidden =
            frame(r#"{"type":"error","id":"1","error":"403 access denied","payload":{}}"#);
        assert!(matches!(forbidden.outcome("u"), Err(Error::Denied { .. })));
        let missing = frame(
            r#"{"type":"error","id":"1","error":"404 no such service or method","payload":{}}"#,
        );
        assert!(
            matches!(missing.outcome("ssap://x/y"), Err(Error::Unsupported { uri }) if uri == "ssap://x/y")
        );
        let failed = frame(
            r#"{"type":"error","id":"1","error":"500 Application error","payload":{"returnValue":false,"errorCode":-1000,"errorText":"Some keys are not allowed for the request. ( pictureMode )"}}"#,
        );
        assert!(
            matches!(failed.outcome("u"), Err(Error::Rejected { message }) if message.contains("pictureMode"))
        );
        let bare = frame(
            r#"{"type":"error","id":"1","error":"1 Could not validate json message against schema"}"#,
        );
        assert!(
            matches!(bare.outcome("u"), Err(Error::Rejected { message }) if message.starts_with("1 Could not"))
        );
        let refused = frame(
            r#"{"type":"response","id":"1","payload":{"returnValue":false,"errorText":"busy"}}"#,
        );
        assert!(
            matches!(refused.outcome("u"), Err(Error::Rejected { message }) if message == "busy")
        );
        let hello = frame(r#"{"type":"hello","payload":{}}"#);
        assert!(matches!(hello.outcome("u"), Err(Error::Protocol(_))));
    }

    #[test]
    fn uris_get_the_ssap_scheme_unless_they_have_one() {
        assert_eq!(uri("audio/getVolume"), "ssap://audio/getVolume");
        assert_eq!(uri("ssap://audio/getVolume"), "ssap://audio/getVolume");
        assert_eq!(
            uri("luna://com.webos.service/x"),
            "luna://com.webos.service/x"
        );
    }

    #[test]
    fn register_carries_the_manifest_as_written_and_the_key() {
        let text = register("register_0", Manifest::Signed, Some(&ClientKey::new("abc")));
        let sent: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(sent["type"], "register");
        assert_eq!(sent["payload"]["client-key"], "abc");
        assert_eq!(sent["payload"]["pairingType"], "PROMPT");
        assert_eq!(
            sent["payload"]["manifest"]["signed"]["appId"],
            "com.lge.test"
        );
        let bundled: Value = serde_json::from_str(Manifest::Signed.json()).unwrap();
        assert_eq!(sent["payload"]["manifest"], bundled);
        let fresh: Value = serde_json::from_str(&register("r", Manifest::Unsigned, None)).unwrap();
        assert!(fresh["payload"].get("client-key").is_none());
        assert!(fresh["payload"]["manifest"].get("signed").is_none());
    }
}
