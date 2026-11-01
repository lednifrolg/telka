//! End to end: the commands that change something, against a fake TV that reports its changes.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn volume_changes_wait_for_the_tv() {
    let telka = Telka::paired().await;
    for (args, expected, level) in [
        (vec!["vol", "15"], "volume 12 → 15", 15),
        (vec!["vol", "+3"], "volume 15 → 18", 18),
        (vec!["vol", "-30"], "volume 18 → 0", 0),
    ] {
        let output = telka.run(&args).await;
        assert_eq!(code(&output), 0, "{args:?}: {}", stderr(&output));
        assert!(stdout(&output).contains(expected), "{}", stdout(&output));
        assert_eq!(telka.tv.state().volume, level);
    }
    let sent: Vec<Value> = telka
        .tv
        .requests("audio/setVolume")
        .iter()
        .map(|request| request["payload"].clone())
        .collect();
    assert_eq!(
        sent,
        [
            json!({"volume": 15}),
            json!({"volume": 18}),
            json!({"volume": 0})
        ]
    );

    let output = telka.run(&["vol", "loud"]).await;
    assert_eq!(code(&output), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_the_tv_does_not_report_exits_with_5() {
    let telka = Telka::paired().await;
    telka.tv.state().ignores_changes = true;
    let output = telka.run(&["vol", "30"]).await;
    assert_eq!(code(&output), 5, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("sent volume 30, but the TV did not report it"),
        "{}",
        stderr(&output)
    );

    let output = telka.run(&["--no-wait", "vol", "30"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
}

#[tokio::test(flavor = "multi_thread")]
async fn mute_and_unmute() {
    let telka = Telka::paired().await;
    let output = telka.run(&["mute"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("✓ muted"));
    assert!(telka.tv.state().muted);
    let output = telka.run(&["unmute"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(!telka.tv.state().muted);
}

#[tokio::test(flavor = "multi_thread")]
async fn apps_start_by_name_and_come_to_the_front() {
    let telka = Telka::paired().await;
    let output = telka.run(&["app", "you"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("✓ YouTube  in front in"),
        "{}",
        stdout(&output)
    );
    assert_eq!(telka.tv.state().app, "youtube.leanback.v4");

    let output = telka.run(&["app", "hbo"]).await;
    assert_eq!(code(&output), 2);
    assert!(
        stderr(&output)
            .contains("\"hbo\" matches 2 apps: HBO Max (com.wbd.stream), HBO Max (com.hbo.hbomax)"),
        "{}",
        stderr(&output)
    );
    let output = telka.run(&["app", "com.hbo.hbomax"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    let url = "https://www.youtube.com/watch?v=x";
    let output = telka.run(&["app", "youtube", "--url", url]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let launch = telka.tv.requests("system.launcher/launch").pop().unwrap();
    assert_eq!(
        launch["payload"],
        json!({"id": "youtube.leanback.v4", "contentId": url, "params": {"contentTarget": url}})
    );
    let output = telka
        .run(&["app", "browser", "--url", "https://example.com"])
        .await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let open = telka.tv.requests("system.launcher/open").pop().unwrap();
    assert_eq!(open["payload"], json!({"target": "https://example.com"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn inputs_switch_by_label_or_device() {
    let telka = Telka::paired().await;
    let output = telka.run(&["input", "game console x"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("✓ HDMI 2 · Game Console  in front in"),
        "{}",
        stdout(&output)
    );
    assert_eq!(telka.tv.state().app, "com.webos.app.hdmi2");
    let output = telka.run(&["input", "garage"]).await;
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("`telka inputs` shows them"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_screen_goes_off_and_on() {
    let telka = Telka::paired().await;
    let output = telka.run(&["screen", "off"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(telka.tv.state().power, "Screen Off");
    let status = telka.run(&["status"]).await;
    assert!(
        stdout(&status).contains("screen off"),
        "{}",
        stdout(&status)
    );
    let output = telka.run(&["screen", "on"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(telka.tv.state().power, "Active");
}

#[tokio::test(flavor = "multi_thread")]
async fn off_presses_power_and_on_wakes_the_tv_over_the_network() {
    let telka = Telka::paired().await;
    let output = telka.run(&["off"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("✓ TV off"), "{}", stdout(&output));
    assert_eq!(telka.tv.state().buttons, ["POWER"]);
    assert_eq!(telka.tv.requests("system/turnOff"), Vec::<Value>::new());
    assert!(!telka.tv.state().on);

    let output = telka.run(&["on"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("waking the TV over the network"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("✓ TV on"), "{}", stdout(&output));
    assert!(telka.tv.state().magic_packets > 0);
    assert_eq!(telka.tv.state().power, "Active");

    let output = telka.run(&["on"]).await;
    assert!(
        stdout(&output).contains("the TV is already on"),
        "{}",
        stdout(&output)
    );
    let output = telka.run(&["toggle"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(!telka.tv.state().on);
    let output = telka.run(&["toggle"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(telka.tv.state().on);
}

#[tokio::test(flavor = "multi_thread")]
async fn on_from_active_standby_presses_power() {
    let telka = Telka::paired().await;
    telka.tv.state().power = "Active Standby".into();
    let output = telka.run(&["on"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(telka.tv.state().buttons, ["POWER"]);
    assert_eq!(telka.tv.state().power, "Active");
    assert_eq!(telka.tv.state().magic_packets, 0);

    telka.tv.state().power = "Active Standby".into();
    let output = telka.run(&["off"]).await;
    assert!(
        stdout(&output).contains("the TV is already off"),
        "{}",
        stdout(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn buttons_text_playback_and_toasts() {
    let telka = Telka::paired().await;
    let output = telka
        .run(&["key", "home", "down", "ok", "--gap", "10"])
        .await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("pressed HOME DOWN ENTER"));
    assert_eq!(telka.tv.state().buttons, ["HOME", "DOWN", "ENTER"]);
    let output = telka.run(&["key", "in_start"]).await;
    assert_eq!(code(&output), 2);
    let list = telka.run(&["key", "--list"]).await;
    assert!(stdout(&list).contains("HOME") && stdout(&list).contains("FASTFORWARD"));

    let output = telka.run(&["type", "breaking bad", "--enter"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(telka.tv.state().typed, "breaking bad\n");

    for action in ["play", "pause", "stop"] {
        let output = telka.run(&[action]).await;
        assert_eq!(code(&output), 0, "{action}: {}", stderr(&output));
        assert_eq!(
            telka.tv.requests(&format!("media.controls/{action}")).len(),
            1
        );
    }

    let output = telka.run(&["notify", "Laundry is done"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(telka.tv.state().toasts, ["Laundry is done"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn picture_settings_go_through_the_alert_and_are_read_back() {
    let telka = Telka::paired().await;
    let output = telka.run(&["picture"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("OLED light"),
        "{}",
        stdout(&output)
    );

    let output = telka
        .run(&[
            "picture",
            "--light",
            "70",
            "--energy",
            "MAX",
            "--mode",
            "filmmaker",
        ])
        .await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("OLED light 80 → 70"), "{out}");
    assert!(out.contains("energy saving max"), "{out}");
    assert!(out.contains("picture mode filmMaker"), "{out}");
    {
        let state = telka.tv.state();
        assert_eq!(
            (
                state.backlight,
                state.energy.as_str(),
                state.picture_mode.as_str()
            ),
            (70, "max", "filmMaker")
        );
    }

    let output = telka.run(&["picture", "--mode", "dazzling"]).await;
    assert_eq!(code(&output), 2);
    telka.tv.state().alerts_stick = true;
    let output = telka.run(&["picture", "--light", "60"]).await;
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("press OK on the remote"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn picture_changes_need_the_signed_manifest() {
    let telka = Telka::paired().await;
    let file = telka.file("state/tv.json");
    let mut paired: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    paired["manifest"] = json!("unsigned");
    std::fs::write(&file, paired.to_string()).unwrap();
    let output = telka.run(&["picture", "--light", "60"]).await;
    assert_eq!(code(&output), 2);
    assert!(
        stderr(&output).contains("LG's signed manifest"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_tells_changes_from_anywhere_and_the_tv_going_away() {
    let telka = Telka::paired().await;
    let mut child = telka.command(&["watch"]).spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut next = async || {
        tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    };
    for expected in [
        "power   on",
        "app     Netflix",
        "volume  12",
        "mute    unmuted",
        "sound   TV speaker",
    ] {
        let line = next().await;
        assert!(line.ends_with(expected), "{line:?} for {expected:?}");
    }
    telka.tv.change(|state| state.volume = 20);
    assert!(next().await.ends_with("volume  12 → 20"));
    telka
        .tv
        .change(|state| state.app = "youtube.leanback.v4".into());
    assert!(next().await.ends_with("app     Netflix → YouTube"));
    telka.tv.go_off();
    assert!(next().await.ends_with("power   on → off"));
    child.kill().await.unwrap();
}
