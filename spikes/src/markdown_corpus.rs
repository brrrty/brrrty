//! Compare Markdown candidates against markdown-it (through 11ty) on every real `.md` body.
//! Everything written here names private files: it goes only under `target/spike/`.

use crate::corpus::{SiteInput, collect};
use crate::reference::{Build, run_11ty};
use crate::score::{SilentPanics, guarded};
use brrrty_markdown::MarkdownRenderer;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct Body {
    pub n: usize,
    pub site: String,
    /// Path relative to the site root; kept only in `index.json`.
    pub path: String,
    pub text: String,
    pub has_template_syntax: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Counts {
    pub scored: usize,
    pub identical: usize,
    pub equivalent: usize,
    pub mismatch: usize,
    pub panic: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Group {
    pub count: usize,
    pub examples: Vec<Value>,
}

#[derive(Debug, Default, Clone)]
pub struct CandidateResult {
    pub name: String,
    pub per_site: BTreeMap<String, Counts>,
    pub groups: BTreeMap<String, Group>,
    /// Best of 5 single-thread passes over the timed site's bodies (`MarkdownReport::timed_site`), in milliseconds.
    pub timed_ms: Option<f64>,
}

#[derive(Debug, Default)]
pub struct MarkdownReport {
    pub bodies: usize,
    pub front_matter_errors: usize,
    /// Files that could not be read as UTF-8 text; skipped and counted.
    pub unreadable: usize,
    pub timed_site: String,
    pub template_syntax: usize,
    pub template_syntax_per_site: BTreeMap<String, usize>,
    pub reference_failures: usize,
    pub candidates: Vec<CandidateResult>,
}

/// Every `.md` body (front matter removed) under the sites' inputs.
/// Returns the bodies, the front matter error count and the unreadable file count.
pub fn collect_bodies(sites: &[SiteInput]) -> (Vec<Body>, usize, usize) {
    let mut bodies = Vec::new();
    let mut fm_errors = 0;
    let mut unreadable = 0;
    for site in sites {
        let mut files = Vec::new();
        collect(&site.input, &["md"], &mut files);
        for file in files {
            let Ok(text) = fs::read_to_string(&file) else {
                unreadable += 1;
                continue;
            };
            let Ok((_, body)) = brrrty_core::split_front_matter(&text) else {
                fm_errors += 1;
                continue;
            };
            bodies.push(Body {
                n: bodies.len(),
                site: site.name.clone(),
                path: file
                    .strip_prefix(&site.root)
                    .unwrap_or(&file)
                    .display()
                    .to_string(),
                has_template_syntax: body.contains("{{") || body.contains("{%"),
                text: body.to_string(),
            });
        }
    }
    (bodies, fm_errors, unreadable)
}

/// Writes `<n>.md` files with `---json` front matter, and `index.json` beside them.
pub fn write_site(dir: &Path, bodies: &[Body]) -> Result<(), String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    for b in bodies {
        write_one(dir, b)?;
    }
    let index: BTreeMap<String, String> = bodies
        .iter()
        .map(|b| (b.n.to_string(), format!("{}/{}", b.site, b.path)))
        .collect();
    let p = dir.join("index.json");
    fs::write(&p, serde_json::to_string_pretty(&index).unwrap()).map_err(|e| io(&p, e))
}

fn write_one(dir: &Path, b: &Body) -> Result<(), String> {
    let front = json!({ "permalink": format!("/md/{}.html", b.n), "templateEngineOverride": "md" });
    let p = dir.join(format!("{}.md", b.n));
    fs::write(&p, format!("---json\n{front}\n---\n{}", b.text))
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// 11ty's output per body, `None` where 11ty itself failed on that body.
fn reference(
    dir: &Path,
    scratch: &Path,
    eleventy: &Path,
    bodies: &[Body],
) -> Result<Vec<Option<String>>, String> {
    let read = |site: &Path, n: usize| {
        let p = site.join(format!("_site/md/{n}.html"));
        fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
    };
    if let Build::Ok = run_11ty(dir, eleventy)? {
        return bodies.iter().map(|b| read(dir, b.n).map(Some)).collect();
    }
    // 11ty writes nothing when any template fails: retry each body alone.
    let mut out = Vec::new();
    for b in bodies {
        // Under `target/spike/`, never the OS temp dir: the bodies are private.
        let solo = scratch.join(b.n.to_string());
        fs::create_dir_all(&solo).map_err(|e| format!("{}: {e}", solo.display()))?;
        write_one(&solo, b)?;
        out.push(match run_11ty(&solo, eleventy)? {
            Build::Ok => Some(read(&solo, b.n)?),
            Build::Failed(_) => None,
        });
    }
    let _ = fs::remove_dir_all(scratch);
    Ok(out)
}

/// First `<tag` name on `line`, or "text" when the line has no tag.
fn tag_of(line: &str) -> String {
    let b = line.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'<' {
            let rest = &line[i + 1..];
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !name.is_empty() {
                return name.to_ascii_lowercase();
            }
        }
    }
    "text".into()
}

fn clip(s: &str) -> String {
    s.chars().take(160).collect()
}

/// (group tag, reference line, candidate line) for the first differing line.
fn first_diff(want: &str, got: &str) -> (String, String, String) {
    let mut a = want.lines();
    let mut b = got.lines();
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => {}
            (Some(x), y) => return (tag_of(x), clip(x), clip(y.unwrap_or("<end of output>"))),
            (None, Some(y)) => return (tag_of(y), "<end of reference>".into(), clip(y)),
            (None, None) => return ("trailing".into(), String::new(), String::new()),
        }
    }
}

