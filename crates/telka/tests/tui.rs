//! The TUI end to end: `telka` in a terminal of its own against a fake TV, keys in, the screen
//! read back the way a person sees it.

mod common;

use std::io::{Read, Write};
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{State, Telka};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Wide enough for the wide layout.
const SIZE: PtySize = PtySize {
    rows: 30,
    cols: 120,
    pixel_width: 0,
    pixel_height: 0,
};
/// Longer than the TUI waits for an alert that does not close.
const WAIT: Duration = Duration::from_secs(10);

/// `telka` without a command, in a pseudo-terminal.
struct Screen {
    parser: Arc<Mutex<vt100::Parser>>,
    keys: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    // Closing it hangs the terminal up.
    _master: Box<dyn MasterPty + Send>,
}

impl Screen {
    fn open(telka: &Telka) -> Self {
        let pty = native_pty_system().openpty(SIZE).unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_telka"));
        command.env("TELKA_HOME", telka.home.path());
        command.env("TELKA_TV_URL", format!("ws://{}", telka.tv.address));
        command.env("TELKA_WOL_TARGET", telka.tv.wake_address.to_string());
        command.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(command).unwrap();
        drop(pty.slave);
        let (rows, cols) = (NonZero::new(SIZE.rows), NonZero::new(SIZE.cols));
        let parser = vt100::Parser::new(rows.unwrap(), cols.unwrap(), 0);
        let parser = Arc::new(Mutex::new(parser));
        let mut output = pty.master.try_clone_reader().unwrap();
        let screen = parser.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while let Ok(read @ 1..) = output.read(&mut buffer) {
                screen.lock().unwrap().process(&buffer[..read]);
            }
        });
        Self {
            parser,
            keys: pty.master.take_writer().unwrap(),
            child,
            _master: pty.master,
        }
    }

    fn text(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    fn press(&mut self, keys: &str) {
        self.keys.write_all(keys.as_bytes()).unwrap();
        self.keys.flush().unwrap();
    }

    /// Waits until a line on the screen has all of `parts`.
    async fn shows(&self, parts: &[&str]) {
        let deadline = Instant::now() + WAIT;
        loop {
            let text = self.text();
            if text
                .lines()
                .any(|line| parts.iter().all(|part| line.contains(part)))
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no line with {parts:?} on the screen:\n{text}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the screen passes `check`; `what` says what was waited for.
    async fn until(&self, what: &str, check: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !check(self.parser.lock().unwrap().screen()) {
            assert!(
                Instant::now() < deadline,
                "{what} never on the screen:\n{}",
                self.text()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Quits with `q` and waits for telka to end.
    async fn quit(mut self) -> i32 {
        self.press("q");
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return i32::try_from(status.exit_code()).unwrap();
            }
            assert!(Instant::now() < deadline, "telka did not quit");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The text on `background`, row by row.
fn on(screen: &vt100::Screen, background: vt100::Color) -> String {
    let (rows, cols) = (screen.size().0.get(), screen.size().1.get());
    (0..rows)
        .flat_map(|row| (0..cols).map(move |col| (row, col)))
        .filter_map(|(row, col)| screen.cell(row, col))
        .filter(|cell| cell.bgcolor() == background)
        .map(vt100::Cell::contents)
        .collect()
}

/// The background of the line that has `text`, where it starts.
fn background(screen: &vt100::Screen, text: &str) -> Option<vt100::Color> {
    screen
        .rows(0, screen.size().1.get())
        .enumerate()
        .find_map(|(row, line)| {
            let at = line.find(text)?;
            let col = line[..at].chars().count();
            let row = u16::try_from(row).ok()?;
            Some(screen.cell(row, u16::try_from(col).ok()?)?.bgcolor())
        })
}

/// Waits until the TV's state passes `check`.
async fn tv_has(telka: &Telka, check: impl Fn(&State) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check(&telka.tv.state()) {
        assert!(Instant::now() < deadline, "{:?}", telka.tv.state());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn picture_keys_change_the_tv_through_the_alert() {
    let telka = Telka::paired().await;
    let mut screen = Screen::open(&telka);
    screen.shows(&["│ mode", "unknown · v sets it"]).await;

    // Two steps fold into one change, read back from the TV.
    screen.press("]]");
    tv_has(&telka, |tv| tv.backlight == 90).await;
    screen.shows(&["✓ picture", "OLED light 80 → 90"]).await;
    screen.shows(&["│ light", "90 │"]).await;

    // Two quick presses go two levels on, in one line.
    screen.press("ee");
    tv_has(&telka, |tv| tv.energy == "min").await;
    screen.shows(&["✓ picture", "energy saving min"]).await;
    screen.shows(&["│ energy", "min"]).await;
    assert!(!screen.text().contains("energy saving off"));

    screen.press("v");
    screen.shows(&["picture mode"]).await;
    screen.press("film\r");
    tv_has(&telka, |tv| tv.picture_mode == "filmMaker").await;
    screen.shows(&["✓ picture", "picture mode filmMaker"]).await;
    screen.shows(&["│ mode", "filmMaker · set by telka"]).await;

    // An alert that stays on the TV: telka presses OK, which closes it and makes the change.
    telka.tv.state().alerts_stick = true;
    screen.press("[");
    screen
        .shows(&["⚠ picture", "OLED light 90 → 85", "telka pressed OK"])
        .await;
    tv_has(&telka, |tv| tv.backlight == 85 && tv.alert.is_none()).await;
    assert_eq!(telka.tv.state().buttons, ["ENTER"]);
    screen.shows(&["│ light", "85 │"]).await;

    assert_eq!(screen.quit().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_signed_pairing_picture_keys_say_why() {
    let telka = Telka::paired().await;
    let file = telka.file("state/tv.json");
    let paired = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, paired.replace("\"signed\"", "\"unsigned\"")).unwrap();
    let mut screen = Screen::open(&telka);
    screen.shows(&["│ mode", "the TV does not tell"]).await;
    screen.press("]");
    screen
        .shows(&["need a pairing with LG's signed manifest"])
        .await;
    assert_eq!(telka.tv.state().backlight, 80);
    assert_eq!(screen.quit().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pressed_button_lights_up_and_a_change_from_elsewhere_flashes() {
    // The accent behind a pressed button, the grey behind a flashing line.
    const LIT: vt100::Color = vt100::Color::Idx(4);
    const FLASH: vt100::Color = vt100::Color::Idx(8);
    let telka = Telka::paired().await;
    let mut screen = Screen::open(&telka);
    screen.shows(&["│ mode"]).await;

    screen.press("h");
    screen
        .until("home lit", |screen| on(screen, LIT) == "home")
        .await;
    screen
        .until("home dark again", |screen| on(screen, LIT).is_empty())
        .await;
    assert_eq!(telka.tv.state().buttons, ["HOME"]);

    telka.tv.change(|tv| tv.volume = 30);
    screen
        .until("the change flashing", |screen| {
            background(screen, "12 → 30") == Some(FLASH)
        })
        .await;
    screen
        .until("the flash gone", |screen| {
            background(screen, "12 → 30") == Some(vt100::Color::Default)
        })
        .await;
    screen.shows(&["│ volume", "30 │"]).await;
    assert_eq!(screen.quit().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn animations_can_be_turned_off() {
    let telka = Telka::paired().await;
    std::fs::create_dir_all(telka.file("config")).unwrap();
    std::fs::write(telka.file("config/config.toml"), "animations = false\n").unwrap();
    let mut screen = Screen::open(&telka);
    screen.shows(&["│ mode"]).await;
    screen.press("h");
    tv_has(&telka, |tv| tv.buttons == ["HOME"]).await;
    telka.tv.change(|tv| tv.volume = 30);
    screen.shows(&["12 → 30"]).await;
    // Nothing lights up or flashes, at any moment after.
    for _ in 0..20 {
        {
            let parser = screen.parser.lock().unwrap();
            let shown = parser.screen();
            assert_eq!(on(shown, vt100::Color::Idx(4)), "");
            assert_eq!(background(shown, "12 → 30"), Some(vt100::Color::Default));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(screen.quit().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_field_on_the_tv_opens_the_line_and_letters_go_there() {
    let telka = Telka::paired().await;
    let mut screen = Screen::open(&telka);
    screen.shows(&["│ mode"]).await;

    telka.tv.change(|tv| tv.keyboard = Some("url".into()));
    screen
        .shows(&["an address field on the TV has focus"])
        .await;
    // Letters go out as they are typed, Backspace deletes on the TV.
    screen.press("tvx");
    tv_has(&telka, |tv| tv.typed == "tvx").await;
    screen.press("\x7f");
    tv_has(&telka, |tv| tv.typed == "tv").await;
    screen.shows(&["❯ tv"]).await;
    // Enter is Enter on the TV, closes the line and tells what was typed.
    screen.press("\r");
    tv_has(&telka, |tv| tv.typed == "tv\n").await;
    screen.shows(&["type", "\"tv\"", "telka"]).await;
    assert!(!screen.text().contains("has focus"));

    // The field losing focus closes the line, also one opened by hand.
    screen.press("t");
    screen.shows(&["goes to the text field on the TV"]).await;
    telka.tv.change(|tv| tv.keyboard = None);
    screen
        .until("the line closed", |screen| {
            !screen.contents().contains("goes to the text field")
        })
        .await;
    assert_eq!(screen.quit().await, 0);
}
