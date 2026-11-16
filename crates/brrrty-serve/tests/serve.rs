//! Behavior pinned against `@11ty/eleventy-dev-server` 2.1.2 (probed with
//! Node 26 on a temp output folder), plus the path traversal suite.
//!
//! Where the dev server is stricter or looser than Brrrty on purpose, the
//! test says so.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use brrrty_serve::{
    DEFAULT_HOST, DEFAULT_PORT, HtmlRewrite, Limits, PORT_RETRIES, RunningServer, ServeError,
    Server,
};

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Send `raw` bytes and read one response, using the content length for the
/// body so a kept-alive connection works too.
fn read_reply(stream: &mut TcpStream, head_only: bool) -> Reply {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).expect("read head");
        assert!(n == 1, "connection closed inside the head: {buf:?}");
        buf.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let status: u16 = status_line.split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (n, v) = l.split_once(':').unwrap();
            (n.to_string(), v.trim().to_string())
        })
        .collect();
    let len: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = vec![0u8; if head_only { 0 } else { len }];
    stream.read_exact(&mut body).expect("read body");
    Reply {
        status,
        headers,
        body,
    }
}

fn raw(srv: &RunningServer, request: &[u8]) -> Reply {
    let mut s = TcpStream::connect(srv.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(request).unwrap();
    read_reply(&mut s, request.starts_with(b"HEAD "))
}

fn request(srv: &RunningServer, method: &str, target: &str, extra: &[(&str, &str)]) -> Reply {
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (n, v) in extra {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    raw(srv, req.as_bytes())
}

fn get(srv: &RunningServer, target: &str) -> Reply {
    request(srv, "GET", target, &[])
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// A site folder inside a parent that also holds a secret and a sibling folder
/// whose name starts with the site folder's name.
struct Fixture {
    _tmp: tempfile::TempDir,
    site: std::path::PathBuf,
    parent: std::path::PathBuf,
}

fn fixture(with_404: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().canonicalize().unwrap();
    let site = parent.join("site");
    write(&site, "index.html", "<html><body>home</body></html>");
    write(&site, "docs/index.html", "docs index");
    write(&site, "about.html", "about page");
    write(&site, "blog.html", "blog page");
    write(&site, "blog/index.html", "blog index");
    write(&site, "both.html", "both file");
    write(&site, "both/index.html", "both index");
    write(&site, "caf\u{e9}/index.html", "cafe index");
    write(&site, "caf\u{e9}/\u{f1} \u{e9}.html", "cafe page");
    write(&site, "sp ace.txt", "spaced");
    write(&site, "a%b.txt", "percent");
    write(&site, ".hidden", "hidden");
    write(&site, "style.css", "body{}");
    write(&site, "empty/placeholder.txt", "p");
    if with_404 {
        write(&site, "404.html", "<html><body>custom 404</body></html>");
    }
    write(&parent, "secret.txt", "TOP SECRET");
    write(&parent, "site2/x.txt", "SIBLING");
    Fixture {
        _tmp: tmp,
        site,
        parent,
    }
}

fn start(f: &Fixture) -> RunningServer {
    Server::start(&f.site, DEFAULT_HOST, 0).unwrap()
}

// ---- ports and hosts -------------------------------------------------

#[test]
fn defaults_match_the_dev_server() {
    assert_eq!(DEFAULT_PORT, 8080);
    assert_eq!(PORT_RETRIES, 10);
    assert_eq!(DEFAULT_HOST, "localhost");
}

#[test]
fn binds_to_loopback_by_default() {
    let f = fixture(true);
    let srv = start(&f);
    assert!(srv.local_addr().ip().is_loopback());
    assert!(srv.url().starts_with("http://localhost:"));
}

#[test]
fn host_flag_can_widen_the_bind() {
    let f = fixture(true);
    let srv = Server::start(&f.site, "0.0.0.0", 0).unwrap();
    assert!(srv.local_addr().ip().is_unspecified());
    // Bound for other machines, any name for this one is fine.
    let mut s = TcpStream::connect(("127.0.0.1", srv.port())).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /about HTTP/1.1\r\nHost: my-laptop.lan:8080\r\nConnection: close\r\n\r\n")
        .unwrap();
    assert_eq!(read_reply(&mut s, false).text(), "about page");
}

#[test]
fn on_loopback_only_requests_naming_this_machine_are_answered() {
    // A page whose own name was rebound to 127.0.0.1 sends that name as Host; it gets
    // nothing, neither files nor a socket. The dev server answers it.
    let f = fixture(true);
    let srv = start(&f);
    for host in [
        "evil.example:8080",
        "localhost.evil.example",
        "10.0.0.5:8080",
    ] {
        let r = raw(
            &srv,
            format!("GET /about HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        );
        assert_eq!(r.status, 403, "{host}");
        assert!(!r.text().contains("about page"));
    }
    for host in ["localhost:1234", "127.0.0.1", "[::1]:8080", "LOCALHOST"] {
        let r = raw(
            &srv,
            format!("GET /about HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        );
        assert_eq!(r.text(), "about page", "{host}");
    }
}

#[test]
fn an_unresolvable_host_is_an_error() {
    let f = fixture(true);
    let err = Server::start(&f.site, "no-such-host.invalid", 0)
        .err()
        .unwrap();
    assert!(matches!(err, ServeError::Host { .. }), "{err}");
}

#[test]
fn a_missing_output_folder_is_an_error() {
    let f = fixture(true);
    let err = Server::start(&f.site.join("nope"), DEFAULT_HOST, 0)
        .err()
        .unwrap();
    assert!(matches!(err, ServeError::Root { .. }), "{err}");
}

/// The first port of a run of `n` free ports on loopback, with the run held
/// by the listeners returned.
///
/// The runs are looked for below the OS's ephemeral range (49152 and up on macOS, 32768 and up
/// on Linux): client connections, which other tests open by the hundred, take their ports
/// from that range, so a run there is often broken by one.
fn free_run(n: u16) -> (u16, Vec<TcpListener>) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        ^ std::process::id();
    for attempt in 0..200u32 {
        let base = 20_000 + ((seed.wrapping_add(attempt.wrapping_mul(7919))) % 12_000) as u16;
        let held: Vec<_> = (0..n)
            .map_while(|i| TcpListener::bind(("127.0.0.1", base + i)).ok())
            .collect();
        if held.len() == n as usize {
            return (base, held);
        }
    }
    panic!("no free run of ports");
}

#[test]
fn a_busy_port_moves_to_the_next_free_one() {
    let f = fixture(true);
    let (base, held) = free_run(2);
    let srv = Server::start(&f.site, DEFAULT_HOST, base).unwrap();
    // The next port up, or a little higher if another test holds it.
    assert!(
        (base + 2..=base + 10).contains(&srv.port()),
        "{}",
        srv.port()
    );
    assert!(srv.port_changed());
    assert_eq!(get(&srv, "/about").status, 200);
    drop(held);
}

#[test]
fn the_requested_port_is_used_when_free() {
    let f = fixture(true);
    let (base, mut held) = free_run(1);
    held.clear();
    let srv = Server::start(&f.site, DEFAULT_HOST, base).unwrap();
    assert_eq!(srv.port(), base);
    assert!(!srv.port_changed());
}

#[test]
fn gives_up_after_ten_retries() {
    let f = fixture(true);
    // Eleven busy ports: the first and ten retries, as the dev server tries.
    let (base, held) = free_run(PORT_RETRIES + 1);
    let err = Server::start(&f.site, DEFAULT_HOST, base).err().unwrap();
    assert!(
        matches!(err, ServeError::PortsInUse { first, tried } if first == base && tried == 11),
        "{err}"
    );
    drop(held);
}

#[test]
fn the_eleventh_retry_is_not_attempted() {
    let f = fixture(true);
    // Ports base..base+10 busy, base+11 free: still gives up.
    let (base, held) = free_run(PORT_RETRIES + 2);
    let mut held = held;
    held.pop();
    let err = Server::start(&f.site, DEFAULT_HOST, base).err().unwrap();
    assert!(matches!(err, ServeError::PortsInUse { .. }), "{err}");
}

#[test]
fn stopping_releases_the_port() {
    let f = fixture(true);
    let srv = start(&f);
    let port = srv.port();
    // An idle keep-alive connection must not hold the port.
    let _idle = TcpStream::connect(srv.local_addr()).unwrap();
    srv.stop();
    TcpListener::bind(("127.0.0.1", port)).expect("port released after stop");
}

#[test]
fn dropping_stops_the_server() {
    let f = fixture(true);
    let srv = start(&f);
    let addr = srv.local_addr();
    drop(srv);
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err());
}

// ---- index, extensions, redirects ------------------------------------

#[test]
fn root_serves_index_html() {
    let f = fixture(true);
    let r = get(&start(&f), "/");
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "<html><body>home</body></html>");
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
}

#[test]
fn dir_without_slash_redirects_with_301() {
    let f = fixture(true);
    let r = get(&start(&f), "/docs");
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/docs/"));
    assert!(r.body.is_empty());
}

#[test]
fn dir_with_slash_serves_its_index() {
    let f = fixture(true);
    let r = get(&start(&f), "/docs/");
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "docs index");
}

#[test]
fn page_serves_its_html_file_without_the_extension() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/about").text(), "about page");
    assert_eq!(get(&srv, "/about.html").text(), "about page");
}

#[test]
fn page_with_trailing_slash_redirects_to_no_slash() {
    let f = fixture(true);
    let r = get(&start(&f), "/about/");
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/about"));
}

#[test]
fn file_and_dir_with_the_same_name_each_win_their_own_url() {
    let f = fixture(true);
    let srv = start(&f);
    // /both matches both.html, /both/ matches both/index.html.
    assert_eq!(get(&srv, "/both").text(), "both file");
    assert_eq!(get(&srv, "/both/").text(), "both index");
    // blog.html and blog/index.html as well.
    assert_eq!(get(&srv, "/blog").text(), "blog page");
    assert_eq!(get(&srv, "/blog/").text(), "blog index");
}

#[test]
fn a_file_with_a_trailing_slash_is_not_found() {
    let f = fixture(true);
    assert_eq!(get(&start(&f), "/about.html/").status, 404);
}

#[test]
fn a_dir_with_no_index_is_not_found_not_listed() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/empty/").status, 404);
    assert_eq!(get(&srv, "/empty").status, 404);
}

#[test]
fn query_strings_are_ignored_and_dropped_from_redirects() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/about?x=1").text(), "about page");
    assert_eq!(get(&srv, "/style.css?v=2").text(), "body{}");
    let r = get(&srv, "/docs?x=1");
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/docs/"));
    assert_eq!(get(&srv, "/docs/?x=1").text(), "docs index");
    assert_eq!(get(&srv, "/?x=1").status, 200);
}

