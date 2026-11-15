mod case;
mod reference;
mod score;

use case::{Lang, load_cases};
use clap::{Parser, Subcommand, ValueEnum};
use score::{CandidateRun, Engine};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const DEFAULT_ELEVENTY: &str =
    "/Users/josh/Code/brrrty/sites/joshtronic.com/node_modules/@11ty/eleventy/cmd.cjs";

#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Liquid,
    Nunjucks,
}

impl Kind {
    fn lang(self) -> Lang {
        match self {
            Kind::Liquid => Lang::Liquid,
            Kind::Nunjucks => Lang::Nunjucks,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "brrrty-spike",
    about = "Measure candidate libraries against Eleventy output"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate the expected files with Eleventy.
    Reference {
        kind: Kind,
        #[arg(long, default_value = DEFAULT_ELEVENTY)]
        eleventy: PathBuf,
    },
    /// Score every candidate against the expected files.
    Score { kind: Kind },
    /// Run candidates over a private site corpus (results stay under target/spike/).
    Corpus {
        kind: Kind,
        #[arg(long = "site", required = true)]
        sites: Vec<String>,
    },
    /// Compare Markdown candidates over a private site corpus.
    Markdown {
        #[arg(long = "site", required = true)]
        sites: Vec<String>,
    },
}

fn spikes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn results_path(name: &str) -> PathBuf {
    spikes_dir()
        .join("../target/spike")
        .join(format!("{name}.json"))
}

/// Candidate adapters are registered here by later tasks.
fn candidates(_kind: Kind) -> Vec<Box<dyn Engine>> {
    Vec::new()
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Reference { kind, eleventy } => {
            let lang = kind.lang();
            let dir = spikes_dir().join("cases").join(lang.dir());
            let cases: Vec<_> = load_cases(&dir)?.into_iter().map(|(c, _)| c).collect();
            reference::reference(lang, &cases, Path::new(&eleventy), &dir.join("expected"))?;
            println!(
                "reference: {} {} cases written to {}",
                cases.len(),
                lang.dir(),
                dir.join("expected").display()
            );
            Ok(())
        }
        Command::Score { kind } => {
            let dir = spikes_dir().join("cases").join(kind.lang().dir());
            let cases = load_cases(&dir)?;
            let skipped = score::unreferenced(&cases);
            let runs: Vec<CandidateRun> = candidates(kind)
                .iter()
                .map(|e| CandidateRun {
                    name: e.name(),
                    outcomes: score::score(e.as_ref(), &cases),
                    render_ms: score::time_render(e.as_ref(), &cases),
                })
                .collect();
            print_table(&runs, skipped);
            let kind_name = kind.lang().dir();
            score::write_results(&results_path(kind_name), kind_name, &runs)
                .map_err(|e| e.to_string())
        }
        Command::Corpus { .. } | Command::Markdown { .. } => {
            Err("corpus and markdown runs need candidate adapters, which later tasks add".into())
        }
    }
}

fn print_table(runs: &[CandidateRun], unreferenced: usize) {
    println!(
        "{:<24} {:>6} {:>6} {:>10}",
        "candidate", "pass", "total", "render_ms"
    );
    for r in runs {
        let pass = r
            .outcomes
            .iter()
            .filter(|o| o.2 == score::Outcome::Pass)
            .count();
        println!(
            "{:<24} {:>6} {:>6} {:>10.2}",
            r.name,
            pass,
            r.outcomes.len(),
            r.render_ms
        );
    }
    println!("unreferenced cases (no expected file): {unreferenced}");
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(1)
        }
    }
}
