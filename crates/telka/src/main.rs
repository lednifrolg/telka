//! `telka`: terminal remote for LG webOS TVs over their local API.

mod commands;
mod config;
mod connect;
mod control;
mod error;
mod files;
mod names;
mod output;
mod pairing;
mod paths;
mod store;
mod theme;
mod tui;
mod watch;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use webos::Media;

use crate::connect::Tv;
use crate::control::{Level, PictureChange};
use crate::error::AppError;
use crate::paths::Paths;

#[derive(Debug, Parser)]
#[command(
    name = "telka",
    version,
    propagate_version = true,
    about,
    after_help = "Without a command, telka opens the interactive TUI."
)]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    /// Return once the TV took a change, without waiting until it reports it.
    #[arg(long, global = true)]
    no_wait: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pair with the TV: accept the prompt on the TV with the remote.
    Pair {
        /// The TV's IP address; found on the network when left out.
        address: Option<IpAddr>,
    },
    /// Forget the pairing.
    Unpair,
    /// Look for LG TVs on the network.
    Discover,
    /// Show what the TV is doing: power, what is in front, volume, sound and picture.
    Status,
    /// List the apps on the home screen.
    Apps,
    /// List the inputs, with the devices on them.
    Inputs,
    /// Turn the TV on: with its power button when it is awake, else over Wi-Fi (Wake-on-LAN).
    On,
    /// Turn the TV off with its power button, so that `telka on` can wake it again.
    Off,
    /// Turn the TV off when it is on, on when it is off.
    Toggle,
    /// Turn the screen off or on; the TV stays on and the sound keeps playing.
    Screen { state: OnOff },
    /// Set the volume: 15, or relative to now: +2, -2.
    Vol {
        #[arg(allow_negative_numbers = true, value_name = "LEVEL")]
        level: Level,
    },
    /// Mute the sound.
    Mute,
    /// Unmute the sound.
    Unmute,
    /// Start an app by its title or id; part of the title is enough.
    App {
        name: String,
        /// What the app should open: a video URL for YouTube, a page for the web browser.
        #[arg(long)]
        url: Option<String>,
    },
    /// Switch to an input: HDMI 3, its label, or the name of the device on it.
    ///
    /// Over HDMI-CEC, switching can turn on the device on the input.
    Input { name: String },
    /// Press remote buttons in order: home, up, down, left, right, ok, back, settings, …
    Key {
        #[arg(required_unless_present = "list", value_name = "BUTTON")]
        buttons: Vec<String>,
        /// Milliseconds between two buttons.
        #[arg(long, default_value_t = 250, value_name = "MS")]
        gap: u64,
        /// List every button.
        #[arg(long)]
        list: bool,
    },
    /// Type into the text field on the TV that has focus.
    ///
    /// Only fields that use the TV's own keyboard take it, like the web browser's; apps with
    /// their own keyboard, like YouTube, do not.
    Type {
        text: String,
        /// Press Enter afterwards, which usually submits the field.
        #[arg(long)]
        enter: bool,
        /// Replace what is in the field.
        #[arg(long)]
        replace: bool,
    },
    /// Play in the app in front.
    Play,
    /// Pause in the app in front.
    Pause,
    /// Stop in the app in front.
    Stop,
    /// Show a message on the TV; not over an app on the full screen.
    Notify { message: String },
    /// Show the picture settings, or change them.
    ///
    /// Changing them needs a pairing with LG's signed manifest. The TV does not tell its picture
    /// mode, so a new one is not confirmed.
    Picture {
        /// vivid, normal, eco, cinema, sports, game, photo, filmMaker, expert1, expert2, or an
        /// HDR or Dolby Vision mode like hdrCinema.
        #[arg(long)]
        mode: Option<String>,
        /// OLED light, 0 to 100.
        #[arg(long, value_name = "0-100")]
        light: Option<u8>,
        /// Energy saving: auto, off, min, med or max.
        #[arg(long)]
        energy: Option<String>,
    },
    /// Print every change on the TV as it happens, until stopped.
    Watch,
    /// Print a shell completion script.
    ///
    /// For example: telka completions bash > ~/.local/share/bash-completion/completions/telka
    Completions { shell: Shell },
    /// Write the man pages for telka and each of its commands into a directory.
    ///
    /// For example: telka man ~/.local/share/man/man1
    Man {
        /// Directory for the pages; created when missing.
        dir: PathBuf,
    },
}