#[test]
fn a_fragment_is_ignored() {
    let f = fixture(true);
    let r = get(&start(&f), "/docs#frag");
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/docs/"));
}

#[test]
fn doubled_slashes_collapse() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/docs//").text(), "docs index");
    assert_eq!(get(&srv, "//docs/").text(), "docs index");
}

#[test]
fn encoded_paths_decode_once() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/sp%20ace.txt").text(), "spaced");
    assert_eq!(get(&srv, "/a%25b.txt").text(), "percent");
    assert_eq!(get(&srv, "/%61bout").text(), "about page");
}

#[test]
fn non_ascii_names_work_encoded_or_raw() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/caf%C3%A9/").text(), "cafe index");
    assert_eq!(get(&srv, "/caf\u{e9}/").text(), "cafe index");
    assert_eq!(
        get(&srv, "/caf%C3%A9/%C3%B1%20%C3%A9.html").text(),
        "cafe page"
    );
    assert_eq!(get(&srv, "/caf%C3%A9/%C3%B1%20%C3%A9").text(), "cafe page");
}

#[test]
fn redirect_locations_are_percent_encoded() {
    let f = fixture(true);
    let srv = start(&f);
    // The dev server answers "/caf%C3%A9/" for both spellings.
    let r = get(&srv, "/caf%C3%A9");
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/caf%C3%A9/"));
    let r = get(&srv, "/caf\u{e9}");
    assert_eq!(r.header("location"), Some("/caf%C3%A9/"));
}

#[test]
fn dot_files_are_served() {
    let f = fixture(true);
    assert_eq!(get(&start(&f), "/.hidden").text(), "hidden");
}

// ---- 404 -------------------------------------------------------------

