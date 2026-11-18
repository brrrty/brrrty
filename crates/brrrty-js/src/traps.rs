//! Reads Brrrty can't answer from the data alone, noticed while a template
//! renders.
//!
//! An object can hold a property whose value is `undefined` as a marker for
//! "11ty has a value here that the data doesn't carry". Two kinds exist:
//!
//! - A collection item's `templateContent` and `content`. The build installs
//!   a [`ContentSource`] on the rendering thread; a read asks it for the
//!   item's rendered content and is recorded (see [`take_item_reads`]), so
//!   the build knows which pages each page read and can order them like
//!   11ty. Without a source (a permalink or computed value renders before
//!   any content exists) the read is a trap.
//! - Everything else in [`TRAP_KEYS`] (`template`, an item's own
//!   `data.collections`): reading it, or listing the object's properties,
//!   records a trap. The renderer collects the traps after each render, and
//!   the caller refuses the page instead of printing a wrong value.
//!
//! A registered ("open") object also traps a read of a property it doesn't
//! have: the `collections` object of a site whose 11ty config may add
//! collections Brrrty can't see.
//!
//! A registered ("withholding") object traps a read of the keys it was
//! registered with, and a listing of its properties: a collection item's
//! `data`, which lacks the keys whose value Brrrty can't supply (see
//! [`withhold`]).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

use indexmap::IndexMap;

use crate::Value;

/// Properties that are traps when they hold `undefined`.
pub const TRAP_KEYS: [&str; 4] = ["templateContent", "content", "template", "collections"];

/// The properties that hold a collection item's rendered content.
pub const CONTENT_KEYS: [&str; 2] = ["templateContent", "content"];

/// Prefix of a trap recorded for a marker property (`item.templateContent`).
pub const ITEM_PREFIX: &str = "item.";
/// Prefix of a trap recorded for a missing property of an open object.
pub const OPEN_PREFIX: &str = "collections.";

/// Prefix of a trap recorded for a read of a withheld key (`withheld.site`).
pub const WITHHELD_PREFIX: &str = "withheld.";

static OPEN: RwLock<Vec<usize>> = RwLock::new(Vec::new());

/// Withholding objects, by [`object_id`], with the keys they lack.
static WITHHELD: LazyLock<RwLock<HashMap<usize, Arc<[String]>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
/// How many objects are withholding, so a read can skip the lock when none.
static WITHHELD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// What a [`ContentSource`] has for an item.
#[derive(Debug, Clone)]
pub enum Content {
    /// The rendered content.
    Ready(Value),
    /// Not rendered yet: the read gives `undefined`, and the render that
    /// made it is only a draft.
    Pending,
    /// It will never be there: the page failed, or 11ty doesn't render it.
    Refused,
}

/// How a read of an item's content went, as [`take_item_reads`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ready,
    Pending,
    Refused,
}

/// The rendered content of collection items, for the pages rendering on
/// this thread.
pub trait ContentSource: Send + Sync {
    /// The content of the item whose [`object_id`] is `item`, or `None` when
    /// `item` is not an item this source knows.
    fn content(&self, item: usize) -> Option<Content>;
}

thread_local! {
    static HITS: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    static SOURCE: RefCell<Option<Arc<dyn ContentSource>>> = const { RefCell::new(None) };
    static ITEM_READS: RefCell<Vec<(usize, Outcome)>> = const { RefCell::new(Vec::new()) };
}

/// Register an object: reading a property it doesn't have is a trap.
/// `id` is [`object_id`] of the object. Call [`close`] when the build ends.
pub fn open(id: usize) {
    if let Ok(mut v) = OPEN.write() {
        v.push(id);
    }
}

pub fn close(id: usize) {
    if let Ok(mut v) = OPEN.write() {
        v.retain(|x| *x != id);
    }
}

