//! Reading files from a script: what a JavaScript data file does with `fs`
//! and csv-parse at build time.
//!
//! - `read_text(path)` is `fs.readFileSync(path, 'utf8')`: bytes that aren't
//!   UTF-8 become U+FFFD as Node replaces them, and a byte order mark is kept.
//! - `read_json(path)` is `JSON.parse(fs.readFileSync(path, 'utf8'))`.
//! - `read_dir(path)` is `fs.readdirSync(path)`: names sorted by their bytes,
//!   as libuv sorts them on macOS and Linux, without `.` and `..`.
//!   `read_dir(path, #{ recursive: true })` is `{ recursive: true }` as Node
//!   26 lists it: the folder's names, then each subfolder's (breadth first,
//!   in that order) as `sub/name`. A symlink to a folder is entered, as Node
//!   enters it; one that leads outside the project, or back into a folder
//!   being listed (a loop, which Node follows until the path is too long),
//!   is refused.
//! - `file_exists(path)` is `fs.existsSync(path)`.
//! - `parse_csv(text)` and `parse_csv(text, options)` are csv-parse's `parse`
//!   (see the `csv` module).
//!
//! Every path is opened relative to a handle on the project folder, one
//! component at a time, with `cap-std`: a symlink is followed only as an
//! explicit step that must stay inside the project, so no path, and no
//! symlink swapped in while the script runs, reaches a file outside it. On
//! Linux that is `openat2` with `RESOLVE_BENEATH`; elsewhere `openat` with
//! `O_NOFOLLOW`. Paths are relative to the project folder (11ty's working
//! directory); an absolute path and a `..` are refused before anything is
//! opened. A file is opened with `O_NONBLOCK` and checked through its handle:
//! anything but a regular file (a FIFO, a socket, a device) is refused, never
//! waited on. A file over [`MAX_FILE_SIZE`] is refused before it is read.
//! Every path a script reads, lists or checks, found or not, and every
//! folder a recursive listing enters, goes in the build's [`FileReads`], so
//! `brrrty serve` rebuilds when it changes.

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

const OUTSIDE: &str =
    "a symlink leads outside the project folder, and a script reads only inside it";

/// cap-std's error for a path that would leave the folder it was opened in.
fn is_escape(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
        && e.to_string() == "a path led outside of the filesystem"
}

/// `ELOOP`: a symlink that leads to itself, or too many in a row.
fn is_loop(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// How Node words a failed call, without its `ENOENT:` prefix.
fn io_why(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    if is_escape(e) {
        return OUTSIDE.into();
    }
    if is_loop(e) {
        return "too many symbolic links encountered".into();
    }
    match e.kind() {
        K::NotFound => "no such file or directory".into(),
        K::PermissionDenied => "permission denied".into(),
        K::IsADirectory => "illegal operation on a directory".into(),
        K::NotADirectory => "not a directory".into(),
        _ => e.to_string(),
    }
}

/// A path as a script gave it, checked and split.
struct Given {
    /// Relative to the project folder; empty for the folder itself.
    rel: PathBuf,
    /// It ends in `/` or `/.`, so Node requires a folder there.
    wants_dir: bool,
}

impl Given {
    /// The path for cap-std, which takes `.` for the folder itself.
    fn at(&self) -> &Path {
        if self.rel.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &self.rel
        }
    }
}