#[test]
fn a_missing_page_shows_404_html_with_status_404() {
    let f = fixture(true);
    let srv = start(&f);
    for target in ["/nope", "/nope/", "/a%20b", "/deep/nope/x"] {
        let r = get(&srv, target);
        assert_eq!(r.status, 404, "{target}");
        assert_eq!(r.text(), "<html><body>custom 404</body></html>");
        assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    }
}

#[test]
fn a_missing_non_html_file_also_shows_404_html() {
    let f = fixture(true);
    let r = get(&start(&f), "/missing.css");
    assert_eq!(r.status, 404);
    assert_eq!(r.text(), "<html><body>custom 404</body></html>");
}

#[test]
fn the_404_page_is_also_served_directly_with_200() {
    let f = fixture(true);
    let r = get(&start(&f), "/404.html");
    assert_eq!(r.status, 200);
}

#[test]
fn without_404_html_the_builtin_page_is_sent() {
    let f = fixture(false);
    let r = get(&start(&f), "/nope");
    assert_eq!(r.status, 404);
    assert_eq!(
        r.text(),
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>Cannot GET /nope</pre>\n</body>\n</html>\n"
    );
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    assert_eq!(
        r.header("content-security-policy"),
        Some("default-src 'none'")
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
}

#[test]
fn the_builtin_page_names_the_method_and_escapes_the_path() {
    let f = fixture(false);
    let srv = start(&f);
    let r = request(&srv, "HEAD", "/nope?q=1", &[]);
    assert_eq!(r.status, 404);
    // The length is that of the "HEAD" page, one more than "GET".
    assert_eq!(r.header("content-length"), Some("144"));
    assert!(r.body.is_empty());
    let r = get(&srv, "/%3Cb%3E");
    assert!(r.text().contains("Cannot GET /%3Cb%3E"), "{}", r.text());
    let r = get(&srv, "/\u{e9}<b>");
    assert!(
        r.text().contains("Cannot GET /%C3%A9&lt;b&gt;"),
        "{}",
        r.text()
    );
}

// ---- content types, headers, methods ---------------------------------

#[test]
fn content_types_follow_the_mime_package() {
    let f = fixture(true);
    let cases: &[(&str, Option<&str>)] = &[
        ("a.html", Some("text/html; charset=utf-8")),
        ("a.htm", Some("text/html; charset=utf-8")),
        ("a.css", Some("text/css")),
        ("a.js", Some("application/javascript")),
        ("a.mjs", Some("application/javascript")),
        ("a.json", Some("application/json")),
        ("a.map", Some("application/json")),
        ("a.svg", Some("image/svg+xml")),
        ("a.xml", Some("application/xml")),
        ("a.atom", Some("application/atom+xml")),
        ("a.rss", Some("application/rss+xml")),
        ("a.txt", Some("text/plain")),
        ("a.md", Some("text/markdown")),
        ("a.csv", Some("text/csv")),
        ("a.png", Some("image/png")),
        ("a.jpg", Some("image/jpeg")),
        ("a.gif", Some("image/gif")),
        ("a.webp", Some("image/webp")),
        ("a.avif", Some("image/avif")),
        ("a.ico", Some("image/vnd.microsoft.icon")),
        ("a.woff2", Some("font/woff2")),
        ("a.pdf", Some("application/pdf")),
        ("a.mp4", Some("video/mp4")),
        ("a.wasm", Some("application/wasm")),
        ("a.webmanifest", Some("application/manifest+json")),
        ("A.CSS", Some("text/css")),
        ("noext", None),
        ("a.unknownext", None),
        (".hidden", None),
    ];
    for (name, _) in cases {
        write(&f.site, &format!("types/{name}"), "x");
    }
    let srv = start(&f);
    for (name, want) in cases {
        let r = get(&srv, &format!("/types/{name}"));
        assert_eq!(r.status, 200, "{name}");
        assert_eq!(r.header("content-type"), *want, "{name}");
    }
}

#[test]
fn the_type_comes_from_the_request_name_not_the_symlink_target() {
    #[cfg(unix)]
    {
        let f = fixture(true);
        std::os::unix::fs::symlink("about.html", f.site.join("link.css")).unwrap();
        let r = get(&start(&f), "/link.css");
        assert_eq!(r.text(), "about page");
        assert_eq!(r.header("content-type"), Some("text/css"));
    }
}

#[test]
fn responses_carry_no_caching_headers() {
    let f = fixture(true);
    let srv = start(&f);
    for target in ["/about", "/style.css", "/docs", "/nope"] {
        let r = get(&srv, target);
        for h in ["cache-control", "etag", "last-modified", "expires"] {
            assert_eq!(r.header(h), None, "{target} {h}");
        }
    }
}

#[test]
fn head_has_the_headers_and_no_body() {
    let f = fixture(true);
    let srv = start(&f);
    let r = request(&srv, "HEAD", "/about", &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-length"), Some("10"));
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    let r = request(&srv, "HEAD", "/style.css", &[]);
    assert_eq!(r.header("content-type"), Some("text/css"));
    assert_eq!(r.header("content-length"), Some("6"));
    let r = request(&srv, "HEAD", "/docs", &[]);
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/docs/"));
    let r = request(&srv, "HEAD", "/nope", &[]);
    assert_eq!(r.status, 404);
    assert_eq!(r.header("content-length"), Some("36"));
}

#[test]
fn other_methods_are_served_like_get() {
    // The dev server does not look at the method.
    let f = fixture(true);
    let srv = start(&f);
    for m in ["POST", "PUT", "OPTIONS", "DELETE"] {
        let r = request(&srv, m, "/about", &[]);
        assert_eq!(r.status, 200, "{m}");
        assert_eq!(r.text(), "about page", "{m}");
    }
}

#[test]
fn a_connection_can_carry_several_requests() {
    let f = fixture(true);
    let srv = start(&f);
    let mut s = TcpStream::connect(srv.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    for _ in 0..3 {
        s.write_all(b"GET /about HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let r = read_reply(&mut s, false);
        assert_eq!(r.text(), "about page");
        assert_eq!(r.header("connection"), Some("keep-alive"));
    }
}

#[test]
fn connection_close_is_honored() {
    let f = fixture(true);
    let r = get(&start(&f), "/about");
    assert_eq!(r.header("connection"), Some("close"));
}

#[test]
fn byte_ranges_get_206_with_validators() {
    let f = fixture(true);
    write(&f.site, "t.txt", "hello world");
    let srv = start(&f);
    let r = request(&srv, "GET", "/t.txt", &[("Range", "bytes=0-4")]);
    assert_eq!(r.status, 206);
    assert_eq!(r.text(), "hello");
    assert_eq!(r.header("content-range"), Some("bytes 0-4/11"));
    assert_eq!(r.header("accept-ranges"), Some("bytes"));
    assert_eq!(r.header("cache-control"), Some("public, max-age=0"));
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    assert!(r.header("etag").unwrap().starts_with("W/\"b-"));
    assert!(r.header("last-modified").unwrap().ends_with(" GMT"));
    let r = request(&srv, "GET", "/t.txt", &[("Range", "bytes=6-")]);
    assert_eq!(r.text(), "world");
    let r = request(&srv, "GET", "/t.txt", &[("Range", "bytes=-3")]);
    assert_eq!(r.text(), "rld");
    let r = request(&srv, "GET", "/t.txt", &[("Range", "bytes=50-60")]);
    assert_eq!(r.status, 416);
    assert_eq!(r.header("content-range"), Some("bytes */11"));
    let r = request(&srv, "GET", "/t.txt", &[("Range", "bytes=0-1,3-4")]);
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "hello world");
}

#[test]
fn malformed_requests_are_rejected() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(raw(&srv, b"NONSENSE\r\n\r\n").status, 400);
    assert_eq!(raw(&srv, b"GET /a HTTP/9.9\r\n\r\n").status, 505);
    assert_eq!(raw(&srv, b"GET about HTTP/1.1\r\n\r\n").status, 400);
    let mut big = b"GET /".to_vec();
    big.extend(std::iter::repeat_n(b'a', 20_000));
    big.extend_from_slice(b" HTTP/1.1\r\n\r\n");
    assert_eq!(raw(&srv, &big).status, 431);
    // A bad percent escape and bytes that are not UTF-8 are 400s.
    assert_eq!(get(&srv, "/a%zz").status, 400);
    assert_eq!(get(&srv, "/%c3%28").status, 400);
    assert_eq!(get(&srv, "/a%").status, 400);
}

// ---- limits on what a client can hold ---------------------------------

fn limited(f: &Fixture, limits: Limits) -> RunningServer {
    Server::start_with_limits(&f.site, DEFAULT_HOST, 0, limits).unwrap()
}

fn connect(srv: &RunningServer) -> TcpStream {
    let s = TcpStream::connect(srv.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

/// Whether the server has closed `s` (EOF or reset) without sending more.
fn closed_by_peer(s: &mut TcpStream) -> bool {
    let mut b = [0u8; 1];
    matches!(s.read(&mut b), Ok(0) | Err(_))
}

#[test]
fn a_slow_drip_head_hits_one_total_deadline() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            head_timeout: Duration::from_millis(800),
            ..Limits::default()
        },
    );
    let mut s = connect(&srv);
    let start = Instant::now();
    s.write_all(b"GET /about HTTP/1.1\r\n").unwrap();
    // One header every 300 ms: each read arrives well inside any per-read
    // timeout, but the head as a whole is far too slow.
    let mut reply = None;
    for i in 0..30 {
        std::thread::sleep(Duration::from_millis(300));
        if s.write_all(format!("X-Drip-{i}: x\r\n").as_bytes())
            .is_err()
        {
            break;
        }
        s.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        let mut b = [0u8; 1];
        if matches!(s.peek(&mut b), Ok(1)) {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            reply = Some(read_reply(&mut s, false));
            break;
        }
    }
    let reply = reply.expect("the server answered while the client was still dripping");
    assert_eq!(reply.status, 408);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(closed_by_peer(&mut s));
}

#[test]
fn a_fast_client_is_not_held_to_the_head_deadline() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            head_timeout: Duration::from_millis(800),
            ..Limits::default()
        },
    );
    assert_eq!(get(&srv, "/about").status, 200);
}

