//! Brrrty's values in the worker's wire forms, written as JSON text, and
//! the table of the values the workers already have.
//!
//! A list or object that something else also holds (the data the pages
//! share, as 11ty shares it) crosses once per build: the first request
//! that mentions it defines it, in a line of its own
//! (`{"op":"def","id":n,"v":...}`), and every request after refers to it
//! (`{"$R": n}`). So a request costs what it adds, not what the page's
//! data weighs, and a shared value is one JavaScript object however many
//! pages hold it.
//!
//! A definition numbers the lists and objects inside it itself: the root
//! is `n`, and each list or object first met while reading the value in
//! order takes the next number. The worker counts the same way, so a
//! value inside a definition can be referred to later (`pagination.items`
//! holding an item of the list it pages over) and is the same object.
//!
//! A list or object nothing else holds (one a template made for a call)
//! goes in the request itself and is forgotten.
//!
//! Wire forms, for the worker's `fromWire`:
//!   plain object   `{...}` when no key starts with `$`, else
//!                  `{"$o": [[key, value], ...]}`
//!   guarded object `{"$o": [...], "$c": {"missing": why, "list": why}}`
//!   a kept value   `{"$R": id}`
//!   a trap         `{"$t": why}` (an object entry's value)
//!   undefined, special numbers, dates and functions as `encode.mjs` has
//!   them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use brrrty_js::Value;

/// A hasher for addresses: they are spread already.
#[derive(Default)]
pub(crate) struct PtrHasher(u64);

impl Hasher for PtrHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = (n as u64 >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type PtrMap<V> = HashMap<usize, V, BuildHasherDefault<PtrHasher>>;

/// The value, or a value inside it, is an object a function may only be
/// handed whole by a render (`collections` when Brrrty can't answer every
/// question about it).
const GUARDED: u8 = 1;
/// The value, or a value inside it, is a function a call made: its handle
/// is the first run's, and a replay's worker has another.
const DYNAMIC: u8 = 2;

/// Why a JavaScript template can't read a name an open `collections`
/// object doesn't have (`{key}` is the name).
const OPEN_MISSING: &str = "reads `collections.{key}`, which Brrrty doesn't have: the site's 11ty config may add it, or a page in it can't be built (see the other problems)";

/// Why a JavaScript template can't list an open `collections` object.
const OPEN_LIST: &str = "lists the names in `collections`, and the site's 11ty config may add collections Brrrty can't see";

const WHOLE_COLLECTIONS: &str = "passes the whole `collections` object to a function, which Brrrty doesn't do yet; pass one collection, like `collections.post`";

/// Why a property Brrrty marks as unknown can't be handed to a function.
fn trap_reason(key: &str) -> String {
    match key {
        "templateContent" | "content" => format!(
            "reads `{key}` of a collection item, which 11ty fills in only as it renders pages"
        ),
        "template" => {
            "reads `template` of a collection item, 11ty's internal template object".to_string()
        }
        "rawInput" => {
            "reads `rawInput` of a JavaScript template's page, which in 11ty is its instance object, not text".to_string()
        }
        _ => format!("reads `{key}`, which Brrrty doesn't hand to JavaScript functions yet"),
    }
}

fn ptr_of(v: &Value) -> Option<usize> {
    match v {
        Value::Object(o) => Some(Arc::as_ptr(o) as usize),
        Value::Array(a) => Some(Arc::as_ptr(a) as usize),
        _ => None,
    }
}

/// Whether something besides the caller holds the list or object.
fn shared(v: &Value) -> bool {
    match v {
        Value::Object(o) => Arc::strong_count(o) > 1,
        Value::Array(a) => Arc::strong_count(a) > 1,
        _ => false,
    }
}

/// A definition whose value holds a function a call made: kept as text,
/// for a replay to send with its own handles.
pub(crate) struct DynamicDef {
    pub line: Arc<str>,
    /// The definitions like it that it refers to, by root.
    pub needs: Vec<u32>,
}

/// What writing one request added.
#[derive(Default)]
pub(crate) struct Added {
    /// The definitions to send before the request, in order, and whether
    /// each is [`DynamicDef`].
    pub lines: Vec<(Arc<str>, bool)>,
    /// The roots of the dynamic definitions the request needs, each after
    /// the ones it needs itself.
    pub dynamic: Vec<u32>,
    /// The request itself holds a function a call made.
    pub made: bool,
}

/// The lists and objects the workers have, by address. Each is held, so
/// its address stays its own.
#[derive(Default)]
pub(crate) struct Table {
    ids: PtrMap<u32>,
    /// The root of each definition, held.
    holds: Vec<Value>,
    /// Per id: [`GUARDED`] and [`DYNAMIC`], of the value and what is in it.
    flags: Vec<u8>,
    /// Per id: the root of its definition.
    roots: Vec<u32>,
    /// Function handles from here on were made by calls.
    pub load_handles: Option<u32>,
    pub dynamic: HashMap<u32, DynamicDef>,
    /// Every definition's line, in order, and whether it is dynamic: a
    /// worker that must make its values afresh is sent them again.
    pub lines: Vec<(Arc<str>, bool)>,
}

/// Where a value is being written.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// In a request: a list or object nothing else holds goes in whole.
    Request,
    /// In a definition: each list and object in it takes the next id.
    Definition,
}

