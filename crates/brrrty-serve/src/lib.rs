//! A local static file server for the output folder, for `brrrty serve`.
//!
//! It behaves the way `@11ty/eleventy-dev-server` 2.1.2 does for the plain
//! file-serving part:
//!
//! - `/dir/` serves `dir/index.html`, and `/page` serves `page.html`;
//! - `/dir` redirects to `/dir/` when only `dir/index.html` exists, and
//!   `/page/` redirects to `/page` when only `page.html` exists (301, and the
//!   query string is dropped);
//! - a missing path is a 404 that shows the folder's `404.html` when it has
//!   one, and the dev server's plain "Cannot GET" page when it does not;
//! - content types come from the same table the dev server uses, with no
//!   caching headers.
//!
//! Where it differs, it is stricter about paths: it never leaves the output
//! folder, whether by `..`, an encoded `..`, a backslash, a NUL, or a symlink.
//! It binds to `localhost` unless asked for another host.
//!
//! The served folder can be swapped while the server runs
//! ([`RunningServer::set_root`]), HTML responses can be rewritten
//! ([`RunningServer::set_html_hook`]), and requests can be answered before
//! files are looked at, including by taking them over as WebSockets
//! ([`RunningServer::set_handler`]); the rebuild and live-reload layers are
//! built on those.

mod http;
mod mime;
mod mime_table;
mod path;
mod ws;

pub use ws::WebSocket;

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, UNIX_EPOCH};

use http::{Body, ParseError, Request, Response};
use path::{Found, Resolved, Root};

/// The port the dev server starts from.
pub const DEFAULT_PORT: u16 = 8080;
/// The host the server binds to unless told otherwise.
pub const DEFAULT_HOST: &str = "localhost";
/// How many higher ports are tried when the first one is in use. The dev
/// server's `portReassignmentRetryCount`.
pub const PORT_RETRIES: u16 = 10;

/// A folder name never served at the top of the folder: `brrrty serve`'s staging folder
/// inside the output folder, whatever it holds.
pub const HIDDEN_DIR: &str = ".brrrty-serve";

/// A write that makes no progress for this long fails.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Limits on what one client can hold. [`Limits::default`] suits `brrrty
/// serve`; tests shrink them.
#[derive(Debug, Clone)]
pub struct Limits {
    /// How long a kept-alive connection may sit with no request.
    pub idle_timeout: Duration,
    /// The total time a request head (line and headers) may take to arrive
    /// once its first byte has. A client that is slower gets a 408. This is a
    /// deadline for the whole head, not for each read, so sending a byte at a
    /// time cannot hold a connection open.
    pub head_timeout: Duration,
    /// How many connections may be open at once. Past this, a new connection
    /// is answered with a 503 and closed straight away. Refusing out loud is
    /// better than leaving the connection unanswered in the backlog, where a
    /// browser just hangs, and it keeps threads bounded either way.
    ///
    /// A connection taken over as a WebSocket stops counting here and counts
    /// against [`Limits::max_sockets`] instead.
    pub max_connections: usize,
    /// How many WebSockets may be open at once. Past this, an upgrade is
    /// answered with a 503. Each open tab holds one for as long as it is open,
    /// so they get a cap of their own: however many tabs there are, file
    /// requests keep all of [`Limits::max_connections`].
    pub max_sockets: usize,
    /// How long a WebSocket may stay silent before the server pings it. One
    /// that doesn't answer before the next period ends is closed, so a browser
    /// that vanished without closing doesn't hold its slot for good.
    pub ping_every: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            idle_timeout: Duration::from_secs(5),
            head_timeout: Duration::from_secs(15),
            max_connections: 256,
            max_sockets: 64,
            ping_every: Duration::from_secs(30),
        }
    }
}

/// How long a closing connection keeps reading what the client still sends.
const LINGER_TIME: Duration = Duration::from_millis(500);
/// How much a closing connection reads before giving up on the client.
const LINGER_BYTES: usize = 64 * 1024;
/// How many refused connections may linger at once; past this they close at once.
const MAX_LINGERING: usize = 64;

