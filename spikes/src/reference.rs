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

/// First message line after 11ty's "Problem writing Eleventy templates:" header,
/// without the `[11ty] ` prefix or the trailing ` (via ErrorType)`.
/// Falls back to the first non-empty line when the header is absent.
pub fn parse_eleventy_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(|l| l.strip_prefix("[11ty] ").unwrap_or(l).trim_end())
        .collect();
    let first = match lines.iter().position(|l| l.contains(ERROR_MARKER)) {
        Some(i) => lines.get(i + 1).copied().unwrap_or(""),
        None => lines
            .iter()
            .copied()
            .find(|l| !l.trim().is_empty())
            .unwrap_or(""),
    };
    let via = Regex::new(r" \(via [A-Za-z]+\)$").unwrap();
    via.replace(first, "").into_owned()
}

enum Build {
    Ok,
    Failed(String),
}

fn run_11ty(dir: &Path, eleventy: &Path) -> Result<Build, String> {
    let out = Command::new("node")
        .arg(eleventy)
        .args(["--input=.", "--output=_site"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run node: {e}"))?;
    if out.status.success() {
        Ok(Build::Ok)
    } else {
        Ok(Build::Failed(parse_eleventy_error(
            &String::from_utf8_lossy(&out.stderr),
        )))
    }
}

fn copy_outputs(site: &Path, lang: Lang, cases: &[&Case], out_dir: &Path) -> Result<(), String> {
    for case in cases {
        let from = site
            .join("_site")
            .join(lang.dir())
            .join(format!("{}.txt", case.name));
        let to = out_dir.join(format!("{}.out", case.name));
        fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    Ok(())
}

/// Builds the cases with 11ty and records `<name>.out` or `<name>.error` in `out_dir`.
/// 11ty writes nothing when any template fails, so a failed build is retried case by case.
pub fn reference(
    lang: Lang,
    cases: &[Case],
    eleventy: &Path,
    out_dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for case in cases {
        for ext in ["out", "error"] {
            let _ = fs::remove_file(out_dir.join(format!("{}.{ext}", case.name)));
        }
    }
    let all: Vec<&Case> = cases.iter().collect();
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    write_mini_site(tmp.path(), lang, cases)?;
    match run_11ty(tmp.path(), eleventy)? {
        Build::Ok => return copy_outputs(tmp.path(), lang, &all, out_dir),
        Build::Failed(_) => {}
    }
    for case in cases {
        let solo = tempfile::tempdir().map_err(|e| e.to_string())?;
        write_mini_site(solo.path(), lang, std::slice::from_ref(case))?;
        match run_11ty(solo.path(), eleventy)? {
            Build::Ok => copy_outputs(solo.path(), lang, &[case], out_dir)?,
            Build::Failed(msg) => {
                let p = out_dir.join(format!("{}.error", case.name));
                fs::write(&p, msg).map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
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
    fn parses_first_message_line() {
        assert_eq!(
            parse_eleventy_error(SAMPLE),
            "1. Having trouble rendering liquid template ./x.liquid"
        );
    }

    #[test]
    fn parse_falls_back_to_first_line() {
        assert_eq!(parse_eleventy_error("boom\nmore\n"), "boom");
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
