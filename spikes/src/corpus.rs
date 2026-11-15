//! Parse every real template file on the private sites with each candidate.
//! Results name private files: write them only under `target/spike/`.

use crate::case::Lang;
use crate::score::{Engine, parse_checked};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SiteInput {
    pub name: String,
    /// Site root.
    pub root: PathBuf,
    /// Input directory (absolute).
    pub input: PathBuf,
}

#[derive(serde::Deserialize)]
struct Settings {
    #[serde(default = "dot")]
    input: String,
}

fn dot() -> String {
    ".".into()
}

impl SiteInput {
    /// Reads `<settings_dir>/<name>.toml` (the same format as xtask's settings) and
    /// resolves the site's input directory under `<sites_dir>/<name>`.
    pub fn load(sites_dir: &Path, settings_dir: &Path, name: &str) -> Result<SiteInput, String> {
        let path = settings_dir.join(format!("{name}.toml"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let settings: Settings = toml::from_str(&text).map_err(|e| e.to_string())?;
        let root = sites_dir.join(name);
        let input = root.join(&settings.input);
        Ok(SiteInput {
            name: name.to_string(),
            root,
            input,
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct FailureGroup {
    pub count: usize,
    /// The full text of the first error in the group.
    pub sample: String,
    pub examples: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct EngineCorpus {
    pub name: String,
    pub pass: usize,
    pub fail: usize,
    /// Keyed by the first 80 characters of the error.
    pub groups: BTreeMap<String, FailureGroup>,
    /// (pass, fail) per site name.
    pub per_site: BTreeMap<String, (usize, usize)>,
}

#[derive(Debug, Default, Clone)]
pub struct CorpusReport {
    pub files: usize,
    /// Files whose front matter could not be split; excluded for every engine.
    pub front_matter_errors: usize,
    pub engines: Vec<EngineCorpus>,
}

const SKIP_DIRS: [&str; 3] = ["node_modules", "_site", ".git"];

fn collect(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect(&path, exts, out);
            }
        } else if path
            .extension()
            .is_some_and(|x| exts.iter().any(|e| x == *e))
        {
            out.push(path);
        }
    }
}

/// First 80 characters of the error, ignoring source-position and source-excerpt lines
/// so that the same problem at different places groups together.
fn key_of(msg: &str) -> String {
    let lines: Vec<&str> = msg
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-->") && !l.starts_with('|'))
        .filter(|l| {
            !l.split_once('|')
                .is_some_and(|(n, _)| n.trim().parse::<u32>().is_ok())
        })
        .collect();
    lines.join(" ").chars().take(80).collect()
}

pub fn corpus(lang: Lang, engines: &[&dyn Engine], sites: &[SiteInput]) -> CorpusReport {
    let exts = [lang.ext(), "md", "html"];
    let mut report = CorpusReport {
        engines: engines
            .iter()
            .map(|e| EngineCorpus {
                name: e.name(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    for site in sites {
        let mut files = Vec::new();
        collect(&site.input, &exts, &mut files);
        for file in files {
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let Ok((_, body)) = brrrty_core::split_front_matter(&text) else {
                report.front_matter_errors += 1;
                continue;
            };
            report.files += 1;
            let shown = format!(
                "{}/{}",
                site.name,
                file.strip_prefix(&site.root).unwrap_or(&file).display()
            );
            for (engine, stats) in engines.iter().zip(report.engines.iter_mut()) {
                let site_stats = stats.per_site.entry(site.name.clone()).or_default();
                match parse_checked(*engine, body) {
                    Ok(()) => {
                        stats.pass += 1;
                        site_stats.0 += 1;
                    }
                    Err(msg) => {
                        stats.fail += 1;
                        site_stats.1 += 1;
                        let group = stats.groups.entry(key_of(&msg)).or_default();
                        if group.count == 0 {
                            group.sample = msg.clone();
                        }
                        group.count += 1;
                        if group.examples.len() < 3 {
                            group.examples.push(shown.clone());
                        }
                    }
                }
            }
        }
    }
    report
}

impl CorpusReport {
    pub fn to_json(&self, lang: Lang) -> Value {
        let engines: Vec<Value> = self
            .engines
            .iter()
            .map(|e| {
                let groups: Vec<Value> = e
                    .groups
                    .iter()
                    .map(|(k, g)| json!({ "error": k, "count": g.count, "sample": g.sample, "examples": g.examples }))
                    .collect();
                let per_site: BTreeMap<_, _> = e
                    .per_site
                    .iter()
                    .map(|(k, (p, f))| (k.clone(), json!({ "pass": p, "fail": f })))
                    .collect();
                json!({ "name": e.name, "pass": e.pass, "fail": e.fail,
                        "per_site": per_site, "failures": groups })
            })
            .collect();
        json!({ "lang": lang.dir(), "files": self.files,
                "front_matter_errors": self.front_matter_errors, "engines": engines })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::Case;

    struct Picky;
    impl Engine for Picky {
        fn name(&self) -> String {
            "picky".into()
        }
        fn render(&self, _: &Case) -> Result<String, String> {
            Ok(String::new())
        }
        fn parse(&self, s: &str) -> Result<(), String> {
            if s.contains("BAD") {
                Err("bad token".into())
            } else if s.contains("BOOM") {
                panic!("boom")
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn counts_files_groups_and_strips_front_matter() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("node_modules")).unwrap();
        fs::write(src.join("a.liquid"), "---\nBAD: 1\n---\nfine").unwrap();
        fs::write(src.join("b.md"), "BAD").unwrap();
        fs::write(src.join("c.html"), "BOOM").unwrap();
        fs::write(src.join("d.txt"), "BAD").unwrap();
        fs::write(src.join("node_modules/e.md"), "BAD").unwrap();
        let site = SiteInput {
            name: "s".into(),
            root: d.path().to_path_buf(),
            input: src,
        };
        let r = corpus(Lang::Liquid, &[&Picky], &[site]);
        assert_eq!(r.files, 3);
        let e = &r.engines[0];
        assert_eq!((e.pass, e.fail), (1, 2));
        assert_eq!(e.groups["bad token"].count, 1);
        assert!(e.groups.keys().any(|k| k.starts_with("panic: boom")));
        assert_eq!(e.per_site["s"], (1, 2));
    }
}
