//! Which Node runs the data: `BRRRTY_NODE` when set, else `node` on `PATH`.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 11ty 3's own floor.
pub const MIN_NODE: (u32, u32, u32) = (18, 0, 0);

/// How long `node -p` may take to say what it is.
const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// A Node that can run the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// As given (`BRRRTY_NODE`) or found on `PATH`.
    pub path: PathBuf,
    pub version: (u32, u32, u32),
    /// `process.versions.tz`, the time zone data it formats dates with.
    pub tz: String,
    /// `process.versions.icu`.
    pub icu: String,
    /// `module.registerHooks` exists (22.15 and later), so the worker can
    /// record every module it loads.
    pub load_hooks: bool,
}

impl NodeInfo {
    /// `v24.22.3`.
    pub fn version_text(&self) -> String {
        let (a, b, c) = self.version;
        format!("v{a}.{b}.{c}")
    }
}

/// Why there is no Node to run the data with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeMissing {
    /// Nothing ran: each place tried, as shown to the user.
    NotFound { tried: Vec<String> },
    /// Older than 18.0.0.
    TooOld { path: PathBuf, version: String },
    /// It ran, but didn't answer as Node does.
    Broken { path: PathBuf, stderr: String },
}

impl std::fmt::Display for NodeMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeMissing::NotFound { tried } => {
                write!(f, "Node not found (tried {})", tried.join(", "))
            }
            NodeMissing::TooOld { path, version } => write!(
                f,
                "Node {version} at {} is too old: JavaScript data needs Node 18 or later",
                path.display()
            ),
            NodeMissing::Broken { path, stderr } => {
                write!(
                    f,
                    "{} didn't run as Node: {}",
                    path.display(),
                    stderr.trim()
                )
            }
        }
    }
}

/// Find Node: `env_override` (the value of `BRRRTY_NODE`) when set, else
/// `node` on `PATH`.
pub fn find_node(env_override: Option<&OsStr>) -> Result<NodeInfo, NodeMissing> {
    let (program, tried): (OsString, Vec<String>) = match env_override {
        Some(p) if !p.is_empty() => (
            p.to_os_string(),
            vec![format!("BRRRTY_NODE={}", p.to_string_lossy())],
        ),
        _ => ("node".into(), vec!["node on PATH".to_string()]),
    };
    // The last item names the runtime: Bun and Deno answer like Node, but
    // load modules and read files their own way.
    let script = r#"process.stdout.write(JSON.stringify([process.version, process.versions.tz || "", process.versions.icu || "", typeof require("node:module").registerHooks, process.versions.bun ? "Bun" : process.versions.deno ? "Deno" : (process.release && process.release.name) || ""]))"#;
    let mut child = match Command::new(&program)
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Err(NodeMissing::NotFound { tried }),
    };
    let path = PathBuf::from(&program);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < PROBE_LIMIT => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(NodeMissing::Broken {
                    path,
                    stderr: format!("no answer within {} s", PROBE_LIMIT.as_secs()),
                });
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    if !status.success() {
        return Err(NodeMissing::Broken { path, stderr });
    }
    parse_probe(&path, &stdout).ok_or(NodeMissing::Broken {
        path: path.clone(),
        stderr: format!("unexpected answer: {}", stdout.trim()),
    })?
}

fn parse_probe(path: &Path, stdout: &str) -> Option<Result<NodeInfo, NodeMissing>> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let a = v.as_array()?;
    let runtime = a.get(4)?.as_str()?;
    if runtime != "node" {
        return Some(Err(NodeMissing::Broken {
            path: path.to_path_buf(),
            stderr: format!("it is {runtime}, which runs JavaScript differently from Node"),
        }));
    }
    let version_text = a.first()?.as_str()?;
    let version = parse_version(version_text)?;
    if version < MIN_NODE {
        return Some(Err(NodeMissing::TooOld {
            path: path.to_path_buf(),
            version: version_text.to_string(),
        }));
    }
    Some(Ok(NodeInfo {
        path: path.to_path_buf(),
        version,
        tz: a.get(1)?.as_str()?.to_string(),
        icu: a.get(2)?.as_str()?.to_string(),
        load_hooks: a.get(3)?.as_str()? == "function",
    }))
}

/// `v24.22.3` (or `24.22.3`) as numbers.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let t = text.trim().strip_prefix('v').unwrap_or(text.trim());
    let mut parts = t.split(['.', '-']);
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("v24.22.3"), Some((24, 22, 3)));
        assert_eq!(parse_version("18.0.0"), Some((18, 0, 0)));
        assert_eq!(parse_version("v26.0.0-nightly"), Some((26, 0, 0)));
        assert_eq!(parse_version("x"), None);
    }

    #[test]
    fn an_old_node_is_too_old() {
        let r = parse_probe(
            Path::new("n"),
            r#"["v16.20.2","2023c","73.1","undefined","node"]"#,
        );
        assert!(matches!(r, Some(Err(NodeMissing::TooOld { .. }))));
        let r = parse_probe(
            Path::new("n"),
            r#"["v22.15.0","2025a","76.1","function","node"]"#,
        );
        assert!(matches!(
            r,
            Some(Ok(NodeInfo {
                load_hooks: true,
                ..
            }))
        ));
    }

    #[test]
    fn a_runtime_that_only_answers_like_node_is_not_node() {
        let r = parse_probe(
            Path::new("bun"),
            r#"["v24.3.0","2025b","77.1","function","Bun"]"#,
        );
        match r {
            Some(Err(NodeMissing::Broken { stderr, .. })) => assert!(stderr.contains("Bun")),
            other => panic!("{other:?}"),
        }
    }
}
