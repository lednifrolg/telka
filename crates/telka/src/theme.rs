//! Words shared by the command line and the TUI, so both show the TV the same way.

use webos::{Inputs, LaunchPoints, PowerMode, PowerState};

/// How the power state reads: `on`, `screen off`, `screen saver`, or `off` for every kind of
/// standby.
pub fn power(state: &PowerState) -> &'static str {
    if !state.is_on() {
        return "off";
    }
    match state.mode {
        PowerMode::ScreenOff => "screen off",
        PowerMode::ScreenSaver => "screen saver",
        _ => "on",
    }
}

/// What is in front, in words: an app's title, an input like `HDMI 3 · PS5 Game Console`, or the
/// app's id when telka knows no better.
pub fn in_front(app_id: &str, apps: Option<&LaunchPoints>, inputs: Option<&Inputs>) -> String {
    if let Some(input) =
        inputs.and_then(|inputs| inputs.0.iter().find(|input| input.app_id == app_id))
    {
        return input_name(&input.id, &input.label);
    }
    if let Some(app) = apps.and_then(|apps| apps.0.iter().find(|app| app.id == app_id)) {
        return app.title.clone();
    }
    match app_id {
        "" => "nothing".to_owned(),
        "com.webos.app.home" => "home screen".to_owned(),
        other => other.to_owned(),
    }
}

/// `HDMI 3 · PS5 Game Console`, or `HDMI 3` when the label says nothing more.
pub fn input_name(id: &str, label: &str) -> String {
    let port = id.replace('_', " ");
    if label.eq_ignore_ascii_case(&port) || label.is_empty() {
        port
    } else {
        format!("{port} · {label}")
    }
}

/// Where the sound goes, in words.
pub fn sound_output(output: &str) -> String {
    match output {
        "tv_speaker" => "TV speaker".to_owned(),
        "external_arc" => "HDMI ARC".to_owned(),
        "external_optical" => "optical".to_owned(),
        "bt_soundbar" => "Bluetooth".to_owned(),
        "headphone" => "headphones".to_owned(),
        "lineout" => "line out".to_owned(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use webos::Report;

    use super::*;

    #[test]
    fn power_states_read_in_words() {
        let state = |payload| PowerState::read(&payload).unwrap();
        assert_eq!(power(&state(json!({"state": "Active"}))), "on");
        assert_eq!(power(&state(json!({"state": "Screen Off"}))), "screen off");
        assert_eq!(power(&state(json!({"state": "Active Standby"}))), "off");
        assert_eq!(
            power(&state(
                json!({"state": "Active", "processing": "Request Power Off"})
            )),
            "off"
        );
    }

    #[test]
    fn what_is_in_front_reads_as_a_title_or_an_input() {
        let apps =
            LaunchPoints::read(&json!({"launchPoints": [{"id": "netflix", "title": "Netflix"}]}))
                .unwrap();
        let inputs = Inputs::read(&json!({"devices": [
            {"id": "HDMI_1", "label": "HDMI 1", "appId": "com.webos.app.hdmi1"},
            {"id": "HDMI_3", "label": "Game Console", "appId": "com.webos.app.hdmi3"}]}))
        .unwrap();
        assert_eq!(in_front("netflix", Some(&apps), Some(&inputs)), "Netflix");
        assert_eq!(
            in_front("com.webos.app.hdmi3", Some(&apps), Some(&inputs)),
            "HDMI 3 · Game Console"
        );
        assert_eq!(
            in_front("com.webos.app.hdmi1", Some(&apps), Some(&inputs)),
            "HDMI 1"
        );
        assert_eq!(in_front("com.webos.app.home", None, None), "home screen");
        assert_eq!(in_front("x.y", None, None), "x.y");
        assert_eq!(sound_output("tv_speaker"), "TV speaker");
        assert_eq!(sound_output("tv_external_speaker"), "tv external speaker");
    }
}
