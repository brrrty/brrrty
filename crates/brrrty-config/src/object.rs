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
//!
//! Collection items are different: 11ty shares one item with every page
//! and every collection, so a script gets the item itself, read-only
//! ([`JsObject::Frozen`]), and so is everything read through it (objects
//! are frozen too, and arrays are [`FrozenArray`]s). Returning an item, or
//! anything holding one, gives back that same item.

use std::cell::RefCell;
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_js::traps::TRAP_KEYS;
use indexmap::IndexMap;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};

use crate::api::{MAX_MAP_SIZE, invalid};
use crate::convert::{to_dynamic, to_value_at};

/// JavaScript's `null`, as a value a script can hold. It is equal to `()`
/// (`undefined`), and falsy, but it stays `null` when it comes back out:
/// `null_value()` makes one. Everything that was `null` in Brrrty's data
/// arrives as this, an object key included, so copying a value keeps its
/// `null`. (`x ?? d` only defaults `()`; test `x == ()` to catch both.)
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
    /// A collection item, or an object read through one: the same object,
    /// which can't be changed.
    Frozen(Arc<IndexMap<String, Value>>),
}

/// An array read through a collection item: the same array, read-only.
/// `to_array()` copies it into an array the script can change.
#[derive(Debug, Clone)]
pub struct FrozenArray(pub Arc<Vec<Value>>);

thread_local! {
    /// A read a collection made that 11ty would have stopped at, such as an
    /// item's `templateContent`. A script can catch the error it throws, so
    /// the run checks this too.
    static PREMATURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Forget premature reads recorded on this thread.
pub fn clear_premature() {
    PREMATURE.with(|p| p.borrow_mut().take());
}

/// The first premature read since [`clear_premature`].
pub fn take_premature() -> Option<String> {
    PREMATURE.with(|p| p.borrow_mut().take())
}

/// `v` read through a collection item: objects and arrays stay the same
/// values, read-only.
pub fn frozen_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::Object(o) => Dynamic::from(JsObject::Frozen(o.clone())),
        Value::Array(a) => Dynamic::from(FrozenArray(a.clone())),
        other => to_dynamic(other),
    }
}

/// True when `new`, written back to a key that holds `old`, is the same
/// value. Rhai writes a value back after a method call on a property chain
/// (`post.data.title.len()`), changed or not.
fn same(old: &Value, new: &Dynamic) -> bool {
    match (
        old,
        new.read_lock::<JsObject>(),
        new.read_lock::<FrozenArray>(),
    ) {
        (Value::Object(a), Some(o), _) => match &*o {
            JsObject::Frozen(b) | JsObject::Shared(b) if Arc::ptr_eq(a, b) => true,
            other => other.to_value(0).is_ok_and(|v| v == *old),
        },
        (Value::Array(a), _, Some(b)) => Arc::ptr_eq(a, &b.0) || **a == *b.0,
        _ => to_value_at(new, 0).is_ok_and(|v| v == *old),
    }
}

fn read_only(what: String) -> Box<EvalAltResult> {
    invalid(
        format!(
            "{what}: a collection item is read-only. 11ty shares one item with every page, so a change would show everywhere, and Brrrty can't follow that. Build a new `object()` with the values you need"
        ),
        Position::NONE,
    )
}

/// A read of `key` from an item, which 11ty doesn't have yet when it
/// builds collections: `templateContent` and `content` throw "Tried to use
/// templateContent too early", and `template` and `data.collections` are
/// 11ty's internals.
fn premature(o: &IndexMap<String, Value>, key: &str) -> Result<(), Box<EvalAltResult>> {
    if matches!(o.get(key), Some(Value::Undefined)) && TRAP_KEYS.contains(&key) {
        let why = if brrrty_js::traps::CONTENT_KEYS.contains(&key) {
            format!(
                "it reads `{key}` of a collection item, before 11ty renders any content (11ty stops the build here: \"Tried to use templateContent too early\")"
            )
        } else {
            format!("it reads `{key}` of a collection item, which Brrrty doesn't have")
        };
        PREMATURE.with(|p| {
            p.borrow_mut().get_or_insert_with(|| why.clone());
        });
        return Err(invalid(why, Position::NONE));
    }
    Ok(())
}

impl Default for JsObject {
    fn default() -> Self {
        JsObject::Owned(IndexMap::new())
    }
}

