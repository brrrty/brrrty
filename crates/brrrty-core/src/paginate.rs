//! Pagination: one template, many pages, as 11ty 3.1.8's
//! `Plugins/Pagination.js` makes them.
//!
//! Probed against 11ty 3.1.8 and read from its source:
//!
//! - The target is `lodash.get(data, pagination.data)`: a list, or an
//!   object's keys in JavaScript order (its values with `resolve: values`).
//!   Then `reverse: true` (strictly `true`), then `filter` (one value or a
//!   list, compared with `===`), then lodash's `chunk` by `size`. Empty data
//!   gives no pages, unless `generatePageOnEmptyData` gives one empty page.
//! - Each page's data is the template's data with an overlay, as 11ty's
//!   `ProxyWrap` reads it: keys of the data first, then keys only the overlay
//!   has; plain objects on both sides merge, anything else the overlay wins.
//!   The overlay holds `pagination` (the generated keys) and the `alias`.
//! - `pagination` is `{data, size, alias, pages}`, then the template's own
//!   `pagination` keys, then `items`, `page`, `pageNumber`, the `*Link`
//!   keys, `links`, `pageLinks`, the `*Href` keys, `hrefs` and `href`.
//!   With `size: 1`, `pages`, `page.*` and the alias hold the item itself.
//! - Pages after the first get a `N/` folder before the file name when
//!   there is no `permalink` (or computed `permalink`) to place them.
//! - `links` are `"/"` plus each written page's output path as the
//!   permalink spells it, so `/a/` gives `//a/index.html`.
//! - A missing key, a target that is neither list nor object, a missing
//!   `size` and a `pagination` that is not an object all stop 11ty's build.
//!   Brrrty refuses the page and builds the rest.

use std::sync::Arc;

use brrrty_js::Value;

use crate::data::json_text;
use crate::frontmatter::Data;
use crate::problem::{Problem, codes};

/// 11ty stops its build where these are reported.
fn invalid(message: String) -> Problem {
    Problem::error(codes::PAGINATION_INVALID, message).with_help("11ty stops the whole build here.")
}

fn unsupported(message: String, help: &str) -> Problem {
    Problem::error(codes::PAGINATION_UNSUPPORTED, message).with_help(help)
}

/// A template's `pagination`, checked, before its data is read.
pub(crate) struct Spec {
    config: Arc<Data>,
    /// `pagination.data` as written.
    key: Arc<str>,
    /// `pagination.data` as lodash splits it.
    pub path: Vec<String>,
    /// lodash's `chunk` size: `None` makes no pages.
    chunk: Option<usize>,
    /// `size === 1`: items are unwrapped.
    single: bool,
}

impl Spec {
    /// The template's pagination, if `data` has the key: refused where 11ty
    /// stops or where Brrrty can't follow it.
    pub fn new(data: &Data) -> Result<Option<Spec>, Problem> {
        let Some(value) = data.get("pagination") else {
            return Ok(None);
        };
        let config = match value {
            Value::Object(o) => o.clone(),
            other => {
                return Err(invalid(format!(
                    "`pagination` must be a map with `data` and `size`, found {}",
                    shown(other)
                )));
            }
        };
        let size = match config.get("size") {
            None => return Err(invalid("`pagination.size` is missing".to_string())),
            Some(Value::Number(n)) => *n,
            Some(Value::Null) => 0.0,
            Some(other) => {
                return Err(unsupported(
                    format!("`pagination.size` must be a number, found {}", shown(other)),
                    "Write the page size as a number, like `size: 10`.",
                ));
            }
        };
        let key: Arc<str> = match config.get("data") {
            None | Some(Value::Undefined) => {
                return Err(invalid("`pagination.data` is not set".to_string()));
            }
            Some(Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(unsupported(
                    format!(
                        "`pagination.data` must be a string naming the data to paginate, found {}",
                        shown(other)
                    ),
                    "Write the data's name, like `data: posts`.",
                ));
            }
        };
        // lodash's `toInteger`, then `Math.max(size, 0)`; 0 makes no chunks.
        let chunk = if size.is_nan() || size < 1.0 {
            None
        } else if size.is_infinite() || size >= usize::MAX as f64 {
            Some(usize::MAX)
        } else {
            Some(size.trunc() as usize)
        };
        let path = lodash_path(&key, data);
        Ok(Some(Spec {
            config,
            key,
            path,
            chunk,
            single: size == 1.0,
        }))
    }