struct Writer<'t> {
    table: &'t mut Table,
    added: &'t mut Added,
    /// A render: a guarded `collections` object may cross.
    guard: bool,
    /// The root of the definition being written.
    root: Option<u32>,
    /// Dynamic definitions the one being written refers to.
    needs: Vec<u32>,
    /// The addresses the definition being written numbered.
    numbered: Vec<usize>,
}

impl Table {
    /// Forget every value: a call changed one in the worker. Ids go on
    /// rising, so none is reused.
    pub fn forget(&mut self) {
        self.ids.clear();
    }

    /// How many ids there are.
    #[cfg(test)]
    pub fn len(&self) -> u32 {
        self.flags.len() as u32
    }

    /// A page's data for `pageOpen` or a render: the object itself is the
    /// page's own, written in place; what it holds is referred to.
    pub fn page_data(
        &mut self,
        data: &Value,
        guard: bool,
        out: &mut String,
        added: &mut Added,
    ) -> Result<(), String> {
        let mut w = Writer {
            table: self,
            added,
            guard,
            root: None,
            needs: Vec::new(),
            numbered: Vec::new(),
        };
        match data {
            Value::Object(o) => w.object(o, Place::Request, out, guard).map(|_| ()),
            other => w.value(other, Place::Request, out).map(|_| ()),
        }
    }

    /// A value handed to a call.
    pub fn argument(
        &mut self,
        v: &Value,
        guard: bool,
        out: &mut String,
        added: &mut Added,
    ) -> Result<(), String> {
        let mut w = Writer {
            table: self,
            added,
            guard,
            root: None,
            needs: Vec::new(),
            numbered: Vec::new(),
        };
        w.value(v, Place::Request, out).map(|_| ())
    }

    /// Whether `h` is a function a call made.
    pub fn made_by_call(&self, h: u32) -> bool {
        self.load_handles.is_some_and(|n| h >= n)
    }
}

fn is_trap(k: &str, x: &Value) -> bool {
    matches!(x, Value::Undefined) && brrrty_js::traps::TRAP_KEYS.contains(&k)
}

fn withheld_reason(k: &str) -> String {
    format!("reads `{k}` of another page's `data`, which Brrrty can't supply")
}

fn trap(out: &mut String, why: &str) {
    out.push_str("{\"$t\":");
    string(out, why);
    out.push('}');
}

/// The `$c` of an object that traps a read or a listing, as JSON text.
fn guard_of(o: &indexmap::IndexMap<String, Value>) -> Option<String> {
    let open = brrrty_js::traps::is_open_object(o);
    let unordered = brrrty_js::traps::unordered_reason(o);
    if !open && unordered.is_none() {
        return None;
    }
    let mut c = String::from("{");
    if open {
        c.push_str("\"missing\":");
        string(&mut c, OPEN_MISSING);
    }
    let list = match (&unordered, open) {
        (Some(why), _) => Some(format!("lists the names in `collections`, {why}")),
        (None, true) => Some(OPEN_LIST.to_string()),
        (None, false) => None,
    };
    if let Some(list) = list {
        if open {
            c.push(',');
        }
        c.push_str("\"list\":");
        string(&mut c, &list);
    }
    c.push('}');
    Some(c)
}

