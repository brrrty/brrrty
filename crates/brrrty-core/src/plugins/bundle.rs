//! 11ty's bundle plugin (`eleventyConfig.addBundle`), the inline part: `{% css %}...{% endcss %}`
//! (any bundle's name) collects code for the page being rendered, and `{% getBundle "css" %}`
//! prints what the page collected.
//!
//! 11ty prints a placeholder where `getBundle` is called and swaps in the code once the page
//! is rendered, so code collected after the call (a footer include, say) is in. [`Bundles::resolve`]
//! does the same swap. Entries are kept once each, in the order added, and joined with a
//! newline. Not supported (named refusals): `addBundle` options, `getBundleFileUrl`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brrrty_js::{FilterFn, Value};
use brrrty_nunjucks::Engine;

const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

type Entries = HashMap<(String, String), Vec<String>>;

/// The code the pages being rendered have collected, by page url, then bundle and bucket.
#[derive(Debug, Default)]
pub struct Bundles {
    names: Vec<String>,
    pages: Mutex<HashMap<String, Entries>>,
}

fn page_url(page: &Option<Value>) -> Result<String, String> {
    match page.as_ref().map(|p| p.get("url")) {
        Some(Value::String(s)) => Ok(s.to_string()),
        _ => Ok(String::new()),
    }
}

fn bucket(args: &[Value], kwargs_empty: bool, what: &str) -> Result<String, String> {
    if !kwargs_empty || args.len() > 1 {
        return Err(format!(
            "{}`{what}` takes only a bucket name here; Brrrty doesn't support more yet",
            brrrty_js::REFUSAL_PREFIX
        ));
    }
    match args.first() {
        None | Some(Value::Undefined) | Some(Value::Null) => Ok("default".to_string()),
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(_) => Err(format!("`{what}`'s bucket must be a string")),
    }
}

/// A bucket argument as 11ty reads it: a comma list of names, trimmed, empties dropped.
fn buckets(arg: &str) -> Vec<String> {
    let v: Vec<String> = arg
        .split(',')
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .collect();
    if v.is_empty() {
        vec!["default".to_string()]
    } else {
        v
    }
}

/// A tag's attributes, lowercased names, with their values when they have one.
fn attrs(tag: &str) -> Vec<(String, Option<String>)> {
    // `tag` is `<name attrs...>`; a small scanner, quotes respected.
    let inner = tag[1..tag.len() - 1].trim_end_matches('/');
    let b = inner.as_bytes();
    let mut i = inner.find(|c: char| c.is_whitespace()).unwrap_or(b.len());
    let mut out = Vec::new();
    while i < b.len() {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let s = i;
        while i < b.len() && !(b[i] as char).is_whitespace() && b[i] != b'=' {
            i += 1;
        }
        if s == i {
            i += 1;
            continue;
        }
        let name = inner[s..i].to_ascii_lowercase();
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let mut val = None;
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && (b[i] as char).is_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                i = vs;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                val = Some(inner[vs..i.min(b.len())].to_string());
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !(b[i] as char).is_whitespace() {
                    i += 1;
                }
                val = Some(inner[vs..i].to_string());
            }
        }
        out.push((name, val));
    }
    out
}

/// End of the tag starting at `from` (a `<`), after its `>`, quotes respected.
fn tag_end(html: &str, from: usize) -> Option<usize> {
    let b = html.as_bytes();
    let mut q = 0u8;
    for (i, &c) in b.iter().enumerate().skip(from) {
        if q != 0 {
            if c == q {
                q = 0;
            }
        } else if c == b'"' || c == b'\'' {
            q = c;
        } else if c == b'>' {
            return Some(i + 1);
        }
    }
    None
}

/// What 11ty's `pruneEmptyBundles` transform does to a page once any bundle is added: drops
/// an empty `<style>`, a `<script>` with no code and no `src`, and a `<link rel=stylesheet>`
/// with an empty `href`; an element with `eleventy:keep` stays, minus that attribute.
/// Only the tag text is edited, so no other byte of the page changes. "Empty" is no
/// characters at all between the tags.
pub fn prune_empty(html: &str) -> String {
    if !html.contains("<style") && !html.contains("<script") && !html.contains("<link") {
        return html.to_string();
    }
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    let mut i = 0;
    while let Some(rel) = lower[i..].find('<') {
        let s = i + rel;
        let name = ["style", "script", "link"].into_iter().find(|n| {
            lower[s + 1..].starts_with(n)
                && lower
                    .as_bytes()
                    .get(s + 1 + n.len())
                    .is_some_and(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/')
        });
        let Some(name) = name else {
            i = s + 1;
            continue;
        };
        let Some(e) = tag_end(html, s) else { break };
        let tag = &html[s..e];
        let a = attrs(tag);
        let has = |n: &str| a.iter().any(|(k, _)| k == n);
        let val = |n: &str| a.iter().find(|(k, _)| k == n).and_then(|(_, v)| v.clone());
        let close = format!("</{name}>");
        let empty_body = name == "link" || lower[e..].starts_with(&close);
        let end = if name == "link" { e } else { e + close.len() };
        let drop = empty_body
            && match name {
                "style" => true,
                "script" => val("src").is_none_or(|v| v.is_empty()),
                _ => {
                    val("rel").is_some_and(|r| r.eq_ignore_ascii_case("stylesheet"))
                        && val("href").is_some_and(|h| h.is_empty())
                }
            };
        if has("eleventy:keep") {
            out.push_str(&html[at..s]);
            out.push_str(&strip_keep(tag));
            at = e;
            i = e;
        } else if drop {
            out.push_str(&html[at..s]);
            at = end;
            i = end;
        } else {
            i = e;
        }
    }
    out.push_str(&html[at..]);
    out
}

fn strip_keep(tag: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    let Some(p) = lower.find("eleventy:keep") else {
        return tag.to_string();
    };
    let mut end = p + "eleventy:keep".len();
    let rest = &tag[end..];
    let t = rest.trim_start();
    if let Some(v) = t.strip_prefix('=') {
        let v = v.trim_start();
        let n = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].find(q).map_or(v.len(), |k| k + 2),
            _ => v
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(v.len()),
        };
        end = tag.len() - v.len() + n;
    }
    let start = tag[..p].trim_end().len();
    format!("{}{}", &tag[..start], &tag[end..])
}