    /// `pagination.data` as written.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The pages: their items from `data`, chunked.
    pub fn expand(&self, data: &Data) -> Result<Paging, Problem> {
        let target = get(data, &self.path);
        if matches!(target, Value::Undefined) {
            return Err(invalid(format!(
                "pagination data `{}` was not found",
                self.key
            )));
        }
        let config = &self.config;
        let mut items: Vec<Value> = match &target {
            Value::Array(a) => a.as_ref().clone(),
            Value::Object(o) => {
                let values =
                    matches!(config.get("resolve"), Some(Value::String(s)) if &**s == "values");
                Value::js_entries(o)
                    .into_iter()
                    .map(|(k, v)| {
                        if values {
                            v.clone()
                        } else {
                            Value::String(k.as_str().into())
                        }
                    })
                    .collect()
            }
            other => {
                return Err(invalid(format!(
                    "pagination data `{}` must be a list or an object, found {}",
                    self.key,
                    shown(other)
                )));
            }
        };
        if matches!(config.get("reverse"), Some(Value::Bool(true))) {
            items.reverse();
        }
        if let Some(filter) = config.get("filter").filter(|f| f.js_truthy()) {
            items.retain(|v| match filter {
                Value::Array(list) => !list.iter().any(|f| strict_eq(f, v)),
                f => !strict_eq(f, v),
            });
        }
        let mut chunks: Vec<Value> = match self.chunk {
            None => Vec::new(),
            Some(size) => items
                .chunks(size)
                .map(|c| Value::Array(Arc::new(c.to_vec())))
                .collect(),
        };
        if chunks.is_empty()
            && config
                .get("generatePageOnEmptyData")
                .is_some_and(Value::js_truthy)
        {
            chunks.push(Value::Array(Arc::new(Vec::new())));
        }
        let normalized: Vec<Value> = chunks
            .iter()
            .map(|c| match c {
                Value::Array(a) if self.single => a.first().cloned().unwrap_or(Value::Undefined),
                other => other.clone(),
            })
            .collect();

        // `DeepCopy({pagination: {data, size, alias, pages}}, data)`.
        let mut parent = Data::new();
        parent.insert("data".into(), Value::String(self.key.clone()));
        parent.insert(
            "size".into(),
            config.get("size").cloned().unwrap_or(Value::Undefined),
        );
        parent.insert(
            "alias".into(),
            config.get("alias").cloned().unwrap_or(Value::Undefined),
        );
        parent.insert("pages".into(), Value::Array(Arc::new(normalized.clone())));
        for (k, v) in Value::js_entries(config) {
            let merged = deep_copy_item(parent.get(k.as_str()), v);
            parent.insert(k.clone(), merged);
        }

        let alias = match config.get("alias") {
            None | Some(Value::Undefined) => None,
            Some(v) if !v.js_truthy() => None,
            Some(Value::String(a)) => {
                let path = lodash_path(a, &Data::new());
                if matches!(
                    path.first().map(String::as_str),
                    Some("pagination" | "page")
                ) {
                    return Err(unsupported(
                        format!("`pagination.alias` `{a}` would replace 11ty's own data"),
                        "Choose an alias other than `pagination` or `page`.",
                    ));
                }
                Some(path)
            }
            Some(other) => {
                return Err(unsupported(
                    format!(
                        "`pagination.alias` must be a string, found {}",
                        shown(other)
                    ),
                    "Write the alias as a name, like `alias: post`.",
                ));
            }
        };

        Ok(Paging {
            chunks,
            normalized,
            parent: Value::Object(Arc::new(parent)),
            alias,
        })
    }
}

/// A template's pages, ready to give each its data.
pub(crate) struct Paging {
    chunks: Vec<Value>,
    /// What `pages`, `page.*` and the alias hold for each page.
    normalized: Vec<Value>,
    /// The `pagination` every page shares.
    parent: Value,
    alias: Option<Vec<String>>,
}