/// Writes a man page for `command` and for each of its commands into `dir`, every one with
/// telka's name and version in its footer.
fn man_pages(command: clap::Command, dir: &Path) -> std::io::Result<()> {
    fn write(command: clap::Command, dir: &Path, source: &str) -> std::io::Result<()> {
        for sub in command.get_subcommands().filter(|s| !s.is_hide_set()) {
            write(sub.clone(), dir, source)?;
        }
        clap_mangen::Man::new(command)
            .source(source.to_owned())
            .generate_to(dir)
            .map(drop)
    }
    let mut command = command.disable_help_subcommand(true);
    command.build();
    write(
        command,
        dir,
        &format!("telka {}", env!("CARGO_PKG_VERSION")),
    )
}

async fn run(cli: Cli) -> Result<(), AppError> {
    // These need no TV and no files, so they work before anything is set up.
    match &cli.command {
        Some(Command::Completions { shell }) => {
            let mut script = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "telka", &mut script);
            return output::raw(&script);
        }
        Some(Command::Man { dir }) => {
            if dir.exists() && !dir.is_dir() {
                return Err(AppError::Usage(format!(
                    "{} is not a directory",
                    dir.display()
                )));
            }
            std::fs::create_dir_all(dir)
                .and_then(|()| man_pages(Cli::command(), dir))
                .map_err(|e| AppError::io(dir, e))?;
            output::success(format_args!(
                "man pages written to {0}; read one with `man -l {0}/telka.1`",
                dir.display()
            ));
            return Ok(());
        }
        _ => {}
    }
    let paths = Paths::discover()?;
    let Some(command) = cli.command else {
        return tui::run(paths).await;
    };
    let config = config::load(&paths)?;
    let (json, no_wait) = (cli.json, cli.no_wait);
    match command {
        Command::Pair { address } => return commands::pair(&paths, address).await,
        Command::Unpair => return commands::unpair(&paths),
        Command::Discover => return commands::discover(&paths, json).await,
        Command::Status => return commands::status(&paths, &config, json).await,
        Command::Apps => return commands::apps(&paths, &config, json).await,
        Command::Inputs => return commands::inputs(&paths, &config, json).await,
        Command::On => return control::on(&paths, &config, no_wait).await,
        Command::Toggle => return control::toggle(&paths, &config, no_wait).await,
        Command::Watch => return watch::watch(&paths, &config, json).await,
        Command::Key { list: true, .. } => {
            control::list_buttons();
            return Ok(());
        }
        _ => {}
    }
    let tv = Tv::connect(&paths, &config).await?;
    let result = match command {
        Command::Off => control::off(&tv, no_wait).await,
        Command::Screen { state } => {
            control::screen(&tv, matches!(state, OnOff::On), no_wait).await
        }
        Command::Vol { level } => control::volume(&tv, level, no_wait).await,
        Command::Mute => control::mute(&tv, true, no_wait).await,
        Command::Unmute => control::mute(&tv, false, no_wait).await,
        Command::App { name, url } => control::app(&tv, &name, url.as_deref(), no_wait).await,
        Command::Input { name } => control::input(&tv, &name, no_wait).await,
        Command::Key { buttons, gap, .. } => {
            control::keys(&tv, &buttons, Duration::from_millis(gap)).await
        }
        Command::Type {
            text,
            enter,
            replace,
        } => control::type_text(&tv, &text, enter, replace).await,
        Command::Play => control::media(&tv, Media::Play).await,
        Command::Pause => control::media(&tv, Media::Pause).await,
        Command::Stop => control::media(&tv, Media::Stop).await,
        Command::Notify { message } => control::notify(&tv, &message).await,
        Command::Picture {
            mode,
            light,
            energy,
        } => {
            control::picture(
                &tv,
                PictureChange {
                    mode,
                    light,
                    energy,
                },
                json,
            )
            .await
        }
        _ => unreachable!("every other command is handled above"),
    };
    tv.client.close().await;
    result
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a single-threaded runtime always starts");
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            output::error(&error);
            error.exit_code()
        }
    }
}
