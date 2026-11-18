use brrrty_core::{BuildOptions, codes, discover};
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path, input: &str, output: &str) -> BuildOptions {
    BuildOptions {
        write_to: None,
        run_mode: Default::default(),
        root: root.to_path_buf(),
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        passthrough: vec![],
        strict: false,
        config: Default::default(),
        js: Default::default(),
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
        "src/d.11ty.js",
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
            "c.njk",
            "index.md",
            "page.html"
        ])
    );
    assert_eq!(d.unsupported, paths(&["d.11ty.js"]));
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
    write(root, "src/a.md", "x");
    for out in [".", "src"] {
        let p = opts(root, "src", out).validate().unwrap_err();
        assert_eq!(p.code, codes::OUTPUT_OVERLAPS_SOURCE);
    }
}

#[test]
fn relative_root_and_default_input_work() {
    // Cargo runs tests with the crate dir as cwd, which exists and is readable.
    let o = opts(Path::new("."), ".", "_site");
    o.validate().unwrap();
    assert!(discover(&o).is_ok());
    assert!(discover(&opts(Path::new("."), "src", "_site")).is_ok());
}

#[test]
fn input_path_spellings_are_equivalent() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "x");
    for input in ["src", "./src", "src/"] {
        let d = discover(&opts(root, input, "_site")).unwrap();
        assert_eq!(d.templates, paths(&["a.md"]), "input {input}");
    }
}

#[test]
fn absolute_output_inside_input_is_excluded() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "x");
    write(root, "src/out/old.html", "x");
    let abs = root.join("src/out");
    let d = discover(&opts(root, "src", abs.to_str().unwrap())).unwrap();
    assert_eq!(d.templates, paths(&["a.md"]));
}

#[test]
fn output_that_is_an_ancestor_of_input_is_refused() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "site/src/a.md", "x");
    let p = opts(t.path(), "site/src", "site").validate().unwrap_err();
    assert_eq!(p.code, codes::OUTPUT_OVERLAPS_SOURCE);
}

#[cfg(unix)]
#[test]
fn symlinked_output_is_still_refused_and_excluded() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "x");
    write(root, "src/out/old.html", "x");
    std::os::unix::fs::symlink(root.join("src"), root.join("link")).unwrap();
    // output reaches the input dir through a symlink
    let p = opts(root, "src", root.join("link").to_str().unwrap())
        .validate()
        .unwrap_err();
    assert_eq!(p.code, codes::OUTPUT_OVERLAPS_SOURCE);
    // output inside input via a symlink is excluded from discovery
    let via = root.join("link/out");
    let d = discover(&opts(root, "src", via.to_str().unwrap())).unwrap();
    assert_eq!(d.templates, paths(&["a.md"]));
}