fn render_guarded(r: &dyn MarkdownRenderer, src: &str) -> Result<String, String> {
    guarded(|| r.render(src))
}

fn score(
    r: &dyn MarkdownRenderer,
    name: &str,
    time_site: &str,
    bodies: &[Body],
    reference: &[Option<String>],
    out_dir: &Path,
) -> CandidateResult {
    let _quiet = SilentPanics::new();
    let mut res = CandidateResult {
        name: name.into(),
        ..Default::default()
    };
    let _ = fs::create_dir_all(out_dir);
    for (b, want) in bodies.iter().zip(reference) {
        let Some(want) = want else { continue };
        let counts = res.per_site.entry(b.site.clone()).or_default();
        counts.scored += 1;
        let got = match render_guarded(r, &b.text) {
            Ok(g) => g,
            Err(_) => {
                counts.panic += 1;
                continue;
            }
        };
        let _ = fs::write(out_dir.join(format!("{}.html", b.n)), &got);
        if got == *want {
            counts.identical += 1;
        } else if brrrty_compat::normalize_markup(&got) == brrrty_compat::normalize_markup(want) {
            counts.equivalent += 1;
        } else {
            counts.mismatch += 1;
            let (tag, ref_line, got_line) = first_diff(want, &got);
            let g = res.groups.entry(tag).or_default();
            g.count += 1;
            if g.examples.len() < 3 {
                g.examples.push(json!({
                    "n": b.n, "site": b.site, "reference_line": ref_line, "candidate_line": got_line
                }));
            }
        }
    }
    let timed: Vec<&Body> = bodies.iter().filter(|b| b.site == time_site).collect();
    if !timed.is_empty() {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            for b in &timed {
                let _ = guarded(|| r.render(&b.text));
            }
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
        }
        res.timed_ms = Some(best);
    }
    res
}

