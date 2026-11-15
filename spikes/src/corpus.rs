//! Parse every real template file on the private sites with each candidate.
//! Results name private files: write them only under `target/spike/`.

use crate::case::Case;
use crate::case::Lang;
use crate::score::{Engine, parse_checked, render_checked};
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

/// Result of rendering a third-party script that must pass through a template engine unchanged.
#[derive(Debug, Clone)]
pub struct PassThrough {
    pub engine: String,
    /// `<site>/<path>` of the file.
    pub file: String,
    pub bytes: usize,
    pub identical: bool,
    /// Error text when the engine failed, else the first difference when not identical.
    pub detail: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct CorpusReport {
    /// One entry per engine per `lunr.js` found (Nunjucks corpus only).
    pub pass_through: Vec<PassThrough>,
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

/// Extensions whose `_includes/` files belong to another engine; the Nunjucks corpus skips them.
const OTHER_ENGINE_EXTS: [&str; 3] = ["liquid", "md", "html"];

/// The files to parse for `lang` on one site, each with whether it is a template file
/// (front matter is split) or a plain include (parsed whole).
/// Liquid: every `.liquid`, `.md` and `.html`. Nunjucks: every `.njk`, plus every file under
/// the input's `_includes/` that is not `.liquid`, `.md` or `.html`.
fn site_files(lang: Lang, site: &SiteInput) -> Vec<(PathBuf, bool)> {
    match lang {
        Lang::Liquid => {
            let mut files = Vec::new();
            collect(&site.input, &["liquid", "md", "html"], &mut files);
            files.into_iter().map(|f| (f, true)).collect()
        }
        Lang::Nunjucks => {
            let mut njk = Vec::new();
            collect(&site.input, &["njk"], &mut njk);
            let mut plain = Vec::new();
            collect_all(&site.input.join("_includes"), &mut plain);
            plain.retain(|p| {
                !p.extension()
                    .is_some_and(|x| OTHER_ENGINE_EXTS.iter().any(|e| x == *e))
            });
            let mut out: Vec<(PathBuf, bool)> = njk.iter().map(|f| (f.clone(), true)).collect();
            out.extend(
                plain
                    .into_iter()
                    .filter(|p| !njk.contains(p))
                    .map(|f| (f, false)),
            );
            out
        }
    }
}

fn collect_all(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_all(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn first_difference(want: &str, got: &str) -> String {
    let at = want
        .bytes()
        .zip(got.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));
    let line = want[..at.min(want.len())].matches('\n').count() + 1;
    let snippet = |s: &str| {
        s.get(at..)
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>()
    };
    format!(
        "first difference at byte {at} (input line {line}); input {:?}, output {:?}; lengths {} vs {}",
        snippet(want),
        snippet(got),
        want.len(),
        got.len()
    )
}

fn pass_through(engine: &dyn Engine, shown: &str, text: &str) -> PassThrough {
    let case = Case {
        name: "pass-through".into(),
        group: "core".into(),
        template: text.to_string(),
        data: Default::default(),
        includes: Default::default(),
    };
    let (identical, detail) = match render_checked(engine, &case) {
        Ok(out) if out == text => (true, None),
        Ok(out) => (false, Some(first_difference(text, &out))),
        Err(msg) => (false, Some(format!("error: {msg}"))),
    };
    PassThrough {
        engine: engine.name(),
        file: shown.to_string(),
        bytes: text.len(),
        identical,
        detail,
    }
}

pub fn corpus(lang: Lang, engines: &[&dyn Engine], sites: &[SiteInput]) -> CorpusReport {
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
        for (file, is_template) in site_files(lang, site) {
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let body = if is_template {
                let Ok((_, body)) = brrrty_core::split_front_matter(&text) else {
                    report.front_matter_errors += 1;
                    continue;
                };
                body
            } else {
                text.as_str()
            };
            report.files += 1;
            let shown = format!(
                "{}/{}",
                site.name,
                file.strip_prefix(&site.root).unwrap_or(&file).display()
            );
            if lang == Lang::Nunjucks && file.file_name().is_some_and(|n| n == "lunr.js") {
                for engine in engines {
                    report
                        .pass_through
                        .push(pass_through(*engine, &shown, body));
                }
            }
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
        let pass_through: Vec<Value> = self
            .pass_through
            .iter()
            .map(|p| {
                json!({ "engine": p.engine, "file": p.file, "bytes": p.bytes,
                            "identical": p.identical, "detail": p.detail })
            })
            .collect();
        json!({ "lang": lang.dir(), "files": self.files, "pass_through": pass_through,
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

    struct Echo;
    impl Engine for Echo {
        fn name(&self) -> String {
            "echo".into()
        }
        fn render(&self, c: &Case) -> Result<String, String> {
            Ok(c.template.replace('x', "y"))
        }
        fn parse(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn nunjucks_selects_njk_and_plain_includes_and_checks_pass_through() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("_includes/sub")).unwrap();
        fs::write(src.join("a.njk"), "---\nt: 1\n---\nbody").unwrap();
        fs::write(src.join("b.liquid"), "no").unwrap();
        fs::write(src.join("page.html"), "no").unwrap();
        fs::write(src.join("_includes/nav.njk"), "counted once").unwrap();
        fs::write(src.join("_includes/lunr.js"), "var a = 1;").unwrap();
        fs::write(src.join("_includes/sub/s.css"), "a{}").unwrap();
        fs::write(src.join("_includes/skip.liquid"), "no").unwrap();
        fs::write(src.join("_includes/skip.md"), "no").unwrap();
        fs::write(src.join("_includes/skip.html"), "no").unwrap();
        let site = SiteInput {
            name: "s".into(),
            root: d.path().to_path_buf(),
            input: src,
        };
        let r = corpus(Lang::Nunjucks, &[&Echo], &[site]);
        assert_eq!(r.files, 4);
        assert_eq!(r.pass_through.len(), 1);
        assert!(r.pass_through[0].identical);
        fs::write(d.path().join("src/_includes/lunr.js"), "var x = 1;").unwrap();
        let site = SiteInput {
            name: "s".into(),
            root: d.path().to_path_buf(),
            input: d.path().join("src"),
        };
        let r = corpus(Lang::Nunjucks, &[&Echo], &[site]);
        assert!(!r.pass_through[0].identical);
        assert!(
            r.pass_through[0]
                .detail
                .as_ref()
                .unwrap()
                .contains("byte 4")
        );
    }
}
