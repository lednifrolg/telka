//! End to end: the real `telka` binary against a fake TV.

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::Value;
use tokio::process::Command;

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn pairing_waits_for_the_prompt_and_keeps_the_key_private() {
    let telka = Telka::new().await;

    let output = telka.run(&["pair"]).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("Accept telka on the TV"), "{out}");
    assert!(out.contains("\"LG Remote App\""), "{out}");
    assert!(
        out.contains(&format!(
            "paired with {MODEL} at 127.0.0.1 (LG's signed manifest)"
        )),
        "{out}"
    );
    let register = &telka.tv.frames("register")[0];
    assert!(register["payload"].get("client-key").is_none());
    assert_eq!(
        register["payload"]["manifest"]["signed"]["appId"],
        "com.lge.test"
    );
    let file = telka.file("state/tv.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let stored: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(stored["key"], KEY);
    assert_eq!(stored["uuid"], UUID);
    assert_eq!(stored["model"], MODEL);
    assert_eq!(stored["serial"], SERIAL);
    assert_eq!(stored["mac"], MAC);
    assert_eq!(stored["manifest"], "signed");

    let again = telka.run(&["pair"]).await;
    assert_eq!(code(&again), 0, "{}", stderr(&again));
    assert!(
        stdout(&again).contains(&format!("already paired with {MODEL}")),
        "{}",
        stdout(&again)
    );
}

/// Runs telka without `TELKA_HOME`, so it keeps its files in the usual locations of the system,
/// which `locations` points elsewhere.
async fn run_without_telka_home(
    telka: &Telka,
    args: &[&str],
    locations: &impl Fn(&mut Command) -> &mut Command,
) -> Output {
    let mut command = telka.command(args);
    locations(command.env_remove("TELKA_HOME"));
    command.output().await.unwrap()
}

