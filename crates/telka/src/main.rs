//! `telka`: terminal remote for LG webOS TVs over their local API.

mod commands;
mod config;
mod connect;
mod error;
mod files;
mod output;
mod paths;
mod store;
mod theme;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use crate::error::AppError;
use crate::paths::Paths;

#[derive(Debug, Parser)]
#[command(
    name = "telka",
    version,
    propagate_version = true,
    about,
    arg_required_else_help = true
)]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
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
        Command::Completions { shell } => {
            let mut script = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "telka", &mut script);
            return output::raw(&script);
        }
        Command::Man { dir } => {
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
    let config = config::load(&paths)?;
    match cli.command {
        Command::Pair { address } => commands::pair(&paths, address).await,
        Command::Unpair => commands::unpair(&paths),
        Command::Discover => commands::discover(&paths, cli.json).await,
        Command::Status => commands::status(&paths, &config, cli.json).await,
        Command::Apps => commands::apps(&paths, &config, cli.json).await,
        Command::Inputs => commands::inputs(&paths, &config, cli.json).await,
        Command::Completions { .. } | Command::Man { .. } => unreachable!("handled above"),
    }
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
