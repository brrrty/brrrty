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
    /// Set up an 11ty site to build with Brrrty: package.json and brrrty.config.rhai
    Init(InitArgs),
    /// List everything that keeps pages from building, without writing anything
    Doctor(DoctorArgs),
}

#[derive(Args, Clone)]
struct InitArgs {
    /// The site's folder [default: the current folder]
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Make the changes without asking
    #[arg(long, short = 'y')]
    yes: bool,
    /// Show the changes as a diff and write nothing
    #[arg(long, conflicts_with = "yes")]
    dry_run: bool,
}

#[derive(Args, Clone)]
struct DoctorArgs {
    /// Print one JSON object instead of the text report
    #[arg(long)]
    json: bool,
    /// Also build the site with its own 11ty and with Brrrty, in the system temp folder, and
    /// compare every file. This runs your site's JavaScript (through 11ty)
    #[arg(long)]
    verify: bool,
    /// Run 11ty for --verify without asking (needed when not on a terminal)
    #[arg(long, requires = "verify")]
    yes: bool,
    /// Folder with your source files [default: `dir.input` from
    /// brrrty.config.rhai, or .]
    #[arg(long)]
    input: Option<PathBuf>,
    /// The output folder your build uses, as pages see it [default: `dir.output` from
    /// brrrty.config.rhai, or _site]. Nothing is written there
    #[arg(long)]
    output: Option<PathBuf>,
    /// Run JavaScript data files in the site's own Node (`node`), or refuse the pages that
    /// need them (`off`) [default: `config.set_js_data(...)`, or off]
    #[arg(long, value_name = "MODE", value_parser = ["node", "off"])]
    js_data: Option<String>,
    /// File or folder to copy as-is, relative to the current folder (repeatable)
    #[arg(long)]
    passthrough: Vec<PathBuf>,
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
    /// Run JavaScript data files in the site's own Node (`node`), or refuse the pages that
    /// need them (`off`) [default: `config.set_js_data(...)`, or off]
    #[arg(long, value_name = "MODE", value_parser = ["node", "off"])]
    js_data: Option<String>,
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
    /// Run JavaScript data files in the site's own Node (`node`), or refuse the pages that
    /// need them (`off`) [default: `config.set_js_data(...)`, or off]
    #[arg(long, value_name = "MODE", value_parser = ["node", "off"])]
    js_data: Option<String>,
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

/// `--js-data`, which clap has already checked is `node` or `off`.
fn js_data_flag(v: Option<&str>) -> Option<brrrty_core::JsData> {
    v.and_then(brrrty_core::JsData::parse)
}

fn run_build(args: BuildArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(r) => r,
        Err(e) => {
            brrrty::say_err!("Brrrty could not find the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    let color = std::io::stderr().is_terminal();
    if let Err(e) = brrrty::cleanup::stop_workers_on_signals() {
        brrrty::say_err!("Brrrty could not listen for Ctrl-C: {e}");
        return ExitCode::from(1);
    }
    let flags = Flags {
        input: args.input,
        output: args.output,
        passthrough: args.passthrough,
        strict: args.strict,
        js_data: js_data_flag(args.js_data.as_deref()),
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
            brrrty::say_err!("Brrrty could not find the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    let color = std::io::stderr().is_terminal();
    let quiet = args.quiet;
    // Before the first build, so Ctrl-C during it stops cleanly too.
    let interrupts = match Interrupts::install() {
        Ok(i) => i,
        Err(e) => {
            brrrty::say_err!("Brrrty could not listen for Ctrl-C: {e}");
            return ExitCode::from(1);
        }
    };
    brrrty::serve::signals::stop_with_launcher();
    if let Err(e) = brrrty::cleanup::stop_workers_on_hangup() {
        brrrty::say_err!("Brrrty could not listen for signals: {e}");
        return ExitCode::from(1);
    }
    let settings = Settings {
        flags: Flags {
            input: args.input,
            output: args.output,
            passthrough: args.passthrough,
            strict: args.strict,
            js_data: js_data_flag(args.js_data.as_deref()),
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
            brrrty::say_err!("Brrrty could not start serving: {e}");
            return ExitCode::from(1);
        }
    };
    // The first build's summary and problems are printed by now.
    if !session.first_ok() && args.strict {
        brrrty::say_err!("The first build failed, so Brrrty stopped (--strict).");
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
            brrrty::say_err!("Brrrty could not start the server: {e}");
            end_unserved(session, &root, color);
            return ExitCode::from(1);
        }
    };
    session.live_reload(&server);
    if !quiet {
        session.on_rebuild(std::sync::Arc::new(|_: &brrrty::serve::Rebuild| {
            brrrty::say!("Watching…");
        }));
        brrrty::say!("Watching…");
    }
    // Printed under `--quiet` too, as 11ty does: the port may not be the one asked for.
    brrrty::say!("Server at {}", shown_url(&server, args.port));
    if !quiet && !session.first_served() {
        brrrty::say!(
            "The first build stopped before writing any page: serving what {} held before, until a build gets that far.",
            session.output().display()
        );
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

/// Ask on the terminal before running the site's 11ty, or refuse off one without `--yes`.
/// `Err` carries the exit code.
fn confirm_verify(yes: bool) -> Result<(), ExitCode> {
    brrrty::say_err!(
        "--verify runs your site's own 11ty build, which runs your site's JavaScript (its config, data files and plugins) with your permissions. Brrrty itself never runs JavaScript."
    );
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        brrrty::say_err!("Not on a terminal, so Brrrty can't ask: pass --yes to run it.");
        return Err(ExitCode::from(2));
    }
    eprint!("Run it now? [y/N] ");
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    if matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
        Ok(())
    } else {
        brrrty::say_err!("Not run.");
        Err(ExitCode::from(2))
    }
}

/// The instant both `--verify` builds run at: `SOURCE_DATE_EPOCH` when it is set, else now.
fn verify_epoch() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        })
}

fn run_doctor(args: DoctorArgs) -> ExitCode {
    use brrrty::doctor::{self, verify};

    let root = match std::env::current_dir() {
        Ok(r) => r,
        Err(e) => {
            brrrty::say_err!("Brrrty could not find the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    let color = std::io::stderr().is_terminal();
    let mut eleventy = None;
    let mut temp: Option<brrrty::cleanup::TempFolder> = None;
    // Where a signal is acted on: between steps, here on the main thread, which removes the
    // temporary folder (dropping it), says so and exits. See `brrrty::cleanup`.
    macro_rules! stop_point {
        () => {
            if let Some(sig) = brrrty::cleanup::stopping() {
                return stopped(sig, temp.take());
            }
        };
    }
    if args.verify {
        match verify::find(&root) {
            Ok(e) => eleventy = Some(e),
            Err(why) => {
                brrrty::say_err!("{why}");
                return ExitCode::from(2);
            }
        }
        if let Err(code) = confirm_verify(args.yes) {
            return code;
        }
        // SAFETY: nothing else runs yet: no thread has started, and the build that reads
        // the variable starts after this.
        unsafe { std::env::set_var("SOURCE_DATE_EPOCH", verify_epoch().to_string()) };
        // From here, Ctrl-C or SIGTERM stops 11ty, removes the temporary folder and exits
        // 130 or 143.
        if let Err(e) = brrrty::cleanup::install_signals()
            .and_then(|()| brrrty::cleanup::stop_workers_on_hangup())
        {
            brrrty::say_err!("Brrrty could not listen for Ctrl-C: {e}");
            return ExitCode::from(1);
        }
        match brrrty::cleanup::TempFolder::new("brrrty-doctor-") {
            Ok(t) => temp = Some(t),
            Err(e) => {
                brrrty::say_err!("Brrrty could not make a folder in the system temp folder: {e}");
                return ExitCode::from(1);
            }
        }
    } else if let Err(e) = brrrty::cleanup::stop_workers_on_signals() {
        brrrty::say_err!("Brrrty could not listen for Ctrl-C: {e}");
        return ExitCode::from(1);
    }
    let flags = Flags {
        input: args.input,
        output: match &temp {
            Some(t) => Some(t.path().join("brrrty")),
            None => args.output,
        },
        passthrough: args.passthrough,
        strict: false,
        js_data: js_data_flag(args.js_data.as_deref()),
    };
    let opts = match build_options(&root, &flags) {
        Ok(opts) => opts,
        Err(problems) => {
            report::print_problems(&problems, &root, color);
            return ExitCode::from(1);
        }
    };
    stop_point!();
    // Doctor runs the site's JavaScript data when it is on: say so first.
    if opts.js.mode == brrrty_core::JsData::Node
        && let Some(Ok(n)) = &opts.js.node
    {
        brrrty::say_err!(
            "Running JavaScript data in Node {} ({}), as `brrrty build` would.",
            n.version,
            n.path.display()
        );
    }
    let report = brrrty_core::survey(&opts, &MarkdownIt::new(), temp.is_none());
    stop_point!();
    let diagnosis = brrrty_core::doctor::diagnose(report);
    // With JavaScript data off, the Node that would run it.
    let node_when_off = (opts.js.mode == brrrty_core::JsData::Off
        && !diagnosis.report.js_files.is_empty())
    .then(|| {
        brrrty_node::find_node(std::env::var_os("BRRRTY_NODE").as_deref())
            .map(|n| brrrty_core::JsNode {
                path: n.path.clone(),
                version: n.version_text(),
                tz: n.tz.clone(),
                load_hooks: n.load_hooks,
            })
            .map_err(|m| m.to_string())
    });
    let js_report = doctor::JsReport::new(&opts.js, &diagnosis, &root, node_when_off);
    let mut verified = None;
    if let (Some(e), Some(t)) = (&eleventy, &temp) {
        let shim = t.path().join("freeze-time.cjs");
        let out = t.path().join("11ty");
        let ours = t.path().join("brrrty");
        let ran = std::fs::write(&shim, verify::SHIM)
            .map_err(|err| format!("Brrrty could not write {}: {err}", shim.display()));
        stop_point!();
        let ran = ran.and_then(|()| verify::run_eleventy(e, &root, &out, &shim, verify_epoch()));
        stop_point!();
        verified = Some(ran.and_then(|eleventy_time| {
            // 11ty writes no folder for a site with no output.
            let _ = std::fs::create_dir_all(&out);
            let _ = std::fs::create_dir_all(&ours);
            let report =
                brrrty_compat::compare_dirs(&out, &ours, &brrrty_compat::AllowList::empty())
                    .map_err(|err| format!("Brrrty could not compare the two builds: {err}"))?;
            let (random, random_paths) = random_outputs(&diagnosis.report);
            Ok(verify::Verified {
                eleventy_version: e.version.clone(),
                eleventy_time,
                brrrty_time: diagnosis.report.elapsed,
                report,
                random,
                random_paths,
            })
        }));
    }
    // The last place a signal stops the run. One that comes while the report prints, or
    // after, lets it finish with its own exit status.
    stop_point!();
    if args.json {
        let v = verified.as_ref().map(|v| match v {
            Ok(v) => verify::json(v),
            Err(why) => serde_json::json!({ "error": why }),
        });
        brrrty::say!("{:#}", doctor::json(&diagnosis, v, js_report.json()));
    } else {
        let mut out = js_report.text();
        out.push_str(&doctor::text(&diagnosis));
        if let Some(Ok(v)) = &verified {
            out.push('\n');
            out.push_str(&verify::text(v));
        }
        let _ = std::io::stdout().lock().write_all(out.as_bytes());
    }
    if let Some(Err(why)) = &verified {
        brrrty::say_err!("{why}");
    }
    let ok = diagnosis.builds()
        && verified
            .as_ref()
            .is_none_or(|v| v.as_ref().is_ok_and(verify::Verified::passes));
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// The files written for random pages, and those of them at a random path.
fn random_outputs(
    report: &brrrty_core::BuildReport,
) -> (
    std::collections::BTreeSet<PathBuf>,
    std::collections::BTreeSet<PathBuf>,
) {
    let all = report
        .pages
        .iter()
        .filter(|p| p.random)
        .map(|p| p.output.clone())
        .collect();
    let paths = report
        .pages
        .iter()
        .filter(|p| p.random_path)
        .map(|p| p.output.clone())
        .collect();
    (all, paths)
}

/// Stop after a signal: remove the temporary folder, say so and exit 130 or 143.
fn stopped(sig: i32, temp: Option<brrrty::cleanup::TempFolder>) -> ExitCode {
    drop(temp);
    brrrty::say_err!("Stopped; the temporary folders are removed.");
    ExitCode::from(u8::try_from(128 + sig).unwrap_or(1))
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

fn run_init(args: InitArgs) -> ExitCode {
    let dir = match args.dir {
        Some(d) => d,
        None => match std::env::current_dir() {
            Ok(r) => r,
            Err(e) => {
                brrrty::say_err!("Brrrty could not find the current folder: {e}");
                return ExitCode::from(1);
            }
        },
    };
    ExitCode::from(brrrty::init::run(&brrrty::init::Options {
        dir,
        yes: args.yes,
        dry_run: args.dry_run,
        interactive: std::io::stdin().is_terminal(),
    }))
}

fn main() -> ExitCode {
    bug::install_panic_hook();
    // A bare `brrrty` in an 11ty site that isn't set up yet points at `init`.
    if std::env::args_os().len() == 1
        && let Ok(cwd) = std::env::current_dir()
        && brrrty::init::should_hint(&cwd)
    {
        brrrty::say_err!("This looks like an 11ty site. Run `npx brrrty init` to set it up.");
    }
    let cli = Cli::parse();
    #[cfg(debug_assertions)]
    if cli.panic_test {
        panic!("deliberate test panic from --__panic");
    }
    match cli.command {
        Some(Command::Build(args)) => run_build(args),
        Some(Command::Serve(args)) => run_serve(args),
        Some(Command::Init(args)) => run_init(args),
        Some(Command::Doctor(args)) => run_doctor(args),
        None => run_build(cli.build),
    }
}