/// Pairs, then reads the status of the TV, off by then, with the config at `config` giving its
/// address: the status shows that address only when telka read the config there.
async fn pair_and_read_the_config(
    telka: &Telka,
    config: &Path,
    locations: impl Fn(&mut Command) -> &mut Command,
) {
    let pair = run_without_telka_home(telka, &["pair"], &locations).await;
    assert_eq!(code(&pair), 0, "{}", stderr(&pair));

    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, "tv = \"192.0.2.7\"\n").unwrap();
    telka.tv.state().on = false;
    let status = run_without_telka_home(telka, &["status"], &locations).await;
    assert_eq!(code(&status), 0, "{}", stderr(&status));
    assert!(
        stdout(&status).contains("no answer at 192.0.2.7"),
        "{}",
        stdout(&status)
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn files_follow_the_xdg_directories() {
    let telka = Telka::new().await;
    let home = telka.home.path();

    let config = home.join("home/.config/telka/config.toml");
    pair_and_read_the_config(&telka, &config, |command| {
        command
            .env_remove("XDG_CONFIG_HOME")
            .env("HOME", home.join("home"))
            .env("XDG_STATE_HOME", home.join("xdg-state"))
    })
    .await;

    assert!(home.join("xdg-state/telka/tv.json").exists());
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn files_follow_the_app_data_directories() {
    let telka = Telka::new().await;
    let home = telka.home.path();

    let config = home.join(r"roaming\telka\config.toml");
    pair_and_read_the_config(&telka, &config, |command| {
        command
            .env("APPDATA", home.join("roaming"))
            .env("LOCALAPPDATA", home.join("local"))
    })
    .await;

    assert!(home.join(r"local\telka\state\tv.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_prompt_pairs_nothing() {
    let telka = Telka::new().await;
    telka.tv.state().prompt = Prompt::Decline;
    let output = telka.run(&["pair"]).await;
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("pairing was declined on the TV"),
        "{}",
        stderr(&output)
    );
    assert!(!telka.file("state/tv.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_warns_when_the_tv_cannot_be_woken() {
    let telka = Telka::new().await;
    telka.tv.state().wake_on_lan = false;
    let output = telka.run(&["pair"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Turn on via Wi-Fi"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_shows_what_the_tv_is_doing() {
    let telka = Telka::paired().await;
    telka.tv.state().app = "com.webos.app.hdmi2".into();

    let output = telka.run(&["status"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    for expected in [
        "● on        HDMI 2 · Game Console",
        "volume",
        "  12",
        "TV speaker · movie",
        "OLED light 80 · energy auto",
        &format!("{MODEL} · webOS 6.5.3 · 127.0.0.1"),
    ] {
        assert!(out.contains(expected), "{expected:?} in\n{out}");
    }

    telka.tv.state().muted = true;
    let json = telka.run(&["status", "--json"]).await;
    assert_eq!(code(&json), 0, "{}", stderr(&json));
    let status: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(status["power"], "on");
    assert_eq!(status["app"]["id"], "com.webos.app.hdmi2");
    assert_eq!(status["app"]["title"], "HDMI 2 · Game Console");
    assert_eq!(status["volume"]["level"], 12);
    assert_eq!(status["volume"]["muted"], true);
    assert_eq!(status["sound"]["mode"], "movie");
    assert_eq!(status["picture"]["backlight"], 80);
    assert_eq!(status["tv"]["transport"], "ws");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tv_that_is_off_is_a_state_not_an_error() {
    let telka = Telka::paired().await;
    telka.tv.state().on = false;
    let output = telka.run(&["status"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("○ off"), "{}", stdout(&output));
    assert!(
        stdout(&output).contains("`telka on` wakes it"),
        "{}",
        stdout(&output)
    );
    let json = telka.run(&["--json", "status"]).await;
    let status: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(status["power"], "off");

    let apps = telka.run(&["apps"]).await;
    assert_eq!(code(&apps), 1);
    assert!(
        stderr(&apps).contains("does not answer at 127.0.0.1"),
        "{}",
        stderr(&apps)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn apps_and_inputs_are_listed_with_what_is_in_front() {
    let telka = Telka::paired().await;
    let apps = telka.run(&["apps"]).await;
    assert_eq!(code(&apps), 0, "{}", stderr(&apps));
    let out = stdout(&apps);
    assert!(out.contains("● Netflix "), "{out}");
    assert!(out.contains("  HBO Max      com.wbd.stream"), "{out}");
    assert!(out.contains("  HBO Max      com.hbo.hbomax"), "{out}");
    let json = telka.run(&["apps", "--json"]).await;
    let list: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 5);
    assert_eq!(list[1]["in_front"], true);

    let inputs = telka.run(&["inputs"]).await;
    assert_eq!(code(&inputs), 0, "{}", stderr(&inputs));
    let out = stdout(&inputs);
    assert!(
        out.contains("HDMI 2 · Game Console  Game Console X · plugged in"),
        "{out}"
    );
    let json = telka.run(&["inputs", "--json"]).await;
    let list: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(list[1]["devices"][0], "Game Console X");
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_problems_exit_with_4() {
    let telka = Telka::new().await;
    let output = telka.run(&["status"]).await;
    assert_eq!(code(&output), 4);
    assert!(
        stderr(&output).contains("not paired with a TV yet; run `telka pair`"),
        "{}",
        stderr(&output)
    );

    let telka = Telka::paired().await;
    telka.tv.state().key = "another-key".into();
    let output = telka.run(&["status"]).await;
    assert_eq!(code(&output), 4, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no longer accepts telka's key"),
        "{}",
        stderr(&output)
    );

    let telka = Telka::paired().await;
    telka.tv.state().uuid = "another-tv".into();
    let output = telka.run(&["status"]).await;
    assert_eq!(code(&output), 4, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("another TV answers at 127.0.0.1"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unpairing_forgets_the_key() {
    let telka = Telka::paired().await;
    let output = telka.run(&["unpair"]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains(&format!("forgot the pairing with {MODEL}")));
    assert!(!telka.file("state/tv.json").exists());
    let again = telka.run(&["unpair"]).await;
    assert!(stdout(&again).contains("not paired with a TV"));
}

#[tokio::test(flavor = "multi_thread")]
async fn completions_and_man_pages_need_no_tv() {
    let telka = Telka::new().await;
    for shell in ["bash", "zsh", "fish"] {
        let output = telka.run(&["completions", shell]).await;
        assert_eq!(code(&output), 0, "{shell}: {}", stderr(&output));
        assert!(stdout(&output).contains("telka"), "{shell}");
    }
    let pages = telka.file("man");
    let output = telka.run(&["man", pages.to_str().unwrap()]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    for page in ["telka.1", "telka-pair.1", "telka-status.1", "telka-apps.1"] {
        assert!(pages.join(page).exists(), "{page}");
    }
    let help = telka.run(&[]).await;
    assert_eq!(code(&help), 2);
    assert!(
        stderr(&help).contains("opens the TUI, which needs a terminal"),
        "{}",
        stderr(&help)
    );
}