/// Runs the whole measurement. `out_root` is `target/spike`; the mini-site goes in
/// `<out_root>/md/` and candidate outputs in `<out_root>/md-out/<candidate>/`.
pub fn run(
    sites: &[SiteInput],
    eleventy: &Path,
    out_root: &Path,
    time_site: &str,
    candidates: &[(&str, &dyn MarkdownRenderer)],
) -> Result<MarkdownReport, String> {
    if !eleventy.is_file() {
        return Err(format!("eleventy not found at {}", eleventy.display()));
    }
    let (bodies, fm_errors, unreadable) = collect_bodies(sites);
    let site_dir: PathBuf = out_root.join("md");
    let _ = fs::remove_dir_all(&site_dir);
    let _ = fs::remove_dir_all(out_root.join("md-out"));
    write_site(&site_dir, &bodies)?;
    let reference = reference(&site_dir, &out_root.join("md-solo"), eleventy, &bodies)?;
    let mut report = MarkdownReport {
        bodies: bodies.len(),
        front_matter_errors: fm_errors,
        unreadable,
        timed_site: time_site.to_string(),
        reference_failures: reference.iter().filter(|r| r.is_none()).count(),
        ..Default::default()
    };
    for b in bodies.iter().filter(|b| b.has_template_syntax) {
        report.template_syntax += 1;
        *report
            .template_syntax_per_site
            .entry(b.site.clone())
            .or_default() += 1;
    }
    // Deep nesting can recurse far: score on a thread with a large stack.
    let bodies = &bodies;
    let reference = &reference;
    report.candidates = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn_scoped(s, || {
                candidates
                    .iter()
                    .map(|(name, r)| {
                        score(
                            *r,
                            name,
                            time_site,
                            bodies,
                            reference,
                            &out_root.join("md-out").join(name),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .expect("spawn scoring thread")
            .join()
            .expect("scoring thread")
    });
    Ok(report)
}

impl MarkdownReport {
    pub fn to_json(&self) -> Value {
        let candidates: Vec<Value> = self
            .candidates
            .iter()
            .map(|c| {
                let per_site: BTreeMap<_, _> = c
                    .per_site
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            json!({ "scored": v.scored, "identical": v.identical,
                                    "equivalent": v.equivalent, "mismatch": v.mismatch,
                                    "panic": v.panic }),
                        )
                    })
                    .collect();
                let groups: BTreeMap<_, _> = c
                    .groups
                    .iter()
                    .map(|(k, g)| {
                        (
                            k.clone(),
                            json!({ "count": g.count, "examples": g.examples }),
                        )
                    })
                    .collect();
                json!({ "name": c.name, "timed_ms": c.timed_ms,
                        "per_site": per_site, "mismatch_groups": groups })
            })
            .collect();
        json!({
            "bodies": self.bodies,
            "front_matter_errors": self.front_matter_errors,
            "unreadable": self.unreadable,
            "timed_site": self.timed_site,
            "reference_failures": self.reference_failures,
            "template_syntax": self.template_syntax,
            "template_syntax_per_site": self.template_syntax_per_site,
            "candidates": candidates,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::markdown::Pulldown;

    #[test]
    fn tags_the_first_differing_line() {
        assert_eq!(tag_of("  <li>x</li>"), "li");
        assert_eq!(tag_of("plain text"), "text");
        let (tag, a, b) = first_diff("<p>a</p>\n<hr>\n", "<p>a</p>\n<hr />\n");
        assert_eq!(
            (tag.as_str(), a.as_str(), b.as_str()),
            ("hr", "<hr>", "<hr />")
        );
    }

    #[test]
    fn collects_bodies_and_writes_the_mini_site() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.md"), "---\ntitle: x\n---\nhello {{ y }}\n").unwrap();
        fs::write(src.join("b.txt"), "no").unwrap();
        fs::write(src.join("bad.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let site = SiteInput {
            name: "s".into(),
            root: d.path().to_path_buf(),
            input: src,
        };
        let (bodies, errs, unreadable) = collect_bodies(&[site]);
        assert_eq!((bodies.len(), errs, unreadable), (1, 0, 1));
        assert!(bodies[0].has_template_syntax);
        let out = d.path().join("md");
        write_site(&out, &bodies).unwrap();
        let got = fs::read_to_string(out.join("0.md")).unwrap();
        assert!(got.starts_with(
            "---json\n{\"permalink\":\"/md/0.html\",\"templateEngineOverride\":\"md\"}\n---\nhello"
        ));
    }

    #[test]
    fn scores_identical_equivalent_mismatch() {
        let bodies: Vec<Body> = ["a\n", "b\n", "c\n"]
            .iter()
            .enumerate()
            .map(|(n, t)| Body {
                n,
                site: "s".into(),
                path: "p".into(),
                text: t.to_string(),
                has_template_syntax: false,
            })
            .collect();
        let reference = vec![
            Some("<p>a</p>\n".to_string()),
            Some("<p>b</p>\n\n".to_string()),
            Some("<p>X</p>\n".to_string()),
        ];
        let d = tempfile::tempdir().unwrap();
        let r = score(&Pulldown, "pd", "s", &bodies, &reference, d.path());
        let c = &r.per_site["s"];
        assert_eq!(
            (c.identical, c.equivalent, c.mismatch, c.panic),
            (1, 1, 1, 0)
        );
        assert_eq!(r.groups["p"].count, 1);
    }
}
