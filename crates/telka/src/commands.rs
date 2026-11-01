//! The commands that read and pair: `pair`, `unpair`, `discover`, `status`, `apps`, `inputs`.

use std::net::IpAddr;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use webos::{
    ForegroundApp, Inputs, LaunchPoints, Manifest, Picture, PowerState, SoundSettings, Volume,
};

use crate::config::Config;
use crate::connect::Tv;
use crate::error::AppError;
use crate::output::{self, BOLD, DIM, GREEN, YELLOW, paint};
use crate::pairing;
use crate::paths::Paths;
use crate::store::Paired;
use crate::theme;

const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

pub async fn pair(paths: &Paths, address: Option<IpAddr>) -> Result<(), AppError> {
    if address.is_none()
        && let Some(paired) = Paired::load(paths)?
    {
        output::line(format!(
            "already paired with {} at {}; `telka unpair` first to pair again",
            paired.model, paired.address
        ));
        return Ok(());
    }
    let started = pairing::start(address).await?;
    let manifest = started.manifest();
    let window = webos::PROMPT_WINDOW.as_secs();
    match manifest {
        Manifest::Signed => output::line(format!(
            "Accept {} on the TV with the remote: it shows a prompt for \"LG Remote App\" ({window} s)",
            paint(BOLD, "telka")
        )),
        Manifest::Unsigned => output::line(format!(
            "Accept the prompt on the TV with the remote ({window} s)"
        )),
    }
    let (client, paired) = pairing::finish(paths, started).await?;
    let manifest_text = match manifest {
        Manifest::Signed => "LG's signed manifest",
        Manifest::Unsigned => "the unsigned manifest",
    };
    output::success(format!(
        "paired with {} at {} ({manifest_text})",
        paired.model, paired.address
    ));
    if manifest == Manifest::Unsigned {
        output::note("the TV refused LG's signed manifest, so picture settings stay read-only");
    }
    if let Some(problem) = pairing::wake_problem(&client, &paired).await {
        output::warn(problem);
    }
    client.close().await;
    Ok(())
}

pub fn unpair(paths: &Paths) -> Result<(), AppError> {
    let paired = Paired::load(paths)?;
    if !Paired::forget(paths)? {
        output::line("not paired with a TV");
        return Ok(());
    }
    let model = paired.map_or_else(|| "the TV".to_owned(), |paired| paired.model);
    output::success(format!("forgot the pairing with {model}"));
    output::note("the TV keeps no list of paired apps, so there is nothing to remove on it");
    Ok(())
}

pub async fn discover(paths: &Paths, json: bool) -> Result<(), AppError> {
    let paired = Paired::load(paths)?;
    let found = webos::discover(DISCOVERY_WAIT).await?;
    let is_paired = |tv: &webos::Discovered| {
        paired
            .as_ref()
            .is_some_and(|paired| paired.serial.is_some() && paired.serial == tv.serial)
    };
    if json {
        let list: Vec<Value> = found
            .iter()
            .map(|tv| {
                json!({"name": tv.name, "model": tv.model, "firmware": tv.firmware,
                       "addresses": tv.addresses, "paired": is_paired(tv)})
            })
            .collect();
        return output::json(&list);
    }
    if found.is_empty() {
        output::line("no LG TV answered on the network");
        return Ok(());
    }
    for tv in &found {
        let addresses: Vec<String> = tv.addresses.iter().map(ToString::to_string).collect();
        let firmware = tv
            .firmware
            .as_deref()
            .map(|firmware| format!("  firmware {firmware}"))
            .unwrap_or_default();
        let mark = if is_paired(tv) {
            paint(GREEN, "  paired")
        } else {
            String::new()
        };
        output::line(format!(
            "{}  {}{}{mark}",
            paint(BOLD, &tv.name),
            addresses.join(", "),
            paint(DIM, firmware)
        ));
    }
    Ok(())
}

/// The state `status` shows. Parts the TV does not share stay out.
#[derive(Serialize)]
struct Status {
    power: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    app: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sound: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    picture: Option<Value>,
    tv: Value,
}

