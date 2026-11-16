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
}
