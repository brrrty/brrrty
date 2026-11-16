//! Live reload for `brrrty serve`: the injected script, the reload channel, and a real
//! (headless) Chrome following edits.
//!
//! Behavior pinned against `@11ty/eleventy-dev-server` 2.1.2 under eleventy 3.1.8
//! `--serve`, probed with headless Chrome: an edit to the page's own template is patched
//! into the page without a reload, a stylesheet-only edit swaps the stylesheet without a
//! reload, any other edit reloads, and a build error is logged to the console while the
//! page keeps the last good build.

#[path = "support/ws_client.rs"]
mod ws_client;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use brrrty::options::Flags;
use brrrty::serve::live::SCRIPT_TAG;
use brrrty::serve::{LiveReload, Rebuild, Session, Settings, StopHandle};
use brrrty_serve::{RunningServer, Server};
use serde_json::Value;
use ws_client::Client;

const DEADLINE: Duration = Duration::from_secs(30);
const QUIET: Duration = Duration::from_millis(1500);

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

const LAYOUT: &str = "<!doctype html>\n<html><head><title>T</title><link rel=\"stylesheet\" href=\"/css/style.css\"></head>\n<body>{{ content }}</body></html>\n";

fn page(body: &str) -> String {
    format!("---\nlayout: base.liquid\n---\n<p id=\"msg\">{body}</p>\n")
}

fn site() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/base.liquid", LAYOUT);
    write(root, "src/index.md", page("one"));
    write(root, "src/other.md", page("other"));
    write(root, "src/css/style.css", "p { color: rgb(255, 0, 0); }\n");
    write(root, "src/css/notes.txt", "hello </head></body>\n");
    write(root, "src/bare.html", "<p>no head no body</p>\n");
    write(
        root,
        "src/img/dot.png",
        [0x89, b'P', b'N', b'G', 0, 0xff, b'<'],
    );
    t
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + DEADLINE;
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

struct Http {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

impl Http {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }
}

fn get(server: &RunningServer, path: &str, extra: &str) -> Http {
    let mut s = TcpStream::connect(server.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}\r\n"
    )
    .unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let headers = String::from_utf8_lossy(&buf[..split]).into_owned();
    Http {
        status: headers[9..12].parse().unwrap(),
        headers,
        body: buf[split + 4..].to_vec(),
    }
}

struct Live<'a> {
    root: &'a Path,
    server: &'a RunningServer,
    live: LiveReload,
    stop: StopHandle,
    rebuilds: Receiver<Rebuild>,
}

impl Live<'_> {
    /// Wait out the rebuilds from writing the site just before the watcher started. A few at
    /// most: more is a rebuild loop.
    fn settle(&self) {
        let mut n = 0;
        while let Ok(r) = self.rebuilds.recv_timeout(QUIET) {
            n += 1;
            assert!(n < 5, "rebuilding in a loop: {:?}", r.changed_paths);
        }
    }

    fn no_rebuild(&self) {
        if let Ok(r) = self.rebuilds.recv_timeout(QUIET) {
            panic!("unexpected rebuild for {:?}", r.changed_paths);
        }
    }

    fn next_rebuild(&self) -> Rebuild {
        self.rebuilds
            .recv_timeout(DEADLINE)
            .expect("a rebuild within the deadline")
    }

    fn socket(&self) -> Client {
        let addr = self.server.local_addr();
        let origin = format!("http://{addr}");
        Client::connect(addr, "/.brrrty/live", Some(&origin)).unwrap()
    }
}

fn flags() -> Flags {
    Flags {
        input: Some(PathBuf::from("src")),
        passthrough: vec![PathBuf::from("src/css"), PathBuf::from("src/img")],
        ..Flags::default()
    }
}

