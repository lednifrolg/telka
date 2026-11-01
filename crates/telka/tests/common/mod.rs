//! Helpers shared by the end-to-end tests: the `telka` binary run against a [`FakeTv`].
// Each test crate uses a different part of this module.
#![allow(dead_code)]

mod tv;

use std::path::PathBuf;
use std::process::{Output, Stdio};

use tokio::process::Command;

pub use tv::*;

/// A home directory for telka and the TV it talks to.
pub struct Telka {
    pub home: tempfile::TempDir,
    pub tv: FakeTv,
}

impl Telka {
    pub async fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            tv: FakeTv::start().await,
        }
    }

    /// Paired already, as `telka pair` leaves it.
    pub async fn paired() -> Self {
        let telka = Self::new().await;
        telka.tv.write_pairing(telka.home.path());
        telka
    }

    pub fn file(&self, relative: &str) -> PathBuf {
        self.home.path().join(relative)
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_telka"));
        command
            .args(args)
            .env("TELKA_HOME", self.home.path())
            .env("TELKA_TV_URL", format!("ws://{}", self.tv.address))
            .env("TELKA_WOL_TARGET", self.tv.wake_address.to_string())
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    pub async fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().await.unwrap()
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}
