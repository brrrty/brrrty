use crate::case::{Case, Lang};
use regex::Regex;
use std::fs;
use std::path::Path;
use std::process::Command;

const ERROR_MARKER: &str = "Problem writing Eleventy templates:";

/// Writes `cases/<name>.<ext>` (JSON front matter plus the template) and `_includes/*`.
pub fn write_mini_site(dir: &Path, lang: Lang, cases: &[Case]) -> Result<(), String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let cases_dir = dir.join("cases");
    fs::create_dir_all(&cases_dir).map_err(|e| io(&cases_dir, e))?;
    for case in cases {
        let mut data = case.data.clone();
        data.insert(
            "permalink".into(),
            format!("/{}/{}.txt", lang.dir(), case.name).into(),
        );
        let front = serde_json::to_string(&data).map_err(|e| e.to_string())?;
        let body = format!("---json\n{front}\n---\n{}", case.template);
        let path = cases_dir.join(format!("{}.{}", case.name, lang.ext()));
        fs::write(&path, body).map_err(|e| io(&path, e))?;
        for (name, content) in &case.includes {
            let path = dir.join("_includes").join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
            }
            fs::write(&path, content).map_err(|e| io(&path, e))?;
        }
    }
    Ok(())
}

/// Root cause of a template failure: the last numbered line of 11ty's error chain
/// after the "Problem writing Eleventy templates:" header, without the `N. `
/// numbering, the `[11ty] ` prefix or the trailing ` (via ErrorType)`.
/// Returns `None` when stderr has no such header (an environment failure, not a template error).
pub fn parse_eleventy_error(stderr: &str) -> Option<String> {
    let numbered = Regex::new(r"^\d+\. (.*)$").unwrap();
    let via = Regex::new(r" \(via [A-Za-z]+\)$").unwrap();
    if !stderr.contains(ERROR_MARKER) {
        return None;
    }
    let mut lines = stderr
        .lines()
        .map(|l| l.strip_prefix("[11ty] ").unwrap_or(l).trim_end())
        .skip_while(|l| !l.contains(ERROR_MARKER))
        .skip(1);
    let mut last = None;
    for l in lines.by_ref() {
        match numbered.captures(l) {
            Some(c) => last = Some(c[1].to_string()),
            None => break,
        }
    }
    let msg = last.unwrap_or_default();
    Some(via.replace(&msg, "").into_owned())
}

pub enum Build {
    Ok,
    Failed(String),
}

fn tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().collect();
    lines[lines.len().saturating_sub(10)..].join("\n")
}

