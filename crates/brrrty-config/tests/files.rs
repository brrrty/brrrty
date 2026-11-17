//! Scripts that read files: `read_text`, `read_json`, `read_dir`,
//! `file_exists` and `parse_csv`, as a JavaScript data file uses `fs` and
//! csv-parse. What Node gives was probed with Node 26.2.0 on macOS; the CSV
//! parser itself is checked case by case in `csv_probes.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use brrrty_config::{CONFIG_FILE, Clock, data_runner, load_with};
use brrrty_core::frontmatter::Data;
use brrrty_core::{BuildOptions, DataRequest, FileReads, Severity, build};
use brrrty_js::Value;

const NOON_MS: i64 = 1_699_963_200_000;

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Run `script` as `src/_data/x.rhai` in `root`: its value, or the problem's
/// message. Also gives what it read.
fn run_in(root: &Path, script: &str) -> (Result<Value, String>, Vec<PathBuf>) {
    write(root, "src/_data/x.rhai", script);
    let reads = FileReads::default();
    let runner = data_runner(Clock::Fixed(NOON_MS), root.to_path_buf(), reads.clone());
    let file = root.join("src/_data/x.rhai");
    let got = (runner.0)(&DataRequest {
        file: &file,
        shown: Path::new("src/_data/x.rhai"),
        global: &Data::new(),
    })
    .map_err(|p| p.message);
    (got, reads.paths())
}

fn run(root: &Path, script: &str) -> Value {
    match run_in(root, script).0 {
        Ok(v) => v,
        Err(e) => panic!("script failed: {e}"),
    }
}

