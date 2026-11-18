//! Developer tasks: `cargo xtask <command>`.

mod clock;
mod compat;
mod corpus;
mod corpus_report;
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
        /// Build in Docker instead, once per `--image`, with no network, and
        /// keep the output only when every image agrees. Needs a fixture name.
        #[arg(long, requires = "name")]
        docker: bool,
        /// The images for `--docker`.
        #[arg(long = "image", default_values = ["node:22", "node:24"])]
        images: Vec<String>,
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
    /// Build a list of public 11ty sites with 11ty and Brrrty, and record the results.
    Corpus {
        /// TOML file with `[[site]] repo = "owner/name"` entries.
        #[arg(long)]
        list: PathBuf,
        /// Where the clones go, one folder per site, plus `_work/`.
        #[arg(long)]
        sites_dir: PathBuf,
        /// Logs and host copies, one folder per site; Docker must be able to mount it [default: <sites-dir>/_work].
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// Build outputs, one folder per site; a case-sensitive volume holds every site's paths [default: the work dir].
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// JSON results file; existing entries for other sites are kept.
        #[arg(long)]
        results: PathBuf,
        /// A brrrty binary to test, instead of building this checkout's.
        #[arg(long)]
        brrrty: Option<PathBuf>,
        /// A Linux build of brrrty, for timing it in the same container as 11ty.
        #[arg(long)]
        linux_brrrty: Option<PathBuf>,
        /// Only these sites (`owner/name` or its folder name); repeatable.
        #[arg(long)]
        only: Vec<String>,
        /// Only the first N sites of the list.
        #[arg(long)]
        limit: Option<usize>,
        /// Seconds each site may take in all; a site out of time is recorded as a failure.
        #[arg(long, default_value_t = 1500)]
        time_limit: u64,
        /// Timed builds per tool.
        #[arg(long, default_value_t = 3)]
        runs: usize,
        /// Remove the Docker images this run pulled when it's done.
        #[arg(long)]
        remove_images: bool,
        /// Keep every site's outputs and host copy, not just those with a different file.
        #[arg(long)]
        keep_outputs: bool,
        /// Where the sites' JavaScript runs: `native` uses the host's Node and is
        /// for disposable CI runners only; `auto` is native on Linux with `CI` set.
        #[arg(long, value_enum, default_value = "auto")]
        runner: corpus::Runner,
    },
    /// Merge `corpus` results files into one, such as one per CI job.
    CorpusMerge {
        /// Results files; a later file's entry for a site wins.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Order the sites as this site list does.
        #[arg(long)]
        list: Option<PathBuf>,
        /// The merged results file.
        #[arg(long)]
        out: PathBuf,
    },
    /// Print Markdown tables from a `corpus` results file.
    CorpusReport { results: PathBuf },
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
        Cmd::FixturesRefresh {
            name,
            docker,
            images,
        } => match (docker, name) {
            (true, Some(n)) => fixtures::refresh_docker(&root, &n, &images),
            (_, name) => fixtures::refresh(&root, name.as_deref()),
        },
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
        Cmd::Corpus {
            list,
            sites_dir,
            work_dir,
            output_dir,
            results,
            brrrty,
            linux_brrrty,
            only,
            limit,
            time_limit,
            runs,
            remove_images,
            keep_outputs,
            runner,
        } => corpus::run_corpus(
            &root,
            corpus::Options {
                list,
                output_dir: output_dir
                    .or_else(|| work_dir.clone())
                    .unwrap_or_else(|| sites_dir.join("_work")),
                work_dir: work_dir.unwrap_or_else(|| sites_dir.join("_work")),
                sites_dir,
                results,
                brrrty,
                linux_brrrty,
                only,
                limit,
                time_limit: std::time::Duration::from_secs(time_limit),
                runs,
                remove_images,
                keep_outputs,
                runner,
            },
        ),
        Cmd::CorpusMerge { inputs, list, out } => corpus::merge(&inputs, list.as_deref(), &out),
        Cmd::CorpusReport { results } => corpus_report::run_report(&results),
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
