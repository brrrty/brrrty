//! Reading files from a script: what a JavaScript data file does with `fs`
//! and csv-parse at build time.
//!
//! - `read_text(path)` is `fs.readFileSync(path, 'utf8')`: bytes that aren't
//!   UTF-8 become U+FFFD as Node replaces them, and a byte order mark is kept.
//! - `read_json(path)` is `JSON.parse(fs.readFileSync(path, 'utf8'))`.
//! - `read_dir(path)` is `fs.readdirSync(path)`: names sorted by their bytes,
//!   as libuv sorts them on macOS and Linux, without `.` and `..`.
//!   `read_dir(path, #{ recursive: true })` is `{ recursive: true }`: the
//!   folder's names, then each subfolder's (breadth first, in that order) as
//!   `sub/name`; a symlink to a folder is listed but not entered.
//! - `file_exists(path)` is `fs.existsSync(path)`.
//! - `parse_csv(text)` and `parse_csv(text, options)` are csv-parse's `parse`
//!   (see the `csv` module).
//!
//! Paths are relative to the project folder (11ty's working directory), and
//! can't leave it: an absolute path, a `..`, and a symlink that leads
//! outside are refused. A file over [`MAX_FILE_SIZE`] is refused before it is
//! read. Every path a script reads, lists or checks, found or not, goes in the
//! build's [`FileReads`], so `brrrty serve` rebuilds when it changes.

use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use brrrty_core::FileReads;
use brrrty_js::Value;
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};

use crate::api::{MAX_ARRAY_SIZE, MAX_STRING_SIZE, invalid};
use crate::convert::{to_dynamic, to_value};

type Res<T> = Result<T, Box<EvalAltResult>>;

/// Largest file a script may read: the largest string a script may hold.
pub const MAX_FILE_SIZE: usize = MAX_STRING_SIZE;
/// Most names one `read_dir` may give.
pub const MAX_DIR_ENTRIES: usize = MAX_ARRAY_SIZE;

/// The project folder, and the log the reads go in.
struct Files {
    root: PathBuf,
    reads: FileReads,
}

fn fail(what: &str, path: &str, why: impl std::fmt::Display) -> Box<EvalAltResult> {
    invalid(format!("{what}(\"{path}\"): {why}"), Position::NONE)
}

/// How Node words a failed call, without its `ENOENT:` prefix.
fn io_why(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "no such file or directory".into(),
        K::PermissionDenied => "permission denied".into(),
        K::IsADirectory => "illegal operation on a directory".into(),
        K::NotADirectory => "not a directory".into(),
        _ => e.to_string(),
    }
}

