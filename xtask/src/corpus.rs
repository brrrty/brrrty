//! `cargo xtask corpus`: build a list of public 11ty sites with their own 11ty
//! and with Brrrty, and record what happened to each.
//!
//! The site list is a TOML file kept outside this repo:
//!
//! ```toml
//! [[site]]
//! repo = "owner/name"   # cloned from https://github.com/owner/name
//! license = "MIT"       # optional, copied into the results
//! url = "https://..."   # optional, overrides the clone URL
//! ```
//!
//! Sites are strangers' code, so their JavaScript (npm install scripts, the
//! 11ty config, data files, plugins) only ever runs inside Docker: `npm ci`
//! with the network on, then every 11ty build with `--network none`. The
//! clone is mounted read-only and copied into a volume. No host environment,
//! SSH agent or npm credentials reach the container. Brrrty never runs
//! JavaScript, so `brrrty init`, `brrrty doctor` and `brrrty build` run on the
//! host against a copy of the clone.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use brrrty_compat::{AllowList, Outcome, compare_dirs};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock::{COMPAT_EPOCH, COMPAT_LOCALE, COMPAT_TZ, pin_env, shim_path};

/// Label on every container and volume the harness makes, for cleanup.
const LABEL: &str = "brrrty-corpus=1";
/// Node image when the site doesn't pin a version.
const DEFAULT_NODE: u32 = 24;
/// Paths listed per outcome in the results.
const LISTED: usize = 40;