impl Files {
    /// Check `given` and note it as read. Nothing is opened.
    fn given(&self, what: &str, given: &str) -> Res<Given> {
        #[cfg(not(unix))]
        {
            return Err(fail(
                what,
                given,
                "reading files is only supported on macOS and Linux so far: Node's order and devices on Windows haven't been checked",
            ));
        }
        if given.is_empty() {
            return Err(fail(
                what,
                given,
                "give a path from the project folder, like \"src/_data/list.csv\"",
            ));
        }
        let mut rel = PathBuf::new();
        for c in Path::new(given).components() {
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
        let wants_dir = given.ends_with('/') || given.ends_with("/.");
        self.reads.add(rel.clone());
        Ok(Given { rel, wants_dir })
    }

    /// A handle on the project folder: every open goes through it.
    fn root(&self, what: &str, given: &str) -> Res<cap_std::fs::Dir> {
        cap_std::fs::Dir::open_ambient_dir(&self.root, cap_std::ambient_authority())
            .map_err(|e| fail(what, given, format!("cannot open the project folder: {e}")))
    }

    fn read_bytes(&self, what: &str, given: &str) -> Res<Vec<u8>> {
        let g = self.given(what, given)?;
        let root = self.root(what, given)?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt as _;
            // A FIFO opened for reading without a writer would block here.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = root
            .open_with(g.at(), &options)
            .map_err(|e| fail(what, given, io_why(&e)))?;
        let meta = file.metadata().map_err(|e| fail(what, given, io_why(&e)))?;
        if meta.is_dir() {
            return Err(fail(what, given, "illegal operation on a directory"));
        }
        if g.wants_dir {
            return Err(fail(what, given, "not a directory"));
        }
        if !meta.is_file() {
            return Err(fail(
                what,
                given,
                "not a regular file (a FIFO, socket or device); a script reads only files",
            ));
        }
        let cap = || format!("{} MiB", MAX_FILE_SIZE / 1024 / 1024);
        if meta.len() > MAX_FILE_SIZE as u64 {
            return Err(fail(
                what,
                given,
                format!(
                    "the file is {} bytes; a script reads files up to {}",
                    meta.len(),
                    cap()
                ),
            ));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_FILE_SIZE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| fail(what, given, io_why(&e)))?;
        if bytes.len() > MAX_FILE_SIZE {
            return Err(fail(
                what,
                given,
                format!("the file grew past {} while it was read", cap()),
            ));
        }
        Ok(bytes)
    }

    fn read_text(&self, given: &str) -> Res<String> {
        let bytes = self.read_bytes("read_text", given)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        // Each bad byte can become a 3-byte U+FFFD.
        if text.len() > MAX_STRING_SIZE {
            return Err(fail(
                "read_text",
                given,
                format!(
                    "the text is {} bytes once its invalid UTF-8 is replaced; a script holds strings up to {} MiB",
                    text.len(),
                    MAX_STRING_SIZE / 1024 / 1024
                ),
            ));
        }
        Ok(text)
    }

    fn read_json(&self, given: &str) -> Res<Dynamic> {
        let bytes = self.read_bytes("read_json", given)?;
        let text = String::from_utf8_lossy(&bytes);
        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| fail("read_json", given, json_why(&e)))?;
        let items = array_items(&json);
        if items > MAX_ARRAY_SIZE {
            return Err(fail(
                "read_json",
                given,
                format!(
                    "its arrays hold {items} items in all; a script holds up to {MAX_ARRAY_SIZE}"
                ),
            ));
        }
        Ok(to_dynamic(&Value::from_json(&json)))
    }