#[test]
fn an_idle_connection_is_closed_after_the_idle_timeout() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            idle_timeout: Duration::from_millis(300),
            ..Limits::default()
        },
    );
    let mut s = connect(&srv);
    let start = Instant::now();
    assert!(closed_by_peer(&mut s));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn connections_past_the_cap_get_503_and_a_slot_frees_up() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            max_connections: 3,
            ..Limits::default()
        },
    );
    let mut held: Vec<TcpStream> = (0..3).map(|_| connect(&srv)).collect();
    // Make sure all three are counted before the fourth arrives.
    for s in &mut held {
        s.write_all(b"GET /about HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        assert_eq!(read_reply(s, false).status, 200);
    }
    let mut over = connect(&srv);
    // A request the server never reads: closing with it unread must not reset the
    // connection before the 503 arrives.
    over.write_all(&padded_request()).unwrap();
    let r = read_reply(&mut over, false);
    assert_eq!(r.status, 503);
    assert_eq!(r.header("retry-after"), Some("1"));
    assert!(closed_by_peer(&mut over));
    // Dropping one frees its slot.
    held.pop();
    let mut ok = false;
    for _ in 0..100 {
        let mut s = connect(&srv);
        s.write_all(b"GET /about HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        if read_reply(&mut s, false).status == 200 {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok, "a slot never freed up");
}

/// A request with a large header, so plenty of it sits unread in the server's buffer.
fn padded_request() -> Vec<u8> {
    format!(
        "GET /about HTTP/1.1\r\nHost: localhost\r\nX-Pad: {}\r\n\r\n",
        "p".repeat(6000)
    )
    .into_bytes()
}

/// Send `head` and then `trailing` bytes the server will not read, and read the reply
/// after giving the trailing bytes time to land.
fn reply_with_unread_bytes(srv: &RunningServer, head: &[u8], trailing: usize) -> Reply {
    let mut s = connect(srv);
    s.write_all(head).unwrap();
    s.write_all(&vec![b'x'; trailing]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    read_reply(&mut s, false)
}

#[test]
fn early_closes_drain_unread_input_so_the_reply_arrives() {
    let f = fixture(true);
    let srv = start(&f);
    // 400: conflicting lengths, then a body the server never reads.
    let r = reply_with_unread_bytes(
        &srv,
        b"POST /about HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
        20_000,
    );
    assert_eq!(r.status, 400);
    // 200 with Connection: close and an unread body.
    let r = reply_with_unread_bytes(
        &srv,
        b"POST /about HTTP/1.1\r\nHost: localhost\r\nContent-Length: 20000\r\nConnection: close\r\n\r\n",
        20_000,
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "about page");
}

#[test]
fn a_408_arrives_even_with_unread_input() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            head_timeout: Duration::from_millis(300),
            ..Limits::default()
        },
    );
    let mut s = connect(&srv);
    // An unfinished head the server gives up on, then more bytes it never reads.
    s.write_all(b"GET /about HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    s.write_all(&vec![b'x'; 20_000]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(read_reply(&mut s, false).status, 408);
}

// ---- request framing --------------------------------------------------

#[test]
fn conflicting_content_lengths_are_refused_and_the_connection_closes() {
    let f = fixture(true);
    let srv = start(&f);
    // The reviewer's case: a 0 then a 30, with a pipelined request in the
    // "body". It must not be served as a second request.
    let smuggled = "GET /about HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let req = format!(
        "GET /about HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 30\r\n\r\n{smuggled}"
    );
    let mut s = connect(&srv);
    s.write_all(req.as_bytes()).unwrap();
    let r = read_reply(&mut s, false);
    assert_eq!(r.status, 400);
    assert_eq!(r.header("connection"), Some("close"));
    assert!(closed_by_peer(&mut s), "a second response followed");
}