/// Close a connection the server is ending without losing the response.
///
/// Closing a socket that still has unread input makes the kernel send a reset, and a
/// reset can throw away the response the client has not read yet. So the write side
/// is shut first, which sends the response and then end-of-stream, and the input is
/// drained until the client closes too, for at most [`LINGER_TIME`] and
/// [`LINGER_BYTES`].
fn lingering_close(stream: &TcpStream) {
    if stream.shutdown(Shutdown::Write).is_err() {
        return;
    }
    let until = Instant::now() + LINGER_TIME;
    let mut buf = [0u8; 4096];
    let mut read = 0;
    let mut s = stream;
    while read < LINGER_BYTES {
        let Some(left) = until
            .checked_duration_since(Instant::now())
            .filter(|l| !l.is_zero())
        else {
            break;
        };
        if s.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => read += n,
        }
    }
}

/// How long to wait after the `n`th accept error in a row (1 is the first).
/// Doubles from 10 ms to a 1 s cap, so a descriptor shortage does not spin a
/// core.
fn accept_backoff(n: u32) -> Duration {
    Duration::from_millis(10u64 << n.saturating_sub(1).min(7)).min(Duration::from_secs(1))
}

/// Why the server could not start or change folders.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("cannot serve {path}: {source}")]
    Root {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("cannot resolve host {host:?}: {reason}")]
    Host { host: String, reason: String },
    #[error(
        "tried {tried} ports from {first} but they were all in use; pass a different port to start from"
    )]
    PortsInUse { first: u16, tried: u16 },
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },
}

/// A page about to be sent, given to the HTML hook.
#[derive(Debug)]
pub struct HtmlPage<'a> {
    pub body: &'a str,
    pub status: u16,
    /// The server's own "Cannot GET" page, as opposed to a file from the
    /// output folder (which includes its `404.html`).
    pub is_builtin_error: bool,
    /// False for a `fetch` or other sub-request (`Sec-Fetch-Mode` is present
    /// and not `navigate`); the dev server leaves those alone.
    pub is_navigation: bool,
}

/// What the HTML hook returns: the body to send, and headers that replace any
/// header of the same name.
#[derive(Debug, Clone)]
pub struct HtmlRewrite {
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// Rewrites HTML responses. Called only for responses whose content type is
/// `text/html`, never for ranged responses.
pub type HtmlHook = dyn for<'a> Fn(&HtmlPage<'a>) -> HtmlRewrite + Send + Sync;

/// A request, as a [`Handler`] sees it.
pub struct Incoming<'a>(&'a http::Request);

impl Incoming<'_> {
    /// The method, as sent.
    pub fn method(&self) -> &str {
        &self.0.method
    }

    /// The path, still percent-encoded, without the query or fragment.
    /// `None` for a target that isn't a path.
    pub fn path(&self) -> Option<&str> {
        self.0.path()
    }

    /// A header's value; `name` is matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.0.header(&name.to_ascii_lowercase())
    }
}

/// A response from a [`Handler`].
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What a [`Handler`] does with a request it claims.
pub enum Handled {
    /// Send this response.
    Reply(Reply),
    /// Accept the request as a WebSocket and give it to the function, which is
    /// called on the connection's thread and should return promptly (keep the
    /// [`WebSocket`] to send on it later). The server then reads the socket
    /// until it closes, answering pings. A request that isn't a valid upgrade
    /// gets a 426 (403 when its `Origin` is another site), and one past
    /// [`Limits::max_sockets`] a 503.
    WebSocket(Box<dyn FnOnce(WebSocket) + Send>),
}

/// Answers requests before files are looked at. Returning `None` leaves the
/// request to the file server. Whatever paths a handler claims hide files of
/// the same name, so it should claim only a reserved prefix.
pub type Handler = dyn for<'a> Fn(&Incoming<'a>) -> Option<Handled> + Send + Sync;