    fn read_dir(&self, given: &str, recursive: bool) -> Res<rhai::Array> {
        const WHAT: &str = "read_dir";
        let g = self.given(WHAT, given)?;
        let root = self.root(WHAT, given)?;
        let start = root
            .metadata(g.at())
            .map_err(|e| fail(WHAT, given, io_why(&e)))?;
        if !start.is_dir() {
            return Err(fail(WHAT, given, "not a directory"));
        }
        let mut out: Vec<String> = Vec::new();
        // Each folder still to list: its prefix in the output, its path, and
        // the folders above it in this listing, to tell a loop.
        let mut queue: std::collections::VecDeque<(String, PathBuf, Vec<(u64, u64)>)> =
            std::collections::VecDeque::from([(String::new(), g.rel.clone(), vec![id(&start)])]);
        while let Some((prefix, rel, above)) = queue.pop_front() {
            let at = if rel.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                rel.clone()
            };
            let dir = root
                .open_dir(&at)
                .map_err(|e| fail(WHAT, given, io_why(&e)))?;
            let mut entries: Vec<(String, bool)> = Vec::new();
            for entry in dir.entries().map_err(|e| fail(WHAT, given, io_why(&e)))? {
                let entry = entry.map_err(|e| fail(WHAT, given, io_why(&e)))?;
                if out.len() + entries.len() >= MAX_DIR_ENTRIES {
                    return Err(fail(
                        WHAT,
                        given,
                        format!("Brrrty lists up to {MAX_DIR_ENTRIES} names, and this has more"),
                    ));
                }
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
                entries.push((name, false));
            }
            // libuv's `scandir` sorts with `strcmp`.
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            for (name, _) in entries {
                let shown = format!("{prefix}{name}");
                out.push(shown.clone());
                if !recursive {
                    continue;
                }
                // Node stats each name, following symlinks; one that can't be
                // followed (dangling, or a symlink loop) is just a name.
                let child = rel.join(&name);
                let meta = match root.metadata(&child) {
                    Ok(m) => m,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound || is_loop(&e) => continue,
                    Err(e) if is_escape(&e) => {
                        return Err(fail(
                            WHAT,
                            given,
                            format!(
                                "`{shown}` is a symlink that leads outside the project folder. Node would list what is there; a script reads only inside the project"
                            ),
                        ));
                    }
                    Err(e) => return Err(fail(WHAT, given, format!("`{shown}`: {}", io_why(&e)))),
                };
                if !meta.is_dir() {
                    continue;
                }
                let me = id(&meta);
                if above.contains(&me) {
                    return Err(fail(
                        WHAT,
                        given,
                        format!(
                            "`{shown}` leads back into a folder this listing is inside, a loop. Node follows it until the path is too long; Brrrty refuses"
                        ),
                    ));
                }
                self.reads.add(child.clone());
                let mut chain = above.clone();
                chain.push(me);
                queue.push_back((format!("{shown}/"), child, chain));
            }
        }
        Ok(out.into_iter().map(Dynamic::from).collect())
    }

    fn file_exists(&self, given: &str) -> Res<bool> {
        let g = self.given("file_exists", given)?;
        let root = self.root("file_exists", given)?;
        match root.metadata(g.at()) {
            Ok(m) => Ok(!g.wants_dir || m.is_dir()),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::NotADirectory
                    || is_loop(&e) =>
            {
                Ok(false)
            }
            Err(e) => Err(fail("file_exists", given, io_why(&e))),
        }
    }
}

/// A folder's identity, to tell a loop.
fn id(m: &cap_std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as _;
        (m.dev(), m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        (0, 0)
    }
}

/// Every item of every array in `v`, as Rhai would count them.
fn array_items(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Array(a) => a.len() + a.iter().map(array_items).sum::<usize>(),
        serde_json::Value::Object(o) => o.values().map(array_items).sum(),
        _ => 0,
    }
}

/// serde's error, with what it means for a JavaScript port.
fn json_why(e: &serde_json::Error) -> String {
    let m = e.to_string();
    let hint = if m.contains("number out of range") {
        "; JSON.parse would give Infinity, and Brrrty refuses rather than approximate"
    } else if m.contains("recursion limit") {
        "; Brrrty reads JSON nested up to 128 levels"
    } else if m.contains("surrogate") || m.contains("hex escape") {
        "; a \\u escape that is half of a surrogate pair is valid for JSON.parse, but Brrrty can't hold it in a string"
    } else {
        ""
    };
    format!("not valid JSON: {m}{hint}")
}

/// `read_dir`'s options: only `recursive`, a boolean as Node requires.
fn recursive_option(given: &str, options: &Map) -> Res<bool> {
    let mut recursive = false;
    for (k, v) in options {
        match k.as_str() {
            "recursive" => {
                recursive = match to_value(v).map_err(|why| fail("read_dir", given, why))? {
                    Value::Bool(b) => b,
                    Value::Undefined => false,
                    other => {
                        return Err(fail(
                            "read_dir",
                            given,
                            format!(
                                "`recursive` must be true or false, as Node requires (ERR_INVALID_ARG_TYPE), not {}",
                                brrrty_liquid::json_stringify(&other)
                                    .unwrap_or_else(|| "that".into())
                            ),
                        ));
                    }
                };
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