/// Start a session with live reload on `root`, and run `test` while the watch loop runs.
fn with_live(root: &Path, test: impl FnOnce(&Live)) {
    let settings = Settings {
        flags: flags(),
        quiet: true,
        ..Settings::default()
    };
    let mut session = Session::start(root, settings).unwrap();
    let server = Server::start(session.serving(), "localhost", 0).unwrap();
    let live = session.live_reload(&server);
    let (tx, rx) = mpsc::channel();
    session.on_rebuild(Arc::new(move |r: &Rebuild| {
        let _ = tx.send(r.clone());
    }));
    let stop = session.stop_handle();
    std::thread::scope(|s| {
        let running = s.spawn(|| session.run(&server));
        let harness = Live {
            root,
            server: &server,
            live,
            stop: stop.clone(),
            rebuilds: rx,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&harness)));
        harness.stop.stop();
        running.join().unwrap();
        if let Err(p) = outcome {
            std::panic::resume_unwind(p);
        }
    });
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn the_script_goes_into_html_only_and_other_bytes_are_untouched() {
    let t = site();
    write(
        t.path(),
        "src/404.html",
        "---\npermalink: 404.html\n---\n<html><head></head><body>gone</body></html>",
    );
    with_live(t.path(), |live| {
        let s = live.server;
        let home = get(s, "/", "");
        assert_eq!(
            home.text(),
            format!(
                "<!doctype html>\n<html><head><title>T</title><link rel=\"stylesheet\" href=\"/css/style.css\">{SCRIPT_TAG}</head>\n<body><p id=\"msg\">one</p>\n</body></html>\n"
            )
        );
        assert_eq!(
            home.header("content-length"),
            Some(home.body.len().to_string())
        );
        // No </head>, </title>, </body>, </html> or doctype: at the end, as the dev server does.
        assert_eq!(
            get(s, "/bare/", "").text(),
            format!("<p>no head no body</p>\n{SCRIPT_TAG}")
        );
        // Not for a fetch from a page, only for a navigation.
        let fetched = get(s, "/", "Sec-Fetch-Mode: cors\r\n");
        assert!(!fetched.text().contains(SCRIPT_TAG));
        // Other files are byte for byte what the build wrote, tags or not.
        let root = live.root;
        for (url, file) in [
            ("/css/style.css", "src/css/style.css"),
            ("/css/notes.txt", "src/css/notes.txt"),
            ("/img/dot.png", "src/img/dot.png"),
        ] {
            let r = get(s, url, "");
            assert_eq!(r.status, 200, "{url}");
            assert_eq!(r.body, fs::read(root.join(file)).unwrap(), "{url}");
        }
        // The custom 404 page gets the script and keeps its headers.
        let missing = get(s, "/nope", "");
        assert_eq!(missing.status, 404);
        assert_eq!(
            missing.text(),
            format!("<html><head>{SCRIPT_TAG}</head><body>gone</body></html>")
        );
        assert_eq!(missing.header("content-security-policy"), None);
        // The client and morphdom are served as scripts.
        for js in ["/.brrrty/reload-client.js", "/.brrrty/morphdom.js"] {
            let r = get(s, js, "");
            assert_eq!(r.status, 200, "{js}");
            assert_eq!(
                r.header("content-type").as_deref(),
                Some("application/javascript")
            );
            assert!(r.body.len() > 1000, "{js}");
        }
    });
}

#[test]
fn the_built_in_404_page_lets_the_client_run() {
    let t = site();
    with_live(t.path(), |live| {
        let r = get(live.server, "/nope", "");
        assert_eq!(r.status, 404);
        assert!(r.text().contains(&format!("{SCRIPT_TAG}</head>")));
        // The page's own `default-src 'none'` would block the script, as the dev server notes.
        assert_eq!(
            r.header("content-security-policy").as_deref(),
            Some("script-src 'self'")
        );
    });
}

#[test]
fn pages_are_told_what_each_rebuild_changed() {
    let t = site();
    with_live(t.path(), |live| {
        // Connect once the rebuilds from writing the site are over, so none is heard.
        live.settle();
        let mut ws = live.socket();
        assert_eq!(json(&ws.recv().unwrap())["type"], "connected");
        wait_for("the page to be counted", || live.live.clients() == 1);

        // The page's own template: its new HTML, for patching in place.
        write(live.root, "src/index.md", page("two"));
        let r = live.next_rebuild();
        assert_eq!(r.pages.len(), 1, "{r:?}");
        let m = json(&ws.recv().unwrap());
        assert_eq!(m["type"], "reload");
        assert_eq!(m["css"], false);
        assert_eq!(m["pages"][0]["url"], "/");
        let content = m["pages"][0]["content"].as_str().unwrap();
        assert!(content.contains("<p id=\"msg\">two</p>"), "{content}");
        assert!(!content.contains(SCRIPT_TAG));

        // A stylesheet alone: its URL, for swapping in place.
        write(
            live.root,
            "src/css/style.css",
            "p { color: rgb(0, 0, 255); }\n",
        );
        let m = json(&ws.recv().unwrap());
        assert_eq!(m["css"], true, "{m}");
        assert_eq!(m["stylesheets"], serde_json::json!(["/css/style.css"]));

        // A layout: no page of its own, so pages reload.
        write(
            live.root,
            "src/_includes/base.liquid",
            LAYOUT.replace("<title>T", "<title>T2"),
        );
        let m = json(&ws.recv().unwrap());
        assert_eq!(m["css"], false);
        assert_eq!(m["pages"], serde_json::json!([]));

        // A broken template: the error, which a page connecting later is told too.
        write(live.root, "src/index.md", page("{% if %}"));
        let m = json(&ws.recv().unwrap());
        assert_eq!(m["type"], "error", "{m}");
        let message = m["message"].as_str().unwrap();
        assert!(message.starts_with("src/index.md:"), "{message}");
        let mut late = live.socket();
        assert_eq!(json(&late.recv().unwrap())["type"], "connected");
        assert_eq!(json(&late.recv().unwrap())["message"], message);

        // The fix reloads every page, and clears the error for pages that connect after.
        write(live.root, "src/index.md", page("three"));
        for c in [&mut ws, &mut late] {
            let m = json(&c.recv().unwrap());
            assert_eq!(m["type"], "reload");
            assert_eq!(m["pages"][0]["url"], "/");
        }
        let mut after = live.socket();
        assert_eq!(json(&after.recv().unwrap())["type"], "connected");
        after.set_timeout(Duration::from_millis(500));
        let extra = after.recv();
        assert!(extra.is_err(), "no error after the fix: {extra:?}");
        wait_for("all three pages to be counted", || live.live.clients() == 3);
        drop((ws, late, after));
        wait_for("closed pages to be dropped", || live.live.clients() == 0);
    });
}

