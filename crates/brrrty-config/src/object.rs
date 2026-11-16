//! `object()`: a map that keeps its keys in JavaScript's order.
//!
//! Rhai's own maps (`#{ ... }`) are sorted by key, but a JavaScript object
//! keeps the order its keys were added in, except that integer-like keys
//! (`"0"`, `"17"`) come first in numeric order. That order shows in output
//! whenever an object is dumped or looped over, so a script that builds
//! data for a template should use `object()`:
//!
//! ```rhai
//! let o = object();
//! o.zebra = 1;       // or o["zebra"] = 1
//! o.apple = 2;
//! o.keys()           // ["zebra", "apple"]
//! ```
//!
//! `object(map)` copies a map in (its keys arrive sorted). Data that comes
//! into a script from Brrrty (page data, collections) arrives as objects
//! too, so reading and returning it keeps its order.

use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map};

use crate::api::{MAX_MAP_SIZE, invalid};
use crate::convert::{to_dynamic, to_value_at};

/// JavaScript's `null`, as a value a script can hold. It is equal to `()`
/// (`undefined`), and falsy, but it stays `null` when it comes back out:
/// `null_value()` makes one. Scalars and array items that were `null` in
/// Brrrty's data arrive as this; a key of an object that holds `null`
/// reads as `()`, and the object remembers it was `null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsNull;

/// A JavaScript object. Keys are in insertion order; [`js_keys`] gives the
/// order JavaScript lists them in.
///
/// Data from Brrrty is shared, not copied: an object stays one `Arc`
/// until the script changes it.
///
/// [`js_keys`]: JsObject::js_keys
#[derive(Debug, Clone)]
pub enum JsObject {
    Shared(Arc<IndexMap<String, Value>>),
    Owned(IndexMap<String, Dynamic>),
}

impl Default for JsObject {
    fn default() -> Self {
        JsObject::Owned(IndexMap::new())
    }
}

/// A canonical array index, `"0"` to `"4294967294"`.
fn array_index(k: &str) -> Option<u32> {
    let canonical = k == "0" || (!k.starts_with('0') && k.bytes().all(|b| b.is_ascii_digit()));
    if !canonical || k.is_empty() || k.len() > 10 {
        return None;
    }
    k.parse::<u32>().ok().filter(|i| *i != u32::MAX)
}

impl JsObject {
    fn insertion_keys(&self) -> Vec<&String> {
        match self {
            JsObject::Shared(m) => m.keys().collect(),
            JsObject::Owned(m) => m.keys().collect(),
        }
    }

    /// The keys as `Object.keys` lists them: integer-like keys first in
    /// numeric order, then the rest in insertion order.
    pub fn js_keys(&self) -> Vec<&String> {
        let mut index: Vec<(u32, &String)> = Vec::new();
        let mut rest = Vec::new();
        for k in self.insertion_keys() {
            match array_index(k) {
                Some(i) => index.push((i, k)),
                None => rest.push(k),
            }
        }
        index.sort_by_key(|(i, _)| *i);
        index.into_iter().map(|(_, k)| k).chain(rest).collect()
    }

    pub fn len(&self) -> usize {
        match self {
            JsObject::Shared(m) => m.len(),
            JsObject::Owned(m) => m.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, k: &str) -> bool {
        match self {
            JsObject::Shared(m) => m.contains_key(k),
            JsObject::Owned(m) => m.contains_key(k),
        }
    }

    /// The value at `k` as stored: a `null` is a [`JsNull`].
    pub fn raw(&self, k: &str) -> Option<Dynamic> {
        match self {
            JsObject::Shared(m) => m.get(k).map(to_dynamic),
            JsObject::Owned(m) => m.get(k).cloned(),
        }
    }

    /// The map, copied out of its `Arc` first if it is shared.
    fn owned(&mut self) -> &mut IndexMap<String, Dynamic> {
        if let JsObject::Shared(m) = self {
            *self = JsObject::Owned(m.iter().map(|(k, v)| (k.clone(), to_dynamic(v))).collect());
        }
        match self {
            JsObject::Owned(m) => m,
            JsObject::Shared(_) => unreachable!(),
        }
    }

    /// As data. A shared object that wasn't changed is returned as is.
    pub fn to_value(&self, depth: usize) -> Result<Value, String> {
        match self {
            JsObject::Shared(m) => Ok(Value::Object(m.clone())),
            JsObject::Owned(m) => {
                let mut out = IndexMap::new();
                for (k, x) in m {
                    out.insert(k.clone(), to_value_at(x, depth + 1)?);
                }
                Ok(Value::Object(Arc::new(out)))
            }
        }
    }
}

impl IntoIterator for JsObject {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;