pub fn run_11ty(dir: &Path, eleventy: &Path) -> Result<Build, String> {
    let out = Command::new("node")
        .arg(eleventy)
        .args(["--input=.", "--output=_site"])
        .current_dir(dir)
        .env("TZ", "UTC")
        .output()
        .map_err(|e| format!("cannot run node: {e}"))?;
    if out.status.success() {
        return Ok(Build::Ok);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match parse_eleventy_error(&stderr) {
        Some(msg) => Ok(Build::Failed(msg)),
        None => Err(format!(
            "11ty failed without reporting a template error (environment problem?):\n{}",
            tail(&stderr)
        )),
    }
}

fn read_output(site: &Path, lang: Lang, case: &Case) -> Result<String, String> {
    let from = site
        .join("_site")
        .join(lang.dir())
        .join(format!("{}.txt", case.name));
    fs::read_to_string(&from).map_err(|e| format!("{}: {e}", from.display()))
}

/// Builds the cases with 11ty and records `<name>.out` or `<name>.error` in `out_dir`.
/// 11ty writes nothing when any template fails, so a failed build is retried case by case.
/// Nothing in `out_dir` changes unless every case's outcome is known.
pub fn reference(
    lang: Lang,
    cases: &[Case],
    eleventy: &Path,
    out_dir: &Path,
) -> Result<(), String> {
    if !eleventy.is_file() {
        return Err(format!("eleventy not found at {}", eleventy.display()));
    }
    let mut results: Vec<(&Case, Result<String, String>)> = Vec::new();
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    write_mini_site(tmp.path(), lang, cases)?;
    match run_11ty(tmp.path(), eleventy)? {
        Build::Ok => {
            for case in cases {
                results.push((case, Ok(read_output(tmp.path(), lang, case)?)));
            }
        }
        Build::Failed(_) => {
            for case in cases {
                let solo = tempfile::tempdir().map_err(|e| e.to_string())?;
                write_mini_site(solo.path(), lang, std::slice::from_ref(case))?;
                let r = match run_11ty(solo.path(), eleventy)? {
                    Build::Ok => Ok(read_output(solo.path(), lang, case)?),
                    Build::Failed(msg) => Err(msg),
                };
                results.push((case, r));
            }
        }
    }
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for (case, result) in results {
        for ext in ["out", "error"] {
            let _ = fs::remove_file(out_dir.join(format!("{}.{ext}", case.name)));
        }
        let (ext, body) = match result {
            Ok(out) => ("out", out),
            Err(msg) => ("error", msg),
        };
        let p = out_dir.join(format!("{}.{ext}", case.name));
        fs::write(&p, body).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn one(name: &str, template: &str, data: &str) -> Case {
        Case {
            name: name.into(),
            group: "core".into(),
            template: template.into(),
            data: serde_json::from_str(data).unwrap(),
            includes: Default::default(),
        }
    }

    #[test]
    fn mini_site_layout_and_front_matter() {
        let d = tempfile::tempdir().unwrap();
        let mut c = one("greet", "{% include 'p.liquid' %}{{ a }}", r#"{"a":"hi"}"#);
        c.includes.insert("p.liquid".into(), "part".into());
        write_mini_site(d.path(), Lang::Liquid, &[c]).unwrap();
        let got = fs::read_to_string(d.path().join("cases/greet.liquid")).unwrap();
        assert_eq!(
            got,
            "---json\n{\"a\":\"hi\",\"permalink\":\"/liquid/greet.txt\"}\n---\n{% include 'p.liquid' %}{{ a }}"
        );
        assert_eq!(
            fs::read_to_string(d.path().join("_includes/p.liquid")).unwrap(),
            "part"
        );
    }

    #[test]
    fn nunjucks_uses_njk_extension() {
        let d = tempfile::tempdir().unwrap();
        write_mini_site(d.path(), Lang::Nunjucks, &[one("n", "x", "{}")]).unwrap();
        let got = fs::read_to_string(d.path().join("cases/n.njk")).unwrap();
        assert!(got.contains("\"permalink\":\"/nunjucks/n.txt\""));
    }

    // Captured from a real failing 11ty 3.1.8 run (Liquid `{{ x | nosuchfilter }}`).
    const SAMPLE: &str = "[11ty] Problem writing Eleventy templates:\n\
[11ty] 1. Having trouble rendering liquid template ./x.liquid (via TemplateContentRenderError)\n\
[11ty] 2. Having trouble compiling template ./x.liquid (via TemplateContentCompileError)\n\
[11ty] 3. undefined filter: nosuchfilter, file:./x.liquid, line:1, col:1 (via ParseError)\n\
[11ty] \n\
[11ty] Original error stack trace: AssertionError: undefined filter: nosuchfilter\n\
[11ty] Wrote 0 files in 0.04 seconds (v3.1.8)\n";

    #[test]
    fn parses_root_cause_line() {
        assert_eq!(
            parse_eleventy_error(SAMPLE).as_deref(),
            Some("undefined filter: nosuchfilter, file:./x.liquid, line:1, col:1")
        );
    }

    #[test]
    fn stderr_without_marker_is_not_a_template_error() {
        assert_eq!(
            parse_eleventy_error("Error: Cannot find module 'x'\n"),
            None
        );
    }

    #[test]
    fn missing_eleventy_path_errors_and_keeps_expected_files() {
        let out = tempfile::tempdir().unwrap();
        fs::write(out.path().join("a.out"), "keep").unwrap();
        let err = reference(
            Lang::Liquid,
            &[one("a", "x", "{}")],
            Path::new("/nonexistent/cmd.cjs"),
            out.path(),
        )
        .unwrap_err();
        assert!(err.contains("not found"));
        assert_eq!(
            fs::read_to_string(out.path().join("a.out")).unwrap(),
            "keep"
        );
    }

    #[test]
    #[ignore = "needs Node"]
    fn broken_eleventy_install_errors_and_keeps_expected_files() {
        let d = tempfile::tempdir().unwrap();
        let fake = d.path().join("cmd.cjs");
        fs::write(&fake, "throw new Error('boom')").unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(out.path().join("a.out"), "keep").unwrap();
        let err = reference(Lang::Liquid, &[one("a", "x", "{}")], &fake, out.path()).unwrap_err();
        assert!(err.contains("environment"));
        assert_eq!(
            fs::read_to_string(out.path().join("a.out")).unwrap(),
            "keep"
        );
    }

    #[test]
    #[ignore = "needs Node and the 11ty install"]
    fn reference_runs_11ty() {
        let out = tempfile::tempdir().unwrap();
        let eleventy = Path::new(crate::DEFAULT_ELEVENTY);
        reference(
            Lang::Liquid,
            &[one("trivial", "{{ a }}", r#"{"a":"hi"}"#)],
            eleventy,
            out.path(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.path().join("trivial.out")).unwrap(),
            "hi"
        );
    }

    #[test]
    #[ignore = "needs Node and the 11ty install"]
    fn reference_isolates_failing_case() {
        let out = tempfile::tempdir().unwrap();
        let eleventy = Path::new(crate::DEFAULT_ELEVENTY);
        let cases = [
            one("good", "{{ a }}", r#"{"a":"hi"}"#),
            one("bad", "{{ a | nosuchfilter }}", "{}"),
        ];
        reference(Lang::Liquid, &cases, eleventy, out.path()).unwrap();
        assert_eq!(
            fs::read_to_string(out.path().join("good.out")).unwrap(),
            "hi"
        );
        assert!(!out.path().join("bad.out").exists());
        assert!(out.path().join("bad.error").exists());
    }
}
