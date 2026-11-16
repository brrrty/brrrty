use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use brrrty::{bug, report};
use brrrty_core::{BuildOptions, build};
use brrrty_markdown::MarkdownIt;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "brrrty",
    version,
    about = "Eleventy, but make it go brrr.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    build: BuildArgs,

    /// Crash on purpose to test the bug handler.
    #[cfg(debug_assertions)]
    #[arg(long = "__panic", hide = true)]
    panic_test: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Build the site (the default)
    Build(BuildArgs),
}

#[derive(Args, Clone)]
struct BuildArgs {
    /// Folder with your source files [default: `dir.input` from
    /// brrrty.config.rhai, or .]
    #[arg(long)]
    input: Option<PathBuf>,
    /// Folder to write the site to [default: `dir.output` from
    /// brrrty.config.rhai, or _site]
    #[arg(long)]
    output: Option<PathBuf>,
    /// File or folder to copy as-is, relative to the current folder (repeatable)
    #[arg(long)]
    passthrough: Vec<PathBuf>,
    /// Stop at the first error (for CI)
    #[arg(long)]
    strict: bool,
    /// Don't print the summary line
    #[arg(long)]
    quiet: bool,
}

fn run_build(args: BuildArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Brrrty could not find the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    let color = std::io::stderr().is_terminal();
    // The config sets the folders; flags on the command line win, as 11ty's do.
    let site = match brrrty_config::load(&root) {
        Ok(site) => site,
        Err(problems) => {
            report::print_problems(&problems, &root, color);
            return ExitCode::from(1);
        }
    };
    let dir = site.as_ref().map(|s| s.dir.clone()).unwrap_or_default();
    let mut config = site.map(|s| s.build_options()).unwrap_or_default();
    // `.rhai` data files work with or without a config file.
    config.data = Some(brrrty_config::data_runner(
        brrrty_config::Clock::from_env().unwrap_or(brrrty_config::Clock::System),
        root.clone(),
    ));
    let opts = BuildOptions {
        input: args
            .input
            .or_else(|| dir.input.map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(".")),
        output: args
            .output
            .or_else(|| dir.output.map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("_site")),
        root,
        passthrough: args.passthrough,
        strict: args.strict,
        config,
    };
    let report = build(&opts, &MarkdownIt::new());
    if !args.quiet {
        println!(
            "Wrote {} files, copied {} files in {:.2} seconds (brrrty {})",
            report.written,
            report.copied,
            report.elapsed.as_secs_f64(),
            env!("CARGO_PKG_VERSION")
        );
    }
    if !report.problems.is_empty() {
        let root = opts.root_dir().unwrap_or_else(|_| opts.root.clone());
        report::print_problems(&report.problems, &root, color);
    }
    if report.has_errors() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    bug::install_panic_hook();
    let cli = Cli::parse();
    #[cfg(debug_assertions)]
    if cli.panic_test {
        panic!("deliberate test panic from --__panic");
    }
    match cli.command {
        Some(Command::Build(args)) => run_build(args),
        None => run_build(cli.build),
    }
}