impl Writer<'_> {
    /// Write `v`; what it holds, as [`GUARDED`] and [`DYNAMIC`].
    fn value(&mut self, v: &Value, place: Place, out: &mut String) -> Result<u8, String> {
        let Some(p) = ptr_of(v) else {
            if let Value::Function(h) = v
                && self.table.made_by_call(h.0)
            {
                if self.root.is_none() {
                    self.added.made = true;
                }
                scalar(out, v);
                return Ok(DYNAMIC);
            }
            scalar(out, v);
            return Ok(0);
        };
        if let Some(&id) = self.table.ids.get(&p) {
            return self.refer(id, out);
        }
        match place {
            Place::Definition => self.container(v, p, out),
            Place::Request if shared(v) => {
                let id = self.define(v, p)?;
                self.refer(id, out)
            }
            Place::Request => match v {
                Value::Array(a) => self.array(a, Place::Request, out),
                Value::Object(o) => self.object(o, Place::Request, out, false),
                _ => Ok(0),
            },
        }
    }

    fn refer(&mut self, id: u32, out: &mut String) -> Result<u8, String> {
        let flags = self.table.flags[id as usize];
        if flags & GUARDED != 0 && !self.guard {
            return Err(WHOLE_COLLECTIONS.to_string());
        }
        if flags & DYNAMIC != 0 {
            let root = self.table.roots[id as usize];
            if Some(root) != self.root {
                self.need(root);
            }
        }
        let _ = write!(out, "{{\"$R\":{id}}}");
        Ok(flags)
    }

    /// Note that what is being written needs the dynamic definition `root`.
    fn need(&mut self, root: u32) {
        if !self.table.dynamic.contains_key(&root) {
            return;
        }
        if !self.needs.contains(&root) {
            self.needs.push(root);
        }
        let mut stack = vec![root];
        while let Some(r) = stack.pop() {
            if self.added.dynamic.contains(&r) {
                continue;
            }
            self.added.dynamic.push(r);
            if let Some(d) = self.table.dynamic.get(&r) {
                stack.extend(d.needs.iter().copied());
            }
        }
        self.added.dynamic.sort_unstable();
    }

    /// Define `v` (at address `p`) in a line of its own.
    fn define(&mut self, v: &Value, p: usize) -> Result<u32, String> {
        let id = self.table.flags.len() as u32;
        let (root_before, needs_before) = (self.root, std::mem::take(&mut self.needs));
        self.root = Some(id);
        self.numbered.clear();
        let mut text = String::with_capacity(256);
        let _ = write!(text, "{{\"op\":\"def\",\"id\":{id},\"v\":");
        let start = self.table.flags.len();
        let r = self.container(v, p, &mut text);
        let needs = std::mem::replace(&mut self.needs, needs_before);
        self.root = root_before;
        let flags = match r {
            Ok(f) => f,
            Err(why) => {
                // Nothing of it was sent: forget what it numbered.
                for p in self.numbered.drain(..) {
                    self.table.ids.remove(&p);
                }
                self.table.flags.truncate(start);
                self.table.roots.truncate(start);
                return Err(why);
            }
        };
        let _ = write!(text, ",\"n\":{}}}", self.table.flags.len() - start);
        self.table.holds.push(v.clone());
        let line: Arc<str> = Arc::from(text);
        let dynamic = flags & DYNAMIC != 0;
        if dynamic {
            self.table.dynamic.insert(
                id,
                DynamicDef {
                    line: line.clone(),
                    needs,
                },
            );
            self.need(id);
        }
        self.table.lines.push((line.clone(), dynamic));
        self.added.lines.push((line, dynamic));
        Ok(id)
    }

    /// A list or object of a definition: it takes the next id.
    fn container(&mut self, v: &Value, p: usize, out: &mut String) -> Result<u8, String> {
        let id = self.table.flags.len() as u32;
        self.table.ids.insert(p, id);
        self.numbered.push(p);
        self.table.flags.push(0);
        self.table.roots.push(self.root.unwrap_or(id));
        let flags = match v {
            Value::Array(a) => self.array(a, Place::Definition, out)?,
            Value::Object(o) => self.object(o, Place::Definition, out, false)?,
            _ => 0,
        };
        self.table.flags[id as usize] = flags;
        Ok(flags)
    }

    fn array(&mut self, a: &[Value], place: Place, out: &mut String) -> Result<u8, String> {
        let mut flags = 0;
        out.push('[');
        for (i, x) in a.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            flags |= self.value(x, place, out)?;
        }
        out.push(']');
        Ok(flags)
    }

    /// An object's own wire form. `top`: a render's own data, whose
    /// `content` (a layout's, given `undefined` by a template's render) is
    /// a value, not an item's marker.
    fn object(
        &mut self,
        o: &indexmap::IndexMap<String, Value>,
        place: Place,
        out: &mut String,
        top: bool,
    ) -> Result<u8, String> {
        let guard = guard_of(o);
        if guard.is_some() && !self.guard {
            return Err(WHOLE_COLLECTIONS.to_string());
        }
        let mut flags = if guard.is_some() { GUARDED } else { 0 };
        let escaped = guard.is_some() || o.keys().any(|k| k.starts_with('$'));
        out.push_str(if escaped { "{\"$o\":[" } else { "{" });
        let mut first = true;
        let mut entry = |out: &mut String, k: &str| {
            if !first {
                out.push(',');
            }
            first = false;
            if escaped {
                out.push('[');
                string(out, k);
                out.push(',');
            } else {
                string(out, k);
                out.push(':');
            }
        };
        for (k, x) in o.iter() {
            entry(out, k);
            if is_trap(k, x) && !(top && brrrty_js::traps::CONTENT_KEYS.contains(&k.as_str())) {
                trap(out, &trap_reason(k));
            } else {
                flags |= self.value(x, place, out)?;
            }
            if escaped {
                out.push(']');
            }
        }
        if let Some(keys) = brrrty_js::traps::withheld_keys(o) {
            for k in keys.iter().filter(|k| !o.contains_key(k.as_str())) {
                entry(out, k);
                trap(out, &withheld_reason(k));
                if escaped {
                    out.push(']');
                }
            }
        }
        match guard {
            Some(c) => {
                out.push_str("],\"$c\":");
                out.push_str(&c);
                out.push('}');
            }
            None => out.push_str(if escaped { "]}" } else { "}" }),
        }
        Ok(flags)
    }
}