struct Shared {
    root: RwLock<Arc<Root>>,
    hook: RwLock<Option<Arc<HtmlHook>>>,
    handler: RwLock<Option<Arc<Handler>>>,
    stopping: AtomicBool,
    conns: Mutex<HashMap<u64, TcpStream>>,
    next_conn: AtomicU64,
    active: AtomicUsize,
    /// Open WebSockets; these don't count in `active`.
    sockets: AtomicUsize,
    /// Refused connections still draining their input.
    lingering: AtomicUsize,
    limits: Limits,
    /// Bound to a loopback address: only requests naming this machine are answered.
    loopback_only: bool,
    /// The host the server was asked to bind, also accepted as a `Host`.
    host: String,
}

/// Removes a connection from the books when its thread ends.
struct ConnGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.shared
            .conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.shared.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A stream whose reads stop at a deadline, however the bytes trickle in.
struct DeadlineStream {
    stream: TcpStream,
    deadline: Option<Instant>,
}

impl ws::Deadline for BufReader<DeadlineStream> {
    fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.get_mut().deadline = deadline;
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.deadline {
            Some(d) => {
                let left = d
                    .checked_duration_since(Instant::now())
                    .filter(|l| !l.is_zero())
                    .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
                self.stream.set_read_timeout(Some(left))?;
            }
            None => self.stream.set_read_timeout(None)?,
        }
        self.stream.read(buf)
    }
}

/// Entry point: [`Server::start`].
pub struct Server;

impl Server {
    /// Serve `output` on `host`, starting from `port`. When `port` is in use
    /// the next ones are tried, up to [`PORT_RETRIES`] more, as the dev server
    /// does; port 0 asks the OS for any free port.
    ///
    /// `host` is `localhost` (the loopback address) by default; pass an
    /// address such as `0.0.0.0` to let other machines connect.
    pub fn start(output: &Path, host: &str, port: u16) -> Result<RunningServer, ServeError> {
        Server::start_with_limits(output, host, port, Limits::default())
    }

    /// [`Server::start`] with explicit [`Limits`].
    pub fn start_with_limits(
        output: &Path,
        host: &str,
        port: u16,
        limits: Limits,
    ) -> Result<RunningServer, ServeError> {
        let root = Root::new(output).map_err(|source| ServeError::Root {
            path: output.display().to_string(),
            source,
        })?;
        let ip = resolve_host(host, port)?;
        let (listener, addr) = bind(ip, port)?;
        let shared = Arc::new(Shared {
            root: RwLock::new(Arc::new(root)),
            hook: RwLock::new(None),
            handler: RwLock::new(None),
            stopping: AtomicBool::new(false),
            conns: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(0),
            active: AtomicUsize::new(0),
            sockets: AtomicUsize::new(0),
            lingering: AtomicUsize::new(0),
            limits,
            loopback_only: addr.ip().is_loopback(),
            host: host.to_ascii_lowercase(),
        });
        let accept_shared = Arc::clone(&shared);
        let accept = std::thread::Builder::new()
            .name("brrrty-serve-accept".into())
            .spawn(move || accept_loop(listener, accept_shared))
            .map_err(|source| ServeError::Bind { addr, source })?;
        Ok(RunningServer {
            shared,
            addr,
            requested_port: port,
            host: host.to_string(),
            accept: Some(accept),
        })
    }
}

fn resolve_host(host: &str, port: u16) -> Result<std::net::IpAddr, ServeError> {
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(std::net::Ipv4Addr::LOCALHOST.into());
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse() {
        return Ok(ip);
    }
    (bare, port)
        .to_socket_addrs()
        .map_err(|e| ServeError::Host {
            host: host.to_string(),
            reason: e.to_string(),
        })?
        .next()
        .map(|a| a.ip())
        .ok_or_else(|| ServeError::Host {
            host: host.to_string(),
            reason: "no addresses".into(),
        })
}