#[test]
fn copying_passthrough_files_never_rebuilds_on_its_own() {
    // On macOS, each build's copy of a passthrough file made FSEvents report the source
    // again, so every build started the next one.
    let t = site();
    with_live(t.path(), |live| {
        live.settle();
        write(
            live.root,
            "src/css/style.css",
            "p { color: rgb(0, 0, 255); }\n",
        );
        let r = live.next_rebuild();
        assert_eq!(r.changed_paths, [PathBuf::from("src/css/style.css")]);
        assert!(r.css_only);
        live.no_rebuild();
        // An edit right after is still seen.
        write(live.root, "src/img/dot.png", [1, 2, 3]);
        let r = live.next_rebuild();
        assert_eq!(r.changed_paths, [PathBuf::from("src/img/dot.png")]);
        live.no_rebuild();
    });
}

#[test]
fn a_failed_first_build_is_shown_to_pages_that_connect() {
    let t = site();
    write(t.path(), "src/index.md", page("{% if %}"));
    with_live(t.path(), |live| {
        let mut ws = live.socket();
        assert_eq!(json(&ws.recv().unwrap())["type"], "connected");
        let m = json(&ws.recv().unwrap());
        assert_eq!(m["type"], "error");
        assert!(m["message"].as_str().unwrap().starts_with("src/index.md:"));
    });
}

#[test]
fn a_page_on_another_site_cannot_listen() {
    let t = site();
    with_live(t.path(), |live| {
        let addr = live.server.local_addr();
        let refused = Client::connect(addr, "/.brrrty/live", Some("http://evil.example"));
        assert!(refused.is_err());
        assert_eq!(live.live.clients(), 0);
    });
}

/// Restores a folder's permissions, so the temp folder can be removed even after a panic.
#[cfg(unix)]
struct Writable(PathBuf);

#[cfg(unix)]
impl Drop for Writable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[cfg(unix)]
#[test]
fn output_folder_write_failures_reach_the_browser() {
    use std::os::unix::fs::PermissionsExt;
    let t = site();
    write(t.path(), "src/blog.md", page("blog"));
    with_live(t.path(), |live| {
        live.settle();
        let mut ws = live.socket();
        assert_eq!(json(&ws.recv().unwrap())["type"], "connected");
        let blog = live.root.join("_site/blog");
        let _restore = Writable(blog.clone());
        fs::set_permissions(&blog, fs::Permissions::from_mode(0o555)).unwrap();
        if fs::write(blog.join(".probe"), "").is_ok() {
            eprintln!("skipping: permissions don't stop writes here (running as root?)");
            return;
        }
        write(live.root, "src/blog.md", page("blog two"));
        let r = live.next_rebuild();
        assert!(r.ok);
        assert!(!r.output_problems.is_empty(), "{r:?}");
        // The page reloads, since the build itself is good and served...
        assert_eq!(json(&ws.recv().unwrap())["type"], "reload");
        // ...and is told the output folder is behind, as is any page connecting after.
        let m = json(&ws.recv().unwrap());
        assert_eq!(m["type"], "error");
        let message = m["message"].as_str().unwrap();
        assert!(message.contains("blog"), "{message}");
        let mut late = live.socket();
        late.recv().unwrap();
        assert_eq!(json(&late.recv().unwrap())["message"], message);
    });
}

