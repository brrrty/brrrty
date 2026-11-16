//! Reads Brrrty can't answer yet, noticed while a template renders.
//!
//! An object can hold a property whose value is `undefined` as a marker for
//! "11ty has a value here that Brrrty does not build". Reading that
//! property, or listing the object's properties, records a trap. The
//! renderer collects the traps after each render, and the caller refuses the
//! page instead of printing a wrong value. This is how a collection item's
//! `templateContent` and `content` stay refused until Brrrty renders them.
//!
//! A registered ("open") object also traps a read of a property it doesn't
//! have: the `collections` object of a site whose 11ty config may add
//! collections Brrrty can't see.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use indexmap::IndexMap;

use crate::Value;

/// Properties that are traps when they hold `undefined`.
pub const TRAP_KEYS: [&str; 4] = ["templateContent", "content", "template", "collections"];

/// Prefix of a trap recorded for a marker property (`item.templateContent`).
pub const ITEM_PREFIX: &str = "item.";
/// Prefix of a trap recorded for a missing property of an open object.
pub const OPEN_PREFIX: &str = "collections.";

static ACTIVE: AtomicBool = AtomicBool::new(false);
static OPEN: RwLock<Vec<usize>> = RwLock::new(Vec::new());

thread_local! {
    static HITS: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

/// Tell the runtime markers exist, so reads start checking for them.
pub fn activate() {
    ACTIVE.store(true, Ordering::Relaxed);
}

/// Register an object: reading a property it doesn't have is a trap.
/// `id` is [`object_id`] of the object. Call [`close`] when the build ends.
pub fn open(id: usize) {
    activate();
    if let Ok(mut v) = OPEN.write() {
        v.push(id);
    }
}

pub fn close(id: usize) {
    if let Ok(mut v) = OPEN.write() {
        v.retain(|x| *x != id);
    }
}

/// An object's identity while its `Arc` lives.
pub fn object_id(o: &IndexMap<String, Value>) -> usize {
    o as *const IndexMap<String, Value> as usize
}

fn is_open(o: &IndexMap<String, Value>) -> bool {
    let id = object_id(o);
    OPEN.read().map(|v| v.contains(&id)).unwrap_or(false)
}

fn hit(name: String) {
    HITS.with(|h| {
        h.borrow_mut().insert(name);
    });
}

/// Forget traps recorded so far on this thread.
pub fn clear() {
    HITS.with(|h| h.borrow_mut().clear());
}

/// The traps recorded on this thread since the last [`clear`], sorted.
pub fn take() -> Vec<String> {
    HITS.with(|h| std::mem::take(&mut *h.borrow_mut()).into_iter().collect())
}

/// A read of `key` from `o`, which has `found` there.
pub fn read(o: &IndexMap<String, Value>, key: &str, found: Option<&Value>) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    match found {
        Some(Value::Undefined) if TRAP_KEYS.contains(&key) => hit(format!("{ITEM_PREFIX}{key}")),
        None if is_open(o) => hit(format!("{OPEN_PREFIX}{key}")),
        _ => {}
    }
}

/// A listing of all of `o`'s properties.
pub fn entries(o: &IndexMap<String, Value>) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    for key in TRAP_KEYS {
        if matches!(o.get(key), Some(Value::Undefined)) {
            hit(format!("{ITEM_PREFIX}{key}"));
        }
    }
    if is_open(o) {
        hit(format!("{OPEN_PREFIX}*"));
    }
}
