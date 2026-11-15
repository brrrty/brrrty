use brrrty_spike::case::{Lang, load_cases};
use brrrty_spike::corpus::{self, SiteInput};
use brrrty_spike::engines::liquid_crate::LiquidCrate;
use brrrty_spike::{DEFAULT_ELEVENTY, reference, score};
use clap::{Parser, Subcommand, ValueEnum};
use score::{CandidateRun, Engine};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
fn candidates(kind: Kind) -> Vec<Box<dyn Engine>> {
    match kind {
        Kind::Liquid => vec![Box::new(LiquidCrate)],
        Kind::Nunjucks => Vec::new(),
    }
}

const DEFAULT_SITES: &str = "/Users/josh/Code/brrrty/sites";
const DEFAULT_SETTINGS: &str = "/Users/josh/Code/brrrty/planning/compat";

fn run_corpus(kind: Kind, names: &[String]) -> Result<(), String> {
    let sites: Vec<SiteInput> = names
        .iter()
        .map(|n| SiteInput::load(Path::new(DEFAULT_SITES), Path::new(DEFAULT_SETTINGS), n))
        .collect::<Result<_, _>>()?;
    let boxed = candidates(kind);
    let engines: Vec<&dyn Engine> = boxed.iter().map(|b| b.as_ref()).collect();
    let report = corpus::corpus(kind.lang(), &engines, &sites);
    println!(
        "corpus {}: {} files ({} skipped for front matter errors)",
        kind.lang().dir(),
        report.files,
        report.front_matter_errors
    );
    println!("{:<24} {:>6} {:>6}", "candidate", "parse", "fail");
    for e in &report.engines {
        println!("{:<24} {:>6} {:>6}", e.name, e.pass, e.fail);
        for (site, (p, f)) in &e.per_site {
            println!("  {site:<30} {p:>6} {f:>6}");
        }
        for (k, g) in &e.groups {
            println!("  {:>5}x {k}", g.count);
        }
    }
    let path = results_path(&format!("{}-corpus", kind.lang().dir()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body =
        serde_json::to_string_pretty(&report.to_json(kind.lang())).map_err(|e| e.to_string())?;
    std::fs::write(&path, body + "\n").map_err(|e| e.to_string())
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
        Command::Corpus { kind, sites } => run_corpus(kind, &sites),
        Command::Markdown { .. } => {
            Err("markdown runs need candidate adapters, which a later task adds".into())
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
