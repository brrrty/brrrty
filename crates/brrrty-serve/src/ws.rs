//! The server side of WebSocket (RFC 6455), as much as a live-reload channel needs: the
//! handshake, text messages out, and close, ping and pong in. Messages from the browser are
//! read and dropped.

use std::io::{self, BufRead, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::http::Request;

/// The GUID RFC 6455 appends to the client's key.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The largest frame accepted from a browser. The channel only carries messages out, so
/// anything bigger is a misbehaving client.
const MAX_FRAME: u64 = 64 * 1024;

/// A write to one socket that makes no progress for this long fails, so one stuck tab can't
/// hold up messages to the others for long.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// The `Sec-WebSocket-Accept` value for a client's `Sec-WebSocket-Key`.
pub(crate) fn accept_key(key: &str) -> String {
    let mut input = key.trim().as_bytes().to_vec();
    input.extend_from_slice(GUID.as_bytes());
    base64(&sha1(&input))
}

/// Why an upgrade request is refused, as a status.
pub(crate) fn check_upgrade(req: &Request) -> Result<&str, u16> {
    let has_token = |name: &str, token: &str| {
        req.header(name)
            .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    };
    if !req.method.eq_ignore_ascii_case("GET")
        || !req.http11
        || !has_token("upgrade", "websocket")
        || !has_token("connection", "upgrade")
        || req.header("sec-websocket-version") != Some("13")
    {
        return Err(426);
    }
    let key = req.header("sec-websocket-key").ok_or(400u16)?;
    // A browser on another site may open a socket here; only pages from this server may.
    if let Some(origin) = req.header("origin") {
        let host = req.header("host").unwrap_or("");
        let origin_host = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .unwrap_or(origin);
        if host.is_empty() || !origin_host.eq_ignore_ascii_case(host) {
            return Err(403);
        }
    }
    Ok(key)
}

struct Inner {
    stream: Mutex<TcpStream>,
    /// Another handle on the connection, for ending it without waiting on a write in
    /// progress.
    control: Option<TcpStream>,
    open: AtomicBool,
}

/// An open WebSocket to a browser. Clones share the connection. Sending after the browser
/// has gone fails, and [`WebSocket::is_open`] turns false.
#[derive(Clone)]
pub struct WebSocket(Arc<Inner>);

impl std::fmt::Debug for WebSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocket")
            .field("open", &self.is_open())
            .finish()
    }
}

impl WebSocket {
    pub(crate) fn new(stream: TcpStream) -> WebSocket {
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let control = stream.try_clone().ok();
        WebSocket(Arc::new(Inner {
            stream: Mutex::new(stream),
            control,
            open: AtomicBool::new(true),
        }))
    }

    /// Send a text message.
    pub fn send_text(&self, text: &str) -> io::Result<()> {
        self.send(0x1, text.as_bytes())
    }

    /// Whether the connection is still up, as far as the server knows.
    pub fn is_open(&self) -> bool {
        self.0.open.load(Ordering::SeqCst)
    }

    /// End the connection, with a close frame when no other send is under way. Never waits on
    /// a send in progress, so it is safe to call on a peer that stopped reading.
    pub fn close(&self) {
        if self.is_open()
            && let Ok(mut s) = self.0.stream.try_lock()
        {
            let _ = s.write_all(&frame(0x8, &1001u16.to_be_bytes()));
        }
        self.end();
    }

    pub(crate) fn end(&self) {
        self.0.open.store(false, Ordering::SeqCst);
        match &self.0.control {
            Some(c) => {
                let _ = c.shutdown(Shutdown::Both);
            }
            None => {
                if let Ok(s) = self.0.stream.lock() {
                    let _ = s.shutdown(Shutdown::Both);
                }
            }
        }
    }

    fn send(&self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        if !self.is_open() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        let frame = frame(opcode, payload);
        let mut s = self.0.stream.lock().unwrap_or_else(|e| e.into_inner());
        let sent = s.write_all(&frame).and_then(|()| s.flush());
        if sent.is_err() {
            drop(s);
            self.end();
        }
        sent
    }
}

