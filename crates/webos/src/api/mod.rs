//! What the TV reports and what it can be told, typed.
//!
//! Each [`Report`] is one piece of state the TV answers for, like the volume or the app in front;
//! [`Client::get`] reads it once and [`Client::watch`] follows its changes. The commands are
//! methods on [`Client`] in the modules below.

mod apps;
mod audio;
mod inputs;
mod power;
mod settings;
mod system;
mod text;

use serde_json::{Value, json};

pub use apps::{App, ForegroundApp, LaunchPoints, Media};
pub use audio::{SoundOutput, Volume};
pub use inputs::{Input, Inputs};
pub use power::{PowerMode, PowerState};
pub use settings::{Picture, SoundSettings, WakeOnLan};
pub use system::{Interfaces, LinkStatus, SystemInfo};
pub use text::Keyboard;

use crate::client::{Client, Subscription};
use crate::error::Error;
use crate::frame;

/// A piece of state the TV answers for at one URI.
pub trait Report: Sized {
    /// Where to ask, without the `ssap://` scheme.
    const URI: &'static str;

    /// What to ask with; most reports need nothing.
    fn payload() -> Value {
        json!({})
    }

    /// Reads an answer or a change.
    fn read(payload: &Value) -> Result<Self, serde_json::Error>;
}

fn read<T: Report>(payload: &Value) -> Result<T, Error> {
    T::read(payload).map_err(|source| Error::Decode {
        uri: frame::uri(T::URI),
        source,
    })
}

impl Client {
    /// Reads `T` once.
    pub async fn get<T: Report>(&self) -> Result<T, Error> {
        read(&self.request(T::URI, T::payload()).await?)
    }

    /// Follows `T`: the current value, then every change.
    pub async fn watch<T: Report>(&self) -> Result<Watch<T>, Error> {
        let subscription = self.subscribe(T::URI, T::payload()).await?;
        let current = read(subscription.current())?;
        Ok(Watch {
            subscription,
            current,
        })
    }
}

/// The changes of one [`Report`]. Dropping it unsubscribes.
pub struct Watch<T> {
    subscription: Subscription,
    current: T,
}

impl<T: Report> Watch<T> {
    /// The value when the watch started.
    pub fn current(&self) -> &T {
        &self.current
    }

    /// The next change; `None` once the connection ended.
    pub async fn next(&mut self) -> Option<Result<T, Error>> {
        let change = self.subscription.next().await?;
        Some(change.and_then(|payload| read(&payload)))
    }
}

/// A JSON string or number as text: the TV writes most settings as strings, some as numbers.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// A missing or mistyped field, for [`Report::read`].
fn missing(field: &str) -> serde_json::Error {
    serde::de::Error::custom(format!("no usable {field}"))
}