/// Where the pages are, once each is located: `pagination.hrefs` and
/// `pagination.links`.
pub(crate) struct Links {
    /// Every page's `page.url`, `false` for one that isn't written.
    hrefs: Arc<Vec<Value>>,
    /// `"/"` plus the output path of every page that is written.
    links: Arc<Vec<Value>>,
}

impl Links {
    /// Built once and shared by every page, so pagination stays linear.
    pub fn new(hrefs: Vec<Value>, links: Vec<Value>) -> Links {
        Links {
            hrefs: Arc::new(hrefs),
            links: Arc::new(links),
        }
    }
}

impl Paging {
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// The template's data with the `pagination` every page shares, for
    /// [`Paging::page_data`]. Built once: pages share its values.
    pub fn base(&self, data: &Data) -> Data {
        let mut base = data.clone();
        base.insert("pagination".into(), self.parent.clone());
        base
    }

    /// The data of page `n`: `base` (from [`Paging::base`]) with the page's
    /// overlay. Without `links`, the keys 11ty adds once every page is
    /// located are left out, as they are when 11ty renders the permalink.
    pub fn page_data(&self, base: &Data, n: usize, links: Option<&Links>) -> Result<Data, Problem> {
        let count = self.chunks.len();
        let null = || Value::Null;
        let mut own = Data::new();
        own.insert("items".into(), self.chunks[n].clone());
        let mut page = Data::new();
        page.insert(
            "previous".into(),
            if n > 0 {
                self.normalized[n - 1].clone()
            } else {
                null()
            },
        );
        page.insert(
            "next".into(),
            if n + 1 < count {
                self.normalized[n + 1].clone()
            } else {
                null()
            },
        );
        page.insert("first".into(), self.normalized[0].clone());
        page.insert("last".into(), self.normalized[count - 1].clone());
        own.insert("page".into(), Value::Object(Arc::new(page)));
        own.insert("pageNumber".into(), Value::Number(n as f64));
        if let Some(l) = links {
            add_links(&mut own, n, count, l);
        }

        let mut overlay = Data::new();
        overlay.insert("pagination".into(), Value::Object(Arc::new(own)));
        overlay.insert("page".into(), Value::Object(Arc::new(Data::new())));
        if let Some(path) = &self.alias {
            crate::data::set_path(&mut overlay, path, self.normalized[n].clone()).map_err(
                |why| {
                    unsupported(
                        format!("`pagination.alias` can't be set: {why}"),
                        "Choose a plain name for the alias, like `alias: post`.",
                    )
                },
            )?;
        }
        Ok(overlay_data(base, &overlay))
    }
}

/// `Pagination.getOverrideDataLinks` and `getOverrideDataHrefs`.
fn add_links(own: &mut Data, n: usize, count: usize, l: &Links) {
    let at = |list: &[Value], i: usize| list.get(i).cloned().unwrap_or(Value::Undefined);
    let prev = |list: &[Value]| if n > 0 { at(list, n - 1) } else { Value::Null };
    let next = |list: &[Value]| {
        if n + 1 < count {
            at(list, n + 1)
        } else {
            Value::Null
        }
    };
    let first = |list: &[Value]| list.first().cloned().unwrap_or(Value::Null);
    let last = |list: &[Value]| list.last().cloned().unwrap_or(Value::Null);
    let links = Value::Array(l.links.clone());
    let hrefs = Value::Array(l.hrefs.clone());
    own.insert("previousPageLink".into(), prev(&l.links));
    own.insert("previous".into(), prev(&l.links));
    own.insert("nextPageLink".into(), next(&l.links));
    own.insert("next".into(), next(&l.links));
    own.insert("firstPageLink".into(), first(&l.links));
    own.insert("lastPageLink".into(), last(&l.links));
    own.insert("links".into(), links.clone());
    own.insert("pageLinks".into(), links);
    own.insert("previousPageHref".into(), prev(&l.hrefs));
    own.insert("nextPageHref".into(), next(&l.hrefs));
    own.insert("firstPageHref".into(), first(&l.hrefs));
    own.insert("lastPageHref".into(), last(&l.hrefs));
    own.insert("hrefs".into(), hrefs);
    let mut href = Data::new();
    href.insert("previous".into(), prev(&l.hrefs));
    href.insert("next".into(), next(&l.hrefs));
    href.insert("first".into(), first(&l.hrefs));
    href.insert("last".into(), last(&l.hrefs));
    own.insert("href".into(), Value::Object(Arc::new(href)));
}