pub async fn status(paths: &Paths, config: &Config, json: bool) -> Result<(), AppError> {
    let tv = match Tv::connect(paths, config).await {
        Ok(tv) => tv,
        // Off is a state, not an error.
        Err(AppError::Unreachable(_)) => return show_off(paths, config, json),
        Err(error) => return Err(error),
    };
    let client = &tv.client;
    let power: PowerState = client.get().await?;
    let app: Option<ForegroundApp> = client.get().await.ok();
    let apps: Option<LaunchPoints> = client.get().await.ok();
    let inputs: Option<Inputs> = client.get().await.ok();
    let volume: Option<Volume> = client.get().await.ok();
    let sound: Option<SoundSettings> = client.get().await.ok();
    let picture: Option<Picture> = client.get().await.ok();
    let in_front = app
        .as_ref()
        .map(|app| theme::in_front(&app.0, apps.as_ref(), inputs.as_ref()));
    let release = client.hello().release.clone();
    let address = tv.client.endpoint().address.ip();
    let transport = if tv.client.endpoint().secure {
        "wss"
    } else {
        "ws"
    };
    let status = Status {
        power: theme::power(&power),
        app: app
            .as_ref()
            .zip(in_front.as_ref())
            .map(|(app, title)| json!({"id": app.0, "title": title})),
        volume: volume
            .as_ref()
            .map(|v| json!({"level": v.level, "max": v.max, "muted": v.muted})),
        sound: sound
            .as_ref()
            .map(|s| json!({"output": s.output, "mode": s.mode})),
        picture: picture.as_ref().map(|p| {
            json!({"backlight": p.backlight, "contrast": p.contrast, "brightness": p.brightness,
                   "color": p.color, "energy_saving": p.energy_saving})
        }),
        tv: json!({"model": tv.paired.model, "webos": release, "address": address, "transport": transport}),
    };
    client.close().await;
    if json {
        return output::json(&status);
    }
    let label = |text: &str| paint(DIM, output::pad(text, 10));
    let dot = if status.power == "on" {
        paint(GREEN, "●")
    } else {
        paint(YELLOW, "●")
    };
    output::line(format!(
        "{dot} {}  {}",
        output::pad(status.power, 8),
        paint(BOLD, in_front.unwrap_or_default())
    ));
    if let Some(volume) = &volume {
        let level = if volume.muted {
            paint(YELLOW, "muted")
        } else {
            volume.level.to_string()
        };
        output::line(format!(
            "{}{}  {level}",
            label("volume"),
            output::bar(volume.level, volume.max, 24)
        ));
    }
    if let Some(sound) = &sound {
        let mut parts = Vec::new();
        parts.extend(sound.output.as_deref().map(theme::sound_output));
        parts.extend(sound.mode.clone());
        output::line(format!("{}{}", label("sound"), parts.join(" · ")));
    }
    if let Some(picture) = &picture {
        let mut parts = Vec::new();
        parts.extend(picture.backlight.map(|light| format!("OLED light {light}")));
        parts.extend(
            picture
                .energy_saving
                .as_deref()
                .map(|energy| format!("energy {energy}")),
        );
        output::line(format!("{}{}", label("picture"), parts.join(" · ")));
    }
    let mut about = vec![tv.paired.model.clone()];
    about.extend(release.map(|release| format!("webOS {release}")));
    about.push(address.to_string());
    output::line(format!("{}{}", label(""), paint(DIM, about.join(" · "))));
    Ok(())
}

fn show_off(paths: &Paths, config: &Config, json: bool) -> Result<(), AppError> {
    let paired = Paired::load(paths)?.ok_or(AppError::NotPaired)?;
    let address = config.tv.unwrap_or(paired.address);
    if json {
        return output::json(
            &json!({"power": "off", "tv": {"model": paired.model, "address": address}}),
        );
    }
    output::line(format!(
        "{} {}  {}",
        paint(DIM, "○"),
        output::pad("off", 8),
        paint(DIM, format!("no answer at {address}; `telka on` wakes it"))
    ));
    Ok(())
}

pub async fn apps(paths: &Paths, config: &Config, json: bool) -> Result<(), AppError> {
    let tv = Tv::connect(paths, config).await?;
    let apps: LaunchPoints = tv.client.get().await?;
    let front: Option<ForegroundApp> = tv.client.get().await.ok();
    tv.client.close().await;
    let is_front = |id: &str| front.as_ref().is_some_and(|front| front.0 == id);
    if json {
        let list: Vec<Value> = apps
            .0
            .iter()
            .map(|app| json!({"id": app.id, "title": app.title, "system": app.system, "in_front": is_front(&app.id)}))
            .collect();
        return output::json(&list);
    }
    let width = apps
        .0
        .iter()
        .map(|app| app.title.chars().count())
        .max()
        .unwrap_or(0);
    for app in &apps.0 {
        let dot = if is_front(&app.id) {
            paint(GREEN, "●")
        } else {
            " ".to_owned()
        };
        output::line(format!(
            "{dot} {}  {}",
            output::pad(&app.title, width),
            paint(DIM, &app.id)
        ));
    }
    Ok(())
}

pub async fn inputs(paths: &Paths, config: &Config, json: bool) -> Result<(), AppError> {
    let tv = Tv::connect(paths, config).await?;
    let inputs: Inputs = tv.client.get().await?;
    let front: Option<ForegroundApp> = tv.client.get().await.ok();
    tv.client.close().await;
    let is_front = |app_id: &str| front.as_ref().is_some_and(|front| front.0 == app_id);
    if json {
        let list: Vec<Value> = inputs
            .0
            .iter()
            .map(|input| {
                json!({"id": input.id, "label": input.label, "devices": input.devices,
                       "plugged": input.plugged, "connected": input.connected,
                       "in_front": is_front(&input.app_id)})
            })
            .collect();
        return output::json(&list);
    }
    let names: Vec<String> = inputs
        .0
        .iter()
        .map(|input| theme::input_name(&input.id, &input.label))
        .collect();
    let width = names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0);
    for (input, name) in inputs.0.iter().zip(&names) {
        let dot = if is_front(&input.app_id) {
            paint(GREEN, "●")
        } else {
            " ".to_owned()
        };
        let mut about = input.devices.clone();
        if input.connected {
            about.push("signal".into());
        } else if input.plugged {
            about.push("plugged in".into());
        }
        // No padding after the last column: nothing to align it with.
        if about.is_empty() {
            output::line(format!("{dot} {name}"));
        } else {
            output::line(format!(
                "{dot} {}  {}",
                output::pad(name, width),
                paint(DIM, about.join(" · "))
            ));
        }
    }
    Ok(())
}