// ---- headless Chrome ---------------------------------------------------------------------

/// Chrome to drive: `BRRRTY_CHROME`, or the usual macOS install.
#[cfg(unix)]
fn chrome() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BRRRTY_CHROME") {
        return Some(PathBuf::from(p));
    }
    let mac = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    mac.exists().then_some(mac)
}

/// A headless Chrome with a throwaway profile. Dropping it, on every path including a
/// panic, kills Chrome's whole process group and then removes the profile.
#[cfg(unix)]
struct Chrome {
    child: std::process::Child,
    profile: Option<tempfile::TempDir>,
    devtools: String,
}

#[cfg(unix)]
impl Chrome {
    fn launch(binary: &Path) -> Chrome {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        let profile = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(binary)
            .args([
                "--headless=new",
                &format!("--user-data-dir={}", profile.path().display()),
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-extensions",
                "--disable-sync",
                "--metrics-recording-only",
                "--password-store=basic",
                "--use-mock-keychain",
                "--mute-audio",
                "about:blank",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            // Its own process group, so the renderers and helpers go with it.
            .process_group(0)
            .spawn()
            .expect("start Chrome");
        let stderr = child.stderr.take().unwrap();
        let mut chrome = Chrome {
            child,
            profile: Some(profile),
            devtools: String::new(),
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Keep draining, so Chrome never blocks on a full pipe.
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                if let Some(url) = line.strip_prefix("DevTools listening on ") {
                    let _ = tx.send(url.trim().to_string());
                }
            }
        });
        chrome.devtools = rx
            .recv_timeout(DEADLINE)
            .expect("Chrome to print its DevTools address");
        chrome
    }
}

#[cfg(unix)]
impl Drop for Chrome {
    fn drop(&mut self) {
        let group = format!("-{}", self.child.id());
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &group])
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Helpers can take a moment to go; the profile is removed once they have.
        if let Some(profile) = self.profile.take() {
            let path = profile.path().to_path_buf();
            let until = Instant::now() + Duration::from_secs(10);
            while fs::remove_dir_all(&path).is_err() && path.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(profile);
        }
    }
}

/// A DevTools protocol session with one page.
#[cfg(unix)]
struct Cdp {
    ws: Client,
    session: String,
    next: u64,
    console: Vec<(String, String)>,
    loads: usize,
}

#[cfg(unix)]
impl Cdp {
    fn open(devtools: &str) -> Cdp {
        let rest = devtools.strip_prefix("ws://").unwrap();
        let (host, path) = rest.split_at(rest.find('/').unwrap());
        let addr = host.parse().unwrap();
        let ws = Client::connect(addr, path, None).unwrap();
        let mut cdp = Cdp {
            ws,
            session: String::new(),
            next: 0,
            console: Vec::new(),
            loads: 0,
        };
        let target = cdp.call(
            "Target.createTarget",
            serde_json::json!({"url": "about:blank"}),
        );
        let target = target["targetId"].as_str().unwrap().to_string();
        let attached = cdp.call(
            "Target.attachToTarget",
            serde_json::json!({"targetId": target, "flatten": true}),
        );
        cdp.session = attached["sessionId"].as_str().unwrap().to_string();
        cdp.call("Runtime.enable", serde_json::json!({}));
        cdp.call("Page.enable", serde_json::json!({}));
        cdp
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut msg = serde_json::json!({"id": id, "method": method, "params": params});
        if !self.session.is_empty() {
            msg["sessionId"] = Value::String(self.session.clone());
        }
        self.ws.send(&msg.to_string()).unwrap();
        loop {
            let m = json(&self.ws.recv().unwrap());
            if m["id"] == id {
                assert!(m.get("error").is_none(), "{method}: {m}");
                return m["result"].clone();
            }
            self.event(&m);
        }
    }