/// A value that is no list or object, as JSON text.
fn scalar(out: &mut String, v: &Value) {
    match v {
        Value::Undefined => out.push_str("{\"$u\":1}"),
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => number(out, *n),
        Value::String(s) => string(out, s),
        Value::Date(d) => {
            let _ = write!(out, "{{\"$d\":{}}}", d.timestamp_millis());
        }
        Value::Function(h) => {
            let _ = write!(out, "{{\"$f\":{}}}", h.0);
        }
        Value::Array(_) | Value::Object(_) => out.push_str("null"),
    }
}

/// A number as JSON text that reads back as the same number.
pub(crate) fn number(out: &mut String, n: f64) {
    if n.is_nan() {
        out.push_str("{\"$n\":\"NaN\"}");
    } else if n == f64::INFINITY {
        out.push_str("{\"$n\":\"Infinity\"}");
    } else if n == f64::NEG_INFINITY {
        out.push_str("{\"$n\":\"-Infinity\"}");
    } else if n == 0.0 {
        out.push_str(if n.is_sign_negative() {
            "{\"$n\":\"-0\"}"
        } else {
            "0"
        });
    } else if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        let _ = write!(out, "{}", n as i64);
    } else {
        // Rust's shortest text that reads back as `n`: `1.5`, `1e21`.
        let _ = write!(out, "{n:?}");
    }
}