const FN_TEXT: &str = "function () { [native code] }";

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
            JsObject::Shared(m) | JsObject::Frozen(m) => m.keys().collect(),
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
            JsObject::Shared(m) | JsObject::Frozen(m) => m.len(),
            JsObject::Owned(m) => m.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, k: &str) -> bool {
        match self {
            JsObject::Shared(m) | JsObject::Frozen(m) => m.contains_key(k),
            JsObject::Owned(m) => m.contains_key(k),
        }
    }

    /// The value a script reads at `k`: a `null` is a [`JsNull`]. A read of
    /// an item's content gives it when the page being rendered has it (a
    /// filter's argument); a read 11ty would stop at is an error.
    pub fn read(&self, k: &str) -> Result<Option<Dynamic>, Box<EvalAltResult>> {
        Ok(match self {
            JsObject::Shared(m) => brrrty_js::traps::read(m, k, m.get(k).cloned())
                .as_ref()
                .map(to_dynamic),
            JsObject::Frozen(m) => {
                premature(m, k)?;
                m.get(k).map(frozen_dynamic)
            }
            JsObject::Owned(m) => m.get(k).cloned(),
        })
    }

    /// Every value, as [`JsObject::read`] reads it, in key order.
    fn read_all(&self) -> Result<Vec<(String, Dynamic)>, Box<EvalAltResult>> {
        if let JsObject::Shared(m) = self {
            brrrty_js::traps::entries(m);
        }
        self.js_keys()
            .into_iter()
            .map(|k| Ok((k.clone(), self.read(k)?.unwrap_or(Dynamic::UNIT))))
            .collect()
    }

    /// The map, copied out of its `Arc` first if it is shared. An item
    /// can't be changed.
    fn owned(&mut self, what: &str) -> Result<&mut IndexMap<String, Dynamic>, Box<EvalAltResult>> {
        if let JsObject::Frozen(_) = self {
            return Err(read_only(what.to_string()));
        }
        if let JsObject::Shared(m) = self {
            *self = JsObject::Owned(m.iter().map(|(k, v)| (k.clone(), to_dynamic(v))).collect());
        }
        match self {
            JsObject::Owned(m) => Ok(m),
            _ => unreachable!(),
        }
    }

    /// As data. A shared object that wasn't changed, and an item, are
    /// returned as they are.
    pub fn to_value(&self, depth: usize) -> Result<Value, String> {
        match self {
            JsObject::Shared(m) | JsObject::Frozen(m) => Ok(Value::Object(m.clone())),
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

impl IntoIterator for FrozenArray {
    type Item = Dynamic;
    type IntoIter = std::vec::IntoIter<Dynamic>;

    fn into_iter(self) -> Self::IntoIter {
        self.0
            .iter()
            .map(frozen_dynamic)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

fn get(o: &mut JsObject, key: ImmutableString) -> Result<Dynamic, Box<EvalAltResult>> {
    Ok(o.read(key.as_str())?.unwrap_or(Dynamic::UNIT))
}

fn set(o: &mut JsObject, key: ImmutableString, v: Dynamic) -> Result<(), Box<EvalAltResult>> {
    if let JsObject::Frozen(m) = o {
        // A write-back of what was read is not a change.
        return match m.get(key.as_str()) {
            Some(old) if same(old, &v) => Ok(()),
            _ => Err(read_only(format!("it sets `{key}`"))),
        };
    }
    if o.len() >= MAX_MAP_SIZE && !o.contains(key.as_str()) {
        return Err(invalid(
            format!("an object may have at most {MAX_MAP_SIZE} keys"),
            rhai::Position::NONE,
        ));
    }
    o.owned("")?.insert(key.to_string(), v);
    Ok(())
}

/// `sort` or `reverse` on an array read through an item: JavaScript would
/// change the item's own array.
fn frozen_change(what: &str) -> Box<EvalAltResult> {
    read_only(format!(
        "it calls `{what}` on an array read through a collection item, which would change the item's own array. Copy it first with `to_array()`"
    ))
}

/// JavaScript's `ToIntegerOrInfinity` for an array index argument, then
/// the `relativeStart` clamp of `Array.prototype.slice`.
fn relative(v: &Dynamic, len: usize, default: usize) -> Result<usize, Box<EvalAltResult>> {
    if v.is_unit() {
        return Ok(default);
    }
    let n = to_value_at(v, 0)
        .map_err(|why| invalid(format!("slice: {why}"), Position::NONE))?
        .to_js_number();
    let n = if n.is_nan() { 0.0 } else { n.trunc() };
    let len = len as f64;
    Ok(if n < 0.0 {
        (len + n).max(0.0) as usize
    } else {
        n.min(len) as usize
    })
}

/// `arr.slice(start, end)`.
fn slice(a: &mut Array, start: Dynamic, end: Dynamic) -> Result<Array, Box<EvalAltResult>> {
    let from = relative(&start, a.len(), 0)?;
    let to = relative(&end, a.len(), a.len())?;
    Ok(if from < to {
        a[from..to].to_vec()
    } else {
        Vec::new()
    })
}

/// Arrays as JavaScript has them: `reverse()` reverses in place and
/// returns the array, and `slice(start, end)`.
fn register_arrays(e: &mut Engine) {
    e.register_fn("reverse", |a: &mut Array| -> Array {
        a.reverse();
        a.clone()
    })
    .register_fn("slice", |a: &mut Array, s: Dynamic| {
        slice(a, s, Dynamic::UNIT)
    })
    .register_fn("slice", slice);
    e.register_type_with_name::<FrozenArray>("frozen_array")
        .register_iterator::<FrozenArray>()
        .register_fn("len", |a: &mut FrozenArray| a.0.len() as rhai::INT)
        .register_fn("is_empty", |a: &mut FrozenArray| a.0.is_empty())
        .register_fn("to_array", |a: &mut FrozenArray| -> Array {
            a.0.iter().map(frozen_dynamic).collect()
        })
        .register_fn("contains", |a: &mut FrozenArray, x: Dynamic| {
            to_value_at(&x, 0).is_ok_and(|x| a.0.contains(&x))
        })
        .register_indexer_get(
            |a: &mut FrozenArray, i: rhai::INT| -> Result<Dynamic, Box<EvalAltResult>> {
                let len = a.0.len() as rhai::INT;
                let at = if i < 0 { len + i } else { i };
                if (0..len).contains(&at) {
                    Ok(frozen_dynamic(&a.0[at as usize]))
                } else {
                    Err(EvalAltResult::ErrorArrayBounds(a.0.len(), i, Position::NONE).into())
                }
            },
        )
        .register_fn(
            "sort",
            |_: &mut FrozenArray| -> Result<Array, Box<EvalAltResult>> {
                Err(frozen_change("sort"))
            },
        )
        .register_fn(
            "sort",
            |_: &mut FrozenArray, _: rhai::FnPtr| -> Result<Array, Box<EvalAltResult>> {
                Err(frozen_change("sort"))
            },
        )
        .register_fn(
            "reverse",
            |_: &mut FrozenArray| -> Result<Array, Box<EvalAltResult>> {
                Err(frozen_change("reverse"))
            },
        )
        .register_fn("to_string", |_: &mut FrozenArray| "[array]".to_string())
        .register_fn("to_debug", |_: &mut FrozenArray| "[array]".to_string());
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
        .register_fn("to_debug", |_: &mut JsNull| "null".to_string())
        .register_fn("+", |s: ImmutableString, _: JsNull| format!("{s}null"))
        .register_fn("+", |_: JsNull, s: ImmutableString| format!("null{s}"));
    // A function's text has no hash-bearing name in it.
    e.register_fn("to_string", |_: &mut rhai::FnPtr| FN_TEXT.to_string())
        .register_fn("to_debug", |_: &mut rhai::FnPtr| FN_TEXT.to_string())
        .register_fn("+", |s: ImmutableString, _: rhai::FnPtr| {
            format!("{s}{FN_TEXT}")
        })
        .register_fn("+", |_: rhai::FnPtr, s: ImmutableString| {
            format!("{FN_TEXT}{s}")
        });
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
        .register_fn(
            "values",
            |o: &mut JsObject| -> Result<Array, Box<EvalAltResult>> {
                Ok(o.read_all()?.into_iter().map(|(_, v)| v).collect())
            },
        )
        .register_fn(
            "entries",
            |o: &mut JsObject| -> Result<Array, Box<EvalAltResult>> {
                Ok(o.read_all()?
                    .into_iter()
                    .map(|(k, v)| Dynamic::from_array(vec![Dynamic::from(k), v]))
                    .collect())
            },
        )
        .register_fn("len", |o: &mut JsObject| o.len() as rhai::INT)
        .register_fn("is_empty", |o: &mut JsObject| o.is_empty())
        .register_fn("contains", |o: &mut JsObject, k: ImmutableString| {
            o.contains(k.as_str())
        })
        .register_fn(
            "remove",
            |o: &mut JsObject, k: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
                Ok(o.owned(&format!("it removes `{k}`"))?
                    .shift_remove(k.as_str())
                    .unwrap_or(Dynamic::UNIT))
            },
        )
        // Text made from an object is JavaScript's `String(obj)`, never
        // a dump of Brrrty's own types. `entries()` shows the contents.
        .register_fn("to_debug", |_: &mut JsObject| "[object Object]".to_string())
        .register_fn("to_string", |_: &mut JsObject| {
            "[object Object]".to_string()
        })
        .register_fn("+", |s: ImmutableString, _: JsObject| {
            format!("{s}[object Object]")
        })
        .register_fn("+", |_: JsObject, s: ImmutableString| {
            format!("[object Object]{s}")
        });
    register_arrays(e);
}
