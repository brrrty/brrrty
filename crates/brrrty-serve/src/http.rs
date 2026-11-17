//! The small slice of HTTP/1.1 the server speaks: parse a request head and
//! write a response.

use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_LINE: usize = 16 * 1024;
const MAX_HEADERS: usize = 100;

#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) method: String,
    /// The target as sent, such as `/docs/?a=b`.
    pub(crate) target: String,
    pub(crate) http11: bool,
    /// Header names are lowercased.
    pub(crate) headers: Vec<(String, String)>,
}

impl Request {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the connection can carry another request after this one.
    pub(crate) fn keep_alive(&self) -> bool {
        if !self.http11 {
            return false;
        }
        let close = self
            .header("connection")
            .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
        // A request body is never read, so the stream cannot be reused.
        let has_body = self.header("transfer-encoding").is_some()
            || self
                .header("content-length")
                .is_some_and(|v| v.trim() != "0");
        !close && !has_body
    }

    /// The path part of the target, without the query or fragment. `None`
    /// when the target is not an origin-form or absolute-form URL.
    pub(crate) fn path(&self) -> Option<&str> {
        let t = self.target.as_str();
        let t = if t.starts_with('/') {
            t
        } else {
            let rest = t
                .strip_prefix("http://")
                .or_else(|| t.strip_prefix("https://"))?;
            &rest[rest.find('/')?..]
        };
        let end = t.find(['?', '#']).unwrap_or(t.len());
        Some(&t[..end])
    }
}

#[derive(Debug)]
pub(crate) enum ParseError {
    /// The peer closed or timed out before sending a request.
    Closed,
    /// Send this status and close.
    Reject(u16),
}

fn read_line<R: BufRead>(r: &mut R) -> Result<Vec<u8>, ParseError> {
    let mut buf = Vec::new();
    let n = r
        .by_ref()
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(|e| match e.kind() {
            // The head took longer than its budget to arrive.
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => ParseError::Reject(408),
            _ => ParseError::Closed,
        })?;
    if n == 0 {
        return Err(ParseError::Closed);
    }
    if buf.len() > MAX_LINE {
        return Err(ParseError::Reject(431));
    }
    if buf.ends_with(b"\n") {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
    } else {
        // The peer closed in the middle of a line.
        return Err(ParseError::Closed);
    }
    Ok(buf)
}

pub(crate) fn read_request<R: BufRead>(r: &mut R) -> Result<Request, ParseError> {
    let mut line = read_line(r)?;
    // Tolerate blank lines before a request line (RFC 9112 2.2).
    while line.is_empty() {
        line = read_line(r)?;
    }
    let line = String::from_utf8(line).map_err(|_| ParseError::Reject(400))?;
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::Reject(400));
    };
    if method.is_empty()
        || !method
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ParseError::Reject(400));
    }
    let http11 = match version {
        "HTTP/1.1" => true,
        "HTTP/1.0" => false,
        _ => return Err(ParseError::Reject(505)),
    };
    let mut headers = Vec::new();
    loop {
        let l = read_line(r)?;
        if l.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            return Err(ParseError::Reject(431));
        }
        let l = String::from_utf8_lossy(&l);
        let Some((name, value)) = l.split_once(':') else {
            return Err(ParseError::Reject(400));
        };
        if name.is_empty() || name.bytes().any(|b| b == b' ' || b == b'\t') {
            return Err(ParseError::Reject(400));
        }
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    // Two lengths, or a length next to Transfer-Encoding, are how a request
    // gets read differently by two parties. Refuse them rather than guess.
    let mut lengths = headers.iter().filter(|(n, _)| n == "content-length");
    let length = lengths.next();
    if lengths.next().is_some()
        || length.is_some_and(|(_, v)| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
        || (length.is_some() && headers.iter().any(|(n, _)| n == "transfer-encoding"))
    {
        return Err(ParseError::Reject(400));
    }
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        http11,
        headers,
    })
}

pub(crate) enum Body {
    Empty,
    Bytes(Vec<u8>),
    /// `len` bytes read from `reader`.
    Stream(Box<dyn Read + Send>, u64),
}

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Body,
}

impl Response {
    pub(crate) fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    pub(crate) fn header(mut self, name: &str, value: impl Into<String>) -> Response {
        self.set_header(name, value);
        self
    }