fn bind(ip: std::net::IpAddr, port: u16) -> Result<(TcpListener, SocketAddr), ServeError> {
    let retries = if port == 0 { 0 } else { PORT_RETRIES };
    let mut tried = 0u16;
    loop {
        let Some(p) = port.checked_add(tried).filter(|_| tried <= retries) else {
            return Err(ServeError::PortsInUse { first: port, tried });
        };
        let addr = SocketAddr::new(ip, p);
        match TcpListener::bind(addr) {
            Ok(l) => {
                let local = l.local_addr().unwrap_or(addr);
                return Ok((l, local));
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                if retries == 0 {
                    return Err(ServeError::PortsInUse {
                        first: port,
                        tried: 1,
                    });
                }
                tried += 1;
            }
            Err(source) => return Err(ServeError::Bind { addr, source }),
        }
    }
}

/// A running server. Dropping it stops it and releases the port.
pub struct RunningServer {
    shared: Arc<Shared>,
    addr: SocketAddr,
    requested_port: u16,
    host: String,
    accept: Option<JoinHandle<()>>,
}

impl RunningServer {
    /// The address the server is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The port actually bound, which differs from the one asked for when
    /// that was in use.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Whether the server had to move to another port.
    pub fn port_changed(&self) -> bool {
        self.requested_port != 0 && self.requested_port != self.addr.port()
    }

    /// The base URL, such as `http://localhost:8080/`, using the host name
    /// that was asked for.
    pub fn url(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("http://{host}:{}/", self.addr.port())
    }

    /// Serve a different folder from now on. Requests already running finish
    /// against the folder they started with. On error the old folder stays.
    ///
    /// The server holds the folder's path, not an open handle. Swap in a new
    /// directory per build and remove the old one after a short delay, rather
    /// than renaming over the folder being served; a request that has
    /// resolved a file in a directory that vanishes gets a 404.
    pub fn set_root(&self, output: &Path) -> Result<(), ServeError> {
        self.swap_root(output).map(drop)
    }

    /// [`RunningServer::set_root`], giving back the folder that was being
    /// served so the caller can tell when the last request using it is done
    /// ([`RetiredRoot::in_use`]) before removing it.
    pub fn swap_root(&self, output: &Path) -> Result<RetiredRoot, ServeError> {
        let root = Root::new(output).map_err(|source| ServeError::Root {
            path: output.display().to_string(),
            source,
        })?;
        let old = std::mem::replace(
            &mut *self.shared.root.write().unwrap_or_else(|e| e.into_inner()),
            Arc::new(root),
        );
        Ok(RetiredRoot(old))
    }

    /// The folder being served, as its real path.
    pub fn root(&self) -> std::path::PathBuf {
        self.shared
            .root
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .canonical
            .clone()
    }

    /// Rewrite HTML responses with `hook`, or stop doing so with `None`.
    pub fn set_html_hook(&self, hook: Option<Arc<HtmlHook>>) {
        *self.shared.hook.write().unwrap_or_else(|e| e.into_inner()) = hook;
    }

    /// Let `handler` see every request before the file server does, or stop
    /// doing so with `None`.
    pub fn set_handler(&self, handler: Option<Arc<Handler>>) {
        *self
            .shared
            .handler
            .write()
            .unwrap_or_else(|e| e.into_inner()) = handler;
    }

    /// Stop accepting, close open connections, and release the port.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        if let Some(handle) = self.accept.take() {
            // Wake the accept loop so it sees the flag.
            let wake = if self.addr.ip().is_unspecified() {
                SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), self.addr.port())
            } else {
                self.addr
            };
            let _ = TcpStream::connect_timeout(&wake, Duration::from_secs(1));
            let _ = handle.join();
        }
        let conns =
            std::mem::take(&mut *self.shared.conns.lock().unwrap_or_else(|e| e.into_inner()));
        for (_, c) in conns {
            let _ = c.shutdown(Shutdown::Both);
        }
    }
}

