//! Finding what the user means by a name.
//!
//! Names match regardless of case and diacritics. An exact id wins, then an exact name, then a
//! name that starts with the text, then one that contains it; each step has to find exactly one
//! thing.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use webos::{App, Button, Input};

use crate::error::AppError;
use crate::theme;

/// Lower case without diacritics.
pub fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The one of `all` that `query` names. `names` gives each item's names, `label` how it is listed
/// when several match.
fn pick<'a, T>(
    query: &str,
    all: &'a [T],
    id: impl Fn(&T) -> &str,
    names: impl Fn(&T) -> Vec<String>,
    label: impl Fn(&T) -> String,
    noun: &str,
    list: &str,
) -> Result<&'a T, AppError> {
    if let Some(item) = all.iter().find(|item| id(item) == query) {
        return Ok(item);
    }
    let wanted = fold(query.trim());
    if wanted.is_empty() {
        return Err(AppError::Usage(format!("the {noun} name is empty")));
    }
    let steps: [&dyn Fn(&str) -> bool; 3] = [
        &|name| name == wanted,
        &|name| name.starts_with(&wanted),
        &|name| name.contains(&wanted),
    ];
    for matches in steps {
        let found: Vec<&T> = all
            .iter()
            .filter(|item| names(item).iter().any(|name| matches(&fold(name))))
            .collect();
        match found.as_slice() {
            [] => {}
            [one] => return Ok(one),
            several => {
                let labels: Vec<String> = several.iter().map(|item| label(item)).collect();
                return Err(AppError::Usage(format!(
                    "\"{query}\" matches {} {noun}s: {}; use more of the name or the id",
                    several.len(),
                    labels.join(", ")
                )));
            }
        }
    }
    Err(AppError::Usage(format!(
        "no {noun} named \"{query}\"; `telka {list}` shows them"
    )))
}

/// An app by id or title.
pub fn app<'a>(apps: &'a [App], query: &str) -> Result<&'a App, AppError> {
    pick(
        query,
        apps,
        |app| &app.id,
        |app| vec![app.title.clone()],
        |app| format!("{} ({})", app.title, app.id),
        "app",
        "apps",
    )
}

/// An input by id (`HDMI_3`), port (`HDMI 3`), label, or the name of a device on it.
pub fn input<'a>(inputs: &'a [Input], query: &str) -> Result<&'a Input, AppError> {
    pick(
        query,
        inputs,
        |input| &input.id,
        |input| {
            let mut names = vec![input.label.clone(), input.id.replace('_', " ")];
            names.extend(input.devices.iter().cloned());
            names
        },
        |input| theme::input_name(&input.id, &input.label),
        "input",
        "inputs",
    )
}

/// A remote button by its name, or by what the remote calls it: `ok`, `settings`, `ff`, `vol+`.
pub fn button(query: &str) -> Result<Button, AppError> {
    let folded = fold(query.trim());
    let name = match folded.as_str() {
        "ok" | "select" => "ENTER",
        "settings" | "gear" => "MENU",
        "ff" | "forward" => "FASTFORWARD",
        "rew" | "rw" => "REWIND",
        "vol+" | "volup" => "VOLUMEUP",
        "vol-" | "voldown" => "VOLUMEDOWN",
        "ch+" | "chup" => "CHANNELUP",
        "ch-" | "chdown" => "CHANNELDOWN",
        "zoom" => "MAGNIFIER_ZOOM",
        other => other,
    };
    Button::from_name(name).ok_or_else(|| {
        AppError::Usage(format!(
            "no button \"{query}\"; `telka key --list` shows them"
        ))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use webos::{Inputs, LaunchPoints, Report};

    use super::*;

    fn apps() -> Vec<App> {
        LaunchPoints::read(&json!({"launchPoints": [
            {"id": "youtube.leanback.v4", "title": "YouTube"},
            {"id": "com.wbd.stream", "title": "HBO Max"},
            {"id": "com.hbo.hbomax", "title": "HBO Max"},
            {"id": "tv.solocoo.slsk", "title": "Skylink Live TV Application"},
            {"id": "voyo.cz", "title": "Oneplay"},
            {"id": "com.webos.app.browser", "title": "Web Browser"}]}))
        .unwrap()
        .0
    }

    fn inputs() -> Vec<Input> {
        Inputs::read(&json!({"devices": [
            {"id": "HDMI_1", "label": "HDMI 1", "appId": "com.webos.app.hdmi1"},
            {"id": "HDMI_3", "label": "Game Console", "appId": "com.webos.app.hdmi3",
             "subList": [{"osdName": "Spieľkonzola"}]}]}))
        .unwrap()
        .0
    }

    fn app_name(query: &str) -> Result<String, String> {
        let apps = apps();
        app(&apps, query)
            .map(|app| app.id.clone())
            .map_err(|error| error.to_string())
    }

    #[test]
    fn apps_by_title_or_id() {
        assert_eq!(app_name("youtube").unwrap(), "youtube.leanback.v4");
        assert_eq!(app_name("SKY").unwrap(), "tv.solocoo.slsk");
        assert_eq!(app_name("live tv").unwrap(), "tv.solocoo.slsk");
        assert_eq!(app_name("com.hbo.hbomax").unwrap(), "com.hbo.hbomax");
        assert_eq!(
            app_name("hbo").unwrap_err(),
            "\"hbo\" matches 2 apps: HBO Max (com.wbd.stream), HBO Max (com.hbo.hbomax); use \
             more of the name or the id"
        );
        assert_eq!(
            app_name("garage").unwrap_err(),
            "no app named \"garage\"; `telka apps` shows them"
        );
    }

    #[test]
    fn inputs_by_port_label_or_device() {
        let inputs = inputs();
        let id = |query: &str| input(&inputs, query).map(|input| input.id.clone());
        assert_eq!(id("hdmi 3").unwrap(), "HDMI_3");
        assert_eq!(id("HDMI_1").unwrap(), "HDMI_1");
        assert_eq!(id("console").unwrap(), "HDMI_3");
        assert_eq!(id("spielkonzola").unwrap(), "HDMI_3");
        assert!(id("hdmi").is_err(), "both inputs start with hdmi");
    }

    #[test]
    fn buttons_by_name_or_what_the_remote_calls_them() {
        assert_eq!(button("ok").unwrap(), Button::ENTER);
        assert_eq!(button("Home").unwrap(), Button::HOME);
        assert_eq!(button("settings").unwrap(), Button::MENU);
        assert_eq!(button("ff").unwrap(), Button::FAST_FORWARD);
        assert_eq!(button("vol+").unwrap().name(), "VOLUMEUP");
        assert!(button("in_start").is_err());
        assert!(
            button("nope")
                .unwrap_err()
                .to_string()
                .contains("telka key --list")
        );
    }
}