#[test]
fn other_ambiguous_framing_is_refused() {
    let f = fixture(true);
    let srv = start(&f);
    for extra in [
        "Content-Length: 5, 5",
        "Content-Length: 0, 30",
        "Content-Length: abc",
        "Content-Length: -1",
        "Content-Length:",
        "Content-Length: 0\r\nTransfer-Encoding: chunked",
        "Transfer-Encoding: chunked\r\nContent-Length: 0",
    ] {
        let req = format!("POST /about HTTP/1.1\r\nHost: localhost\r\n{extra}\r\n\r\n");
        assert_eq!(raw(&srv, req.as_bytes()).status, 400, "{extra}");
    }
    // One plain length is fine, and a body is not read as a request.
    let r = raw(
        &srv,
        b"POST /about HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd",
    );
    assert_eq!(r.status, 200);
}

// ---- swapping the folder and the HTML hook ----------------------------

#[test]
fn set_root_switches_the_served_folder() {
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/about").status, 200);
    let other = f.parent.join("other");
    write(&other, "index.html", "other home");
    srv.set_root(&other).unwrap();
    assert_eq!(get(&srv, "/").text(), "other home");
    assert_eq!(get(&srv, "/about").status, 404);
    // A bad folder leaves the current one in place.
    assert!(srv.set_root(&f.parent.join("missing")).is_err());
    assert_eq!(get(&srv, "/").text(), "other home");
}

#[test]
fn set_root_follows_a_folder_replaced_in_place() {
    let f = fixture(true);
    let srv = start(&f);
    let staged = f.parent.join("staged");
    write(&staged, "index.html", "new build");
    let old = f.parent.join("old");
    fs::rename(&f.site, &old).unwrap();
    fs::rename(&staged, &f.site).unwrap();
    srv.set_root(&f.site).unwrap();
    assert_eq!(get(&srv, "/").text(), "new build");
}

#[test]
fn swap_root_reports_when_the_old_folder_is_no_longer_used() {
    let f = fixture(true);
    let srv = start(&f);
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    // The hook runs while the request still holds its folder: hold it there.
    srv.set_html_hook(Some(Arc::new(move |page| {
        let _ = entered_tx.send(());
        let _ = release_rx.lock().unwrap().recv();
        HtmlRewrite {
            body: page.body.to_string(),
            headers: vec![],
        }
    })));
    std::thread::scope(|s| {
        let pending = s.spawn(|| get(&srv, "/about"));
        entered.recv().unwrap();
        let other = f.parent.join("other");
        write(&other, "index.html", "other home");
        let retired = srv.swap_root(&other).unwrap();
        assert_eq!(retired.path(), fs::canonicalize(&f.site).unwrap());
        assert!(retired.in_use());
        release.send(()).unwrap();
        // The request finishes against the folder it started with.
        assert_eq!(pending.join().unwrap().text(), "about page");
        assert!(!retired.in_use());
        assert_eq!(srv.root(), fs::canonicalize(&other).unwrap());
    });
}

#[test]
fn the_hook_rewrites_html_only() {
    let f = fixture(true);
    let srv = start(&f);
    srv.set_html_hook(Some(Arc::new(|page| HtmlRewrite {
        body: format!("{}<!--hooked {}-->", page.body, page.status),
        headers: vec![("X-Hooked".into(), "1".into())],
    })));
    let r = get(&srv, "/about");
    assert_eq!(r.text(), "about page<!--hooked 200-->");
    assert_eq!(r.header("content-length"), Some("27"));
    assert_eq!(r.header("x-hooked"), Some("1"));
    // The 404 page is HTML too.
    assert!(get(&srv, "/nope").text().ends_with("<!--hooked 404-->"));
    // Other types, redirects and ranges are untouched.
    assert_eq!(get(&srv, "/style.css").text(), "body{}");
    assert_eq!(get(&srv, "/docs").status, 301);
    let r = request(&srv, "GET", "/about.html", &[("Range", "bytes=0-4")]);
    assert_eq!(r.text(), "about");
    // HEAD reports the rewritten length.
    let r = request(&srv, "HEAD", "/about", &[]);
    assert_eq!(r.header("content-length"), Some("27"));
    srv.set_html_hook(None);
    assert_eq!(get(&srv, "/about").text(), "about page");
}

#[test]
fn the_hook_sees_builtin_pages_and_sub_requests() {
    let f = fixture(false);
    let srv = start(&f);
    srv.set_html_hook(Some(Arc::new(|page| HtmlRewrite {
        body: format!(
            "builtin={} nav={}",
            page.is_builtin_error, page.is_navigation
        ),
        headers: vec![(
            "Content-Security-Policy".into(),
            "script-src 'sha256-x'".into(),
        )],
    })));
    let r = get(&srv, "/nope");
    assert_eq!(r.text(), "builtin=true nav=true");
    // The hook's header replaces the server's.
    assert_eq!(
        r.header("content-security-policy"),
        Some("script-src 'sha256-x'")
    );
    let r = request(&srv, "GET", "/about", &[("Sec-Fetch-Mode", "cors")]);
    assert_eq!(r.text(), "builtin=false nav=false");
    let r = request(&srv, "GET", "/about", &[("Sec-Fetch-Mode", "navigate")]);
    assert_eq!(r.text(), "builtin=false nav=true");
}