    fn event(&mut self, m: &Value) {
        match m["method"].as_str() {
            Some("Runtime.consoleAPICalled") => {
                let text: Vec<String> = m["params"]["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| match &a["value"] {
                        Value::String(s) => s.clone(),
                        Value::Null => a["description"].as_str().unwrap_or("").to_string(),
                        v => v.to_string(),
                    })
                    .collect();
                let kind = m["params"]["type"].as_str().unwrap_or("").to_string();
                self.console.push((kind, text.join(" ")));
            }
            Some("Page.loadEventFired") => self.loads += 1,
            _ => {}
        }
    }

    fn eval(&mut self, expr: &str) -> Value {
        let r = self.call(
            "Runtime.evaluate",
            serde_json::json!({"expression": expr, "returnByValue": true}),
        );
        r["result"]["value"].clone()
    }

    /// Poll `expr` until it is `want`, keeping up with events meanwhile.
    fn wait_eval(&mut self, expr: &str, want: Value) {
        let until = Instant::now() + DEADLINE;
        loop {
            let got = self.eval(expr);
            if got == want {
                return;
            }
            assert!(
                Instant::now() < until,
                "timed out waiting for {expr} == {want}, last {got}; console {:?}",
                self.console
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_console(&mut self, kind: &str, starts: &str) -> String {
        let until = Instant::now() + DEADLINE;
        loop {
            if let Some((_, text)) = self.console.iter().find(|(k, t)| {
                k == kind
                    && t.split_once("UTC] ")
                        .is_some_and(|(_, rest)| rest.starts_with(starts))
            }) {
                return text.clone();
            }
            assert!(
                Instant::now() < until,
                "timed out waiting for console.{kind} {starts:?}; got {:?}",
                self.console
            );
            self.eval("0");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(unix)]
#[test]
fn headless_chrome_follows_edits_without_a_manual_reload() {
    let Some(binary) = chrome() else {
        eprintln!(
            "skipping headless_chrome_follows_edits_without_a_manual_reload: no Chrome found (set BRRRTY_CHROME to run it)"
        );
        return;
    };
    let t = site();
    with_live(t.path(), |live| {
        let chrome = Chrome::launch(&binary);
        let mut cdp = Cdp::open(&chrome.devtools);
        let url = format!("http://{}/", live.server.local_addr());
        cdp.call("Page.navigate", serde_json::json!({"url": url}));
        let msg = "document.getElementById('msg')?.textContent";
        cdp.wait_eval(msg, "one".into());
        cdp.wait_console("log", "Connected");
        wait_for("the page to connect", || live.live.clients() == 1);
        live.settle();
        cdp.eval("window.__marker = 1");
        let loads = cdp.loads;

        // The page's own template: patched in, no reload (the marker survives).
        write(live.root, "src/index.md", page("two"));
        cdp.wait_eval(msg, "two".into());
        cdp.wait_console("log", "HTML delta applied without page reload.");
        assert_eq!(cdp.eval("window.__marker"), 1);
        assert_eq!(cdp.loads, loads);

        // The layout: a full reload with the new HTML.
        write(
            live.root,
            "src/_includes/base.liquid",
            LAYOUT.replace("<title>T", "<title>T2"),
        );
        cdp.wait_eval("document.title", "T2".into());
        cdp.wait_eval("window.__marker === undefined", true.into());
        cdp.wait_eval(msg, "two".into());
        wait_for("the reloaded page to connect", || live.live.clients() == 1);
        live.settle();

        // A stylesheet: swapped in place, no reload.
        cdp.eval("window.__marker = 2");
        let color = "getComputedStyle(document.getElementById('msg')).color";
        assert_eq!(cdp.eval(color), "rgb(255, 0, 0)");
        write(
            live.root,
            "src/css/style.css",
            "p { color: rgb(0, 0, 255); }\n",
        );
        cdp.wait_eval(color, "rgb(0, 0, 255)".into());
        cdp.wait_console("log", "CSS updated without page reload.");
        assert_eq!(cdp.eval("window.__marker"), 2);

        // A build error: logged, and the page keeps the last good build.
        write(live.root, "src/index.md", page("{% if %}"));
        let error = cdp.wait_console("error", "Build error: src/index.md:");
        assert!(error.starts_with("[brrrty]["), "{error}");
        assert_eq!(cdp.eval(msg), "two");
        assert_eq!(cdp.eval("window.__marker"), 2);

        // The fix: patched in, which is what clears the error for the page.
        write(live.root, "src/index.md", page("three"));
        cdp.wait_eval(msg, "three".into());
        assert_eq!(cdp.eval("window.__marker"), 2);
        // A page opened now hears nothing of the old error.
        cdp.console.clear();
        cdp.call("Page.reload", serde_json::json!({}));
        cdp.wait_eval("window.__marker === undefined", true.into());
        cdp.wait_console("log", "Connected");
        wait_for("the page to reconnect", || live.live.clients() == 1);
        cdp.eval("0");
        assert!(
            !cdp.console.iter().any(|(k, _)| k == "error"),
            "{:?}",
            cdp.console
        );
        drop(chrome);
    });
}
