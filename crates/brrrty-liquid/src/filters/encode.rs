//! `base64_encode`, `base64_decode`, `sha256` and `hmac_sha256`.

use sha2::{Digest, Sha256};

use super::{Args, Out, string, stringify, throw};
use crate::context::Val;
use crate::render::Renderer;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// LiquidJS's `isBase64`: after dropping whitespace and, when the length
/// is a multiple of four, up to two trailing `=`, only base64 characters
/// may be left and the length may not be one more than a multiple of four.
fn is_base64(s: &str) -> bool {
    let mut t: String = s
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' '))
        .collect();
    if t.chars().count().is_multiple_of(4) {
        for _ in 0..2 {
            if t.ends_with('=') {
                t.pop();
            }
        }
    }
    t.chars().count() % 4 != 1
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// Node's `Buffer.from(str, 'base64')` for text that passed `is_base64`:
/// whitespace and everything from the first `=` on is skipped, and a
/// partial last group gives the bytes it has.
fn decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.chars() {
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            '=' => break,
            _ => continue,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

pub(super) fn base64_encode(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    r.check_bytes(s.len() / 3 * 4 + 4)?;
    Ok(string(encode(s.as_bytes())))
}

pub(super) fn base64_decode(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    if !is_base64(&s) {
        return Err(throw("invalid base64 provided to base64_decode"));
    }
    Ok(string(String::from_utf8_lossy(&decode(&s)).into_owned()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn sha256(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    Ok(string(hex(&Sha256::digest(s.as_bytes()))))
}

/// HMAC with SHA-256 (RFC 2104).
fn hmac_sha256_bytes(key: &[u8], message: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut k = if key.len() > BLOCK {
        Sha256::digest(key).to_vec()
    } else {
        key.to_vec()
    };
    k.resize(BLOCK, 0);
    let mut inner = Sha256::new();
    inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<_>>());
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<_>>());
    outer.update(inner.finalize());
    outer.finalize().to_vec()
}

pub(super) fn hmac_sha256(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let key = stringify(r, &Args(a).value(0));
    Ok(string(hex(&hmac_sha256_bytes(
        key.as_bytes(),
        s.as_bytes(),
    ))))
}