// ---- path traversal ---------------------------------------------------

fn assert_contained(srv: &RunningServer, r: &Reply, what: &str) {
    let body = r.text();
    assert!(
        !body.contains("TOP SECRET") && !body.contains("SIBLING") && !body.contains("root:"),
        "{what} leaked: {body}"
    );
    assert!(
        r.status == 404 || r.status == 400,
        "{what} gave {}",
        r.status
    );
    let _ = srv;
}

#[test]
fn traversal_paths_never_leave_the_folder() {
    let f = fixture(true);
    let srv = start(&f);
    let targets = [
        "/../secret.txt",
        "/..%2fsecret.txt",
        "/..%2Fsecret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E/secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/.%2e/secret.txt",
        "/%2e./secret.txt",
        "/docs/../../secret.txt",
        "/docs/..%2f..%2fsecret.txt",
        "/docs/%2e%2e/%2e%2e/secret.txt",
        "/a/b/c/../../../../secret.txt",
        "/..\\secret.txt",
        "/..%5csecret.txt",
        "/..%5Csecret.txt",
        "/docs\\..\\..\\secret.txt",
        "/%252e%252e/secret.txt",
        "/%252e%252e%252fsecret.txt",
        "/..%252fsecret.txt",
        "/%2e%2e%5csecret.txt",
        "/../site2/x.txt",
        "/..%2fsite2/x.txt",
        "/%2e%2e/site2/x.txt",
        "/../",
        "/..",
        "/%2e%2e",
        "/%2e%2e/",
        "/secret.txt%00.html",
        "/about%00",
        "/%00",
        "/about.html%00.png",
        "/..%00/secret.txt",
        "/%c0%ae%c0%ae/secret.txt",
        "/%e0%80%ae%e0%80%ae/secret.txt",
        "/..;/secret.txt",
        "http://example.test/../secret.txt",
        "http://example.test/%2e%2e/secret.txt",
        "/../../../../../../../../etc/passwd",
        "/..%2f..%2f..%2f..%2f..%2f..%2f..%2f..%2fetc%2fpasswd",
    ];
    for t in targets {
        let r = get(&srv, t);
        assert_contained(&srv, &r, t);
    }
}

#[test]
fn dot_segments_are_refused_even_when_they_stay_inside() {
    // The dev server normalizes "/docs/../about" to "/about". Brrrty refuses
    // any ".." segment, since browsers never send one.
    let f = fixture(true);
    let srv = start(&f);
    assert_eq!(get(&srv, "/docs/../about.html").status, 404);
    assert_eq!(get(&srv, "/docs/%2e%2e/about.html").status, 404);
    // A single dot is harmless and dropped.
    assert_eq!(get(&srv, "/./about.html").text(), "about page");
}

#[test]
fn double_encoding_decodes_only_once() {
    let f = fixture(true);
    write(&f.site, "%2e%2e/inner.txt", "literal folder");
    let srv = start(&f);
    // %252e%252e is the literal folder name "%2e%2e", not "..".
    assert_eq!(get(&srv, "/%252e%252e/inner.txt").text(), "literal folder");
    assert_eq!(get(&srv, "/%252e%252e/secret.txt").status, 404);
}

#[test]
fn a_sibling_folder_sharing_the_name_prefix_is_not_served() {
    // The dev server compares paths as strings, so "site2" passes for "site".
    let f = fixture(true);
    let srv = start(&f);
    assert_contained(&srv, &get(&srv, "/../site2/x.txt"), "sibling");
}

#[test]
fn a_redirect_never_points_off_site() {
    let f = fixture(true);
    write(&f.site, "evil.example/index.html", "x");
    let srv = start(&f);
    let r = get(&srv, "//evil.example");
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/evil.example/"));
    let r = get(&srv, "/%2Fevil.example");
    assert_eq!(r.header("location"), Some("/evil.example/"));
}

#[cfg(unix)]
mod symlinks {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_file_symlink_out_of_the_folder_is_not_served() {
        let f = fixture(true);
        symlink(f.parent.join("secret.txt"), f.site.join("leak.txt")).unwrap();
        let srv = start(&f);
        let r = get(&srv, "/leak.txt");
        assert_contained(&srv, &r, "file symlink");
    }

    #[test]
    fn a_dir_symlink_out_of_the_folder_is_not_served() {
        let f = fixture(true);
        symlink(&f.parent, f.site.join("up")).unwrap();
        let srv = start(&f);
        for t in ["/up/secret.txt", "/up/site2/x.txt", "/up/"] {
            assert_contained(&srv, &get(&srv, t), t);
        }
    }

    #[test]
    fn a_symlinked_html_or_index_out_of_the_folder_is_not_served() {
        let f = fixture(true);
        symlink(f.parent.join("secret.txt"), f.site.join("page.html")).unwrap();
        fs::create_dir(f.site.join("d")).unwrap();
        symlink(f.parent.join("secret.txt"), f.site.join("d/index.html")).unwrap();
        let srv = start(&f);
        for t in ["/page", "/page.html", "/d/", "/d"] {
            assert_contained(&srv, &get(&srv, t), t);
        }
    }

    #[test]
    fn a_symlinked_404_page_out_of_the_folder_is_not_used() {
        let f = fixture(false);
        symlink(f.parent.join("secret.txt"), f.site.join("404.html")).unwrap();
        let srv = start(&f);
        let r = get(&srv, "/nope");
        assert_contained(&srv, &r, "404 symlink");
        assert!(r.text().contains("Cannot GET /nope"));
    }

    #[test]
    fn a_symlink_that_stays_inside_the_folder_works() {
        let f = fixture(true);
        symlink("about.html", f.site.join("alias.html")).unwrap();
        symlink("docs", f.site.join("docs-link")).unwrap();
        let srv = start(&f);
        assert_eq!(get(&srv, "/alias").text(), "about page");
        assert_eq!(get(&srv, "/docs-link/").text(), "docs index");
    }