/// A string as JSON text.
pub(crate) fn string(out: &mut String, s: &str) {
    out.push('"');
    let bytes = s.as_bytes();
    let mut from = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let esc: Option<&str> = match b {
            b'"' => Some("\\\""),
            b'\\' => Some("\\\\"),
            b'\n' => Some("\\n"),
            b'\r' => Some("\\r"),
            b'\t' => Some("\\t"),
            0..=0x1f => None,
            _ => continue,
        };
        out.push_str(&s[from..i]);
        match esc {
            Some(e) => out.push_str(e),
            None => {
                let _ = write!(out, "\\u{b:04x}");
            }
        }
        from = i + 1;
    }
    out.push_str(&s[from..]);
    out.push('"');
}

/// A list of strings as JSON text.
pub(crate) fn strings<S: AsRef<str>>(out: &mut String, list: &[S]) {
    out.push('[');
    for (i, s) in list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        string(out, s.as_ref());
    }
    out.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn value(j: serde_json::Value) -> Value {
        Value::from_json(&j)
    }

    #[test]
    fn a_shared_value_is_defined_once_and_referred_to_after() {
        let mut t = Table::default();
        let big = value(json!({"a": [1, 2, {"b": "x"}], "c": null}));
        let page = Value::Object(Arc::new(
            [
                ("big".to_string(), big.clone()),
                ("n".to_string(), Value::Number(1.5)),
            ]
            .into_iter()
            .collect(),
        ));
        let (mut out, mut added) = (String::new(), Added::default());
        t.page_data(&page, false, &mut out, &mut added).unwrap();
        assert_eq!(out, r#"{"big":{"$R":0},"n":1.5}"#);
        assert_eq!(added.lines.len(), 1);
        assert_eq!(
            &*added.lines[0].0,
            r#"{"op":"def","id":0,"v":{"a":[1,2,{"b":"x"}],"c":null},"n":3}"#
        );
        // The list inside took id 1 and the object in it id 2.
        let Value::Object(o) = &big else { panic!() };
        let inner = o.get("a").unwrap().clone();
        let (mut out, mut added) = (String::new(), Added::default());
        t.argument(&inner, false, &mut out, &mut added).unwrap();
        assert_eq!(out, r#"{"$R":1}"#);
        assert!(added.lines.is_empty());
    }

    #[test]
    fn a_value_nothing_else_holds_goes_in_the_request() {
        let mut t = Table::default();
        let (mut out, mut added) = (String::new(), Added::default());
        t.argument(&value(json!([1, {"k": "v"}])), false, &mut out, &mut added)
            .unwrap();
        assert_eq!(out, r#"[1,{"k":"v"}]"#);
        assert!(added.lines.is_empty());
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn keys_that_look_like_wire_forms_are_escaped() {
        let mut t = Table::default();
        let (mut out, mut added) = (String::new(), Added::default());
        t.argument(&value(json!({"$R": 3})), false, &mut out, &mut added)
            .unwrap();
        assert_eq!(out, r#"{"$o":[["$R",3]]}"#);
    }

    #[test]
    fn numbers_and_strings_read_back_as_themselves() {
        for n in [
            0.5,
            1e21,
            -1.25e-7,
            123456789.0,
            9.007199254740993e15,
            1e300,
        ] {
            let mut s = String::new();
            number(&mut s, n);
            assert_eq!(serde_json::from_str::<f64>(&s).unwrap(), n, "{s}");
        }
        let mut s = String::new();
        string(&mut s, "a\"b\\c\n\u{1}\u{2028}é");
        assert_eq!(
            serde_json::from_str::<String>(&s).unwrap(),
            "a\"b\\c\n\u{1}\u{2028}é"
        );
    }

    #[test]
    fn forgetting_defines_a_value_again_under_a_new_id() {
        let mut t = Table::default();
        let big = value(json!([1, 2]));
        let keep = big.clone();
        let (mut out, mut added) = (String::new(), Added::default());
        t.argument(&big, false, &mut out, &mut added).unwrap();
        assert_eq!(out, r#"{"$R":0}"#);
        t.forget();
        let (mut out, mut added) = (String::new(), Added::default());
        t.argument(&big, false, &mut out, &mut added).unwrap();
        assert_eq!(out, r#"{"$R":1}"#);
        assert_eq!(added.lines.len(), 1);
        drop(keep);
    }
}
