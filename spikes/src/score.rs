use crate::case::{Case, Expected};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

pub trait Engine: Send + Sync {
    fn name(&self) -> String;
    fn render(&self, case: &Case) -> Result<String, String>;
    fn parse(&self, source: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pass,
    Mismatch { got: String },
    UnexpectedError { msg: String },
    MissingError { got: String },
    ErrorMatch { msg: String, has_line: bool },
    Panic { msg: String },
}

pub struct CandidateRun {
    pub name: String,
    pub outcomes: Vec<(String, String, Outcome)>,
    pub render_ms: f64,
}

pub fn score(
    engine: &dyn Engine,
    cases: &[(Case, Option<Expected>)],
) -> Vec<(String, String, Outcome)> {
    let _quiet = SilentPanics::new();
    let line_re = Regex::new(r"(?i)line\s*\d+|\d+:\d+").unwrap();
    cases
        .iter()
        .filter_map(|(case, expected)| {
            let expected = expected.as_ref()?;
            let outcome = match guarded(|| engine.render(case)) {
                Err(msg) => Outcome::Panic { msg },
                Ok(result) => judge(expected, result, &line_re),
            };
            Some((case.name.clone(), case.group.clone(), outcome))
        })
        .collect()
}

fn judge(expected: &Expected, result: Result<String, String>, line_re: &Regex) -> Outcome {
    match (expected, result) {
        (Expected::Output(want), Ok(got)) if *want == got => Outcome::Pass,
        (Expected::Output(_), Ok(got)) => Outcome::Mismatch { got },
        (Expected::Output(_), Err(msg)) => Outcome::UnexpectedError { msg },
        (Expected::Error(_), Ok(got)) => Outcome::MissingError { got },
        (Expected::Error(_), Err(msg)) => {
            let has_line = line_re.is_match(&msg);
            Outcome::ErrorMatch { msg, has_line }
        }
    }
}

static HOOK_LOCK: Mutex<()> = Mutex::new(());

/// Replaces the panic hook with a silent one until dropped, then restores it.
struct SilentPanics {
    _lock: MutexGuard<'static, ()>,
    prev: Option<PanicHook>,
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

impl SilentPanics {
    fn new() -> Self {
        let lock = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        SilentPanics {
            _lock: lock,
            prev: Some(prev),
        }
    }
}

impl Drop for SilentPanics {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            std::panic::set_hook(prev);
        }
    }
}

/// `Engine::parse` with the same panic protection as scoring a render.
#[allow(dead_code)] // used by the candidate adapters added in later tasks
pub fn parse_checked(engine: &dyn Engine, source: &str) -> Result<(), String> {
    let _quiet = SilentPanics::new();
    guarded(|| engine.parse(source)).unwrap_or_else(|msg| Err(format!("panic: {msg}")))
}

/// Runs `f`, turning a panic into `Err(message)`.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic".into())
    })
}

/// Cases with no expected file; `score` skips them.
pub fn unreferenced(cases: &[(Case, Option<Expected>)]) -> usize {
    cases.iter().filter(|(_, e)| e.is_none()).count()
}

/// Best of 5 runs, each rendering every referenced case 20 times, in milliseconds.
/// Errors and panics are ignored; they still cost time but are scored elsewhere.
pub fn time_render(engine: &dyn Engine, cases: &[(Case, Option<Expected>)]) -> f64 {
    let _quiet = SilentPanics::new();
    let mut best = f64::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..20 {
            for (case, expected) in cases {
                if expected.is_some() {
                    let _ = guarded(|| engine.render(case));
                }
            }
        }
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

fn kind(o: &Outcome) -> &'static str {
    match o {
        Outcome::Pass => "pass",
        Outcome::Mismatch { .. } => "mismatch",
        Outcome::UnexpectedError { .. } => "unexpected_error",
        Outcome::MissingError { .. } => "missing_error",
        Outcome::ErrorMatch { .. } => "error_match",
        Outcome::Panic { .. } => "panic",
    }
}