/// An unmasked frame, as a server sends it.
fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 10);
    f.push(0x80 | opcode);
    let n = payload.len();
    if n < 126 {
        f.push(n as u8);
    } else if n <= u16::MAX as usize {
        f.push(126);
        f.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        f.push(127);
        f.extend_from_slice(&(n as u64).to_be_bytes());
    }
    f.extend_from_slice(payload);
    f
}

/// What [`serve`] needs from the connection's reader: reads that stop at a deadline.
pub(crate) trait Deadline {
    fn set_deadline(&mut self, deadline: Option<Instant>);
}

/// Read frames until the browser closes, the connection drops, or the server stops. Pings
/// are answered, and a browser silent for two `ping_every` periods in a row, even to a ping
/// of ours, is taken as gone.
pub(crate) fn serve<R: BufRead + Deadline>(
    reader: &mut R,
    ws: &WebSocket,
    ping_every: Duration,
    stopping: &AtomicBool,
) {
    let mut awaiting_pong = false;
    // Inside a fragmented data message, which continuation frames carry on.
    let mut fragmented = false;
    while ws.is_open() && !stopping.load(Ordering::SeqCst) {
        reader.set_deadline(Some(Instant::now() + ping_every));
        match reader.fill_buf() {
            Ok([]) => break,
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                if awaiting_pong || ws.send(0x9, b"").is_err() {
                    break;
                }
                awaiting_pong = true;
                continue;
            }
            Err(_) => break,
        }
        awaiting_pong = false;
        // A frame that has started must arrive whole, soon.
        reader.set_deadline(Some(Instant::now() + Duration::from_secs(10)));
        let frame = read_frame(reader).and_then(|(fin, opcode, payload)| {
            // RFC 6455 5.4: a continuation needs a message to continue, and a new data
            // message can't start inside another.
            match opcode {
                0x0 if !fragmented => return Err(Some(1002)),
                0x1 | 0x2 if fragmented => return Err(Some(1002)),
                0x0..=0x2 => fragmented = !fin,
                _ => {}
            }
            Ok((opcode, payload))
        });
        match frame {
            Ok((0x8, payload)) => {
                // Echo the status code, as the closing handshake asks.
                let code = payload.get(..2).unwrap_or(&[0x03, 0xe8]).to_vec();
                let _ = ws.send(0x8, &code);
                break;
            }
            Ok((0x9, payload)) => {
                if ws.send(0xA, &payload).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(code) => {
                if let Some(code) = code {
                    let _ = ws.send(0x8, &code.to_be_bytes());
                }
                break;
            }
        }
    }
    ws.end();
}

/// One frame from the browser: whether it is final, its opcode and its unmasked payload. On
/// error, the close code to send, if the connection is still worth one.
fn read_frame<R: Read>(r: &mut R) -> Result<(bool, u8, Vec<u8>), Option<u16>> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head).map_err(|_| None)?;
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0f;
    let control = opcode & 0x8 != 0;
    // Opcodes 3-7 and B-F are reserved: the connection must fail (RFC 6455 5.2).
    if !matches!(opcode, 0x0..=0x2 | 0x8..=0xA) {
        return Err(Some(1002));
    }
    // Control frames are never fragmented and carry at most 125 bytes (5.5).
    if control && (!fin || head[1] & 0x7f > 125) {
        return Err(Some(1002));
    }
    if head[0] & 0x70 != 0 {
        // No extensions were agreed, so reserved bits must be clear.
        return Err(Some(1002));
    }
    if head[1] & 0x80 == 0 {
        // Browsers must mask what they send.
        return Err(Some(1002));
    }
    let len = match head[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b).map_err(|_| None)?;
            u64::from(u16::from_be_bytes(b))
        }
        127 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b).map_err(|_| None)?;
            u64::from_be_bytes(b)
        }
        n => u64::from(n),
    };
    if len > MAX_FRAME {
        return Err(Some(1009));
    }
    let mut mask = [0u8; 4];
    r.read_exact(&mut mask).map_err(|_| None)?;
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).map_err(|_| None)?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Ok((fin, opcode, payload))
}