impl Files {
    /// `given` as a path inside the project: the path to open, after
    /// following symlinks where it exists. Notes the path as read.
    fn resolve(&self, what: &str, given: &str) -> Res<PathBuf> {
        if given.is_empty() {
            return Err(fail(
                what,
                given,
                "give a path from the project folder, like \"src/_data/list.csv\"",
            ));
        }
        let path = Path::new(given);
        let mut rel = PathBuf::new();
        for c in path.components() {
            match c {
                Component::Normal(part) => rel.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(fail(
                        what,
                        given,
                        "`..` isn't allowed: a script reads only inside the project folder. Give the path from the project folder (where brrrty.config.rhai is)",
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(fail(
                        what,
                        given,
                        "an absolute path isn't allowed: give the path from the project folder (where brrrty.config.rhai is), like \"src/_data/list.csv\"",
                    ));
                }
            }
        }
        self.reads.add(rel.clone());
        let full = self.root.join(&rel);
        let real_root = self
            .root
            .canonicalize()
            .map_err(|e| fail(what, given, format!("cannot find the project folder: {e}")))?;
        // The path, or the nearest folder above it that exists, symlinks
        // followed, has to be inside the project.
        let mut probe = full.as_path();
        let real = loop {
            match probe.canonicalize() {
                Ok(real) => break real,
                Err(_) => match probe.parent() {
                    Some(up) if probe != self.root => probe = up,
                    _ => break real_root.clone(),
                },
            }
        };
        if !real.starts_with(&real_root) {
            return Err(fail(
                what,
                given,
                "a symlink leads outside the project folder, and a script reads only inside it",
            ));
        }
        Ok(if probe == full { real } else { full })
    }

    fn read_bytes(&self, what: &str, given: &str) -> Res<Vec<u8>> {
        let path = self.resolve(what, given)?;
        let meta = std::fs::metadata(&path).map_err(|e| fail(what, given, io_why(&e)))?;
        if meta.is_dir() {
            return Err(fail(what, given, "illegal operation on a directory"));
        }
        if meta.len() > MAX_FILE_SIZE as u64 {
            return Err(fail(
                what,
                given,
                format!(
                    "the file is {} bytes; a script reads files up to {} MiB",
                    meta.len(),
                    MAX_FILE_SIZE / 1024 / 1024
                ),
            ));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        std::fs::File::open(&path)
            .and_then(|f| f.take(MAX_FILE_SIZE as u64 + 1).read_to_end(&mut bytes))
            .map_err(|e| fail(what, given, io_why(&e)))?;
        if bytes.len() > MAX_FILE_SIZE {
            return Err(fail(
                what,
                given,
                format!(
                    "the file grew past {} MiB while it was read",
                    MAX_FILE_SIZE / 1024 / 1024
                ),
            ));
        }
        Ok(bytes)
    }

    fn read_text(&self, given: &str) -> Res<String> {
        let bytes = self.read_bytes("read_text", given)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn read_json(&self, given: &str) -> Res<Dynamic> {
        let bytes = self.read_bytes("read_json", given)?;
        let text = String::from_utf8_lossy(&bytes);
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| fail("read_json", given, format!("not valid JSON: {e}")))?;
        Ok(to_dynamic(&Value::from_json(&json)))
    }

    fn read_dir(&self, given: &str, recursive: bool) -> Res<rhai::Array> {
        const WHAT: &str = "read_dir";
        let path = self.resolve(WHAT, given)?;
        let meta = std::fs::metadata(&path).map_err(|e| fail(WHAT, given, io_why(&e)))?;
        if !meta.is_dir() {
            return Err(fail(WHAT, given, "not a directory"));
        }
        let mut out: Vec<String> = Vec::new();
        let mut queue: std::collections::VecDeque<(String, PathBuf)> =
            std::collections::VecDeque::from([(String::new(), path)]);
        while let Some((prefix, dir)) = queue.pop_front() {
            let mut entries: Vec<(String, bool)> = Vec::new();
            for entry in std::fs::read_dir(&dir).map_err(|e| fail(WHAT, given, io_why(&e)))? {
                let entry = entry.map_err(|e| fail(WHAT, given, io_why(&e)))?;
                let name = entry.file_name().into_string().map_err(|n| {
                    fail(
                        WHAT,
                        given,
                        format!(
                            "the name {} isn't UTF-8, and Brrrty can't give it as JavaScript would",
                            n.to_string_lossy()
                        ),
                    )
                })?;
                // `Dirent.isDirectory()`: a symlink is not entered.
                let is_dir = recursive
                    && entry
                        .file_type()
                        .map_err(|e| fail(WHAT, given, io_why(&e)))?
                        .is_dir();
                entries.push((name, is_dir));
            }
            // libuv's `scandir` sorts with `strcmp`.
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            for (name, is_dir) in entries {
                let shown = format!("{prefix}{name}");
                if is_dir {
                    queue.push_back((format!("{shown}/"), dir.join(&name)));
                }
                out.push(shown);
                if out.len() > MAX_DIR_ENTRIES {
                    return Err(fail(
                        WHAT,
                        given,
                        format!("Brrrty lists up to {MAX_DIR_ENTRIES} names, and this has more"),
                    ));
                }
            }
        }
        Ok(out.into_iter().map(Dynamic::from).collect())
    }

    fn file_exists(&self, given: &str) -> Res<bool> {
        let path = self.resolve("file_exists", given)?;
        Ok(path.exists())
    }
}

/// `read_dir`'s options: only `recursive`.
fn recursive_option(given: &str, options: &Map) -> Res<bool> {
    let mut recursive = false;
    for (k, v) in options {
        match k.as_str() {
            "recursive" => {
                recursive = to_value(v)
                    .map_err(|why| fail("read_dir", given, why))?
                    .js_truthy();
            }
            other => {
                return Err(fail(
                    "read_dir",
                    given,
                    format!(
                        "Brrrty's read_dir takes only `recursive`, not `{other}`; it gives names, as readdirSync does without withFileTypes"
                    ),
                ));
            }
        }
    }
    Ok(recursive)
}

/// `parse_csv`'s options, from a map or an `object()`.
fn csv_options(options: &Dynamic) -> Res<crate::csv::CsvOptions> {
    let v =
        to_value(options).map_err(|why| invalid(format!("parse_csv: {why}"), Position::NONE))?;
    let pairs: Vec<(String, Value)> = match v {
        Value::Undefined | Value::Null => Vec::new(),
        Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => {
            return Err(invalid(
                "parse_csv: the options are a map, like #{ columns: true }".into(),
                Position::NONE,
            ));
        }
    };
    crate::csv::options(&pairs).map_err(|why| invalid(format!("parse_csv: {why}"), Position::NONE))
}

fn parse_csv(text: &str, options: &Dynamic) -> Res<rhai::Array> {
    let o = csv_options(options)?;
    let records = crate::csv::parse(text, &o)
        .map_err(|why| invalid(format!("parse_csv: {why}"), Position::NONE))?;
    Ok(records.iter().map(to_dynamic).collect())
}

/// Register the helpers for a project at `root`, noting reads in `reads`.
pub fn register(e: &mut Engine, root: &Path, reads: FileReads) {
    let files = Arc::new(Files {
        root: root.to_path_buf(),
        reads,
    });
    let f = files.clone();
    e.register_fn("read_text", move |p: ImmutableString| f.read_text(&p));
    let f = files.clone();
    e.register_fn("read_json", move |p: ImmutableString| f.read_json(&p));
    let f = files.clone();
    e.register_fn("read_dir", move |p: ImmutableString| f.read_dir(&p, false));
    let f = files.clone();
    e.register_fn("read_dir", move |p: ImmutableString, o: Map| {
        let recursive = recursive_option(&p, &o)?;
        f.read_dir(&p, recursive)
    });
    let f = files;
    e.register_fn("file_exists", move |p: ImmutableString| f.file_exists(&p));
    e.register_fn("parse_csv", |text: ImmutableString| {
        parse_csv(&text, &Dynamic::UNIT)
    });
    e.register_fn("parse_csv", |text: ImmutableString, options: Dynamic| {
        parse_csv(&text, &options)
    });
}