/// `fallback` read through `ProxyWrap(overlay, fallback)`: the fallback's
/// keys first, then the overlay's own; plain objects on both sides merge.
fn overlay_data(fallback: &Data, overlay: &Data) -> Data {
    let mut out = Data::with_capacity(fallback.len() + overlay.len());
    for (k, f) in Value::js_entries(fallback) {
        let v = match overlay.get(k.as_str()) {
            None => f.clone(),
            Some(Value::Object(o)) => match f {
                Value::Object(fo) => Value::Object(Arc::new(overlay_data(fo, o))),
                _ => Value::Object(o.clone()),
            },
            Some(o) => o.clone(),
        };
        out.insert(k.clone(), v);
    }
    for (k, o) in overlay {
        if !fallback.contains_key(k.as_str()) {
            out.insert(k.clone(), o.clone());
        }
    }
    out
}

/// eleventy-utils' `DeepCopy` for one key: lists join, objects merge, a
/// plain-object target stays, anything else is replaced.
fn deep_copy_item(target: Option<&Value>, source: &Value) -> Value {
    match (target, source) {
        (Some(Value::Array(t)), Value::Array(s)) => {
            Value::Array(Arc::new(t.iter().chain(s.iter()).cloned().collect()))
        }
        (Some(Value::Object(t)), Value::Object(s)) => {
            let mut out = t.as_ref().clone();
            for (k, v) in Value::js_entries(s) {
                let merged = deep_copy_item(out.get(k.as_str()), v);
                out.insert(k.clone(), merged);
            }
            Value::Object(Arc::new(out))
        }
        (Some(t @ Value::Object(_)), _) => t.clone(),
        (_, s) => s.clone(),
    }
}

/// JavaScript's `===` for data values: objects, lists and dates are never
/// equal to a different copy.
fn strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        _ => false,
    }
}

fn shown(v: &Value) -> String {
    match v {
        Value::Undefined => "nothing".to_string(),
        Value::Date(_) => format!("a date ({})", json_text(v)),
        other => json_text(other),
    }
}

/// `lodash.get(root, path)`: `Undefined` when a step is missing.
fn get(root: &Data, path: &[String]) -> Value {
    let Some((first, rest)) = path.split_first() else {
        return Value::Undefined;
    };
    let mut cur = root
        .get(first.as_str())
        .cloned()
        .unwrap_or(Value::Undefined);
    for key in rest {
        if matches!(cur, Value::Undefined | Value::Null) {
            return Value::Undefined;
        }
        cur = cur.index(&Value::String(key.as_str().into()));
    }
    cur
}

/// lodash's `castPath`: a key with no `.` or `[...]`, or one `root` has,
/// is one step; anything else goes through `stringToPath`.
pub(crate) fn lodash_path(key: &str, root: &Data) -> Vec<String> {
    let plain = key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain || !is_deep(key) || root.contains_key(key) {
        return vec![key.to_string()];
    }
    string_to_path(key)
}

/// lodash's `reIsDeepProp`: a `.`, or a `[...]` with no bracket inside
/// (or a quoted string).
fn is_deep(key: &str) -> bool {
    if key.contains('.') {
        return true;
    }
    let b = key.as_bytes();
    (0..b.len()).any(|i| unquoted_bracket(b, i) || (b[i] == b'[' && bracket(key, i).is_some()))
}

fn unquoted_bracket(b: &[u8], i: usize) -> bool {
    if b[i] != b'[' {
        return false;
    }
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b']' => return true,
            b'[' => return false,
            _ => j += 1,
        }
    }
    false
}

/// lodash's `stringToPath`, without its memo.
fn string_to_path(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    if b.first() == Some(&b'.') {
        out.push(String::new());
    }
    let special = |c: u8| c == b'.' || c == b'[' || c == b']';
    let mut i = 0;
    while i < b.len() {
        if !special(b[i]) {
            let start = i;
            while i < b.len() && !special(b[i]) {
                i += 1;
            }
            out.push(s[start..i].to_string());
            continue;
        }
        if b[i] == b'['
            && let Some((key, end)) = bracket(s, i)
        {
            out.push(key);
            i = end;
            continue;
        }
        // An empty step: `.` or `[]` followed by `.`, `[]` or the end.
        let token = |at: usize| -> Option<usize> {
            if b.get(at) == Some(&b'.') {
                Some(at + 1)
            } else if b.get(at) == Some(&b'[') && b.get(at + 1) == Some(&b']') {
                Some(at + 2)
            } else {
                None
            }
        };
        if let Some(after) = token(i)
            && (after == b.len() || token(after).is_some())
        {
            out.push(String::new());
        }
        i += 1;
    }
    out
}