/// SHA-1, for the handshake only (RFC 3174).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handshake_key_matches_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn sha1_and_base64_known_answers() {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let long = vec![b'a'; 1000];
        assert_eq!(
            hex(&sha1(&long)),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn frames_use_the_right_length_form() {
        assert_eq!(frame(0x1, b"hi"), [0x81, 2, b'h', b'i']);
        let mid = frame(0x1, &[0; 300]);
        assert_eq!(&mid[..4], &[0x81, 126, 1, 44]);
        let big = frame(0x1, &[0; 70_000]);
        assert_eq!(&big[..2], &[0x81, 127]);
        assert_eq!(u64::from_be_bytes(big[2..10].try_into().unwrap()), 70_000);
    }

    /// A frame as a browser sends it: masked, with the length in the form its size needs.
    fn client_frame(first: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut out = vec![first];
        match payload.len() {
            n @ 0..=125 => out.push(0x80 | n as u8),
            n @ 126..=0xffff => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    fn read(bytes: &[u8]) -> Result<(bool, u8, Vec<u8>), Option<u16>> {
        read_frame(&mut std::io::Cursor::new(bytes))
    }

    #[test]
    fn masked_frames_of_every_length_form_are_unmasked() {
        // RFC 6455 5.7's masked "Hello".
        let hello = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        assert_eq!(read(&hello), Ok((true, 0x1, b"Hello".to_vec())));
        let mid: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        assert_eq!(
            read(&client_frame(0x02, &mid)),
            Ok((false, 0x2, mid.clone()))
        );
        let big = vec![b'x'; MAX_FRAME as usize];
        assert_eq!(
            read(&client_frame(0x82, &big)),
            Ok((true, 0x2, big.clone()))
        );
        assert_eq!(read(&client_frame(0x89, b"")), Ok((true, 0x9, Vec::new())));
    }

    #[test]
    fn protocol_violations_get_1002() {
        // Reserved opcodes, data and control.
        assert_eq!(read(&client_frame(0x83, b"x")), Err(Some(1002)));
        assert_eq!(read(&client_frame(0x8b, b"x")), Err(Some(1002)));
        // A fragmented ping, and a ping over 125 bytes.
        assert_eq!(read(&client_frame(0x09, b"x")), Err(Some(1002)));
        assert_eq!(read(&client_frame(0x89, &[0; 126])), Err(Some(1002)));
        // A reserved bit with no extension agreed.
        assert_eq!(read(&client_frame(0xc1, b"x")), Err(Some(1002)));
        // An unmasked frame.
        assert_eq!(read(&[0x81, 0x01, b'x']), Err(Some(1002)));
    }

    #[test]
    fn oversized_frames_get_1009_before_anything_is_read() {
        let mut head = vec![0x82, 0x80 | 127];
        head.extend_from_slice(&(MAX_FRAME + 1).to_be_bytes());
        assert_eq!(read(&head), Err(Some(1009)));
    }

    #[test]
    fn a_frame_cut_short_closes_without_a_code() {
        let whole = client_frame(0x81, &[b'y'; 300]);
        for cut in [0, 1, 3, 6, whole.len() - 1] {
            assert_eq!(read(&whole[..cut]), Err(None), "cut at {cut}");
        }
    }

    fn upgrade(extra: &str) -> Result<String, u16> {
        let raw = format!(
            "GET /.reload HTTP/1.1\r\nHost: localhost:8080\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
        );
        let req = crate::http::read_request(&mut std::io::BufReader::new(raw.as_bytes()))
            .map_err(|_| 0u16)?;
        check_upgrade(&req).map(str::to_string)
    }

    #[test]
    fn upgrades_need_the_handshake_headers_and_a_matching_origin() {
        let key = "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
        assert_eq!(upgrade(key).as_deref(), Ok("dGhlIHNhbXBsZSBub25jZQ=="));
        assert!(upgrade(&format!("{key}Origin: http://localhost:8080\r\n")).is_ok());
        assert!(upgrade(&format!("{key}Origin: https://LOCALHOST:8080\r\n")).is_ok());
        assert_eq!(upgrade(""), Err(400));
        assert_eq!(
            upgrade(&format!("{key}Origin: http://evil.example\r\n")),
            Err(403)
        );
        assert_eq!(
            upgrade(&format!("{key}Origin: http://localhost:8081\r\n")),
            Err(403)
        );
        let raw = "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 8\r\nSec-WebSocket-Key: a\r\n\r\n";
        let req = crate::http::read_request(&mut std::io::BufReader::new(raw.as_bytes())).unwrap();
        assert_eq!(check_upgrade(&req), Err(426));
    }
}