    #[test]
    fn a_symlink_through_a_dir_that_leaves_and_returns_is_refused_only_if_outside() {
        let f = fixture(true);
        // A link out and back in again still resolves inside the folder.
        symlink(f.site.join("docs"), f.parent.join("back")).unwrap();
        symlink("../back", f.site.join("round")).unwrap();
        let srv = start(&f);
        assert_eq!(get(&srv, "/round/").text(), "docs index");
    }

    #[test]
    fn the_output_folder_itself_may_be_a_symlink() {
        let f = fixture(true);
        let link = f.parent.join("linked-site");
        symlink(&f.site, &link).unwrap();
        let srv = Server::start(&link, DEFAULT_HOST, 0).unwrap();
        assert_eq!(get(&srv, "/about").text(), "about page");
        let _ = &srv;
    }
}

// ---- handlers and WebSockets -------------------------------------------

use brrrty_serve::{Handled, Incoming, WebSocket};
use std::sync::Mutex;

/// A handler that answers `/.x/hello`, takes `/.x/ws` over as a WebSocket (keeping each
/// socket in `sockets`), and leaves everything else to the files.
fn with_handler(srv: &RunningServer) -> Arc<Mutex<Vec<WebSocket>>> {
    let sockets: Arc<Mutex<Vec<WebSocket>>> = Arc::default();
    let kept = Arc::clone(&sockets);
    srv.set_handler(Some(Arc::new(move |req: &Incoming| match req.path()? {
        "/.x/hello" => Some(Handled::Reply(brrrty_serve::Reply {
            status: 200,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: format!("hi {}", req.header("X-Name").unwrap_or("?")).into_bytes(),
        })),
        "/.x/ws" => {
            let kept = Arc::clone(&kept);
            Some(Handled::WebSocket(Box::new(move |ws| {
                kept.lock().unwrap().push(ws);
            })))
        }
        _ => None,
    })));
    sockets
}

const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

fn upgrade_request(extra: &str) -> String {
    format!(
        "GET /.x/ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {KEY}\r\n{extra}\r\n"
    )
}

/// Open a WebSocket to `/.x/ws`; gives the stream after the 101.
fn open_socket(srv: &RunningServer) -> TcpStream {
    let mut s = connect(srv);
    s.write_all(upgrade_request("Origin: http://localhost\r\n").as_bytes())
        .unwrap();
    let r = read_reply(&mut s, true);
    assert_eq!(r.status, 101);
    assert_eq!(
        r.header("sec-websocket-accept"),
        Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    );
    assert_eq!(r.header("upgrade"), Some("websocket"));
    s
}

/// Read one unmasked frame from the server.
fn read_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).unwrap();
    assert_eq!(head[1] & 0x80, 0, "server frames are not masked");
    let len = match head[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            s.read_exact(&mut b).unwrap();
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            s.read_exact(&mut b).unwrap();
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).unwrap();
    (head[0] & 0x0f, payload)
}

/// Send a masked frame, as a browser does.
fn send_frame(s: &mut TcpStream, opcode: u8, payload: &[u8]) {
    let mask = [1u8, 2, 3, 4];
    let mut f = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f).unwrap();
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_handler_answers_before_the_files_and_passes_the_rest_on() {
    let f = fixture(true);
    let srv = start(&f);
    write(&f.site, ".x/hello", "the file");
    with_handler(&srv);
    let r = request(&srv, "GET", "/.x/hello", &[("x-name", "you")]);
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "hi you");
    assert_eq!(r.header("content-type"), Some("text/plain"));
    let head = request(&srv, "HEAD", "/.x/hello", &[]);
    assert_eq!(head.header("content-length"), Some("4"));
    assert!(head.body.is_empty());
    assert_eq!(get(&srv, "/about").text(), "about page");
    srv.set_handler(None);
    assert_eq!(get(&srv, "/.x/hello").text(), "the file");
}

#[test]
fn a_websocket_carries_messages_out_and_answers_ping_and_close() {
    let f = fixture(true);
    let srv = start(&f);
    let sockets = with_handler(&srv);
    let mut s = open_socket(&srv);
    wait_until("the socket to be kept", || {
        sockets.lock().unwrap().len() == 1
    });
    let ws = sockets.lock().unwrap()[0].clone();
    ws.send_text("hello").unwrap();
    assert_eq!(read_frame(&mut s), (0x1, b"hello".to_vec()));
    let long = "x".repeat(70_000);
    ws.send_text(&long).unwrap();
    assert_eq!(read_frame(&mut s), (0x1, long.into_bytes()));
    // Text from the browser is read and dropped; a ping gets its pong.
    send_frame(&mut s, 0x1, b"ignored");
    send_frame(&mut s, 0x9, b"p1");
    assert_eq!(read_frame(&mut s), (0xA, b"p1".to_vec()));
    // Close is echoed, and the socket is no longer open.
    send_frame(&mut s, 0x8, &1000u16.to_be_bytes());
    assert_eq!(read_frame(&mut s), (0x8, 1000u16.to_be_bytes().to_vec()));
    assert!(closed_by_peer(&mut s));
    wait_until("the socket to close", || !ws.is_open());
    assert!(ws.send_text("late").is_err());
}

#[test]
fn a_browser_that_goes_away_closes_its_socket() {
    let f = fixture(true);
    let srv = start(&f);
    let sockets = with_handler(&srv);
    let s = open_socket(&srv);
    wait_until("the socket to be kept", || {
        sockets.lock().unwrap().len() == 1
    });
    let ws = sockets.lock().unwrap()[0].clone();
    drop(s);
    wait_until("the socket to close", || !ws.is_open());
}

#[test]
fn a_silent_browser_is_pinged_and_dropped_when_it_never_answers() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            ping_every: Duration::from_millis(200),
            ..Limits::default()
        },
    );
    let sockets = with_handler(&srv);
    let mut s = open_socket(&srv);
    wait_until("the socket to be kept", || {
        sockets.lock().unwrap().len() == 1
    });
    let ws = sockets.lock().unwrap()[0].clone();
    assert_eq!(read_frame(&mut s).0, 0x9, "a ping after a quiet period");
    // Answering keeps it open past another period...
    send_frame(&mut s, 0xA, b"");
    std::thread::sleep(Duration::from_millis(300));
    assert!(ws.is_open());
    // ...and staying silent through a ping closes it.
    wait_until("the silent socket to close", || !ws.is_open());
}