    /// Set a header, replacing any with the same name.
    pub(crate) fn set_header(&mut self, name: &str, value: impl Into<String>) {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.into()));
    }

    pub(crate) fn body(mut self, bytes: Vec<u8>) -> Response {
        self.body = Body::Bytes(bytes);
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        301 => "Moved Permanently",
        403 => "Forbidden",
        400 => "Bad Request",
        404 => "Not Found",
        408 => "Request Timeout",
        416 => "Range Not Satisfiable",
        426 => "Upgrade Required",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

pub(crate) fn write_response<W: Write>(
    w: &mut W,
    resp: Response,
    head_only: bool,
    keep_alive: bool,
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (n, v) in &resp.headers {
        // A header value never gets to carry a line break.
        let v: String = v.chars().filter(|c| *c != '\r' && *c != '\n').collect();
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    let len = match &resp.body {
        Body::Empty => 0,
        Body::Bytes(b) => b.len() as u64,
        Body::Stream(_, n) => *n,
    };
    head.push_str(&format!("Content-Length: {len}\r\n"));
    head.push_str(&format!("Date: {}\r\n", http_date(SystemTime::now())));
    head.push_str(if keep_alive {
        "Connection: keep-alive\r\nKeep-Alive: timeout=5\r\n"
    } else {
        "Connection: close\r\n"
    });
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    if !head_only {
        match resp.body {
            Body::Empty => {}
            Body::Bytes(b) => w.write_all(&b)?,
            Body::Stream(mut r, n) => {
                let copied = io::copy(&mut r.by_ref().take(n), w)?;
                if copied != n {
                    // The file shrank under us; the length was already sent.
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                }
            }
        }
    }
    w.flush()
}

/// An HTTP date, such as `Mon, 16 Nov 2026 19:22:49 GMT`.
pub(crate) fn http_date(t: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAYS[days.rem_euclid(7) as usize],
        d,
        MONTHS[(m - 1) as usize],
        y,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_format_as_http_dates() {
        assert_eq!(http_date(UNIX_EPOCH), "Thu, 01 Jan 1970 00:00:00 GMT");
        let t = UNIX_EPOCH + Duration::from_secs(1_794_856_969);
        assert_eq!(http_date(t), "Mon, 16 Nov 2026 19:22:49 GMT");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(http_date(leap), "Tue, 29 Feb 2000 00:00:00 GMT");
    }

    fn parse(raw: &[u8]) -> Result<Request, ParseError> {
        read_request(&mut io::BufReader::new(raw))
    }

    fn rejected(raw: &[u8]) -> Option<u16> {
        match parse(raw) {
            Err(ParseError::Reject(s)) => Some(s),
            _ => None,
        }
    }

    /// RFC 9112 2.2: blank lines before the request line are skipped; bare `\n` ends lines too.
    #[test]
    fn blank_lines_before_the_request_are_skipped() {
        let r = parse(b"\r\n\n\r\nGET /x HTTP/1.1\nHost: a\n\n").unwrap();
        assert_eq!((r.method.as_str(), r.target.as_str()), ("GET", "/x"));
        assert_eq!(r.header("host"), Some("a"));
    }

    #[test]
    fn a_peer_that_goes_away_is_closed_not_rejected() {
        for raw in [
            &b""[..],
            b"GET / HTTP/1.1",
            b"GET / HTTP/1.1\r\nHost: a\r\n",
        ] {
            assert!(matches!(parse(raw), Err(ParseError::Closed)), "{raw:?}");
        }
    }

    #[test]
    fn bad_header_lines_are_400() {
        assert_eq!(rejected(b"GET / HTTP/1.1\r\nno colon\r\n\r\n"), Some(400));
        assert_eq!(rejected(b"GET / HTTP/1.1\r\n: empty\r\n\r\n"), Some(400));
        // Whitespace before the colon is how request smuggling starts (RFC 9112 5.1).
        assert_eq!(rejected(b"GET / HTTP/1.1\r\nHost : a\r\n\r\n"), Some(400));
        assert_eq!(rejected(b"G\xffT / HTTP/1.1\r\n\r\n"), Some(400));
        assert_eq!(rejected(b"G@T / HTTP/1.1\r\n\r\n"), Some(400));
        assert_eq!(rejected(b"GET  / HTTP/1.1\r\n\r\n"), Some(400));
    }

    #[test]
    fn conflicting_lengths_are_400() {
        let two = b"GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\n";
        assert_eq!(rejected(two), Some(400));
        let both = b"GET / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert_eq!(rejected(both), Some(400));
        assert_eq!(
            rejected(b"GET / HTTP/1.1\r\nContent-Length: -1\r\n\r\n"),
            Some(400)
        );
        assert_eq!(
            rejected(b"GET / HTTP/1.1\r\nContent-Length:\r\n\r\n"),
            Some(400)
        );
    }

    #[test]
    fn more_than_the_header_limit_is_431() {
        let mut raw = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..=MAX_HEADERS {
            raw.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        assert_eq!(rejected(&raw), Some(431));
        let mut ok = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..MAX_HEADERS {
            ok.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        ok.extend_from_slice(b"\r\n");
        assert_eq!(parse(&ok).unwrap().headers.len(), MAX_HEADERS);
    }

    #[test]
    fn keep_alive_follows_the_version_connection_and_body() {
        let ka = |raw: &[u8]| parse(raw).unwrap().keep_alive();
        assert!(ka(b"GET / HTTP/1.1\r\n\r\n"));
        assert!(!ka(b"GET / HTTP/1.0\r\n\r\n"));
        assert!(!ka(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n"));
        assert!(!ka(b"GET / HTTP/1.1\r\nConnection: Upgrade, CLOSE\r\n\r\n"));
        assert!(ka(b"GET / HTTP/1.1\r\nContent-Length: 0\r\n\r\n"));
        // The body is never read, so what follows it can't be parsed as the next request.
        assert!(!ka(b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\n"));
        assert!(!ka(
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"
        ));
    }

    #[test]
    fn the_path_drops_the_query_and_fragment_and_accepts_absolute_form() {
        let path = |target: &str| {
            let raw = format!("GET {target} HTTP/1.1\r\n\r\n");
            parse(raw.as_bytes()).unwrap().path().map(str::to_string)
        };
        assert_eq!(path("/a/b?c=d#e").as_deref(), Some("/a/b"));
        assert_eq!(path("http://localhost:8080/x?y").as_deref(), Some("/x"));
        assert_eq!(path("https://h/p#f").as_deref(), Some("/p"));
        assert_eq!(path("http://localhost"), None);
        assert_eq!(path("*"), None);
        assert_eq!(path("ftp://h/p"), None);
    }

    #[test]
    fn responses_strip_line_breaks_from_header_values() {
        let mut out = Vec::new();
        let resp = Response::new(301)
            .header("Location", "/a\r\nSet-Cookie: x=1")
            .body(b"hi".to_vec());
        write_response(&mut out, resp, false, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("HTTP/1.1 301 Moved Permanently\r\n"),
            "{text}"
        );
        assert!(text.contains("Location: /aSet-Cookie: x=1\r\n"), "{text}");
        assert!(text.contains("Content-Length: 2\r\nDate: "), "{text}");
        assert!(text.contains("Connection: close\r\n\r\nhi"), "{text}");
    }

    #[test]
    fn head_responses_keep_the_length_but_send_no_body() {
        let mut out = Vec::new();
        let resp = Response::new(200).body(b"hello".to_vec());
        write_response(&mut out, resp, true, true).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Content-Length: 5\r\n"), "{text}");
        assert!(
            text.contains("Connection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n"),
            "{text}"
        );
        assert!(text.ends_with("\r\n\r\n"), "{text}");
    }

    #[test]
    fn a_file_that_shrinks_mid_send_is_an_error() {
        let mut out = Vec::new();
        let short: Box<dyn Read + Send> = Box::new(io::Cursor::new(b"abc".to_vec()));
        let resp = Response {
            status: 200,
            headers: Vec::new(),
            body: Body::Stream(short, 10),
        };
        let e = write_response(&mut out, resp, false, false).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn every_status_sent_has_its_reason() {
        for s in [
            200, 206, 301, 400, 403, 404, 408, 416, 426, 431, 500, 503, 505,
        ] {
            assert_ne!(reason(s), "Unknown", "{s}");
        }
        assert_eq!(reason(299), "Unknown");
    }
}
