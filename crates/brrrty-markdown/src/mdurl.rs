//! markdown-it's `normalizeLink`, `normalizeLinkText` and `validateLink` (14.3.3 and 15 are the
//! same), with what they call: mdurl 2.1's `parse`, `format`, `encode` and `decode`, and
//! punycode.js 2.3's `toASCII` and `toUnicode`, ported line by line. The port's own link
//! formatter only percent-encodes: no punycode for hosts, and link text isn't decoded.

use markdown_it::parser::linkfmt::LinkFormatter;

use crate::js_space;

/// mdurl's `Url`: each part `None` where mdurl leaves it `null`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Url {
    protocol: Option<String>,
    slashes: bool,
    auth: Option<String>,
    port: Option<String>,
    hostname: Option<String>,
    hash: Option<String>,
    search: Option<String>,
    pathname: Option<String>,
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// JavaScript's `String.prototype.trim`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c| js_space(c) || matches!(c, '\u{2028}' | '\u{2029}'))
}

const NON_HOST_CHARS: &[char] = &[
    '%', '/', '?', ';', '#', '\'', '{', '}', '|', '\\', '^', '`', '<', '>', '"', '`', ' ', '\r',
    '\n', '\t',
];

fn is_hostname_part_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-')
}

/// `/^[+a-z0-9A-Z_-]{0,63}$/`, on UTF-16 units.
fn is_hostname_part(s: &str) -> bool {
    s.chars().all(is_hostname_part_char) && s.len() <= 63
}

fn is_slashed(proto: &str) -> bool {
    matches!(
        proto,
        "http"
            | "https"
            | "ftp"
            | "gopher"
            | "file"
            | "http:"
            | "https:"
            | "ftp:"
            | "gopher:"
            | "file:"
    )
}

fn is_hostless(proto: &str) -> bool {
    matches!(proto, "javascript" | "javascript:")
}

