//! Developer tasks: `cargo xtask <command>`.

mod clock;
mod compat;
mod fixtures;
mod settings;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Brrrty developer tasks")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(clap::Args)]
struct Dirs {
    /// Folder holding the sites to compare (default: ../sites next to the repo).
    #[arg(long)]
    sites_dir: Option<PathBuf>,
    /// Folder holding `<SITE>.toml` settings files (default: ../planning/compat next to the repo).
    #[arg(long)]
    settings_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Regenerate `tests/fixtures/<name>/expected/` with real Eleventy 3.1.8.
    FixturesRefresh {
        /// Only refresh this fixture.
        name: Option<String>,
    },
    /// Build one site with Eleventy and Brrrty and compare the output.
    Compat {
        site: String,
        #[command(flatten)]
        dirs: Dirs,
        /// Rebuild the reference output even if the stamp is current.
        #[arg(long)]
        refresh_reference: bool,
    },
    /// Run the comparison for every site that has a settings file and print a table.
    Scoreboard {
        #[command(flatten)]
        dirs: Dirs,
    },
}

/// The repository root: the parent of this crate's folder.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the repo")
        .to_path_buf()
}

impl Dirs {
    fn resolve(self, root: &Path) -> (PathBuf, PathBuf) {
        (
            self.sites_dir.unwrap_or_else(|| root.join("../sites")),
            self.settings_dir
                .unwrap_or_else(|| root.join("../planning/compat")),
        )
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = repo_root();
    let result = match cli.command {
        Cmd::FixturesRefresh { name } => fixtures::refresh(&root, name.as_deref()),
        Cmd::Compat {
            site,
            dirs,
            refresh_reference,
        } => {
            let (sites, settings) = dirs.resolve(&root);
            compat::run_one(&root, &site, &sites, &settings, refresh_reference)
        }
        Cmd::Scoreboard { dirs } => {
            let (sites, settings) = dirs.resolve(&root);
            compat::scoreboard(&root, &sites, &settings)
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(1)
        }
    }
}