/// A folder the server stopped serving ([`RunningServer::swap_root`]).
pub struct RetiredRoot(Arc<Root>);

impl RetiredRoot {
    /// The folder, as its real path.
    pub fn path(&self) -> &Path {
        &self.0.canonical
    }

    /// Whether a request that started before the swap still holds the
    /// folder. A request holds it while it finds its file; once the file is
    /// open, removing the folder no longer affects the response on Unix.
    pub fn in_use(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    let mut errors = 0u32;
    for stream in listener.incoming() {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let mut stream = match stream {
            Ok(s) => {
                errors = 0;
                s
            }
            Err(_) => {
                errors = errors.saturating_add(1);
                std::thread::sleep(accept_backoff(errors));
                continue;
            }
        };
        if shared.active.fetch_add(1, Ordering::SeqCst) >= shared.limits.max_connections {
            shared.active.fetch_sub(1, Ordering::SeqCst);
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            let resp = plain_error(503).header("Retry-After", "1");
            if http::write_response(&mut stream, resp, false, false).is_err() {
                continue;
            }
            // Linger off the accept thread, so refusing never slows accepting; the
            // number lingering is capped, past which the close is immediate.
            if shared.lingering.fetch_add(1, Ordering::SeqCst) < MAX_LINGERING {
                let linger_shared = Arc::clone(&shared);
                let spawned = std::thread::Builder::new()
                    .name("brrrty-serve-linger".into())
                    .spawn(move || {
                        lingering_close(&stream);
                        linger_shared.lingering.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    shared.lingering.fetch_sub(1, Ordering::SeqCst);
                }
            } else {
                shared.lingering.fetch_sub(1, Ordering::SeqCst);
            }
            continue;
        }
        let id = shared.next_conn.fetch_add(1, Ordering::Relaxed);
        if let Ok(clone) = stream.try_clone() {
            shared
                .conns
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, clone);
        }
        let guard = ConnGuard {
            shared: Arc::clone(&shared),
            id,
        };
        let conn_shared = Arc::clone(&shared);
        // If the spawn fails the closure, and the guard in it, is dropped.
        let _ = std::thread::Builder::new()
            .name("brrrty-serve-conn".into())
            .spawn(move || {
                let _guard = guard;
                handle_connection(stream, &conn_shared);
            });
    }
}

fn handle_connection(stream: TcpStream, shared: &Shared) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(DeadlineStream {
        stream,
        deadline: None,
    });
    loop {
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        // Wait for the first byte of the next request, then hold the client
        // to one deadline for the whole head.
        reader.get_mut().deadline = Some(Instant::now() + shared.limits.idle_timeout);
        match std::io::BufRead::fill_buf(&mut reader) {
            Ok([]) | Err(_) => return,
            Ok(_) => {}
        }
        reader.get_mut().deadline = Some(Instant::now() + shared.limits.head_timeout);
        let req = match http::read_request(&mut reader) {
            Ok(r) => r,
            Err(ParseError::Closed) => return,
            Err(ParseError::Reject(status)) => {
                let resp = plain_error(status);
                if http::write_response(&mut writer, resp, false, false).is_ok() {
                    lingering_close(&writer);
                }
                return;
            }
        };
        reader.get_mut().deadline = None;
        let keep_alive = req.keep_alive();
        let head_only = req.method.eq_ignore_ascii_case("HEAD");
        if shared.loopback_only && !host_allowed(req.header("host"), &shared.host) {
            // A page from elsewhere that resolved its own name to this machine (DNS
            // rebinding) names itself in Host; refuse it, so it can read nothing here.
            let resp = plain_error(403);
            if http::write_response(&mut writer, resp, head_only, false).is_ok() {
                lingering_close(&writer);
            }
            return;
        }
        let handler = shared
            .handler
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let handled = handler.and_then(|h| h(&Incoming(&req)));
        let resp = match handled {
            Some(Handled::WebSocket(start)) => {
                upgrade(&req, reader, writer, shared, start);
                return;
            }
            Some(Handled::Reply(r)) => {
                let mut resp = Response::new(r.status);
                for (n, v) in r.headers {
                    resp.set_header(&n, v);
                }
                resp.body(r.body)
            }
            None => route(&req, shared),
        };
        if http::write_response(&mut writer, resp, head_only, keep_alive).is_err() {
            return;
        }
        if !keep_alive {
            // The client may still be sending a body that is never read.
            lingering_close(&writer);
            return;
        }
    }
}