pub fn write_results(path: &Path, engine_kind: &str, runs: &[CandidateRun]) -> std::io::Result<()> {
    let candidates: Vec<Value> = runs
        .iter()
        .map(|run| {
            let mut summary: BTreeMap<&str, u64> = BTreeMap::new();
            let outcomes: Vec<Value> = run
                .outcomes
                .iter()
                .map(|(case, group, o)| {
                    *summary.entry(kind(o)).or_default() += 1;
                    let mut m = json!({ "case": case, "group": group, "outcome": kind(o) });
                    match o {
                        Outcome::Pass => {}
                        Outcome::Mismatch { got } | Outcome::MissingError { got } => m["got"] = json!(got),
                        Outcome::UnexpectedError { msg } | Outcome::Panic { msg } => m["msg"] = json!(msg),
                        Outcome::ErrorMatch { msg, has_line } => {
                            m["msg"] = json!(msg);
                            m["has_line"] = json!(has_line);
                        }
                    }
                    m
                })
                .collect();
            json!({ "name": run.name, "render_ms": run.render_ms, "summary": summary, "outcomes": outcomes })
        })
        .collect();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let doc = json!({ "engine_kind": engine_kind, "candidates": candidates });
    fs::write(
        path,
        serde_json::to_string_pretty(&doc).expect("json is serializable") + "\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Engine for Echo {
        fn name(&self) -> String {
            "echo".into()
        }
        fn render(&self, c: &Case) -> Result<String, String> {
            if c.template == "ERR" {
                Err("line 3: unexpected".into())
            } else if c.template == "ERRNOLINE" {
                Err("bad".into())
            } else if c.template == "PANIC" {
                panic!("kaboom")
            } else {
                Ok(c.template.clone())
            }
        }
        fn parse(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn case(name: &str, t: &str, e: Option<Expected>) -> (Case, Option<Expected>) {
        (
            Case {
                name: name.into(),
                group: "core".into(),
                template: t.into(),
                data: Default::default(),
                includes: Default::default(),
            },
            e,
        )
    }

    #[test]
    fn scores_every_outcome() {
        let cases = vec![
            case("pass", "hi", Some(Expected::Output("hi".into()))),
            case("mis", "hi", Some(Expected::Output("ho".into()))),
            case("unexp", "ERR", Some(Expected::Output("x".into()))),
            case("missing", "hi", Some(Expected::Error("e".into()))),
            case("errline", "ERR", Some(Expected::Error("e".into()))),
            case("errnoline", "ERRNOLINE", Some(Expected::Error("e".into()))),
            case("panic", "PANIC", Some(Expected::Output("x".into()))),
            case("after", "hi", Some(Expected::Output("hi".into()))),
            case("skipped", "hi", None),
        ];
        let r = score(&Echo, &cases);
        assert_eq!(r.len(), 8);
        let get = |n: &str| r.iter().find(|x| x.0 == n).unwrap().2.clone();
        assert_eq!(get("pass"), Outcome::Pass);
        assert_eq!(get("mis"), Outcome::Mismatch { got: "hi".into() });
        assert_eq!(
            get("unexp"),
            Outcome::UnexpectedError {
                msg: "line 3: unexpected".into()
            }
        );
        assert_eq!(get("missing"), Outcome::MissingError { got: "hi".into() });
        assert_eq!(
            get("errline"),
            Outcome::ErrorMatch {
                msg: "line 3: unexpected".into(),
                has_line: true
            }
        );
        assert_eq!(
            get("errnoline"),
            Outcome::ErrorMatch {
                msg: "bad".into(),
                has_line: false
            }
        );
        assert!(matches!(get("panic"), Outcome::Panic { msg } if msg.contains("kaboom")));
        assert_eq!(get("after"), Outcome::Pass);
        assert_eq!(unreferenced(&cases), 1);
    }

    #[test]
    fn parse_panic_is_caught() {
        struct P;
        impl Engine for P {
            fn name(&self) -> String {
                "p".into()
            }
            fn render(&self, _: &Case) -> Result<String, String> {
                Ok(String::new())
            }
            fn parse(&self, _: &str) -> Result<(), String> {
                panic!("parse boom")
            }
        }
        assert!(parse_checked(&P, "x").unwrap_err().contains("parse boom"));
    }

    #[test]
    fn has_line_matches_colon_form() {
        struct E;
        impl Engine for E {
            fn name(&self) -> String {
                "e".into()
            }
            fn render(&self, _: &Case) -> Result<String, String> {
                Err("at 4:12 oops".into())
            }
            fn parse(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let r = score(&E, &[case("a", "x", Some(Expected::Error("e".into())))]);
        assert!(matches!(r[0].2, Outcome::ErrorMatch { has_line: true, .. }));
    }

    #[test]
    fn writes_results_json() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/liquid.json");
        let runs = vec![CandidateRun {
            name: "echo".into(),
            outcomes: vec![("pass".into(), "core".into(), Outcome::Pass)],
            render_ms: 1.5,
        }];
        write_results(&p, "liquid", &runs).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(v["engine_kind"], "liquid");
        assert_eq!(v["candidates"][0]["name"], "echo");
        assert_eq!(v["candidates"][0]["summary"]["pass"], 1);
    }
}
