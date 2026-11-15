use brrrty_core::{BuildOptions, discover};
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path, input: &str, output: &str) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        passthrough: vec![],
        strict: false,
    }
}

fn paths(p: &[&str]) -> Vec<PathBuf> {
    p.iter().map(PathBuf::from).collect()
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn follows_11ty_default_rules() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for f in [
        "src/index.md",
        "src/_draft.md",
        "src/.dot.md",
        "src/.hidden/a.md",
        "src/page.html",
        "src/notes.txt",
        "src/_includes/x.md",
        "src/_data/d.json",
        "src/c.njk",
        "src/sub/node_modules/y.md",
        "node_modules/x/n.md",
        "src/README.md",
        "src/plain.md",
    ] {
        write(root, f, "x");
    }
    write(root, ".gitignore", "src/plain.md\n");
    write(root, ".eleventyignore", "src/README.md\n");
    let d = discover(&opts(root, "src", "_site")).unwrap();
    assert_eq!(
        d.templates,
        paths(&[
            ".dot.md",
            ".hidden/a.md",
            "_draft.md",
            "index.md",
            "page.html"
        ])
    );
    assert_eq!(d.unsupported, paths(&["c.njk"]));
}

#[test]
fn output_inside_input_is_not_discovered() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "x");
    write(root, "src/_site/old/index.html", "x");
    let d = discover(&opts(root, "src", "src/_site")).unwrap();
    assert_eq!(d.templates, paths(&["a.md"]));
}

#[test]
fn output_overlapping_source_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for out in [".", "src"] {
        let p = opts(root, "src", out).validate().unwrap_err();
        assert_eq!(p.code, "output-overlaps-source");
    }
}