/// mdurl's `parse(url, true)` (`slashesDenoteHost`, as markdown-it calls it).
fn parse(url: &str) -> Url {
    let mut u = Url::default();
    let mut rest: String = js_trim(url).to_string();
    // /^([a-z0-9.+-]+:)/i
    let proto_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
        .count();
    let proto = (proto_len > 0 && rest.as_bytes().get(proto_len) == Some(&b':'))
        .then(|| rest[..=proto_len].to_string());
    let lower_proto = proto.as_ref().map(|p| p.to_ascii_lowercase());
    if let Some(p) = &proto {
        u.protocol = Some(p.clone());
        rest = rest[p.len()..].to_string();
    }
    let hostless = proto.as_deref().is_some_and(is_hostless);
    // `slashesDenoteHost` is true.
    let slashes = rest.starts_with("//");
    if slashes && !hostless {
        rest = rest[2..].to_string();
        u.slashes = true;
    }
    if !hostless && (slashes || proto.as_deref().is_some_and(|p| !is_slashed(p))) {
        let host_end = ['/', '?', '#'].iter().filter_map(|&c| rest.find(c)).min();
        let at_sign = match host_end {
            None => rest.rfind('@'),
            Some(end) => rest[..(end + 1).min(rest.len())].rfind('@'),
        };
        if let Some(at) = at_sign {
            u.auth = Some(rest[..at].to_string());
            rest = rest[at + 1..].to_string();
        }
        let mut host_end = NON_HOST_CHARS
            .iter()
            .filter_map(|&c| rest.find(c))
            .min()
            .unwrap_or(rest.len());
        if host_end > 0 && rest.as_bytes()[host_end - 1] == b':' {
            host_end -= 1;
        }
        let host = rest[..host_end].to_string();
        rest = rest[host_end..].to_string();
        parse_host(&mut u, &host);
        let mut hostname = u.hostname.take().unwrap_or_default();
        let ipv6 = hostname.starts_with('[') && hostname.ends_with(']');
        if !ipv6 {
            let parts: Vec<&str> = hostname.split('.').collect();
            for (i, part) in parts.iter().enumerate() {
                if part.is_empty() || is_hostname_part(part) {
                    continue;
                }
                // Each unit outside ASCII as `x`.
                let ascii_len: usize = part.chars().map(char::len_utf16).sum();
                let new_part_ok = part
                    .chars()
                    .all(|c| !c.is_ascii() || is_hostname_part_char(c))
                    && ascii_len <= 63;
                if new_part_ok {
                    continue;
                }
                let mut valid: Vec<String> = parts[..i].iter().map(|s| s.to_string()).collect();
                let mut not_host: Vec<String> =
                    parts[i + 1..].iter().map(|s| s.to_string()).collect();
                // /^([+a-z0-9A-Z_-]{0,63})(.*)$/: `.` stops at a line terminator.
                if !part.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                    let head = part
                        .char_indices()
                        .take_while(|&(k, c)| k < 63 && is_hostname_part_char(c))
                        .count();
                    valid.push(part[..head].to_string());
                    not_host.insert(0, part[head..].to_string());
                }
                if !not_host.is_empty() {
                    rest = not_host.join(".") + &rest;
                }
                hostname = valid.join(".");
                break;
            }
        }
        if utf16_len(&hostname) > 255 {
            hostname = String::new();
        }
        if ipv6 {
            hostname = hostname[1..hostname.len() - 1].to_string();
        }
        u.hostname = Some(hostname);
    }
    if let Some(hash) = rest.find('#') {
        u.hash = Some(rest[hash..].to_string());
        rest.truncate(hash);
    }
    if let Some(qm) = rest.find('?') {
        u.search = Some(rest[qm..].to_string());
        rest.truncate(qm);
    }
    if !rest.is_empty() {
        u.pathname = Some(rest);
    }
    if lower_proto.as_deref().is_some_and(is_slashed)
        && u.hostname.as_deref().is_some_and(|h| !h.is_empty())
        && u.pathname.as_deref().is_none_or(str::is_empty)
    {
        u.pathname = Some(String::new());
    }
    u
}

/// mdurl's `parseHost`: `/:[0-9]*$/` takes a port off the end.
fn parse_host(u: &mut Url, host: &str) {
    let mut host = host;
    if let Some(colon) = host.rfind(':')
        && host[colon + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        let port = &host[colon..];
        if port != ":" {
            u.port = Some(port[1..].to_string());
        }
        host = &host[..colon];
    }
    if !host.is_empty() {
        u.hostname = Some(host.to_string());
    }
}

fn truthy(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// mdurl's `format`.
fn format(u: &Url) -> String {
    let mut out = String::new();
    out += truthy(&u.protocol).unwrap_or("");
    if u.slashes {
        out += "//";
    }
    if let Some(auth) = truthy(&u.auth) {
        out += auth;
        out.push('@');
    }
    match truthy(&u.hostname) {
        Some(h) if h.contains(':') => {
            out.push('[');
            out += h;
            out.push(']');
        }
        Some(h) => out += h,
        None => {}
    }
    if let Some(port) = truthy(&u.port) {
        out.push(':');
        out += port;
    }
    out += truthy(&u.pathname).unwrap_or("");
    out += truthy(&u.search).unwrap_or("");
    out += truthy(&u.hash).unwrap_or("");
    out
}

/// mdurl's `encode(s)`: the default characters kept, escapes kept.
fn encode(s: &str) -> String {
    const KEEP: &str = ";/?:@&=+$,-_.!~*'()#";
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            out += &s[i..i + 3];
            i += 3;
            continue;
        }
        if c < 0x80 {
            if c.is_ascii_alphanumeric() || KEEP.as_bytes().contains(&c) {
                out.push(c as char);
            } else {
                out += &format!("%{c:02X}");
            }
            i += 1;
            continue;
        }
        let ch = s[i..].chars().next().expect("a character");
        let mut buf = [0u8; 4];
        for byte in ch.encode_utf8(&mut buf).bytes() {
            out += &format!("%{byte:02X}");
        }
        i += ch.len_utf8();
    }
    out
}