fn refused(root: &Path, script: &str) -> String {
    match run_in(root, script).0 {
        Ok(v) => panic!("expected a refusal, got {v:?}"),
        Err(e) => e,
    }
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn strings(v: &Value) -> Vec<String> {
    let Value::Array(a) = v else {
        panic!("not an array: {v:?}")
    };
    a.iter()
        .map(|x| match x {
            Value::String(s) => s.to_string(),
            other => panic!("not a string: {other:?}"),
        })
        .collect()
}

#[test]
fn read_text_is_read_file_sync_utf8() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "data/a.txt", "héllo\r\nthere\n");
    assert_eq!(
        run(t.path(), r#"read_text("data/a.txt")"#),
        s("héllo\r\nthere\n")
    );
    assert_eq!(
        run(t.path(), r#"read_text("./data/a.txt")"#),
        s("héllo\r\nthere\n")
    );
    // Node: `Buffer.from([0xef,0xbb,0xbf,0x61,0xff,0x62,0xe2,0x82]).toString()`
    // is "﻿a�b�": the BOM stays and each bad sequence is one
    // replacement character.
    write(t.path(), "data/b.txt", b"\xef\xbb\xbfa\xffb\xe2\x82");
    assert_eq!(
        run(t.path(), r#"read_text("data/b.txt")"#),
        s("\u{feff}a\u{fffd}b\u{fffd}")
    );
}

#[test]
fn read_json_is_json_parse() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/_data/info.json",
        r#"{"b": 1, "2": [true, null], "a": {"z": "x", "1": 2.5}, "b": 3}"#,
    );
    let v = run(t.path(), r#"read_json("src/_data/info.json")"#);
    // `JSON.stringify(JSON.parse(...))`: integer keys first, a repeated key
    // keeps its place and takes the last value.
    assert_eq!(
        brrrty_liquid::json_stringify(&v).unwrap(),
        r#"{"2":[true,null],"b":3,"a":{"1":2.5,"z":"x"}}"#
    );
    write(t.path(), "bad.json", "{oops}");
    let e = refused(t.path(), r#"read_json("bad.json")"#);
    assert!(
        e.starts_with("read_json(\"bad.json\"): not valid JSON"),
        "{e}"
    );
}

/// The names Node 26 gave for this folder on macOS: libuv sorts each listing
/// with `strcmp`, and `recursive` lists breadth first.
#[test]
fn read_dir_lists_in_nodes_order() {
    let t = tempfile::tempdir().unwrap();
    for f in [
        "b.md",
        "a.md",
        "_index.md",
        "10.md",
        "2.md",
        "é.md",
        "e.md",
        "Z.md",
        "a b.md",
        "a-b.md",
        "a_b.md",
        ".hidden",
        "ä.md",
        "z~",
    ] {
        write(t.path(), &format!("d/{f}"), "");
    }
    for f in ["sub/x.md", "sub/A.md", "sub/deep/y.md", "Zeta/q.md"] {
        write(t.path(), &format!("d/{f}"), "");
    }
    fs::create_dir_all(t.path().join("d/a.dir")).unwrap();
    let flat = [
        ".hidden",
        "10.md",
        "2.md",
        "Z.md",
        "Zeta",
        "_index.md",
        "a b.md",
        "a-b.md",
        "a.dir",
        "a.md",
        "a_b.md",
        "b.md",
        "e.md",
        "sub",
        "z~",
        "ä.md",
        "é.md",
    ];
    assert_eq!(strings(&run(t.path(), r#"read_dir("d")"#)), flat);
    let mut deep: Vec<&str> = flat.to_vec();
    deep.extend([
        "Zeta/q.md",
        "sub/A.md",
        "sub/deep",
        "sub/x.md",
        "sub/deep/y.md",
    ]);
    assert_eq!(
        strings(&run(t.path(), r#"read_dir("d", #{ recursive: true })"#)),
        deep
    );
    let e = refused(t.path(), r#"read_dir("d", #{ withFileTypes: true })"#);
    assert!(e.contains("takes only `recursive`"), "{e}");
    let e = refused(t.path(), r#"read_dir("d/a.md")"#);
    assert_eq!(e, "read_dir(\"d/a.md\"): not a directory");
}

/// Node 26.2.0 on macOS, `fs.readdirSync("d", { recursive: true })` with
/// `link -> real` and `dangle -> nothere`: Node stats each name, following
/// symlinks, and enters a symlinked folder; a dangling one is only a name.
#[cfg(unix)]
#[test]
fn read_dir_enters_a_symlinked_folder_as_node_26_does() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "d/a.txt", "");
    write(t.path(), "d/real/x.md", "");
    write(t.path(), "d/real/sub/y.md", "");
    std::os::unix::fs::symlink("real", t.path().join("d/link")).unwrap();
    std::os::unix::fs::symlink("nothere", t.path().join("d/dangle")).unwrap();
    // `self -> self` can't be followed (ELOOP): Node lists it as a name.
    std::os::unix::fs::symlink("self", t.path().join("d/self")).unwrap();
    let (got, reads) = run_in(t.path(), r#"read_dir("d", #{ recursive: true })"#);
    assert_eq!(
        strings(&got.unwrap()),
        [
            "a.txt",
            "dangle",
            "link",
            "real",
            "self",
            "link/sub",
            "link/x.md",
            "real/sub",
            "real/x.md",
            "link/sub/y.md",
            "real/sub/y.md"
        ]
    );
    // Every folder entered is a dependency, through the symlink too.
    for p in ["d", "d/link", "d/link/sub", "d/real", "d/real/sub"] {
        assert!(reads.contains(&PathBuf::from(p)), "{p} in {reads:?}");
    }
}

/// Node enters a symlink that leads out of the project (and lists what is
/// there): Brrrty can't, so it refuses rather than give fewer names.
#[cfg(unix)]
#[test]
fn read_dir_refuses_a_symlinked_folder_outside_the_project() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "site/d/a.txt", "");
    write(t.path(), "out/o.md", "");
    let root = t.path().join("site");
    std::os::unix::fs::symlink("../../out", root.join("d/away")).unwrap();
    assert_eq!(strings(&run(&root, r#"read_dir("d")"#)), ["a.txt", "away"]);
    let e = refused(&root, r#"read_dir("d", #{ recursive: true })"#);
    assert!(
        e.contains("`away` is a symlink that leads outside the project folder"),
        "{e}"
    );
}

/// `x/up -> ..`: Node follows it round and round until the path is too long
/// (66 names on macOS). Brrrty refuses the loop.
#[cfg(unix)]
#[test]
fn read_dir_refuses_a_symlink_loop() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("l/x")).unwrap();
    std::os::unix::fs::symlink("..", t.path().join("l/x/up")).unwrap();
    let e = refused(t.path(), r#"read_dir("l", #{ recursive: true })"#);
    assert!(
        e.contains("`x/up` leads back into a folder this listing is inside"),
        "{e}"
    );
}

#[test]
fn file_exists_and_missing_files() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "data/a.txt", "x");
    assert_eq!(
        run(t.path(), r#"file_exists("data/a.txt")"#),
        Value::Bool(true)
    );
    assert_eq!(
        run(t.path(), r#"file_exists("data/no.txt")"#),
        Value::Bool(false)
    );
    assert_eq!(run(t.path(), r#"file_exists("data")"#), Value::Bool(true));
    let e = refused(t.path(), r#"read_text("data/no.txt")"#);
    assert_eq!(e, "read_text(\"data/no.txt\"): no such file or directory");
    let e = refused(t.path(), r#"read_text("data")"#);
    assert_eq!(e, "read_text(\"data\"): illegal operation on a directory");
    // A JavaScript `try { ... } catch { }` around a read ports as is.
    assert_eq!(
        run(
            t.path(),
            r#"let x = "none"; try { x = read_text("data/no.txt"); } catch { } x"#
        ),
        s("none")
    );
}

#[test]
fn paths_stay_inside_the_project() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "site/data/a.txt", "x");
    write(t.path(), "secret.txt", "s");
    let root = t.path().join("site");
    for script in [
        r#"read_text("../secret.txt")"#,
        r#"read_text("data/../../secret.txt")"#,
        r#"file_exists("../secret.txt")"#,
        r#"read_dir("..")"#,
    ] {
        let e = refused(&root, script);
        assert!(e.contains("`..` isn't allowed"), "{script}: {e}");
    }
    let abs = t.path().join("secret.txt");
    let e = refused(
        &root,
        &format!("read_text({:?})", abs.display().to_string()),
    );
    assert!(e.contains("an absolute path isn't allowed"), "{e}");
    let e = refused(&root, r#"read_text("")"#);
    assert!(e.contains("give a path from the project folder"), "{e}");
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_project_is_refused() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "site/data/a.txt", "x");
    write(t.path(), "outside/secret.txt", "s");
    let root = t.path().join("site");
    std::os::unix::fs::symlink(t.path().join("outside/secret.txt"), root.join("file")).unwrap();
    std::os::unix::fs::symlink(t.path().join("outside"), root.join("dir")).unwrap();
    std::os::unix::fs::symlink("data/a.txt", root.join("inside")).unwrap();
    for script in [
        r#"read_text("file")"#,
        r#"read_text("dir/secret.txt")"#,
        r#"read_dir("dir")"#,
        r#"file_exists("dir/secret.txt")"#,
        // Missing, but under a folder outside: still not probed.
        r#"file_exists("dir/missing.txt")"#,
        r#"read_text("dir/missing.txt")"#,
    ] {
        let e = refused(&root, script);
        assert!(
            e.contains("a symlink leads outside the project"),
            "{script}: {e}"
        );
    }
    // A symlink that stays inside is followed.
    assert_eq!(run(&root, r#"read_text("inside")"#), s("x"));
}

#[test]
fn a_file_over_the_cap_is_refused_unread() {
    let t = tempfile::tempdir().unwrap();
    let f = fs::File::create(t.path().join("big.csv")).unwrap();
    f.set_len(16 * 1024 * 1024 + 1).unwrap();
    let e = refused(t.path(), r#"read_text("big.csv")"#);
    assert_eq!(
        e,
        "read_text(\"big.csv\"): the file is 16777217 bytes; a script reads files up to 16 MiB"
    );
}

#[test]
fn parse_csv_gives_objects_in_javascripts_key_order() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "data/list.csv",
        "name,2024,size\n\n  Alpha , 1 ,\"big, really\"\nBeta,2,\"say \"\"hi\"\"\"\n",
    );
    let v = run(
        t.path(),
        r#"parse_csv(read_text("data/list.csv"), #{ columns: true, skip_empty_lines: true, trim: true })"#,
    );
    // Node: `JSON.stringify(parse(csv, { columns: true, skip_empty_lines: true, trim: true }))`.
    assert_eq!(
        brrrty_liquid::json_stringify(&v).unwrap(),
        r#"[{"2024":"1","name":"Alpha","size":"big, really"},{"2024":"2","name":"Beta","size":"say \"hi\""}]"#
    );
    let v = run(t.path(), r#"parse_csv("a,b\n1,2")"#);
    assert_eq!(
        brrrty_liquid::json_stringify(&v).unwrap(),
        r#"[["a","b"],["1","2"]]"#
    );
    let e = refused(t.path(), r#"parse_csv("a,b\n1", #{ columns: true })"#);
    assert_eq!(
        e,
        "parse_csv: Invalid Record Length: columns length is 2, got 1 on line 2"
    );
    let e = refused(t.path(), r#"parse_csv("a", #{ cast: true })"#);
    assert!(
        e.contains("doesn't support csv-parse's `cast` option"),
        "{e}"
    );
}

#[test]
fn every_path_read_is_recorded_found_or_not() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "data/a.txt", "x");
    write(t.path(), "data/d/b.md", "");
    let (got, reads) = run_in(
        t.path(),
        r#"
        let a = read_text("./data/a.txt");
        let names = read_dir("data/d");
        let there = file_exists("data/missing.txt");
        try { read_text("data/gone.txt"); } catch { }
        a
        "#,
    );
    assert_eq!(got.unwrap(), s("x"));
    assert_eq!(
        reads,
        ["data/a.txt", "data/d", "data/gone.txt", "data/missing.txt"].map(PathBuf::from)
    );
}

#[test]
fn the_config_reads_files_and_the_build_lists_them() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "data/names.csv", "name\nAda\nGrace\n");
    write(
        t.path(),
        CONFIG_FILE,
        r#"config.add_global_data("names", parse_csv(read_text("data/names.csv"), #{ columns: true }));
        #{ dir: #{ input: "src" } }"#,
    );
    write(
        t.path(),
        "src/_data/extra.rhai",
        r#"read_text("data/extra.txt")"#,
    );
    write(t.path(), "data/extra.txt", "more");
    write(
        t.path(),
        "src/index.liquid",
        "{% for n in names %}{{ n.name }} {% endfor %}{{ extra }}",
    );
    let mut cfg = load_with(t.path(), Clock::Fixed(NOON_MS))
        .unwrap()
        .unwrap()
        .build_options();
    cfg.data = Some(data_runner(
        Clock::Fixed(NOON_MS),
        t.path().to_path_buf(),
        cfg.reads.clone(),
    ));
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg,
        ..Default::default()
    };
    let r = build(&opts, &brrrty_markdown::MarkdownIt::new());
    assert!(
        !r.problems.iter().any(|p| p.severity == Severity::Error),
        "{:#?}",
        r.problems
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "Ada Grace more"
    );
    assert_eq!(
        r.reads,
        ["data/extra.txt", "data/names.csv"].map(PathBuf::from)
    );
}

/// Node: `existsSync("a.txt/")` and `existsSync("a.txt/.")` are false when
/// `a.txt` is a file, and `readFileSync("a.txt/")` throws ENOTDIR.
#[test]
fn a_trailing_slash_needs_a_folder() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "data/a.txt", "x");
    for (script, want) in [
        (r#"file_exists("data/a.txt/")"#, false),
        (r#"file_exists("data/a.txt/.")"#, false),
        (r#"file_exists("data/")"#, true),
        (r#"file_exists("data/.")"#, true),
        (r#"file_exists("data/a.txt")"#, true),
    ] {
        assert_eq!(run(t.path(), script), Value::Bool(want), "{script}");
    }
    let e = refused(t.path(), r#"read_text("data/a.txt/")"#);
    assert_eq!(e, "read_text(\"data/a.txt/\"): not a directory");
}

/// A FIFO with no writer: opening it to read would block forever. It is
/// refused at once.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_not_waited_on() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("data")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(t.path().join("data/fifo"))
        .status()
        .unwrap();
    assert!(made.success());
    let started = std::time::Instant::now();
    let e = refused(t.path(), r#"read_text("data/fifo")"#);
    assert!(e.contains("not a regular file"), "{e}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

/// The reviewer's repro: a symlink to a folder whose real path is longer
/// than PATH_MAX. `realpath` fails with ENAMETOOLONG, and the old check took
/// any failure as "missing" and let the kernel follow the link.
#[cfg(unix)]
#[test]
fn a_path_too_long_to_canonicalize_is_still_confined() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("site");
    fs::create_dir_all(root.join("data")).unwrap();
    let seg = "A".repeat(200);
    let deep = |base: &Path| base.join(&seg).join(&seg).join(&seg);
    let out = t.path().join("out");
    let first = deep(&out);
    fs::create_dir_all(&first).unwrap();
    // The second level is past PATH_MAX as one path, so make it from a
    // handle on the first.
    let rel = format!("{seg}/{seg}/{seg}");
    let first_dir =
        cap_std::fs::Dir::open_ambient_dir(&first, cap_std::ambient_authority()).unwrap();
    first_dir.create_dir_all(&rel).unwrap();
    first_dir
        .write(format!("{rel}/secret.txt"), "TOPSECRET\n")
        .unwrap();
    std::os::unix::fs::symlink(&rel, out.join("s1")).unwrap();
    std::os::unix::fs::symlink(&rel, first.join("s2")).unwrap();
    std::os::unix::fs::symlink(out.join("s1/s2"), root.join("data/x")).unwrap();
    for script in [
        r#"read_text("data/x/secret.txt")"#,
        r#"read_dir("data/x")"#,
        r#"file_exists("data/x/secret.txt")"#,
    ] {
        let e = refused(&root, script);
        assert!(
            e.contains("leads outside the project folder"),
            "{script}: {e}"
        );
    }
}

/// The reviewer's race: one thread flips `data/f` between a file and a
/// symlink to a file outside while reads run. The old check-then-open read
/// the outside file 17 times in 4,000; opening through the project's handle
/// never does.
#[cfg(unix)]
#[test]
fn a_symlink_swapped_in_mid_read_never_reads_outside() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("site");
    write(&root, "data/inside.txt", "inside");
    write(t.path(), "secret.txt", "TOPSECRET");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flipper = {
        let stop = stop.clone();
        let f = root.join("data/f");
        let tmp = root.join("data/f.tmp");
        let outside = t.path().join("secret.txt");
        std::thread::spawn(move || {
            let mut link = false;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = fs::remove_file(&tmp);
                if link {
                    let _ = std::os::unix::fs::symlink(&outside, &tmp);
                } else {
                    let _ = fs::write(&tmp, "inside");
                }
                let _ = fs::rename(&tmp, &f);
                link = !link;
            }
        })
    };
    let script = r#"
        let leaked = 0;
        for i in 0..2000 {
            try {
                if read_text("data/f") == "TOPSECRET" { leaked += 1; }
            } catch { }
        }
        leaked
    "#;
    let got = run(&root, script);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    flipper.join().unwrap();
    assert_eq!(got, Value::Number(0.0));
}

#[test]
fn limits_name_the_call_and_the_file() {
    let t = tempfile::tempdir().unwrap();
    // 6 MiB of 0xFF is 18 MiB of U+FFFD.
    write(t.path(), "data/bad.bin", vec![0xffu8; 6 * 1024 * 1024]);
    let e = refused(t.path(), r#"read_text("data/bad.bin")"#);
    assert!(
        e.starts_with("read_text(\"data/bad.bin\"): the text is 18874368 bytes once its invalid UTF-8 is replaced"),
        "{e}"
    );
    let big = format!("[{}0]", "0,".repeat(100_000));
    write(t.path(), "data/big.json", big);
    let e = refused(t.path(), r#"read_json("data/big.json")"#);
    assert!(
        e.starts_with("read_json(\"data/big.json\"): its arrays hold 100001 items"),
        "{e}"
    );
    write(t.path(), "data/inf.json", "1e400");
    let e = refused(t.path(), r#"read_json("data/inf.json")"#);
    assert!(e.contains("JSON.parse would give Infinity"), "{e}");
}

#[test]
fn option_values_are_checked() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "d/a.txt", "");
    // Node: ERR_INVALID_ARG_TYPE.
    let e = refused(t.path(), r#"read_dir("d", #{ recursive: "false" })"#);
    assert!(e.contains("`recursive` must be true or false"), "{e}");
    // csv-parse takes the spelling written last; a Rhai map has no order.
    let e = refused(
        t.path(),
        r#"parse_csv("a\n\nb", #{ skip_empty_lines: true, skipEmptyLines: false })"#,
    );
    assert!(e.contains("are the same csv-parse option"), "{e}");
}
