//! `telka`: terminal remote for LG webOS TVs over their local API.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "telka", version, about, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
}
