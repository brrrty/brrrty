//! 11ty 3.1.8's `IdAttributePlugin` (`src/Plugins/IdAttributePlugin.js`) with its default
//! options: every element matching `[id],h1,h2,h3,h4,h5,h6` that has no `id` and has content
//! gets one made with the `slugify` filter from its text (entities decoded), numbered when
//! two share one. 11ty runs it as a posthtml plugin of its HTML transformer, on pages written
//! to a `.html` file, so the page is parsed and printed again by posthtml (see
//! [`super::rss::posthtml`]).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::rss::posthtml::{Element, Node, walk_mut};

const IGNORE: &str = "eleventy:id-ignore";

fn attr<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
    e.attrs
        .as_ref()?
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn set_id(e: &mut Element, id: &str) {
    let attrs = e.attrs.get_or_insert_with(Vec::new);
    match attrs.iter_mut().find(|(k, _)| k == "id") {
        Some((_, v)) => *v = id.to_string(),
        None => attrs.push(("id".to_string(), id.to_string())),
    }
}

/// `getTextNodeContent`: the text of a node, comments included, skipping (and un-marking) a
/// node with `eleventy:id-ignore`.
fn text_of(e: &mut Element) -> String {
    if attr(e, IGNORE) == Some("") {
        if let Some(a) = e.attrs.as_mut() {
            a.retain(|(k, _)| k != IGNORE);
        }
        return String::new();
    }
    let mut out = String::new();
    for n in &mut e.content {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Element(c) if !c.content.is_empty() => out.push_str(&text_of(c)),
            Node::Element(_) => {}
        }
    }
    out
}

fn matches(e: &Element) -> bool {
    attr(e, "id").is_some() || matches!(e.tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// Runs the plugin on a parsed page, in place. `input_path` is the page's `page.inputPath`, for 11ty's
/// error about repeated ids.
pub fn apply(tree: &mut [Node], input_path: &str) -> Result<(), String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    // Headings given an id, by that id, as the position of the element in the walk.
    let mut headings: HashMap<String, usize> = HashMap::new();
    let mut renames: Vec<(usize, String)> = Vec::new();
    let mut error = None;
    let mut at = 0usize;
    walk_mut(tree, &mut |e| {
        let me = at;
        at += 1;
        if error.is_some() || !matches(e) {
            return;
        }
        match attr(e, "id").filter(|id| !id.is_empty()) {
            Some(id) => {
                let id = id.to_string();
                if let Some(n) = count.get_mut(&id) {
                    *n += 1;
                    if let Some(prev) = headings.remove(&id) {
                        let new = format!("{id}-{n}");
                        headings.insert(new.clone(), prev);
                        renames.push((prev, new));
                    } else {
                        error = Some(format!(
                            "You have more than one HTML `id` attribute using the same value (id=\"{id}\") in your template ({input_path}). You can disable this error in the IdAttribute plugin with the `checkDuplicates: false` option."
                        ));
                    }
                } else {
                    count.insert(id, 1);
                }
            }
            None if !e.content.is_empty() => {
                let text = decode(&text_of(e));
                let mut id = brrrty_js::slugify(&text);
                match count.get_mut(&id) {
                    Some(n) => {
                        *n += 1;
                        id = format!("{id}-{n}");
                    }
                    None => {
                        count.insert(id.clone(), 1);
                    }
                }
                set_id(e, &id);
                headings.insert(id, me);
            }
            None => {}
        }
    });
    if let Some(why) = error {
        return Err(why);
    }
    if !renames.is_empty() {
        let mut at = 0usize;
        walk_mut(tree, &mut |e| {
            // In the order they were made, so a heading renamed twice ends with the last.
            for (_, id) in renames.iter().filter(|(idx, _)| *idx == at) {
                set_id(e, id);
            }
            at += 1;
        });
    }
    Ok(())
}

/// `decodeHTML` of `entities` (legacy mode): named references, with the semicolon or, for the
/// legacy names, without; numeric ones, with WHATWG's replacements.
fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    static NAMES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        entities::ENTITIES
            .iter()
            .map(|e| (&e.entity[1..], e.characters))
            .collect()
    });
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] != b'&' {
            let next = s[i..].find('&').map_or(s.len(), |n| i + n);
            out.push_str(&s[i..next]);
            i = next;
            continue;
        }
        let rest = &s[i + 1..];
        if let Some(num) = rest.strip_prefix('#') {
            let hex = num.starts_with(['x', 'X']);
            let digits = &num[usize::from(hex)..];
            let len = digits
                .bytes()
                .take_while(|c| {
                    if hex {
                        c.is_ascii_hexdigit()
                    } else {
                        c.is_ascii_digit()
                    }
                })
                .count();
            if len > 0 {
                let n = u32::from_str_radix(&digits[..len], if hex { 16 } else { 10 })
                    .unwrap_or(u32::MAX);
                out.push(code_point(n));
                let consumed = 1 + usize::from(hex) + len;
                let semi = usize::from(rest[consumed..].starts_with(';'));
                i += 1 + consumed + semi;
                continue;
            }
        } else {
            let word = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
            if rest[word..].starts_with(';')
                && let Some(c) = names.get(&rest[..word + 1])
            {
                out.push_str(c);
                i += word + 2;
                continue;
            }
            if let Some(n) = (1..=word).rev().find(|n| names.contains_key(&rest[..*n])) {
                out.push_str(names[&rest[..n]]);
                i += n + 1;
                continue;
            }
        }
        out.push('&');
        i += 1;
    }
    out
}

/// `decodeCodePoint` of `entities`.
fn code_point(n: u32) -> char {
    const WIN1252: [u32; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013,
        0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    let n = if (0x80..=0x9F).contains(&n) {
        WIN1252[(n - 0x80) as usize]
    } else {
        n
    };
    if n == 0 {
        return '\u{FFFD}';
    }
    char::from_u32(n).unwrap_or('\u{FFFD}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_like_entities() {
        for (input, want) in [
            ("a &amp; b", "a & b"),
            ("&amp b", "& b"),
            ("&notit;", "\u{ac}it;"),
            ("&notin;", "\u{2209}"),
            ("&#65;&#x42;&#67", "ABC"),
            ("&#128;", "\u{20ac}"),
            ("&#0;&#xD800;", "\u{fffd}\u{fffd}"),
            ("&bogus; &", "&bogus; &"),
            ("&eacute;", "\u{e9}"),
        ] {
            assert_eq!(decode(input), want, "{input}");
        }
    }
}
