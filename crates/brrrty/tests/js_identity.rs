//! Which object is which: what `===` says about the data a JavaScript
//! template or data function is handed. The site in `js_identity/` was
//! built with 11ty 3.1.8 (Node 22), and `expected.txt` is what it wrote.
//!
//! 11ty gives every page its own copy of each plain object in its data
//! (lists are shared), reads a JSON directory data file again for each
//! template, and wraps a paginated page's data so that reading a plain
//! object from the template's data gives a new wrapper each time.

use std::fs;
use std::path::Path;
use std::process::Command;

fn has_node() -> bool {
    let ok = Command::new("node").arg("--version").output().is_ok();
    if !ok {
        assert!(std::env::var_os("CI").is_none(), "CI needs Node");
        eprintln!("skipped: no node on PATH");
    }
    ok
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &target);
        } else {
            fs::copy(e.path(), target).unwrap();
        }
    }
}

fn built(mode: Option<&str>) -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/js_identity");
    let t = tempfile::tempdir().unwrap();
    copy(&fixture.join("src"), &t.path().join("src"));
    fs::copy(fixture.join("package.json"), t.path().join("package.json")).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_brrrty"));
    c.current_dir(t.path())
        .args(["build", "--input", "src", "--js-data=node"])
        .env_remove("BRRRTY_NODE")
        .env_remove("BRRRTY_JS_EXPORTS");
    if let Some(mode) = mode {
        c.env("BRRRTY_JS_EXPORTS", mode);
    }
    let o = c.output().expect("run brrrty");
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    let mut out = String::new();
    for page in ["d/x", "d/n", "d/l", "pg/0", "pg/1", "po/0", "po/1"] {
        let body = fs::read_to_string(t.path().join("_site").join(page).join("index.html"))
            .unwrap_or_else(|e| panic!("{page}: {e}"));
        out.push_str(&format!("{page}: {}\n", body.trim_end()));
    }
    out
}

fn expected() -> String {
    include_str!("js_identity/expected.txt").to_string()
}

/// Every comparison in the fixture, page by page, as 11ty answers it.
#[test]
fn objects_are_the_same_ones_where_they_are_in_11ty() {
    if !has_node() {
        return;
    }
    let want = expected();
    let got = built(None);
    for (w, g) in want.lines().zip(got.lines()) {
        assert_eq!(g, w);
    }
    assert_eq!(got.lines().count(), want.lines().count());
}

/// The same answers when the exports are walked instead of frozen.
#[test]
fn and_the_same_when_the_exports_are_walked() {
    if !has_node() {
        return;
    }
    assert_eq!(built(Some("walk")), expected());
}