#[derive(Debug, Deserialize)]
struct SiteList {
    #[serde(default)]
    site: Vec<SiteEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct SiteEntry {
    repo: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// Options for one corpus run.
pub struct Options {
    pub list: PathBuf,
    pub sites_dir: PathBuf,
    /// Logs and the host copy, one folder per site. Docker mounts it.
    pub work_dir: PathBuf,
    /// The two builds' outputs, one folder per site. Only the host writes
    /// here, so it can be a case-sensitive volume Docker can't see.
    pub output_dir: PathBuf,
    pub results: PathBuf,
    pub linux_brrrty: Option<PathBuf>,
    pub only: Vec<String>,
    pub limit: Option<usize>,
    pub time_limit: Duration,
    pub runs: usize,
    pub remove_images: bool,
    /// Keep every site's outputs and host copy, not just those with a different file.
    pub keep_outputs: bool,
}

/// How one command ended.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    pub ok: bool,
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub secs: f64,
    /// The last lines of output when it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Todo {
    pub why: String,
    /// The `eleventyConfig` method the quoted code calls, when there is one.
    pub api: Option<String>,
    /// The plugin, for `addPlugin`.
    pub plugin: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Group {
    pub cause: Value,
    pub key: String,
    pub title: String,
    pub blocker: bool,
    pub pages: u64,
    pub pages_exact: bool,
    /// The cause's kind, plus its code for a `problem`: coarser than `key`.
    pub family: String,
    pub templates: Vec<String>,
    pub first_message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Doctor {
    pub verdict: String,
    pub builds: bool,
    pub pages_total: u64,
    pub pages_building: u64,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Compare {
    /// `full` when doctor said every page builds, else `partial`.
    pub mode: String,
    pub identical: usize,
    pub equivalent: usize,
    pub different: usize,
    pub missing: usize,
    pub extra: usize,
    pub different_paths: Vec<String>,
    pub missing_paths: Vec<String>,
    pub extra_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timing {
    pub eleventy_docker_ms: Vec<u64>,
    pub brrrty_docker_ms: Vec<u64>,
    pub brrrty_host_ms: Vec<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SiteResult {
    pub slug: String,
    pub repo: String,
    pub license: Option<String>,
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub node_image: Option<String>,
    pub installer: Option<String>,
    pub eleventy_version: Option<String>,
    pub clone: Option<Step>,
    pub install: Option<Step>,
    pub reference: Option<Step>,
    pub reference_files: Option<usize>,
    pub init: Option<Step>,
    pub init_todos: Vec<Todo>,
    pub doctor: Option<Doctor>,
    pub doctor_error: Option<String>,
    pub build: Option<Step>,
    pub compare: Option<Compare>,
    pub timing: Option<Timing>,
    /// The step that ran out of time, if one did.
    pub timed_out: Option<String>,
    pub secs: f64,
    pub brrrty: String,
}

/// `owner/name` as a folder name: `owner__name`.
pub fn slug(repo: &str) -> String {
    repo.replace('/', "__")
}

/// A name Docker accepts for a container or volume.
fn docker_name(prefix: &str, slug: &str) -> String {
    let clean: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("brrrty-corpus-{prefix}-{clean}")
}

/// The Node major a site pins in `.nvmrc` or `.node-version`.
pub fn node_major(nvmrc: &str) -> Option<u32> {
    let v = nvmrc.lines().next()?.trim().trim_start_matches('v');
    let lts = |name: &str| match name {
        "hydrogen" => Some(18),
        "iron" => Some(20),
        "jod" => Some(22),
        "krypton" => Some(24),
        "*" => Some(DEFAULT_NODE),
        _ => None,
    };
    if let Some(name) = v.strip_prefix("lts/") {
        return lts(&name.to_ascii_lowercase());
    }
    if matches!(v, "node" | "stable" | "latest") {
        return Some(DEFAULT_NODE);
    }
    let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn node_image(clone: &Path) -> String {
    let major = [".nvmrc", ".node-version"]
        .iter()
        .find_map(|f| fs::read_to_string(clone.join(f)).ok())
        .and_then(|s| node_major(&s))
        .filter(|m| *m >= 18)
        .unwrap_or(DEFAULT_NODE);
    format!("node:{major}")
}

/// The last `n` lines of a log file.
fn tail(path: &Path, n: usize) -> String {
    let s = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Run `cmd` with stdout and stderr in `log`, stopping it at `limit`. A
/// Docker command passes its container name so the container is killed too.
fn run(cmd: &mut Command, limit: Duration, container: Option<&str>, log: &Path) -> Step {
    let start = Instant::now();
    let opened = File::create(log).and_then(|f| Ok((f.try_clone()?, f)));
    let (out, err) = match opened {
        Ok(p) => p,
        Err(e) => return failed(format!("cannot write {}: {e}", log.display())),
    };
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("cannot start {:?}: {e}", cmd.get_program())),
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let ok = status.success();
                return Step {
                    ok,
                    exit: status.code(),
                    timed_out: false,
                    secs: start.elapsed().as_secs_f64(),
                    error: (!ok).then(|| tail(log, 25)),
                };
            }
            Ok(None) if start.elapsed() >= limit => {
                if let Some(name) = container {
                    let _ = Command::new("docker")
                        .args(["kill", name])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Step {
                    ok: false,
                    exit: None,
                    timed_out: true,
                    secs: start.elapsed().as_secs_f64(),
                    error: Some(format!(
                        "timed out after {}s\n{}",
                        limit.as_secs(),
                        tail(log, 15)
                    )),
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return failed(format!("cannot wait: {e}")),
        }
    }
}

fn failed(msg: String) -> Step {
    Step {
        error: Some(msg),
        ..Step::default()
    }
}

/// `docker run` with the sandbox every site container gets: no host
/// environment, no credentials, a fixed set of capabilities, and the clock,
/// zone and locale the compat harness pins.
fn docker_run(name: &str, network: bool) -> Command {
    let mut c = Command::new("docker");
    c.args(["run", "--rm", "--name", name, "--label", LABEL])
        .args(["--cap-drop", "ALL"])
        .args([
            "--cap-add",
            "CHOWN",
            "--cap-add",
            "DAC_OVERRIDE",
            "--cap-add",
            "FOWNER",
        ])
        .args(["--cap-add", "SETUID", "--cap-add", "SETGID"])
        .args(["--security-opt", "no-new-privileges"])
        .args(["--pids-limit", "4096", "--memory", "6g"])
        .args(["-e", &format!("SOURCE_DATE_EPOCH={COMPAT_EPOCH}")])
        .args(["-e", &format!("TZ={COMPAT_TZ}")])
        .args(["-e", &format!("LC_ALL={COMPAT_LOCALE}")])
        // A copied clone has another owner; let 11ty's git dates read it.
        .args(["-e", "GIT_CONFIG_COUNT=1"])
        .args(["-e", "GIT_CONFIG_KEY_0=safe.directory"])
        .args(["-e", "GIT_CONFIG_VALUE_0=*"])
        .args(["-e", "npm_config_update_notifier=false"]);
    if !network {
        c.args(["--network", "none"]);
    }
    c
}

fn mount(src: &Path, dst: &str, ro: bool) -> String {
    format!(
        "type=bind,src={},dst={dst}{}",
        src.display(),
        if ro { ",readonly" } else { "" }
    )
}

fn volume(vol: &str, dst: &str, ro: bool) -> String {
    format!(
        "type=volume,src={vol},dst={dst}{}",
        if ro { ",readonly" } else { "" }
    )
}

const NPM_FLAGS: &str = "--no-audit --no-fund --loglevel=error";

/// Shell for timed builds: runs `$@` and prints `BUILD_MS <n>`.
const TIMED: &str = r#"timed() { s=$(date +%s%N); "$@"; rc=$?; e=$(date +%s%N); echo "BUILD_MS $(( (e - s) / 1000000 )) rc=$rc"; return $rc; }"#;

/// The 11ty command inside the container.
const ELEVENTY: &str =
    r#"B=node_modules/@11ty/eleventy/cmd.cjs; [ -f "$B" ] || B=node_modules/@11ty/eleventy/cmd.js"#;

fn build_ms(log: &Path) -> Vec<u64> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("BUILD_MS ")?;
            let (ms, rc) = rest.split_once(" rc=")?;
            (rc.trim() == "0").then(|| ms.parse().ok())?
        })
        .collect()
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    rd.filter_map(Result::ok)
        .map(|e| {
            let p = e.path();
            if p.is_dir() { count_files(&p) } else { 1 }
        })
        .sum()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The `// TODO:` blocks `brrrty init` wrote, with the config method each
/// stands in for.
pub fn parse_todos(config: &str) -> Vec<Todo> {
    let lines: Vec<&str> = config.lines().collect();
    let mut todos = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let Some(why) = t
            .strip_prefix("// TODO: ")
            .or_else(|| t.strip_prefix("// TODO(brrrty): "))
            .or_else(|| t.strip_prefix("// TODO(brrrty-file): "))
        else {
            continue;
        };
        let why = why.trim_end_matches('.').to_string();
        // The quoted code follows: `// See ...`, `// <file>, line N:`, then `//   code`.
        let quoted: String = lines[i + 1..]
            .iter()
            .take_while(|l| l.trim_start().starts_with("//") && !l.contains("// TODO"))
            .skip_while(|l| !l.contains(", line ") && !l.contains(", lines "))
            .skip(1)
            .take(3)
            .map(|l| l.trim_start().trim_start_matches("//").trim())
            .collect::<Vec<_>>()
            .join(" ");
        let api = method(&quoted);
        let plugin = (api.as_deref() == Some("addPlugin"))
            .then(|| plugin_name(&quoted))
            .flatten();
        todos.push(Todo { why, api, plugin });
    }
    todos
}

/// The first `<ident>.<method>(` in `code` whose receiver looks like the 11ty config.
fn method(code: &str) -> Option<String> {
    let bytes = code.as_bytes();
    let mut i = 0;
    while let Some(off) = code[i..].find('.') {
        let dot = i + off;
        let name: String = code[dot + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = dot + 1 + name.len();
        let recv: String = code[..dot]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let lower = recv.to_ascii_lowercase();
        if !name.is_empty()
            && bytes.get(after) == Some(&b'(')
            && (lower.contains("config") || lower == "eleventy" || lower == "ec")
        {
            return Some(name);
        }
        i = dot + 1;
    }
    None
}

fn plugin_name(code: &str) -> Option<String> {
    let start = code.find("addPlugin(")? + "addPlugin(".len();
    let name: String = code[start..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$'))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// A stable key for a doctor cause: its kind and other fields.
pub fn cause_key(cause: &Value) -> String {
    let Some(obj) = cause.as_object() else {
        return cause.to_string();
    };
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("?");
    let rest: Vec<String> = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "kind")
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect();
    if rest.is_empty() {
        kind.to_string()
    } else {
        format!("{kind}: {}", rest.join(", "))
    }
}

/// A doctor cause's kind, with the code of a `problem`.
pub fn cause_family(cause: &Value) -> String {
    let kind = cause.get("kind").and_then(Value::as_str).unwrap_or("?");
    match cause.get("code").and_then(Value::as_str) {
        Some(code) => format!("{kind}: {code}"),
        None => kind.to_string(),
    }
}

pub fn parse_doctor(json: &str) -> Result<Doctor, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad doctor JSON: {e}"))?;
    let num = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
    let groups = v
        .get("groups")
        .and_then(Value::as_array)
        .map(|gs| {
            gs.iter()
                .map(|g| {
                    let cause = g.get("cause").cloned().unwrap_or(Value::Null);
                    Group {
                        key: cause_key(&cause),
                        family: cause_family(&cause),
                        cause,
                        title: g.get("title").and_then(Value::as_str).unwrap_or("").into(),
                        blocker: g.get("blocker").and_then(Value::as_bool).unwrap_or(false),
                        pages: g.get("pages").and_then(Value::as_u64).unwrap_or(0),
                        pages_exact: g
                            .get("pages_exact")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        templates: g
                            .get("templates")
                            .and_then(Value::as_array)
                            .map(|ts| {
                                ts.iter()
                                    .filter_map(|t| t.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        first_message: g
                            .pointer("/locations/0/message")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Doctor {
        verdict: v
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        builds: v.get("builds").and_then(Value::as_bool).unwrap_or(false),
        pages_total: num("/pages/total"),
        pages_building: num("/pages/building"),
        groups,
    })
}

struct Ctx<'a> {
    root: &'a Path,
    opts: &'a Options,
    brrrty: PathBuf,
    version: String,
    pulled: Vec<String>,
}

impl Ctx<'_> {
    fn ensure_image(&mut self, image: &str) -> Result<(), String> {
        let have = Command::new("docker")
            .args(["image", "inspect", image])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if have {
            return Ok(());
        }
        println!("  pulling {image}");
        let ok = Command::new("docker")
            .args(["pull", "-q", image])
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("cannot pull {image}"));
        }
        self.pulled.push(image.to_string());
        Ok(())
    }
}

fn remove_volume(vol: &str) {
    let _ = Command::new("docker")
        .args(["volume", "rm", "-f", vol])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Free space, in GiB, that `df -k <path>` reports for `path`.
fn df_free_gib(out: &[u8]) -> Option<f64> {
    let s = String::from_utf8_lossy(out);
    let line = s.lines().last()?;
    let kib: f64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(kib / 1024.0 / 1024.0)
}

/// Host disk below this many GiB stops a site before it installs.
const HOST_MIN_GIB: f64 = 40.0;
/// Docker's disk below this many GiB does too.
const DOCKER_MIN_GIB: f64 = 5.0;

/// Refuse to install when the host or Docker's disk is nearly full, so a
/// full disk is a recorded failure and not a broken install.
fn room(host_path: &Path, image: &str) -> Result<(), String> {
    let host = Command::new("df").arg("-k").arg(host_path).output().ok();
    if let Some(free) = host.and_then(|o| df_free_gib(&o.stdout))
        && free < HOST_MIN_GIB
    {
        return Err(format!(
            "host disk has {free:.1} GiB free, under {HOST_MIN_GIB} GiB"
        ));
    }
    let docker = Command::new("docker")
        .args(["run", "--rm", "--network", "none", image, "df", "-k", "/"])
        .output()
        .ok();
    if let Some(free) = docker.and_then(|o| df_free_gib(&o.stdout))
        && free < DOCKER_MIN_GIB
    {
        return Err(format!(
            "Docker's disk has {free:.1} GiB free, under {DOCKER_MIN_GIB} GiB"
        ));
    }
    Ok(())
}

fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn run_site(ctx: &mut Ctx, entry: &SiteEntry) -> SiteResult {
    let start = Instant::now();
    let deadline = start + ctx.opts.time_limit;
    let slug = slug(&entry.repo);
    let mut r = SiteResult {
        slug: slug.clone(),
        repo: entry.repo.clone(),
        license: entry.license.clone(),
        brrrty: ctx.version.clone(),
        ..SiteResult::default()
    };
    let clone = ctx.opts.sites_dir.join(&slug);
    let work = ctx.opts.work_dir.join(&slug);
    let logs = work.join("logs");
    if let Err(e) = fs::create_dir_all(&logs) {
        r.clone = Some(failed(format!("cannot create {}: {e}", logs.display())));
        return r;
    }
    let outs = ctx.opts.output_dir.join(&slug);
    if let Err(e) = fs::create_dir_all(&outs) {
        r.clone = Some(failed(format!("cannot create {}: {e}", outs.display())));
        return r;
    }
    let vol = docker_name("work", &slug);
    site_steps(
        ctx, entry, &mut r, &clone, &work, &outs, &logs, &vol, deadline,
    );
    remove_volume(&vol);
    let _ = fs::remove_file(work.join("node_modules.tar"));
    // Outputs are big; keep them only where a file differs, to look into.
    let differs = r.compare.as_ref().is_some_and(|c| c.different > 0);
    if !ctx.opts.keep_outputs && !differs {
        let _ = fs::remove_dir_all(work.join("host"));
        for d in ["_ref", "_brrrty"] {
            let _ = fs::remove_dir_all(outs.join(d));
        }
    }
    r.secs = start.elapsed().as_secs_f64();
    r
}

/// Records which step ran out of time and says whether to go on.
fn keep_going(r: &mut SiteResult, name: &str, step: &Step) -> bool {
    if step.timed_out {
        r.timed_out = Some(name.to_string());
    }
    step.ok
}

#[allow(clippy::too_many_arguments)]
fn site_steps(
    ctx: &mut Ctx,
    entry: &SiteEntry,
    r: &mut SiteResult,
    clone: &Path,
    work: &Path,
    outs: &Path,
    logs: &Path,
    vol: &str,
    deadline: Instant,
) {
    // 1. Clone the default branch, one commit deep.
    if !clone.join(".git").is_dir() {
        let url = entry
            .url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{}.git", entry.repo));
        let mut c = Command::new("git");
        c.args(["clone", "--depth", "1", "--single-branch", "--no-tags"])
            .arg(&url)
            .arg(clone)
            .env("GIT_TERMINAL_PROMPT", "0");
        let step = run(&mut c, left(deadline), None, &logs.join("clone.log"));
        let ok = keep_going(r, "clone", &step);
        r.clone = Some(step);
        if !ok {
            return;
        }
    } else {
        r.clone = Some(Step {
            ok: true,
            exit: Some(0),
            ..Step::default()
        });
    }
    r.commit = git(clone, &["rev-parse", "HEAD"]);
    r.branch = git(clone, &["rev-parse", "--abbrev-ref", "HEAD"]);

    // 2. Install in Docker, with the network on.
    let image = node_image(clone);
    r.node_image = Some(image.clone());
    if let Err(e) = ctx.ensure_image(&image) {
        r.install = Some(failed(e));
        return;
    }
    let lock = ["package-lock.json", "npm-shrinkwrap.json"]
        .iter()
        .any(|f| clone.join(f).is_file());
    r.installer = Some(if lock { "npm ci" } else { "npm install" }.to_string());
    remove_volume(vol);
    let _ = Command::new("docker")
        .args(["volume", "create", "--label", LABEL, vol])
        .stdout(Stdio::null())
        .status();
    if let Err(e) = room(clone, &image) {
        r.install = Some(failed(e));
        return;
    }
    let name = docker_name("install", &r.slug);
    // A lockfile out of step with package.json fails `npm ci`; then
    // `npm install` resolves it. An install script that fails (one that
    // needs Python's pip, say) gets a second try without scripts. The
    // results say which happened.
    let install = format!(
        "set -e; cp -a /src/. /work/; cd /work; \
         inst() {{ if [ -f package-lock.json ] || [ -f npm-shrinkwrap.json ]; then \
           npm ci {NPM_FLAGS} \"$@\" || {{ echo 'CI_FAILED'; rm -rf node_modules; npm install {NPM_FLAGS} \"$@\"; }}; \
         else npm install {NPM_FLAGS} \"$@\"; fi; }}; \
         inst || {{ echo 'SCRIPTS_FAILED'; rm -rf node_modules; inst --ignore-scripts; }}"
    );
    let mut c = docker_run(&name, true);
    c.args(["--mount", &mount(clone, "/src", true)])
        .args(["--mount", &volume(vol, "/work", false)])
        .args(["-w", "/work", &image, "sh", "-c", &install]);
    let step = run(
        &mut c,
        left(deadline),
        Some(&name),
        &logs.join("install.log"),
    );
    let ok = keep_going(r, "install", &step);
    let log = fs::read_to_string(logs.join("install.log")).unwrap_or_default();
    // Only the last attempt counts: the one after any retry without scripts.
    let (retried, last) = match log.rsplit_once("SCRIPTS_FAILED") {
        Some((_, last)) => (true, last),
        None => (false, log.as_str()),
    };
    if lock && last.contains("CI_FAILED") {
        r.installer = Some("npm install (npm ci failed)".into());
    }
    if retried {
        let base = r.installer.take().unwrap_or_default();
        r.installer = Some(format!(
            "{base} --ignore-scripts (an install script failed)"
        ));
    }
    r.install = Some(step);
    if !ok {
        return;
    }

    // 3. The reference: 11ty at the frozen time, with no network. It comes
    // out as one tar file, so the host side never sees colliding names.
    let shim_dir = shim_path(ctx.root).parent().unwrap().to_path_buf();
    let reference = outs.join("_ref");
    let _ = fs::remove_dir_all(&reference);
    let tarball = work.join("ref.tar");
    let _ = fs::remove_file(&tarball);
    let script = format!(
        "{TIMED}; {ELEVENTY}; cd /work; node -p \"require('./node_modules/@11ty/eleventy/package.json').version\" > /out/eleventy-version.txt; \
         timed node --require /shim/freeze-time.cjs \"$B\" --quiet --output=/tmp/_ref || exit $?; \
         cd /tmp && find _ref | tr 'A-Z' 'a-z' | sort | uniq -d | sed 's/^/CASE_COLLISION /'; tar -cf /out/ref.tar _ref"
    );
    let name = docker_name("ref", &r.slug);
    let mut c = docker_run(&name, false);
    c.args(["--mount", &volume(vol, "/work", false)])
        .args(["--mount", &mount(&shim_dir, "/shim", true)])
        .args(["--mount", &mount(work, "/out", false)])
        .args(["-w", "/work", &image, "sh", "-c", &script]);
    let log = logs.join("reference.log");
    let mut step = run(&mut c, left(deadline), Some(&name), &log);
    if let Some(ms) = build_ms(&log).first() {
        step.secs = *ms as f64 / 1000.0;
    }
    r.eleventy_version = fs::read_to_string(work.join("eleventy-version.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let collisions = fs::read_to_string(&log)
        .map(|l| {
            l.lines()
                .filter(|l| l.starts_with("CASE_COLLISION "))
                .count()
        })
        .unwrap_or(0);
    if step.ok {
        // Paths that differ only in case would overwrite each other on a
        // case-insensitive disk; say so rather than compare half an output.
        if collisions > 0 && !case_sensitive(outs) {
            step.ok = false;
            step.error = Some(format!(
                "11ty built, but {collisions} of its output paths differ from others only in case, which {} can't hold; use --output-dir on a case-sensitive volume",
                outs.display()
            ));
        } else {
            let extracted = Command::new("tar")
                .arg("-xf")
                .arg(&tarball)
                .arg("-C")
                .arg(outs)
                .status()
                .is_ok_and(|s| s.success());
            if !extracted {
                step.ok = false;
                step.error = Some(format!("cannot extract {}", tarball.display()));
            }
        }
    }
    let _ = fs::remove_file(&tarball);
    let ref_ok = keep_going(r, "reference", &step);
    r.reference = Some(step);
    if ref_ok {
        r.reference_files = Some(count_files(&reference));
    }
    if r.timed_out.is_some() {
        return;
    }

    // 4. A host copy for Brrrty, which never runs JavaScript.
    let host = work.join("host");
    let _ = fs::remove_dir_all(&host);
    let copied = Command::new("cp")
        .arg("-R")
        .arg(clone)
        .arg(&host)
        .status()
        .is_ok_and(|s| s.success());
    if !copied {
        r.init = Some(failed(format!(
            "cannot copy the clone to {}",
            host.display()
        )));
        return;
    }
    if config_mentions_node_modules(&host) {
        copy_node_modules(&image, vol, work, &host, &r.slug, left(deadline), logs);
    }

    let mut c = Command::new(&ctx.brrrty);
    c.args(["init", "--yes", "--dir"]).arg(&host);
    let step = run(&mut c, left(deadline), None, &logs.join("init.log"));
    keep_going(r, "init", &step);
    r.init = Some(step);
    if let Ok(cfg) = fs::read_to_string(host.join("brrrty.config.rhai")) {
        r.init_todos = parse_todos(&cfg);
    }

    let mut c = Command::new(&ctx.brrrty);
    pin_env(&mut c)
        .args(["doctor", "--json"])
        .current_dir(&host);
    let log = logs.join("doctor.json");
    let step = run(&mut c, left(deadline), None, &log);
    match parse_doctor(&fs::read_to_string(&log).unwrap_or_default()) {
        Ok(d) => r.doctor = Some(d),
        Err(e) => r.doctor_error = Some(format!("{e}; {}", step.error.unwrap_or_default())),
    }
    if step.timed_out {
        r.timed_out = Some("doctor".into());
        return;
    }
    let Some(doctor) = &r.doctor else { return };
    let full = doctor.builds;

    // 5. Brrrty's build, compared with the reference. When doctor says some
    // pages can't build, the pages it does build are still compared.
    let actual = outs.join("_brrrty");
    let _ = fs::remove_dir_all(&actual);
    let _ = fs::create_dir_all(&actual);
    let mut c = Command::new(&ctx.brrrty);
    pin_env(&mut c)
        .args(["build", "--quiet", "--output"])
        .arg(&actual)
        .env("SOURCE_DATE_EPOCH", COMPAT_EPOCH.to_string())
        .current_dir(&host);
    let step = run(&mut c, left(deadline), None, &logs.join("build.log"));
    let built = step.ok;
    keep_going(r, "build", &step);
    r.build = Some(step);
    if r.timed_out.is_some() {
        return;
    }
    if ref_ok {
        r.compare = compare(&reference, &actual, full, outs);
    }

    // 6. Timings, for sites that build in full.
    if !(full && built && ref_ok) {
        return;
    }
    let mut t = Timing::default();
    let runs = ctx.opts.runs;
    let name = docker_name("time11ty", &r.slug);
    let script = format!(
        "{TIMED}; {ELEVENTY}; cd /work; for i in $(seq {runs}); do rm -rf /tmp/t; timed node --require /shim/freeze-time.cjs \"$B\" --quiet --output=/tmp/t >/dev/null; done"
    );
    let mut c = docker_run(&name, false);
    c.args(["--mount", &volume(vol, "/work", false)])
        .args(["--mount", &mount(&shim_dir, "/shim", true)])
        .args(["-w", "/work", &image, "sh", "-c", &script]);
    let log = logs.join("time-eleventy.log");
    let step = run(&mut c, left(deadline), Some(&name), &log);
    t.eleventy_docker_ms = build_ms(&log);
    if step.timed_out {
        r.timed_out = Some("timing 11ty".into());
    }
    if let Some(linux) = ctx.opts.linux_brrrty.clone() {
        let name = docker_name("timebrrrty", &r.slug);
        let script = format!(
            "{TIMED}; mkdir /w; cp -a /src/. /w/; cd /w; for i in $(seq {runs}); do rm -rf /tmp/t; timed /opt/brrrty build --quiet --output /tmp/t >/dev/null; done"
        );
        let mut c = docker_run(&name, false);
        c.args(["--mount", &mount(&host, "/src", true)])
            .args(["--mount", &mount(&linux, "/opt/brrrty", true)])
            .args([&image, "sh", "-c", &script]);
        let log = logs.join("time-brrrty-docker.log");
        let step = run(&mut c, left(deadline), Some(&name), &log);
        t.brrrty_docker_ms = build_ms(&log);
        if step.timed_out {
            r.timed_out = Some("timing brrrty (docker)".into());
        }
    }
    let tmp = outs.join("_time");
    for _ in 0..runs {
        let _ = fs::remove_dir_all(&tmp);
        let mut c = Command::new(&ctx.brrrty);
        pin_env(&mut c)
            .args(["build", "--quiet", "--output"])
            .arg(&tmp)
            .env("SOURCE_DATE_EPOCH", COMPAT_EPOCH.to_string())
            .current_dir(&host);
        let step = run(
            &mut c,
            left(deadline),
            None,
            &logs.join("time-brrrty-host.log"),
        );
        if !step.ok {
            if step.timed_out {
                r.timed_out = Some("timing brrrty (host)".into());
            }
            break;
        }
        t.brrrty_host_ms.push((step.secs * 1000.0).round() as u64);
    }
    let _ = fs::remove_dir_all(&tmp);
    r.timing = Some(t);
}

/// Whether names in `dir` are case-sensitive.
fn case_sensitive(dir: &Path) -> bool {
    let probe = dir.join(".case-probe-Aa");
    if fs::write(&probe, b"").is_err() {
        return false;
    }
    let sensitive = !dir.join(".case-probe-aA").exists();
    let _ = fs::remove_file(probe);
    sensitive
}

fn compare(reference: &Path, actual: &Path, full: bool, work: &Path) -> Option<Compare> {
    let report = compare_dirs(reference, actual, &AllowList::empty()).ok()?;
    let _ = fs::write(work.join("report.md"), report.to_markdown(200));
    let c = report.counts();
    let mut out = Compare {
        mode: if full { "full" } else { "partial" }.into(),
        identical: c.identical,
        equivalent: c.equivalent,
        different: c.different,
        missing: c.missing,
        extra: c.extra,
        ..Compare::default()
    };
    for f in &report.results {
        let p = f.path.display().to_string();
        let list = match f.outcome {
            Outcome::Different { .. } => &mut out.different_paths,
            Outcome::Missing => &mut out.missing_paths,
            Outcome::Extra => &mut out.extra_paths,
            _ => continue,
        };
        if list.len() < LISTED {
            list.push(p);
        }
    }
    Some(out)
}

/// Whether the site's 11ty config names `node_modules` (a passthrough copy
/// out of a package, usually), so the host copy needs the installed files.
fn config_mentions_node_modules(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    rd.filter_map(Result::ok).any(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        let is_config = n.starts_with("eleventy.config.") || n.starts_with(".eleventy.");
        is_config && fs::read_to_string(e.path()).is_ok_and(|s| s.contains("node_modules"))
    })
}

/// Copy the installed `node_modules` from the volume to the host copy, as
/// plain files: Brrrty reads them for passthrough copies and runs nothing.
fn copy_node_modules(
    image: &str,
    vol: &str,
    work: &Path,
    host: &Path,
    slug: &str,
    limit: Duration,
    logs: &Path,
) {
    let name = docker_name("nm", slug);
    let mut c = docker_run(&name, false);
    c.args(["--mount", &volume(vol, "/work", true)])
        .args(["--mount", &mount(work, "/out", false)])
        .args([
            image,
            "tar",
            "-C",
            "/work",
            "-cf",
            "/out/node_modules.tar",
            "node_modules",
        ]);
    if run(&mut c, limit, Some(&name), &logs.join("node-modules.log")).ok {
        let _ = Command::new("tar")
            .arg("-xf")
            .arg(work.join("node_modules.tar"))
            .arg("-C")
            .arg(host)
            .status();
    }
}

fn load_results(path: &Path) -> BTreeMap<String, SiteResult> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<SiteResult>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.slug.clone(), r))
        .collect()
}

fn save_results(path: &Path, order: &[String], results: &BTreeMap<String, SiteResult>) {
    let mut list: Vec<&SiteResult> = order.iter().filter_map(|s| results.get(s)).collect();
    for (slug, r) in results {
        if !order.contains(slug) {
            list.push(r);
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        let _ = fs::write(path, json + "\n");
    }
}

fn build_release(root: &Path) -> Result<PathBuf, String> {
    let ok = Command::new("cargo")
        .args(["build", "--release", "-p", "brrrty"])
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err("cargo build --release -p brrrty failed".into());
    }
    Ok(root.join("target/release/brrrty"))
}

pub fn run_corpus(root: &Path, opts: Options) -> Result<bool, String> {
    let src = fs::read_to_string(&opts.list)
        .map_err(|e| format!("cannot read {}: {e}", opts.list.display()))?;
    let list: SiteList = toml::from_str(&src).map_err(|e| format!("bad site list: {e}"))?;
    let mut sites: Vec<SiteEntry> = list
        .site
        .iter()
        .filter(|s| {
            opts.only.is_empty()
                || opts
                    .only
                    .iter()
                    .any(|o| *o == s.repo || *o == slug(&s.repo))
        })
        .cloned()
        .collect();
    if let Some(n) = opts.limit {
        sites.truncate(n);
    }
    for s in &sites {
        if s.repo.split('/').count() != 2 || s.repo.contains("..") {
            return Err(format!("repo must be owner/name: {:?}", s.repo));
        }
    }
    fs::create_dir_all(&opts.sites_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.sites_dir.display()))?;
    let sites_dir = opts
        .sites_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", opts.sites_dir.display()))?;
    fs::create_dir_all(&opts.work_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.work_dir.display()))?;
    let work_dir = opts
        .work_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", opts.work_dir.display()))?;
    fs::create_dir_all(&opts.output_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.output_dir.display()))?;
    let output_dir = opts
        .output_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", opts.output_dir.display()))?;
    let opts = Options {
        sites_dir,
        work_dir,
        output_dir,
        ..opts
    };
    let brrrty = build_release(root)?;
    if let Some(l) = &opts.linux_brrrty
        && !l.is_file()
    {
        return Err(format!("no Linux brrrty at {}", l.display()));
    }
    let version = Command::new(&brrrty)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let mut ctx = Ctx {
        root,
        opts: &opts,
        brrrty,
        version,
        pulled: Vec::new(),
    };
    let order: Vec<String> = list.site.iter().map(|s| slug(&s.repo)).collect();
    let mut results = load_results(&opts.results);
    for (i, s) in sites.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, sites.len(), s.repo);
        let r = run_site(&mut ctx, s);
        println!("  {}", summary(&r));
        results.insert(r.slug.clone(), r);
        save_results(&opts.results, &order, &results);
    }
    if opts.remove_images {
        for image in &ctx.pulled {
            let _ = Command::new("docker")
                .args(["image", "rm", image])
                .stdout(Stdio::null())
                .status();
        }
    } else if !ctx.pulled.is_empty() {
        println!("pulled (not removed): {}", ctx.pulled.join(", "));
    }
    Ok(true)
}

fn summary(r: &SiteResult) -> String {
    let step = |s: &Option<Step>| match s {
        None => "-".to_string(),
        Some(s) if s.ok => format!("ok {:.1}s", s.secs),
        Some(s) if s.timed_out => "TIMEOUT".to_string(),
        Some(_) => "FAIL".to_string(),
    };
    let doctor = r.doctor.as_ref().map_or("-".into(), |d| {
        format!(
            "{}/{} pages{}",
            d.pages_building,
            d.pages_total,
            if d.builds { " (builds)" } else { "" }
        )
    });
    let cmp = r.compare.as_ref().map_or("-".into(), |c| {
        format!(
            "{} ident {} equiv {} diff {} miss {} extra ({})",
            c.identical, c.equivalent, c.different, c.missing, c.extra, c.mode
        )
    });
    format!(
        "install {} | 11ty {} | doctor {doctor} | {cmp} | {:.0}s",
        step(&r.install),
        step(&r.reference),
        r.secs
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_docker_names() {
        assert_eq!(slug("Owner/Repo.site"), "Owner__Repo.site");
        assert_eq!(
            docker_name("ref", "Owner__Repo.site"),
            "brrrty-corpus-ref-owner__repo.site"
        );
    }

    #[test]
    fn df_output() {
        let out = b"Filesystem 1K-blocks Used Available Use% Mounted on\noverlay 61234567 38000000 20971520 67% /\n";
        assert_eq!(df_free_gib(out), Some(20.0));
    }

    #[test]
    fn node_versions() {
        assert_eq!(node_major("20.11.1\n"), Some(20));
        assert_eq!(node_major("v22"), Some(22));
        assert_eq!(node_major("lts/iron"), Some(20));
        assert_eq!(node_major("lts/*"), Some(24));
        assert_eq!(node_major("node"), Some(24));
        assert_eq!(node_major("banana"), None);
    }

    #[test]
    fn todos_name_the_config_method() {
        let cfg = "// TODO: port this filter to Rhai with config.add_filter.\n// See https://x\n// eleventy.config.js, line 15:\n//       eleventyConfig.addFilter(\"shout\", (s) => s);\n\n// TODO: port this plugin.\n// See https://y\n// eleventy.config.js, lines 3-4:\n//   eleventyConfig.addPlugin(pluginThing, {\n//   });\n\n// TODO: port all of this file by hand: it has a parse error.\n// See https://z\n";
        let t = parse_todos(cfg);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].why, "port this filter to Rhai with config.add_filter");
        assert_eq!(t[0].api.as_deref(), Some("addFilter"));
        assert_eq!(t[1].plugin.as_deref(), Some("pluginThing"));
        assert_eq!(t[2].api, None);
    }

    #[test]
    fn cause_keys() {
        let v: Value =
            serde_json::from_str(r#"{"kind":"template-language","extension":"11ty.js"}"#).unwrap();
        assert_eq!(cause_key(&v), "template-language: extension=11ty.js");
        let v: Value = serde_json::from_str(r#"{"kind":"follows"}"#).unwrap();
        assert_eq!(cause_key(&v), "follows");
        let v: Value =
            serde_json::from_str(r#"{"kind":"problem","code":"liquid-syntax","message":"x"}"#)
                .unwrap();
        assert_eq!(cause_family(&v), "problem: liquid-syntax");
    }

    #[test]
    fn build_times_only_count_clean_runs() {
        let d = std::env::temp_dir().join(format!("xtask-corpus-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let log = d.join("t.log");
        fs::write(
            &log,
            "BUILD_MS 120 rc=0\nnoise\nBUILD_MS 99 rc=1\nBUILD_MS 80 rc=0\n",
        )
        .unwrap();
        assert_eq!(build_ms(&log), [120, 80]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_slow_command_times_out() {
        let d = std::env::temp_dir().join(format!("xtask-corpus-to-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let mut c = Command::new("sleep");
        c.arg("5");
        let s = run(&mut c, Duration::from_millis(200), None, &d.join("log"));
        assert!(s.timed_out && !s.ok);
        assert!(s.secs < 4.0);
        fs::remove_dir_all(&d).unwrap();
    }
}