impl Bundles {
    pub fn new(names: Vec<String>) -> Arc<Bundles> {
        Arc::new(Bundles {
            names,
            pages: Mutex::new(HashMap::new()),
        })
    }

    /// Adds the `{% name %}` paired tag of each bundle, and `getBundle`, to Nunjucks.
    pub fn register(self: &Arc<Self>, engine: &mut Engine) {
        for name in &self.names {
            let (me, n) = (self.clone(), name.clone());
            let f: FilterFn = Arc::new(move |input, args, env| {
                let bucket = bucket(args, env.kwargs.is_empty(), &format!("{{% {n} %}}"))?;
                let url = page_url(&env.page)?;
                let code = match input {
                    Value::String(s) => s.to_string(),
                    other => other.to_js_string(),
                };
                // 11ty trims on add, drops what's left empty, and de-duplicates on the trim.
                let code = code.trim().to_string();
                if !code.is_empty() {
                    let mut pages = me.pages.lock().unwrap();
                    let page = pages.entry(url).or_default();
                    for b in buckets(&bucket) {
                        let list = page.entry((n.clone(), b)).or_default();
                        if !list.contains(&code) {
                            list.push(code.clone());
                        }
                    }
                }
                Ok(Value::String("".into()))
            });
            engine.register_shortcode(name, true, f);
        }
        let me = self.clone();
        let f: FilterFn = Arc::new(move |input, args, env| {
            let Value::String(kind) = input else {
                return Err("`getBundle` needs the name of a bundle".to_string());
            };
            if !me.names.iter().any(|n| **n == **kind) {
                return Err(format!(
                    "{}`getBundle` asks for the bundle `{kind}`, which the config doesn't add (`addBundle`)",
                    brrrty_js::REFUSAL_PREFIX
                ));
            }
            let bucket = bucket(args, env.kwargs.is_empty(), "getBundle")?;
            Ok(Value::String(
                format!("{OPEN}{kind}:{}{CLOSE}", buckets(&bucket).join(",")).into(),
            ))
        });
        engine.register_shortcode("getBundle", false, f);
    }

    /// Registers the same tags for Liquid as named refusals.
    pub fn register_liquid_refusals(&self, engine: &mut brrrty_liquid::Engine) {
        let msg = format!(
            "{}bundles ({{% css %}}, getBundle) are Nunjucks-only for now",
            brrrty_js::REFUSAL_PREFIX
        );
        for name in &self.names {
            let m = msg.clone();
            let f: brrrty_liquid::ShortcodeFn = Arc::new(move |_, _| Err(m.clone()));
            engine.register_shortcode(name, true, f);
        }
        let f: brrrty_liquid::ShortcodeFn = Arc::new(move |_, _| Err(msg.clone()));
        engine.register_shortcode("getBundle", false, f);
    }

    /// `html` with each `getBundle` placeholder replaced by what the page at `url` collected,
    /// which the page then forgets.
    pub fn resolve(&self, url: &str, html: String) -> String {
        let entries = self.pages.lock().unwrap().remove(url).unwrap_or_default();
        if !html.contains(OPEN) {
            return html;
        }
        let mut out = String::with_capacity(html.len());
        let mut rest = html.as_str();
        while let Some(start) = rest.find(OPEN) {
            out.push_str(&rest[..start]);
            let after = &rest[start + OPEN.len_utf8()..];
            let Some(end) = after.find(CLOSE) else {
                rest = &rest[start..];
                break;
            };
            if let Some((kind, bs)) = after[..end].split_once(':') {
                let mut seen: Vec<&String> = Vec::new();
                for b in bs.split(',') {
                    if let Some(list) = entries.get(&(kind.to_string(), b.to_string())) {
                        for c in list {
                            if !seen.contains(&c) {
                                seen.push(c);
                            }
                        }
                    }
                }
                out.push_str(
                    &seen
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            rest = &after[end + CLOSE.len_utf8()..];
        }
        out.push_str(rest);
        out
    }
}