/// Gives a WebSocket's slot back to the connection count when it closes.
struct SocketSlot<'a>(&'a Shared);

impl Drop for SocketSlot<'_> {
    fn drop(&mut self) {
        self.0.sockets.fetch_sub(1, Ordering::SeqCst);
        // The connection's own guard takes this back off when its thread ends.
        self.0.active.fetch_add(1, Ordering::SeqCst);
    }
}

/// Take the connection over as a WebSocket, or refuse the upgrade.
fn upgrade(
    req: &Request,
    mut reader: BufReader<DeadlineStream>,
    mut writer: TcpStream,
    shared: &Shared,
    start: Box<dyn FnOnce(WebSocket) + Send>,
) {
    let refuse = |writer: &mut TcpStream, resp: Response| {
        if http::write_response(writer, resp, false, false).is_ok() {
            lingering_close(writer);
        }
    };
    let key = match ws::check_upgrade(req) {
        Ok(key) => ws::accept_key(key),
        Err(status) => {
            let mut resp = plain_error(status);
            if status == 426 {
                resp.set_header("Upgrade", "websocket");
                resp.set_header("Sec-WebSocket-Version", "13");
            }
            return refuse(&mut writer, resp);
        }
    };
    if shared.sockets.fetch_add(1, Ordering::SeqCst) >= shared.limits.max_sockets {
        shared.sockets.fetch_sub(1, Ordering::SeqCst);
        return refuse(&mut writer, plain_error(503).header("Retry-After", "1"));
    }
    shared.active.fetch_sub(1, Ordering::SeqCst);
    let _slot = SocketSlot(shared);
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {key}\r\n\r\n"
    );
    if io::Write::write_all(&mut writer, head.as_bytes()).is_err() {
        return;
    }
    let socket = WebSocket::new(writer);
    start(socket.clone());
    ws::serve(
        &mut reader,
        &socket,
        shared.limits.ping_every,
        &shared.stopping,
    );
}

/// Whether a `Host` header names this machine: `localhost` (or a name under it), `127.0.0.1`,
/// `[::1]`, or the host the server was bound to, with any port. A request without one (only
/// possible over HTTP/1.0 or from a hand-made client, never from a browser) is let through.
fn host_allowed(header: Option<&str>, bound: &str) -> bool {
    let Some(value) = header else {
        return true;
    };
    let value = value.trim().to_ascii_lowercase();
    let name = if value.starts_with('[') {
        match value.find(']') {
            Some(end) => &value[..=end],
            None => return false,
        }
    } else {
        value.split(':').next().unwrap_or("")
    };
    let name = name.trim_end_matches('.');
    name == "localhost"
        || name.ends_with(".localhost")
        || name == "127.0.0.1"
        || name == "[::1]"
        || name == bound
        || format!("[{bound}]") == name
}