#[test]
fn upgrades_that_are_not_valid_are_refused() {
    let f = fixture(true);
    let srv = start(&f);
    with_handler(&srv);
    // A plain request to the socket's path.
    let r = get(&srv, "/.x/ws");
    assert_eq!(r.status, 426);
    assert_eq!(r.header("upgrade"), Some("websocket"));
    assert_eq!(r.header("sec-websocket-version"), Some("13"));
    // Another version.
    let bad_version = upgrade_request("").replace("Version: 13", "Version: 8");
    assert_eq!(raw(&srv, bad_version.as_bytes()).status, 426);
    // No key.
    let no_key = upgrade_request("").replace(&format!("Sec-WebSocket-Key: {KEY}\r\n"), "");
    assert_eq!(raw(&srv, no_key.as_bytes()).status, 400);
    // A page on another site.
    let other = upgrade_request("Origin: http://evil.example\r\n");
    assert_eq!(raw(&srv, other.as_bytes()).status, 403);
    let lookalike = upgrade_request("Origin: http://localhost.evil.example\r\n");
    assert_eq!(raw(&srv, lookalike.as_bytes()).status, 403);
    // The same host by another scheme spelling is fine, and no Origin (not a browser) too.
    let mut s = connect(&srv);
    s.write_all(upgrade_request("").as_bytes()).unwrap();
    assert_eq!(read_reply(&mut s, true).status, 101);
}

#[test]
fn websockets_have_their_own_cap_and_never_starve_file_requests() {
    let f = fixture(true);
    let srv = limited(
        &f,
        Limits {
            max_connections: 2,
            max_sockets: 3,
            ..Limits::default()
        },
    );
    let sockets = with_handler(&srv);
    let mut open: Vec<TcpStream> = (0..3).map(|_| open_socket(&srv)).collect();
    wait_until("three sockets", || sockets.lock().unwrap().len() == 3);
    // The fourth socket is refused...
    let mut s = connect(&srv);
    s.write_all(upgrade_request("").as_bytes()).unwrap();
    assert_eq!(read_reply(&mut s, false).status, 503);
    // The refused request held a regular slot while it was read; it frees once closed.
    drop(s);
    // Files are still served on both regular slots at once, with three sockets open.
    let mut held = Vec::new();
    for _ in 0..200 {
        let mut c = connect(&srv);
        c.write_all(b"GET /about HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        if read_reply(&mut c, false).status == 200 {
            held.push(c);
            if held.len() == 2 {
                break;
            }
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert_eq!(held.len(), 2, "both regular slots serve files");
    drop(held);
    // Closing a socket frees its slot.
    let gone = open.pop().unwrap();
    drop(gone);
    let mut ok = false;
    for _ in 0..200 {
        let mut s = connect(&srv);
        s.write_all(upgrade_request("").as_bytes()).unwrap();
        if read_reply(&mut s, true).status == 101 {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok, "a socket slot never freed up");
}

#[test]
fn stopping_the_server_ends_open_websockets() {
    let f = fixture(true);
    let srv = start(&f);
    let sockets = with_handler(&srv);
    let mut s = open_socket(&srv);
    wait_until("the socket to be kept", || {
        sockets.lock().unwrap().len() == 1
    });
    let ws = sockets.lock().unwrap()[0].clone();
    srv.stop();
    assert!(closed_by_peer(&mut s));
    wait_until("the socket to close", || !ws.is_open());
}

#[test]
fn a_websocket_from_a_rebound_name_is_refused() {
    let f = fixture(true);
    let srv = start(&f);
    with_handler(&srv);
    let req = upgrade_request("Origin: http://evil.example:8080\r\n")
        .replace("Host: localhost", "Host: evil.example:8080");
    assert_eq!(raw(&srv, req.as_bytes()).status, 403);
}

/// Send raw frame bytes (already masked or not) and expect a close with `code`.
fn expect_close(
    srv: &RunningServer,
    sockets: &Arc<Mutex<Vec<WebSocket>>>,
    bytes: &[u8],
    code: u16,
) {
    let before = sockets.lock().unwrap().len();
    let mut s = open_socket(srv);
    wait_until("the socket to be kept", || {
        sockets.lock().unwrap().len() == before + 1
    });
    let ws = sockets.lock().unwrap()[before].clone();
    s.write_all(bytes).unwrap();
    assert_eq!(
        read_frame(&mut s),
        (0x8, code.to_be_bytes().to_vec()),
        "{bytes:?}"
    );
    assert!(closed_by_peer(&mut s));
    wait_until("the socket to close", || !ws.is_open());
}

#[test]
fn frames_the_rfc_says_must_fail_close_the_socket() {
    let f = fixture(true);
    let srv = start(&f);
    let sockets = with_handler(&srv);
    let masked = |first: u8, payload: &[u8]| {
        let mut b = vec![first];
        if payload.len() < 126 {
            b.push(0x80 | payload.len() as u8);
        } else {
            b.push(0x80 | 126);
            b.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.extend_from_slice(payload);
        b
    };
    // Reserved opcodes.
    for op in [0x3u8, 0x7, 0xB, 0xF] {
        expect_close(&srv, &sockets, &masked(0x80 | op, b""), 1002);
    }
    // A ping over 125 bytes, and a fragmented ping.
    expect_close(&srv, &sockets, &masked(0x89, &[0; 126]), 1002);
    expect_close(&srv, &sockets, &masked(0x09, b"x"), 1002);
    // A continuation with nothing to continue.
    expect_close(&srv, &sockets, &masked(0x80, b"x"), 1002);
    // A new message inside a fragmented one.
    let mut two = masked(0x01, b"a");
    two.extend(masked(0x81, b"b"));
    expect_close(&srv, &sockets, &two, 1002);
    // Too big.
    let mut big = vec![0x81, 0x80 | 127];
    big.extend_from_slice(&(1u64 << 20).to_be_bytes());
    expect_close(&srv, &sockets, &big, 1009);
    // A proper fragmented message, with a ping between its parts, is fine.
    let mut s = open_socket(&srv);
    let mut ok = masked(0x01, b"a");
    ok.extend(masked(0x89, b"p"));
    ok.extend(masked(0x80, b"b"));
    s.write_all(&ok).unwrap();
    assert_eq!(read_frame(&mut s), (0xA, b"p".to_vec()));
}