/// A `[0]`, `[-1.5]`, `["x"]` or `['x']` step at `i`: its key and where it
/// ends.
fn bracket(s: &str, i: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut j = i + 1;
    // A number.
    let start = j;
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    let digits = |j: &mut usize| {
        let from = *j;
        while b.get(*j).is_some_and(u8::is_ascii_digit) {
            *j += 1;
        }
        *j > from
    };
    if digits(&mut j) {
        let mut k = j;
        if b.get(k) == Some(&b'.') {
            k += 1;
            if digits(&mut k) {
                j = k;
            }
        }
        if b.get(j) == Some(&b']') {
            return Some((s[start..j].to_string(), j + 1));
        }
    }
    // A quoted string.
    let quote = *b.get(i + 1)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let mut j = i + 2;
    let mut key = String::new();
    while j < b.len() {
        match b[j] {
            b'\\' => {
                // `\\(\\)?` becomes the second backslash, if any.
                let next = s[j + 1..].chars().next()?;
                key.push(next);
                j += 1 + next.len_utf8();
            }
            c if c == quote => {
                return (b.get(j + 1) == Some(&b']')).then(|| (key, j + 2));
            }
            _ => {
                let c = s[j..].chars().next()?;
                key.push(c);
                j += c.len_utf8();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> Vec<String> {
        lodash_path(s, &Data::new())
    }

    /// Checked against lodash 4's `_.toPath` in node.
    #[test]
    fn paths_split_like_lodash() {
        assert_eq!(path("a"), ["a"]);
        assert_eq!(path("a.b.c"), ["a", "b", "c"]);
        assert_eq!(path("a[0].b"), ["a", "0", "b"]);
        assert_eq!(path("m[\"x y\"]"), ["m", "x y"]);
        assert_eq!(path("m['x.y']"), ["m", "x.y"]);
        assert_eq!(path("a[abc]"), ["a", "abc"]);
        assert_eq!(path("a[-1.5]"), ["a", "-1.5"]);
        assert_eq!(path(".a"), ["", "a"]);
        assert_eq!(path("a..b"), ["a", "", "b"]);
        assert_eq!(path("a."), ["a", ""]);
        assert_eq!(path("x-y"), ["x-y"]);
        assert_eq!(path("a[b"), ["a[b"]);
        let mut root = Data::new();
        root.insert("a.b".into(), Value::Null);
        assert_eq!(lodash_path("a.b", &root), ["a.b"]);
    }

    #[test]
    fn strict_equality_is_javascripts() {
        let n = |x: f64| Value::Number(x);
        assert!(strict_eq(&n(1.0), &n(1.0)));
        assert!(!strict_eq(&n(f64::NAN), &n(f64::NAN)));
        assert!(!strict_eq(&n(1.0), &Value::String("1".into())));
        let a = Value::Array(Arc::new(vec![]));
        assert!(!strict_eq(&a, &a.clone()));
    }

    #[test]
    fn overlays_read_like_11tys_proxy() {
        let obj = |pairs: &[(&str, Value)]| -> Data {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        };
        let s = |x: &str| Value::String(x.into());
        let fallback = obj(&[
            ("a", Value::Object(Arc::new(obj(&[("c", s("C"))])))),
            ("t", s("orig")),
        ]);
        let over = obj(&[
            ("x", s("new")),
            ("a", Value::Object(Arc::new(obj(&[("b", s("B"))])))),
            ("t", s("T")),
        ]);
        let out = overlay_data(&fallback, &over);
        let keys: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(keys, ["a", "t", "x"]);
        assert_eq!(json_text(&out["a"]), r#"{"c":"C","b":"B"}"#);
        assert_eq!(json_text(&out["t"]), r#""T""#);
    }
}