/// Register an object as lacking `keys`, whose value Brrrty can't supply:
/// reading one of them, or listing the object's properties, is a trap.
/// `id` is [`object_id`] of the object, which must stay alive until
/// [`release`].
pub fn withhold(id: usize, keys: Arc<[String]>) {
    if keys.is_empty() {
        return;
    }
    if let Ok(mut m) = WITHHELD.write()
        && m.insert(id, keys).is_none()
    {
        WITHHELD_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn release(id: usize) {
    if let Ok(mut m) = WITHHELD.write()
        && m.remove(&id).is_some()
    {
        WITHHELD_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The keys `o` was registered as lacking, if it was.
fn withheld(o: &IndexMap<String, Value>) -> Option<Arc<[String]>> {
    if WITHHELD_COUNT.load(Ordering::SeqCst) == 0 {
        return None;
    }
    WITHHELD.read().ok()?.get(&object_id(o)).cloned()
}

fn withholds(o: &IndexMap<String, Value>, key: &str) -> bool {
    withheld(o).is_some_and(|keys| keys.iter().any(|k| k == key))
}

/// Record the trap for a listing of a withholding object. True when it was.
fn list_withheld(o: &IndexMap<String, Value>) -> bool {
    match withheld(o) {
        Some(keys) => {
            hit(format!("{WITHHELD_PREFIX}{}", keys[0]));
            true
        }
        None => false,
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

/// Record a trap by name, as a read that hit one does.
pub fn record(name: String) {
    hit(name);
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

/// Installs a content source on this thread until it is dropped, and
/// starts a new record of item reads.
pub struct SourceGuard(Option<Arc<dyn ContentSource>>);

/// Use `source` for item content read on this thread while the guard lives.
pub fn install(source: Arc<dyn ContentSource>) -> SourceGuard {
    ITEM_READS.with(|r| r.borrow_mut().clear());
    let previous = SOURCE.with(|s| s.borrow_mut().replace(source));
    SourceGuard(previous)
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SOURCE.with(|s| *s.borrow_mut() = previous);
    }
}

/// The item content reads on this thread since [`install`]: each item's
/// [`object_id`] and what the read gave, in read order.
pub fn take_item_reads() -> Vec<(usize, Outcome)> {
    ITEM_READS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// The content of `o`, an item, under `key`, when the installed source
/// knows the item. `None` when there's no source or `o` is not its item.
fn item_content(o: &IndexMap<String, Value>) -> Option<Content> {
    let id = object_id(o);
    let content = SOURCE.with(|s| s.borrow().as_ref().and_then(|src| src.content(id)))?;
    let outcome = match &content {
        Content::Ready(_) => Outcome::Ready,
        Content::Pending => Outcome::Pending,
        Content::Refused => Outcome::Refused,
    };
    ITEM_READS.with(|r| r.borrow_mut().push((id, outcome)));
    Some(content)
}

/// A read of `key` from `o`, which has `found` there: the value the read
/// gives. An item's content comes from the installed source; other markers
/// and missing properties of an open object are recorded as traps.
pub fn read(o: &IndexMap<String, Value>, key: &str, found: Option<Value>) -> Option<Value> {
    match &found {
        Some(Value::Undefined) if CONTENT_KEYS.contains(&key) => match item_content(o) {
            Some(Content::Ready(v)) => Some(v),
            Some(Content::Pending | Content::Refused) => found,
            None => {
                hit(format!("{ITEM_PREFIX}{key}"));
                found
            }
        },
        Some(Value::Undefined) if TRAP_KEYS.contains(&key) => {
            hit(format!("{ITEM_PREFIX}{key}"));
            found
        }
        None if withholds(o, key) => {
            hit(format!("{WITHHELD_PREFIX}{key}"));
            found
        }
        None if is_open(o) => {
            hit(format!("{OPEN_PREFIX}{key}"));
            found
        }
        _ => found,
    }
}

/// A top-level read of `key` from `data`, the data of a sub-evaluation
/// (`where: "templateContent"` reads the item as its variables): the item's
/// content when `key` holds it, else `found`.
pub fn read_top(data: &Value, key: &str, found: Option<Value>) -> Option<Value> {
    match (data, &found) {
        (Value::Object(o), Some(Value::Undefined)) if CONTENT_KEYS.contains(&key) => {
            match item_content(o) {
                Some(Content::Ready(v)) => Some(v),
                _ => found,
            }
        }
        _ => found,
    }
}

/// A top-level lookup of `key` in `item`, the data of a sub-evaluation,
/// that gave `undefined`.
pub fn read_item(item: &Value, key: &str) {
    if let Value::Object(o) = item {
        read(o, key, o.get(key).cloned());
    }
}

/// A listing of `o`'s property names (or a count of them), without their
/// values. An item's names are all there, so only an open object, which
/// may have more, is a trap. True when it recorded one.
pub fn keys(o: &IndexMap<String, Value>) -> bool {
    if list_withheld(o) {
        return true;
    }
    if is_open(o) {
        hit(format!("{OPEN_PREFIX}*"));
        return true;
    }
    false
}

/// Whether `o` has `key` as an own property (`key in o`). A name an open
/// object doesn't have is a trap: the config may add it.
pub fn has(o: &IndexMap<String, Value>, key: &str) -> bool {
    if o.contains_key(key) {
        return true;
    }
    if withholds(o, key) {
        hit(format!("{WITHHELD_PREFIX}{key}"));
    } else if is_open(o) {
        hit(format!("{OPEN_PREFIX}{key}"));
    }
    false
}

/// A listing of all of `o`'s properties. True when it recorded a trap: the
/// caller's result will be refused, so a walk of the data can stop there.
pub fn entries(o: &IndexMap<String, Value>) -> bool {
    let mut trapped = false;
    let mut item_known = None;
    for key in TRAP_KEYS {
        if matches!(o.get(key), Some(Value::Undefined)) {
            // An item's content can be listed when a source has it; the
            // listing still trips on `template`.
            if CONTENT_KEYS.contains(&key) {
                let known = *item_known.get_or_insert_with(|| {
                    SOURCE.with(|s| {
                        s.borrow()
                            .as_ref()
                            .is_some_and(|src| src.content(object_id(o)).is_some())
                    })
                });
                if known {
                    continue;
                }
            }
            hit(format!("{ITEM_PREFIX}{key}"));
            trapped = true;
        }
    }
    if list_withheld(o) {
        trapped = true;
    }
    if is_open(o) {
        hit(format!("{OPEN_PREFIX}*"));
        trapped = true;
    }
    trapped
}

#[cfg(test)]
mod tests {
    use super::*;

    struct One(usize, Content);

    impl ContentSource for One {
        fn content(&self, item: usize) -> Option<Content> {
            (item == self.0).then(|| self.1.clone())
        }
    }

    fn item() -> IndexMap<String, Value> {
        let mut o = IndexMap::new();
        o.insert("templateContent".to_string(), Value::Undefined);
        o.insert("content".to_string(), Value::Undefined);
        o.insert("template".to_string(), Value::Undefined);
        o
    }

    #[test]
    fn a_withholding_object_traps_its_keys_and_listings() {
        let mut o = IndexMap::new();
        o.insert("title".to_string(), Value::String("t".into()));
        let id = object_id(&o);
        withhold(id, Arc::from(vec!["site".to_string()]));
        clear();
        assert_eq!(
            read(&o, "title", o.get("title").cloned()),
            o.get("title").cloned()
        );
        assert_eq!(read(&o, "other", None), None);
        assert!(take().is_empty());
        assert_eq!(read(&o, "site", None), None);
        assert_eq!(take(), vec!["withheld.site".to_string()]);
        assert!(!has(&o, "site"));
        assert_eq!(take(), vec!["withheld.site".to_string()]);
        assert!(entries(&o));
        assert_eq!(take(), vec!["withheld.site".to_string()]);
        release(id);
        assert_eq!(read(&o, "site", None), None);
        assert!(!entries(&o));
        assert!(take().is_empty());
    }

    #[test]
    fn a_source_answers_content_reads_and_records_them() {
        let o = item();
        let id = object_id(&o);
        let ready = Value::String("<p>x</p>".into());
        {
            let _g = install(Arc::new(One(id, Content::Ready(ready.clone()))));
            clear();
            let got = read(&o, "content", o.get("content").cloned());
            assert_eq!(got, Some(ready.clone()));
            assert_eq!(take_item_reads(), vec![(id, Outcome::Ready)]);
            assert!(take().is_empty());
        }
        // Without a source, the same read is a trap.
        clear();
        let got = read(&o, "templateContent", o.get("templateContent").cloned());
        assert_eq!(got, Some(Value::Undefined));
        assert_eq!(take(), vec!["item.templateContent".to_string()]);
    }

    #[test]
    fn pending_and_refused_reads_give_undefined() {
        let o = item();
        let id = object_id(&o);
        for (content, outcome) in [
            (Content::Pending, Outcome::Pending),
            (Content::Refused, Outcome::Refused),
        ] {
            let _g = install(Arc::new(One(id, content)));
            clear();
            let got = read(&o, "templateContent", Some(Value::Undefined));
            assert_eq!(got, Some(Value::Undefined));
            assert_eq!(take_item_reads(), vec![(id, outcome)]);
            assert!(take().is_empty());
        }
    }

    #[test]
    fn listing_a_known_item_trips_only_on_template() {
        let o = item();
        let id = object_id(&o);
        let _g = install(Arc::new(One(id, Content::Ready(Value::String("x".into())))));
        clear();
        entries(&o);
        assert_eq!(take(), vec!["item.template".to_string()]);
    }
}
