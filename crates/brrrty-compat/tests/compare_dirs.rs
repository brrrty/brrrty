use brrrty_compat::{AllowList, Counts, Outcome, compare_dirs};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn put(root: &Path, name: &str, bytes: &[u8]) {
    fs::write(root.join(name), bytes).unwrap();
}

fn both(r: &TempDir, a: &TempDir, name: &str, reference: &[u8], actual: &[u8]) {
    put(r.path(), name, reference);
    put(a.path(), name, actual);
}

#[test]
fn compares_directories() {
    let (r, a) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    both(&r, &a, "same.html", b"<p>x</p>", b"<p>x</p>");
    both(&r, &a, "ws.html", b"<p>a\n b</p>\n", b"<p>a b</p>\n");
    both(
        &r,
        &a,
        "inline.html",
        b"<b>x</b> <i>y</i>",
        b"<b>x</b><i>y</i>",
    );
    both(&r, &a, "pre.html", b"<pre>a\n b</pre>", b"<pre>a b</pre>");
    both(&r, &a, "img.png", &[1, 2], &[1, 3]);
    put(r.path(), "only-ref.html", b"x");
    put(a.path(), "only-actual.html", b"x");

    let report = compare_dirs(r.path(), a.path(), &AllowList::empty()).unwrap();
    let get = |p: &str| {
        &report
            .results
            .iter()
            .find(|f| f.path == Path::new(p))
            .unwrap()
            .outcome
    };
    assert_eq!(get("same.html"), &Outcome::Identical);
    assert_eq!(get("ws.html"), &Outcome::Equivalent);
    assert!(matches!(get("inline.html"), Outcome::Different { .. }));
    assert!(matches!(get("pre.html"), Outcome::Different { .. }));
    match get("img.png") {
        Outcome::Different { diff } => assert!(diff.contains("binary files differ")),
        o => panic!("{o:?}"),
    }
    assert_eq!(get("only-ref.html"), &Outcome::Missing);
    assert_eq!(get("only-actual.html"), &Outcome::Extra);

    let want = Counts {
        identical: 1,
        equivalent: 1,
        different: 3,
        missing: 1,
        extra: 1,
    };
    assert_eq!(report.counts(), want);
    assert!(!report.is_pass());
    assert!(
        report
            .to_markdown(50)
            .starts_with("identical: 1, equivalent: 1, different: 3, missing: 1, extra: 1")
    );
}

#[test]
fn nested_dirs_and_allow_list() {
    let (r, a) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    for d in [&r, &a] {
        fs::create_dir_all(d.path().join("sub")).unwrap();
    }
    let toml = r#"
[[allow]]
id = "t"
files = "sub/*.html"
pattern = 'T[0-9]+'
reason = "r"
approved_by = "Josh"
approved_on = "2026-11-15"
"#;
    fs::write(r.path().join("sub/a.html"), "<p>T1</p>").unwrap();
    fs::write(a.path().join("sub/a.html"), "<p>T22</p>").unwrap();
    let report = compare_dirs(r.path(), a.path(), &AllowList::from_toml(toml).unwrap()).unwrap();
    assert_eq!(report.results[0].outcome, Outcome::Equivalent);
    assert!(report.is_pass());
}

fn one(name: &str, r: &[u8], a: &[u8], allow: &AllowList) -> Outcome {
    let (rd, ad) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    both(&rd, &ad, name, r, a);
    compare_dirs(rd.path(), ad.path(), allow)
        .unwrap()
        .results
        .remove(0)
        .outcome
}

#[test]
fn differing_css_is_different() {
    assert!(matches!(
        one("a.css", b"a{}", b"b{}", &AllowList::empty()),
        Outcome::Different { .. }
    ));
}

#[test]
fn non_utf8_text_difference_is_different() {
    let o = one("a.txt", &[0xff, 1], &[0xff, 2], &AllowList::empty());
    assert!(matches!(o, Outcome::Different { .. }));
}

#[test]
fn partial_allow_match_still_different() {
    let toml = r#"
[[allow]]
id = "t"
files = "*.html"
pattern = 'T[0-9]+'
reason = "r"
approved_by = "J"
approved_on = "d"
"#;
    let allow = AllowList::from_toml(toml).unwrap();
    assert!(matches!(
        one("a.html", b"<p>T1 one</p>", b"<p>T2 two</p>", &allow),
        Outcome::Different { .. }
    ));
}

#[test]
fn markup_diff_is_line_based_and_pre_normalization() {
    let r = b"<ul>\n<li>a</li>\n<li>b</li>\n<li>c</li>\n</ul>\n";
    let a = b"<ul>\n<li>a</li>\n<li>X</li>\n<li>c</li>\n</ul>\n";
    let Outcome::Different { diff } = one("a.html", r, a, &AllowList::empty()) else {
        panic!("expected different")
    };
    assert!(diff.lines().count() > 1, "{diff}");
    assert!(
        diff.contains("-<li>b</li>") && diff.contains("+<li>X</li>"),
        "{diff}"
    );
}
