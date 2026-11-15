use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "brrrty", version, about = "Eleventy, but make it go brrr.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Build the site
    Build,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Build) => {
            eprintln!("brrrty build is not implemented yet");
            ExitCode::from(1)
        }
        None => ExitCode::SUCCESS,
    }
}
