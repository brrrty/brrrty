use std::io::{IsTerminal, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use brrrty::options::{Flags, build_options};
use brrrty::{bug, report};
use brrrty_core::build;
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
    /// Build the site, serve it with live reload, and rebuild on every change
    Serve(ServeArgs),
}

#[derive(Args, Clone)]
struct ServeArgs {
    /// Port to serve on; the next free one up to 10 above it is used when it is taken
    #[arg(long, default_value_t = brrrty_serve::DEFAULT_PORT)]
    port: u16,
    /// Address to listen on; use 0.0.0.0 to let other devices on your network in
    #[arg(long, default_value = brrrty_serve::DEFAULT_HOST)]
    host: String,
    /// Folder with your source files [default: `dir.input` from
    /// brrrty.config.rhai, or .]
    #[arg(long)]
    input: Option<PathBuf>,
    /// Folder to write the site to; kept up to date after every good build
    /// [default: `dir.output` from brrrty.config.rhai, or _site]
    #[arg(long)]
    output: Option<PathBuf>,
    /// File or folder to copy as-is, relative to the current folder (repeatable)
    #[arg(long)]
    passthrough: Vec<PathBuf>,
    /// Stop each build at its first error, and exit if the first build fails
    #[arg(long)]
    strict: bool,
    /// Print only errors and warnings
    #[arg(long)]
    quiet: bool,
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
            let _ = writeln!(
                std::io::stderr(),
                "Brrrty could not find the current folder: {e}"
            );
            return ExitCode::from(1);
        }
    };
    let color = std::io::stderr().is_terminal();
    let flags = Flags {
        input: args.input,
        output: args.output,
        passthrough: args.passthrough,
        strict: args.strict,
    };
    let opts = match build_options(&root, &flags) {
        Ok(opts) => opts,
        Err(problems) => {
            report::print_problems(&problems, &root, color);
            return ExitCode::from(1);
        }
    };
    let report = build(&opts, &MarkdownIt::new());
    let root = opts.root_dir().unwrap_or_else(|_| opts.root.clone());
    report::print_build(&report, &root, args.quiet, color);
    if report.has_errors() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_serve(args: ServeArgs) -> ExitCode {
    use brrrty::serve::signals::Interrupts;
    use brrrty::serve::{DEBOUNCE, Session, Settings, StartError};

    let root = match std::env::current_dir() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Brrrty could not find the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    let color = std::io::stderr().is_terminal();
    let quiet = args.quiet;
    // Before the first build, so Ctrl-C during it stops cleanly too.
    let interrupts = match Interrupts::install() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Brrrty could not listen for Ctrl-C: {e}");
            return ExitCode::from(1);
        }
    };
    let settings = Settings {
        flags: Flags {
            input: args.input,
            output: args.output,
            passthrough: args.passthrough,
            strict: args.strict,
        },
        quiet,
        color,
        debounce: DEBOUNCE,
    };
    let mut session = match Session::start(&root, settings) {
        Ok(s) => s,
        Err(StartError::Problems(problems)) => {
            report::print_problems(&problems, &root, color);
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("Brrrty could not start serving: {e}");
            return ExitCode::from(1);
        }
    };
    // The first build's summary and problems are printed by now.
    if !session.first_ok() && args.strict {
        eprintln!("The first build failed, so Brrrty stopped (--strict).");
        end_unserved(session, &root, color);
        return ExitCode::from(1);
    }
    if interrupts.received() {
        end_unserved(session, &root, color);
        interrupts.finish();
        return ExitCode::SUCCESS;
    }
    let server = match brrrty_serve::Server::start(session.serving(), &args.host, args.port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Brrrty could not start the server: {e}");
            end_unserved(session, &root, color);
            return ExitCode::from(1);
        }
    };
    session.live_reload(&server);
    if !quiet {
        session.on_rebuild(std::sync::Arc::new(|_: &brrrty::serve::Rebuild| {
            println!("Watching…");
        }));
        println!("Watching…");
        println!("Server at {}", shown_url(&server, args.port));
        if !session.first_served() {
            println!(
                "The first build stopped before writing any page: serving what {} held before, until a build gets that far.",
                session.output().display()
            );
        }
    }
    interrupts.stop_with(session.stop_handle());
    let finished = session.run(&server);
    server.stop();
    if !finished.problems.is_empty() {
        report::print_problems(&finished.problems, &root, color);
    }
    interrupts.finish();
    if finished.problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// End a session that is not served, printing what went wrong cleaning up.
fn end_unserved(session: brrrty::serve::Session, root: &std::path::Path, color: bool) {
    let finished = session.close();
    if !finished.problems.is_empty() {
        report::print_problems(&finished.problems, root, color);
    }
}

/// The URL to open: `localhost` stands in for an address that means every interface, which
/// is not one a browser can open. Says so when the port asked for was taken.
fn shown_url(server: &brrrty_serve::RunningServer, asked: u16) -> String {
    let url = if server.local_addr().ip().is_unspecified() {
        format!("http://localhost:{}/", server.port())
    } else {
        server.url()
    };
    if server.port_changed() {
        format!("{url} (port {asked} was in use)")
    } else {
        url
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
        Some(Command::Serve(args)) => run_serve(args),
        None => run_build(cli.build),
    }
}
