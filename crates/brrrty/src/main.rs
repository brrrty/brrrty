use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use brrrty::{bug, report};
use brrrty_core::{BuildOptions, build};
use brrrty_markdown::Provisional;
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
    /// Folder with your source files
    #[arg(long, default_value = ".")]
    input: PathBuf,
    /// Folder to write the site to
    #[arg(long, default_value = "_site")]
    output: PathBuf,
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
    let opts = BuildOptions {
        root,
        input: args.input,
        output: args.output,
        passthrough: args.passthrough,
        strict: args.strict,
    };
    let report = build(&opts, &Provisional);
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
        let input_dir = opts
            .input_dir()
            .unwrap_or_else(|_| opts.root.join(&opts.input));
        let color = std::io::stderr().is_terminal();
        report::print_problems(&report.problems, &input_dir, color);
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
