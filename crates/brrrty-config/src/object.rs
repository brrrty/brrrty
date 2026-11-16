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

use indexmap::IndexMap;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map};

use crate::api::{MAX_MAP_SIZE, invalid};

/// A JavaScript object. Keys are stored in insertion order; [`js_keys`]
/// gives the order JavaScript lists them in.
///
/// [`js_keys`]: JsObject::js_keys
#[derive(Debug, Clone, Default)]
pub struct JsObject(pub IndexMap<String, Dynamic>);

/// A canonical array index, `"0"` to `"4294967294"`.
fn array_index(k: &str) -> Option<u32> {
    let canonical = k == "0" || (!k.starts_with('0') && k.bytes().all(|b| b.is_ascii_digit()));
    if !canonical || k.is_empty() || k.len() > 10 {
        return None;
    }
    k.parse::<u32>().ok().filter(|i| *i != u32::MAX)
}

impl JsObject {
    /// The keys as `Object.keys` lists them: integer-like keys first in
    /// numeric order, then the rest in insertion order.
    pub fn js_keys(&self) -> Vec<&String> {
        let mut index: Vec<(u32, &String)> = Vec::new();
        let mut rest = Vec::new();
        for k in self.0.keys() {
            match array_index(k) {
                Some(i) => index.push((i, k)),
                None => rest.push(k),
            }
        }
        index.sort_by_key(|(i, _)| *i);
        index.into_iter().map(|(_, k)| k).chain(rest).collect()
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

fn get(o: &mut JsObject, key: ImmutableString) -> Dynamic {
    o.0.get(key.as_str()).cloned().unwrap_or(Dynamic::UNIT)
}

fn set(o: &mut JsObject, key: ImmutableString, v: Dynamic) -> Result<(), Box<EvalAltResult>> {
    if o.0.len() >= MAX_MAP_SIZE && !o.0.contains_key(key.as_str()) {
        return Err(invalid(
            format!("an object may have at most {MAX_MAP_SIZE} keys"),
            rhai::Position::NONE,
        ));
    }
    o.0.insert(key.to_string(), v);
    Ok(())
}

fn debug(o: &mut JsObject) -> String {
    let items: Vec<String> = o
        .js_keys()
        .into_iter()
        .map(|k| format!("{k:?}: {:?}", o.0[k]))
        .collect();
    format!("object({{{}}})", items.join(", "))
}

pub fn register(e: &mut Engine) {
    e.register_type_with_name::<JsObject>("object")
        .register_fn("object", JsObject::default)
        .register_fn("object", |m: Map| {
            JsObject(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
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
            o.js_keys().into_iter().map(|k| o.0[k].clone()).collect()
        })
        .register_fn("entries", |o: &mut JsObject| -> Array {
            o.js_keys()
                .into_iter()
                .map(|k| Dynamic::from_array(vec![Dynamic::from(k.clone()), o.0[k].clone()]))
                .collect()
        })
        .register_fn("len", |o: &mut JsObject| o.0.len() as rhai::INT)
        .register_fn("is_empty", |o: &mut JsObject| o.0.is_empty())
        .register_fn("contains", |o: &mut JsObject, k: ImmutableString| {
            o.0.contains_key(k.as_str())
        })
        .register_fn("remove", |o: &mut JsObject, k: ImmutableString| {
            o.0.shift_remove(k.as_str()).unwrap_or(Dynamic::UNIT)
        })
        .register_fn("to_debug", debug)
        .register_fn("to_string", debug);
}