/// mdurl's `decode(s, exclude)`.
fn decode(s: &str, exclude: &str) -> String {
    let b = s.as_bytes();
    let hex = |i: usize| -> Option<u32> {
        let h = s.get(i..i + 2)?;
        u32::from_str_radix(h, 16).ok()
    };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        // A run of /(%[a-f0-9]{2})+/i.
        let mut end = i;
        while end + 3 <= b.len()
            && b[end] == b'%'
            && b[end + 1].is_ascii_hexdigit()
            && b[end + 2].is_ascii_hexdigit()
        {
            end += 3;
        }
        if end == i {
            let ch = s[i..].chars().next().expect("a character");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let l = end - i;
        let mut k = 0;
        while k < l {
            let at = i + k;
            let b1 = hex(at + 1).expect("hex");
            if b1 < 0x80 {
                let ch = b1 as u8 as char;
                if exclude.contains(ch) {
                    out += &format!("%{b1:02X}");
                } else {
                    out.push(ch);
                }
                k += 3;
                continue;
            }
            if b1 & 0xE0 == 0xC0 && k + 3 < l {
                let b2 = hex(at + 4).expect("hex");
                if b2 & 0xC0 == 0x80 {
                    let chr = ((b1 << 6) & 0x7C0) | (b2 & 0x3F);
                    if chr < 0x80 {
                        out += "\u{fffd}\u{fffd}";
                    } else {
                        out.push(char::from_u32(chr).expect("below 0x800"));
                    }
                    k += 6;
                    continue;
                }
            }
            if b1 & 0xF0 == 0xE0 && k + 6 < l {
                let b2 = hex(at + 4).expect("hex");
                let b3 = hex(at + 7).expect("hex");
                if b2 & 0xC0 == 0x80 && b3 & 0xC0 == 0x80 {
                    let chr = ((b1 << 12) & 0xF000) | ((b2 << 6) & 0xFC0) | (b3 & 0x3F);
                    if chr < 0x800 || (0xD800..=0xDFFF).contains(&chr) {
                        out += "\u{fffd}\u{fffd}\u{fffd}";
                    } else {
                        out.push(char::from_u32(chr).expect("a BMP character"));
                    }
                    k += 9;
                    continue;
                }
            }
            if b1 & 0xF8 == 0xF0 && k + 9 < l {
                let b2 = hex(at + 4).expect("hex");
                let b3 = hex(at + 7).expect("hex");
                let b4 = hex(at + 10).expect("hex");
                if b2 & 0xC0 == 0x80 && b3 & 0xC0 == 0x80 && b4 & 0xC0 == 0x80 {
                    let chr = ((b1 << 18) & 0x1C0000)
                        | ((b2 << 12) & 0x3F000)
                        | ((b3 << 6) & 0xFC0)
                        | (b4 & 0x3F);
                    if !(0x10000..=0x10FFFF).contains(&chr) {
                        out += "\u{fffd}\u{fffd}\u{fffd}\u{fffd}";
                    } else {
                        out.push(char::from_u32(chr).expect("an astral character"));
                    }
                    k += 12;
                    continue;
                }
            }
            out.push('\u{fffd}');
            k += 3;
        }
        i = end;
    }
    out
}

// punycode.js 2.3.1.
const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;
const MAX_INT: u32 = 0x7FFF_FFFF;

fn adapt(delta: u32, num_points: u32, first_time: bool) -> u32 {
    let mut delta = if first_time { delta / DAMP } else { delta >> 1 };
    delta += delta / num_points;
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) >> 1 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
}

fn digit_to_basic(d: u32) -> char {
    // 0..25 map to a..z, 26..35 to 0..9.
    (if d < 26 {
        b'a' + d as u8
    } else {
        b'0' + (d - 26) as u8
    }) as char
}

