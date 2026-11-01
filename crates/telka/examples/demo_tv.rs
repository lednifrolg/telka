//! A stand-in for the TV, for demos and screenshots: no request reaches a real TV.
//!
//! ```sh
//! export TELKA_HOME=$(mktemp -d) TELKA_TV_URL=ws://127.0.0.1:18893
//! cargo run -p telka --example demo_tv -- 18893 "$TELKA_HOME" &
//! telka
//! ```
//!
//! It answers like the TV of the end-to-end tests, with a few more apps, and leaves telka paired
//! with it in the given home directory. The web browser's address field takes focus a moment
//! after the browser comes to the front, and Enter there sends the address. A while after the
//! TUI connects, someone turns the volume with the remote, and again every so often, as someone
//! on the sofa would.

#[path = "../tests/common/tv.rs"]
mod tv;

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::time::Instant;
use tv::FakeTv;

const USAGE: &str = "usage: demo_tv <port> <new home directory>";
const BROWSER: &str = "com.webos.app.browser";
/// How often the demo looks at the TV.
const LOOK_EVERY: Duration = Duration::from_millis(100);
/// How long the browser takes to put the focus on its address field.
const FOCUS_AFTER: Duration = Duration::from_millis(700);
/// When the remote turns the volume: first after the TUI connects, then every so often.
const REMOTE_AFTER: Duration = Duration::from_secs(16);
const REMOTE_EVERY: Duration = Duration::from_secs(20);

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(port) = args.next().and_then(|p| p.parse::<u16>().ok()) else {
        fail(USAGE);
    };
    // An unset variable, as in `"$TELKA_HOME"`, would make it the current directory.
    let Some(home) = args.next().filter(|home| !home.is_empty()) else {
        fail(USAGE);
    };
    let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await else {
        fail(&format!("port {port} is taken"));
    };
    let tv = FakeTv::start_on(listener).await;
    tv.change(|state| {
        state.app = "youtube.leanback.v4".into();
        state.volume = 14;
        state.launch_points = apps();
    });
    tv.write_pairing(Path::new(&home));
    println!("demo TV on ws://{}", tv.address);

    let mut browser = Browser::default();
    let mut remote: Option<Instant> = None;
    let mut louder = true;
    loop {
        tokio::time::sleep(LOOK_EVERY).await;
        browser.follow(&tv);
        if remote.is_none() && tui_connected(&tv) {
            remote = Some(Instant::now() + REMOTE_AFTER);
        }
        if let Some(at) = remote.filter(|at| *at <= Instant::now()) {
            tv.change(|state| {
                state.volume = if louder {
                    state.volume + 4
                } else {
                    state.volume.saturating_sub(4)
                };
            });
            louder = !louder;
            remote = Some(at + REMOTE_EVERY);
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

/// The TUI follows the TV's keyboard; the one-shot commands do not.
fn tui_connected(tv: &FakeTv) -> bool {
    tv.frames("subscribe")
        .iter()
        .any(|frame| frame["uri"] == "ssap://com.webos.service.ime/registerRemoteKeyboard")
}

/// The web browser's address field.
#[derive(Default)]
struct Browser {
    /// Since when the browser is in front with nothing sent from it yet.
    front_since: Option<Instant>,
    sent: bool,
}

impl Browser {
    fn follow(&mut self, tv: &FakeTv) {
        let (in_front, focused, entered) = {
            let state = tv.state();
            (
                state.app == BROWSER,
                state.keyboard.is_some(),
                state.typed.ends_with('\n'),
            )
        };
        if !in_front {
            *self = Self::default();
            if focused {
                tv.change(|state| state.keyboard = None);
            }
            return;
        }
        let since = *self.front_since.get_or_insert_with(Instant::now);
        if focused && entered {
            // Enter sends the address, and the page takes the focus.
            self.sent = true;
            tv.change(|state| {
                state.keyboard = None;
                state.typed.clear();
            });
        } else if !focused && !self.sent && since.elapsed() >= FOCUS_AFTER {
            tv.change(|state| state.keyboard = Some("url".into()));
        }
    }
}

/// The apps of a TV in use.
fn apps() -> Value {
    let app = |id: &str, title: &str| json!({"id": id, "title": title, "systemApp": false});
    json!([
        app("youtube.leanback.v4", "YouTube"),
        app("netflix", "Netflix"),
        app("com.disney.disneyplus-prod", "Disney+"),
        app("amazon", "Prime Video"),
        app("spotify-beehive", "Spotify"),
        app("com.webos.app.browser", "Web Browser"),
    ])
}