fn plain_error(status: u16) -> Response {
    Response::new(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(format!("{status}\n").into_bytes())
}

fn route(req: &Request, shared: &Shared) -> Response {
    let Some(path) = req.path() else {
        return plain_error(400);
    };
    let root = Arc::clone(&shared.root.read().unwrap_or_else(|e| e.into_inner()));
    match path::resolve(&root, path) {
        Resolved::File(found) => file_response(req, shared, &found, 200),
        Resolved::Redirect(location) => Response::new(301).header("Location", location),
        Resolved::BadRequest => plain_error(400),
        Resolved::NotFound => match path::not_found_page(&root) {
            Some(found) => file_response(req, shared, &found, 404),
            None => builtin_error_page(req, shared, path),
        },
    }
}

fn is_navigation(req: &Request) -> bool {
    req.header("sec-fetch-mode").is_none_or(|m| m == "navigate")
}

fn current_hook(shared: &Shared) -> Option<Arc<HtmlHook>> {
    shared
        .hook
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn file_response(req: &Request, shared: &Shared, found: &Found, status: u16) -> Response {
    let content_type = mime::content_type(&found.name);
    let open = || File::open(&found.real).and_then(|f| f.metadata().map(|m| (f, m)));
    // The file can vanish between resolving it and opening it (a rebuild).
    let Ok((mut file, meta)) = open() else {
        return plain_error(404);
    };
    let size = meta.len();

    if status == 200
        && let Some(range) = req.header("range")
    {
        match parse_range(range, size) {
            RangeOutcome::Full => {}
            RangeOutcome::Unsatisfiable => {
                return Response::new(416)
                    .header("Content-Range", format!("bytes */{size}"))
                    .header("Content-Type", "text/plain; charset=utf-8")
                    .body(b"Range Not Satisfiable".to_vec());
            }
            RangeOutcome::Bytes(start, end) => {
                if file.seek(SeekFrom::Start(start)).is_err() {
                    return plain_error(500);
                }
                let mut resp = Response::new(206)
                    .header("Accept-Ranges", "bytes")
                    .header("Cache-Control", "public, max-age=0");
                if let Ok(m) = meta.modified() {
                    resp.set_header("Last-Modified", http::http_date(m));
                    let ms = m.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
                    resp.set_header("ETag", format!("W/\"{size:x}-{ms:x}\""));
                }
                if let Some(ct) = &content_type {
                    let text = ct.starts_with("text/")
                        || ct == "application/javascript"
                        || ct == "application/json";
                    let ct = if text && !ct.contains("charset") {
                        format!("{ct}; charset=utf-8")
                    } else {
                        ct.clone()
                    };
                    resp.set_header("Content-Type", ct);
                }
                resp.set_header("Content-Range", format!("bytes {start}-{end}/{size}"));
                resp.body = Body::Stream(Box::new(file), end - start + 1);
                return resp;
            }
        }
    }

    let mut resp = Response::new(status);
    if let Some(ct) = &content_type {
        resp.set_header("Content-Type", ct.clone());
    }
    if content_type.as_deref().is_some_and(mime::is_html) {
        let mut bytes = Vec::with_capacity(size as usize);
        if file.read_to_end(&mut bytes).is_err() {
            return plain_error(500);
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        return html_response(req, shared, resp, text, false);
    }
    resp.body = Body::Stream(Box::new(file), size);
    resp
}

/// Finish an HTML response, running the hook if there is one.
fn html_response(
    req: &Request,
    shared: &Shared,
    mut resp: Response,
    text: String,
    is_builtin_error: bool,
) -> Response {
    let body = match current_hook(shared) {
        Some(hook) => {
            let page = HtmlPage {
                body: &text,
                status: resp.status,
                is_builtin_error,
                is_navigation: is_navigation(req),
            };
            let rewrite = hook(&page);
            for (n, v) in rewrite.headers {
                resp.set_header(&n, v);
            }
            rewrite.body
        }
        None => text,
    };
    resp.body = Body::Bytes(body.into_bytes());
    resp
}

/// The page the dev server's request handler (`finalhandler`) shows when
/// nothing matched and there is no `404.html`.
fn builtin_error_page(req: &Request, shared: &Shared, path: &str) -> Response {
    let resource = escape_for_page(path);
    let text = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>Cannot {} {}</pre>\n</body>\n</html>\n",
        escape_for_page(&req.method),
        resource
    );
    let resp = Response::new(404)
        .header("Content-Security-Policy", "default-src 'none'")
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Type", "text/html; charset=utf-8");
    html_response(req, shared, resp, text, true)
}

/// HTML-escape, then percent-encode what a URL cannot carry (non-ASCII,
/// spaces, controls), leaving existing `%XX` escapes alone.
fn escape_for_page(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_ascii() && !c.is_ascii_control() && c != ' ' => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum RangeOutcome {
    /// Not a single satisfiable `bytes` range; send the whole file.
    Full,
    Unsatisfiable,
    /// Inclusive start and end.
    Bytes(u64, u64),
}

fn parse_range(header: &str, size: u64) -> RangeOutcome {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return RangeOutcome::Full;
    };
    if spec.contains(',') {
        return RangeOutcome::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return RangeOutcome::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    let parsed = match (a.is_empty(), b.is_empty()) {
        (true, true) => return RangeOutcome::Full,
        (true, false) => {
            let Ok(n) = b.parse::<u64>() else {
                return RangeOutcome::Full;
            };
            if n == 0 || size == 0 {
                return RangeOutcome::Unsatisfiable;
            }
            (size.saturating_sub(n), size - 1)
        }
        (false, empty_end) => {
            let Ok(start) = a.parse::<u64>() else {
                return RangeOutcome::Full;
            };
            let end = if empty_end {
                u64::MAX
            } else {
                let Ok(e) = b.parse::<u64>() else {
                    return RangeOutcome::Full;
                };
                e
            };
            if end < start {
                return RangeOutcome::Full;
            }
            if start >= size {
                return RangeOutcome::Unsatisfiable;
            }
            (start, end.min(size - 1))
        }
    };
    RangeOutcome::Bytes(parsed.0, parsed.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-2", 10), RangeOutcome::Bytes(0, 2));
        assert_eq!(parse_range("bytes=5-", 10), RangeOutcome::Bytes(5, 9));
        assert_eq!(parse_range("bytes=-3", 10), RangeOutcome::Bytes(7, 9));
        assert_eq!(parse_range("bytes=5-500", 10), RangeOutcome::Bytes(5, 9));
        assert_eq!(parse_range("bytes=10-12", 10), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), RangeOutcome::Full);
        assert_eq!(parse_range("items=0-1", 10), RangeOutcome::Full);
        assert_eq!(parse_range("bytes=4-2", 10), RangeOutcome::Full);
    }

    #[test]
    fn accept_errors_back_off_to_a_cap() {
        assert_eq!(accept_backoff(1), Duration::from_millis(10));
        assert_eq!(accept_backoff(2), Duration::from_millis(20));
        assert_eq!(accept_backoff(8), Duration::from_millis(1000));
        assert_eq!(accept_backoff(u32::MAX), Duration::from_secs(1));
    }

    #[test]
    fn hosts_naming_this_machine_are_allowed() {
        for ok in [
            None,
            Some("localhost"),
            Some("localhost:8080"),
            Some("LocalHost:1"),
            Some("app.localhost:8080"),
            Some("127.0.0.1:8080"),
            Some("127.0.0.1"),
            Some("[::1]:8080"),
            Some("[::1]"),
            Some("localhost.:8080"),
        ] {
            assert!(host_allowed(ok, "localhost"), "{ok:?}");
        }
        for bad in [
            Some("evil.example:8080"),
            Some("localhost.evil.example"),
            Some("127.0.0.2:8080"),
            Some("[::2]:8080"),
            Some("[::1"),
            Some(""),
        ] {
            assert!(!host_allowed(bad, "localhost"), "{bad:?}");
        }
        assert!(host_allowed(Some("127.0.0.2:80"), "127.0.0.2"));
        assert!(host_allowed(Some("[::1]:80"), "::1"));
    }

    #[test]
    fn error_page_text_is_escaped() {
        assert_eq!(escape_for_page("/<a>&\"é"), "/&lt;a&gt;&amp;&quot;%C3%A9");
    }
}