fn basic_to_digit(c: u32) -> u32 {
    if (0x30..0x3A).contains(&c) {
        26 + (c - 0x30)
    } else if (0x41..0x5B).contains(&c) {
        c - 0x41
    } else if (0x61..0x7B).contains(&c) {
        c - 0x61
    } else {
        BASE
    }
}

/// punycode.js `encode`: errors where it throws (`overflow`).
fn puny_encode(input: &str) -> Result<String, ()> {
    let input: Vec<u32> = input.chars().map(u32::from).collect();
    let mut output: String = input
        .iter()
        .filter(|&&c| c < 0x80)
        .map(|&c| char::from_u32(c).expect("ASCII"))
        .collect();
    let basic = output.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        output.push('-');
    }
    let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let len = input.len() as u32;
    while handled < len {
        let m = input
            .iter()
            .copied()
            .filter(|&c| c >= n)
            .min()
            .unwrap_or(MAX_INT);
        let inc = handled + 1;
        if m - n > (MAX_INT - delta) / inc {
            return Err(());
        }
        delta += (m - n) * inc;
        n = m;
        for &c in &input {
            if c < n {
                delta += 1;
                if delta > MAX_INT {
                    return Err(());
                }
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        T_MIN
                    } else if k >= bias + T_MAX {
                        T_MAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    let q_minus_t = q - t;
                    let base_minus_t = BASE - t;
                    output.push(digit_to_basic(t + q_minus_t % base_minus_t));
                    q = q_minus_t / base_minus_t;
                    k += BASE;
                }
                output.push(digit_to_basic(q));
                bias = adapt(delta, inc, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Ok(output)
}

/// punycode.js `decode` on UTF-16 units: errors where it throws.
fn puny_decode(input: &[u16]) -> Result<String, ()> {
    let mut output: Vec<u32> = Vec::new();
    let input_length = input.len();
    let (mut i, mut n, mut bias) = (0u32, INITIAL_N, INITIAL_BIAS);
    let basic = input
        .iter()
        .rposition(|&u| u == u16::from(b'-'))
        .unwrap_or(0);
    for &u in &input[..basic] {
        if u >= 0x80 {
            return Err(());
        }
        output.push(u32::from(u));
    }
    let mut index = if basic > 0 { basic + 1 } else { 0 };
    while index < input_length {
        let old_i = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            if index >= input_length {
                return Err(());
            }
            let digit = basic_to_digit(u32::from(input[index]));
            index += 1;
            if digit >= BASE {
                return Err(());
            }
            if digit > (MAX_INT - i) / w {
                return Err(());
            }
            i += digit * w;
            let t = if k <= bias {
                T_MIN
            } else if k >= bias + T_MAX {
                T_MAX
            } else {
                k - bias
            };
            if digit < t {
                break;
            }
            let base_minus_t = BASE - t;
            if w > MAX_INT / base_minus_t {
                return Err(());
            }
            w *= base_minus_t;
            k += BASE;
        }
        let out = output.len() as u32 + 1;
        bias = adapt(i - old_i, out, old_i == 0);
        if i / out > MAX_INT - n {
            return Err(());
        }
        n += i / out;
        i %= out;
        output.insert(i as usize, n);
        i += 1;
    }
    // `String.fromCodePoint` throws past U+10FFFF; a lone surrogate is written as U+FFFD.
    output
        .into_iter()
        .map(|c| {
            if c > 0x10FFFF {
                Err(())
            } else {
                Ok(char::from_u32(c).unwrap_or('\u{fffd}'))
            }
        })
        .collect()
}

/// punycode.js `mapDomain`: an email's name kept, the domain's labels mapped.
fn map_domain(domain: &str, f: impl Fn(&str) -> Result<String, ()>) -> Result<String, ()> {
    let parts: Vec<&str> = domain.split('@').collect();
    let (prefix, domain) = if parts.len() > 1 {
        (format!("{}@", parts[0]), parts[1])
    } else {
        (String::new(), domain)
    };
    let domain = domain.replace(['\u{3002}', '\u{ff0e}', '\u{ff61}'], ".");
    let labels: Result<Vec<String>, ()> = domain.split('.').map(f).collect();
    Ok(prefix + &labels?.join("."))
}

fn to_ascii(domain: &str) -> Result<String, ()> {
    map_domain(domain, |label| {
        if label.is_ascii() {
            Ok(label.to_string())
        } else {
            Ok(format!("xn--{}", puny_encode(label)?))
        }
    })
}

fn to_unicode(domain: &str) -> Result<String, ()> {
    map_domain(domain, |label| match label.strip_prefix("xn--") {
        Some(rest) => {
            let lower: Vec<u16> = rest.to_lowercase().encode_utf16().collect();
            puny_decode(&lower)
        }
        None => Ok(label.to_string()),
    })
}

/// markdown-it recodes hosts for these protocols, and for a link without one.
fn recodes_host(u: &Url) -> bool {
    match u.protocol.as_deref() {
        None | Some("") => true,
        Some(p) => matches!(p, "http:" | "https:" | "mailto:"),
    }
}

/// markdown-it's link functions, as a formatter for the port.
#[derive(Debug, Default)]
pub(crate) struct JsLinkFormatter;

impl LinkFormatter for JsLinkFormatter {
    fn validate_link(&self, url: &str) -> Option<()> {
        let s = js_trim(url).to_lowercase();
        let bad = ["vbscript:", "javascript:", "file:", "data:"]
            .iter()
            .any(|p| s.starts_with(p));
        if !bad {
            return Some(());
        }
        let good = ["gif", "png", "jpeg", "webp"]
            .iter()
            .any(|t| s.starts_with(&format!("data:image/{t};")));
        good.then_some(())
    }

    fn normalize_link(&self, url: &str) -> String {
        let mut u = parse(url);
        if let Some(h) = truthy(&u.hostname).map(str::to_string)
            && recodes_host(&u)
            && let Ok(ascii) = to_ascii(&h)
        {
            u.hostname = Some(ascii);
        }
        for part in [
            &mut u.auth,
            &mut u.hostname,
            &mut u.pathname,
            &mut u.search,
            &mut u.hash,
        ] {
            if let Some(p) = part.as_mut().filter(|p| !p.is_empty()) {
                *p = encode(p);
            }
        }
        format(&u)
    }

    fn normalize_link_text(&self, url: &str) -> String {
        let mut u = parse(url);
        if let Some(h) = truthy(&u.hostname).map(str::to_string)
            && recodes_host(&u)
            && let Ok(text) = to_unicode(&h)
        {
            u.hostname = Some(text);
        }
        decode(&format(&u), ";/?:@&=+$,#%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punycode_round_trips() {
        assert_eq!(to_ascii("é.com").unwrap(), "xn--9ca.com");
        assert_eq!(to_ascii("пример.рф").unwrap(), "xn--e1afmkfd.xn--p1ai");
        assert_eq!(to_unicode("xn--e1afmkfd.xn--p1ai").unwrap(), "пример.рф");
        assert_eq!(to_ascii("a@b.é").unwrap(), "a@b.xn--9ca");
    }

    #[test]
    fn links_normalize_as_markdown_it_does() {
        let f = JsLinkFormatter;
        assert_eq!(
            f.normalize_link("http://é.com/é?x=é#é"),
            "http://xn--9ca.com/%C3%A9?x=%C3%A9#%C3%A9"
        );
        assert_eq!(f.normalize_link("HTTP://~:8080"), "HTTP://:8080~");
        assert_eq!(
            f.normalize_link_text("http://xn--p1ai/%C3%A9%2F"),
            "http://рф/é%2F"
        );
        assert!(f.validate_link("javascript:x").is_none());
        assert!(f.validate_link("data:image/png;base64,x").is_some());
    }
}
