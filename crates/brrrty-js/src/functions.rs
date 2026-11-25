//! Calling functions from JavaScript data while a page is built.
//!
//! A function in JavaScript data stays in the runtime that made it, and the
//! data holds a [`Value::Function`] handle to it. The build sets a
//! [`Caller`] on each thread that renders or places pages; a template that
//! calls the function goes through it. A page that uses a function any
//! other way (prints it, compares it, passes it to a filter) records a
//! trap, as a read of a withheld key does, and the build refuses the page:
//! 11ty's output there would be JavaScript's own text for the function.

use std::cell::RefCell;
use std::sync::Arc;

use crate::Value;
use crate::value::FnHandle;

/// Prefix of a trap recorded for a function a page couldn't use: the rest
/// is the reason, to show as it is.
pub const FUNCTION_PREFIX: &str = "function.";

/// What `this` is inside a call.
#[derive(Debug, Clone, Copy)]
pub enum This<'a> {
    /// The object the function was read from (`fmt.money(1)`, and Liquid's
    /// `{{ fmt.who }}`), as the page sees it.
    Value(&'a Value),
    /// An object as the page sees it, but reading any of these keys (`key`, and what it is in
    /// 11ty) refuses the page: parts of 11ty's `this` Brrrty doesn't model.
    Guarded(&'a Value, &'a [(&'a str, &'a str)]),
    /// Something Brrrty doesn't model (Nunjucks's render context, 11ty's
    /// filter collection): reading any property of it refuses the page.
    Opaque(&'static str),
}

/// How a call's result is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// Await a promise, as Liquid and 11ty's computed data do.
    Await,
    /// Take the result as it is, as Nunjucks does: a promise can't cross.
    AsIs,
}

/// The runtime that calls functions for the pages on this thread.
pub trait Caller: Send + Sync {
    /// Call `f` with `this` and `args`. `Err` is why the page can't use
    /// the result: the function threw, kept state between calls, changed
    /// its arguments, or returned something that can't cross.
    fn call(
        &self,
        f: FnHandle,
        this: This<'_>,
        args: &[Value],
        settle: Settle,
    ) -> Result<Value, String>;
}

thread_local! {
    static CALLER: RefCell<Option<Arc<dyn Caller>>> = const { RefCell::new(None) };
    static UNIT: RefCell<Option<(u64, Arc<str>)>> = const { RefCell::new(None) };
}

thread_local! {
    static OWN_PAGE: RefCell<Option<(crate::Value, Option<f64>)>> = const { RefCell::new(None) };
}

/// The page whose template is rendering on this thread: its `page`
/// object and, on a paginated page, its number. A function the template
/// calls is told, so that it finds the page's own data among the
/// collections' items as the same objects it was handed (in 11ty they
/// are: a collection item's `data` is the page's data).
pub fn own_page() -> Option<(crate::Value, Option<f64>)> {
    OWN_PAGE.with(|p| p.borrow().clone())
}

/// Marks the renders on this thread, until it drops, as the page's whose
/// data is `data`.
pub struct OwnPageGuard(Option<(crate::Value, Option<f64>)>);

impl OwnPageGuard {
    pub fn enter(data: &indexmap::IndexMap<String, crate::Value>) -> OwnPageGuard {
        let own = match data.get("page") {
            Some(page @ crate::Value::Object(_)) => {
                let number = match data.get("pagination") {
                    Some(crate::Value::Object(p)) => match p.get("pageNumber") {
                        Some(crate::Value::Number(n)) => Some(*n),
                        _ => None,
                    },
                    _ => None,
                };
                Some((page.clone(), number))
            }
            _ => None,
        };
        OwnPageGuard(OWN_PAGE.with(|p| std::mem::replace(&mut *p.borrow_mut(), own)))
    }
}

impl Drop for OwnPageGuard {
    fn drop(&mut self) {
        let before = self.0.take();
        OWN_PAGE.with(|p| *p.borrow_mut() = before);
    }
}

static NEXT_UNIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The render the calls on this thread belong to (a page's content, its
/// layouts, the placing of a template's pages): its id, and a name that
/// orders it among the build's renders the same way every build.
pub fn unit() -> Option<(u64, Arc<str>)> {
    UNIT.with(|u| u.borrow().clone())
}

/// Marks the calls made on this thread, until it drops, as one render
/// named `label`.
pub struct UnitGuard(Option<(u64, Arc<str>)>);

impl UnitGuard {
    pub fn enter(label: impl Into<Arc<str>>) -> UnitGuard {
        let id = NEXT_UNIT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let before = UNIT.with(|u| u.borrow_mut().replace((id, label.into())));
        UnitGuard(before)
    }
}

impl Drop for UnitGuard {
    fn drop(&mut self) {
        let before = self.0.take();
        UNIT.with(|u| *u.borrow_mut() = before);
    }
}

/// Make `caller` this thread's caller from now on (none with `None`).
pub fn set(caller: Option<Arc<dyn Caller>>) {
    let before = CALLER.with(|c| std::mem::replace(&mut *c.borrow_mut(), caller));
    drop(before);
}

/// The caller installed on this thread, if any.
pub fn installed() -> Option<Arc<dyn Caller>> {
    CALLER.with(|c| c.borrow().clone())
}

/// Call `f`; when it can't be called or its result can't be used, record
/// the reason as a trap and give `undefined` (the page will be refused).
pub fn call(f: FnHandle, this: This<'_>, args: &[Value], settle: Settle) -> Value {
    let Some(caller) = installed() else {
        crate::traps::record(format!(
            "{FUNCTION_PREFIX}this page calls a function from JavaScript data where Brrrty can't call it yet"
        ));
        return Value::Undefined;
    };
    match caller.call(f, this, args, settle) {
        Ok(v) => v,
        Err(why) => {
            crate::traps::record(format!("{FUNCTION_PREFIX}{why}"));
            Value::Undefined
        }
    }
}

/// Filters known to treat a function as 11ty's do: JSON leaves it out,
/// `default` keeps it.
const FILTERS_FOR_FUNCTIONS: &[&str] = &["dump", "json", "default", "d"];

/// Record a trap when the filter `name` would get a function from
/// JavaScript data as its input or an argument: each filter would need its
/// own probe against 11ty to say what it does with one.
pub fn check_filter_args<'a>(name: &str, values: impl IntoIterator<Item = &'a Value>) {
    if FILTERS_FOR_FUNCTIONS.contains(&name) {
        return;
    }
    if values.into_iter().any(|v| matches!(v, Value::Function(_))) {
        crate::traps::record(format!(
            "{FUNCTION_PREFIX}this page passes a function from JavaScript data to the `{name}` filter, which Brrrty can't follow"
        ));
    }
}

/// Record that a page used a function from JavaScript data as a value:
/// `how` says how ("prints").
pub fn used_as_value(how: &str) {
    crate::traps::record(format!(
        "{FUNCTION_PREFIX}this page {how} a function from JavaScript data as a value, which 11ty shows as JavaScript's own text for it"
    ));
}
