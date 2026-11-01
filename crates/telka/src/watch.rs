//! `telka watch`: every change on the TV as it happens, from the remote, a phone or telka. When
//! the TV goes away, the stream says so and waits for it to come back.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};
use webos::{ForegroundApp, Inputs, LaunchPoints, PowerState, SoundOutput, Volume};

use crate::config::Config;
use crate::connect::Tv;
use crate::error::AppError;
use crate::output::{self, DIM, paint};
use crate::paths::Paths;
use crate::theme;

/// How often to look for a TV that went away.
const RETRY: Duration = Duration::from_secs(3);

struct Stream {
    json: bool,
    /// The last value of everything the stream tells, and how it read, to print only changes.
    last: HashMap<&'static str, (Value, String)>,
}

impl Stream {
    /// Prints `what` when it differs from the last value: the first value too, from nothing.
    fn tell(&mut self, what: &'static str, now: &Value, text: &str) {
        if self.last.get(what).is_some_and(|(last, _)| last == now) {
            return;
        }
        let from = self.last.insert(what, (now.clone(), text.to_owned()));
        if self.json {
            let event = json!({"time": jiff::Timestamp::now().to_string(), "what": what,
                               "from": from.map(|(value, _)| value), "to": now});
            output::line(event);
            return;
        }
        let time = jiff::Zoned::now().strftime("%H:%M:%S").to_string();
        let change = match from {
            Some((_, before)) => format!("{before} → {text}"),
            None => text.to_owned(),
        };
        output::line(format!(
            "{}  {}{change}",
            paint(DIM, time),
            output::pad(what, 8)
        ));
    }
}

pub async fn watch(paths: &Paths, config: &Config, json: bool) -> Result<(), AppError> {
    let mut stream = Stream {
        json,
        last: HashMap::new(),
    };
    loop {
        match Tv::connect(paths, config).await {
            Ok(tv) => follow(&tv, &mut stream).await?,
            Err(AppError::Unreachable(_)) => {}
            Err(error) => return Err(error),
        }
        stream.tell("power", &json!("off"), "off");
        tokio::time::sleep(RETRY).await;
    }
}

/// Tells every change until the connection ends.
async fn follow(tv: &Tv, stream: &mut Stream) -> Result<(), AppError> {
    let client = &tv.client;
    let apps: Option<LaunchPoints> = client.get().await.ok();
    let inputs: Option<Inputs> = client.get().await.ok();
    let mut power = client.watch::<PowerState>().await?;
    let mut front = client.watch::<ForegroundApp>().await?;
    let mut volume = client.watch::<Volume>().await?;
    let mut output = client.watch::<SoundOutput>().await?;

    let tell_power = |stream: &mut Stream, power: &PowerState| {
        let word = theme::power(power);
        stream.tell("power", &json!(word), word);
    };
    let tell_app = |stream: &mut Stream, front: &ForegroundApp| {
        let title = theme::in_front(&front.0, apps.as_ref(), inputs.as_ref());
        stream.tell("app", &json!(front.0), &title);
    };
    let tell_volume = |stream: &mut Stream, volume: &Volume| {
        stream.tell("volume", &json!(volume.level), &volume.level.to_string());
        let muted = if volume.muted { "muted" } else { "unmuted" };
        stream.tell("mute", &json!(volume.muted), muted);
    };
    let tell_output = |stream: &mut Stream, output: &SoundOutput| {
        stream.tell("sound", &json!(output.0), &theme::sound_output(&output.0));
    };
    tell_power(stream, power.current());
    tell_app(stream, front.current());
    tell_volume(stream, volume.current());
    tell_output(stream, output.current());
    loop {
        tokio::select! {
            change = power.next() => match change {
                Some(Ok(change)) => tell_power(stream, &change),
                Some(Err(_)) => {}
                None => return Ok(()),
            },
            change = front.next() => match change {
                Some(Ok(change)) => tell_app(stream, &change),
                Some(Err(_)) => {}
                None => return Ok(()),
            },
            change = volume.next() => match change {
                Some(Ok(change)) => tell_volume(stream, &change),
                Some(Err(_)) => {}
                None => return Ok(()),
            },
            change = output.next() => match change {
                Some(Ok(change)) => tell_output(stream, &change),
                Some(Err(_)) => {}
                None => return Ok(()),
            },
        }
    }
}