    /// `for k in o` walks the keys, like JavaScript's `for (k in o)`.
    /// (Rhai's `for (k, v) in o` gives a counter, not a value.)
    fn into_iter(self) -> Self::IntoIter {
        self.js_keys()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
    }
}

fn unnull(d: Dynamic) -> Dynamic {
    if d.is::<JsNull>() { Dynamic::UNIT } else { d }
}

fn get(o: &mut JsObject, key: ImmutableString) -> Dynamic {
    o.raw(key.as_str()).map_or(Dynamic::UNIT, unnull)
}

fn set(o: &mut JsObject, key: ImmutableString, v: Dynamic) -> Result<(), Box<EvalAltResult>> {
    if o.len() >= MAX_MAP_SIZE && !o.contains(key.as_str()) {
        return Err(invalid(
            format!("an object may have at most {MAX_MAP_SIZE} keys"),
            rhai::Position::NONE,
        ));
    }
    o.owned().insert(key.to_string(), v);
    Ok(())
}

fn debug(o: &mut JsObject) -> String {
    let items: Vec<String> = o
        .js_keys()
        .into_iter()
        .map(|k| format!("{k:?}: {:?}", o.raw(k).unwrap_or(Dynamic::UNIT)))
        .collect();
    format!("object({{{}}})", items.join(", "))
}

pub fn register(e: &mut Engine) {
    e.register_type_with_name::<JsNull>("null")
        .register_fn("null_value", || JsNull)
        .register_fn("==", |_: JsNull, _: ()| true)
        .register_fn("==", |_: (), _: JsNull| true)
        .register_fn("==", |_: JsNull, _: JsNull| true)
        .register_fn("!=", |_: JsNull, _: ()| false)
        .register_fn("!=", |_: (), _: JsNull| false)
        .register_fn("!=", |_: JsNull, _: JsNull| false)
        .register_fn("to_string", |_: &mut JsNull| "null".to_string())
        .register_fn("to_debug", |_: &mut JsNull| "null".to_string());
    let mut globals = rhai::Module::new();
    globals.set_var("NULL", JsNull);
    e.register_global_module(globals.into());

    e.register_type_with_name::<JsObject>("object")
        .register_fn("object", JsObject::default)
        .register_fn("object", |m: Map| {
            JsObject::Owned(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        })
        .register_indexer_get(get)
        .register_indexer_set(set)
        .register_iterator::<JsObject>()
        .register_fn("keys", |o: &mut JsObject| -> Array {
            o.js_keys()
                .into_iter()
                .map(|k| Dynamic::from(k.clone()))
                .collect()
        })
        .register_fn("values", |o: &mut JsObject| -> Array {
            o.js_keys()
                .into_iter()
                .map(|k| o.raw(k).unwrap_or(Dynamic::UNIT))
                .collect()
        })
        .register_fn("entries", |o: &mut JsObject| -> Array {
            o.js_keys()
                .into_iter()
                .map(|k| {
                    Dynamic::from_array(vec![
                        Dynamic::from(k.clone()),
                        o.raw(k).unwrap_or(Dynamic::UNIT),
                    ])
                })
                .collect()
        })
        .register_fn("len", |o: &mut JsObject| o.len() as rhai::INT)
        .register_fn("is_empty", |o: &mut JsObject| o.is_empty())
        .register_fn("contains", |o: &mut JsObject, k: ImmutableString| {
            o.contains(k.as_str())
        })
        .register_fn("remove", |o: &mut JsObject, k: ImmutableString| {
            unnull(o.owned().shift_remove(k.as_str()).unwrap_or(Dynamic::UNIT))
        })
        .register_fn("to_debug", debug)
        .register_fn("to_string", debug);
}
