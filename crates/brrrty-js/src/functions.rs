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

/// Record that a page used a function from JavaScript data as a value:
/// `how` says how ("prints").
pub fn used_as_value(how: &str) {
    crate::traps::record(format!(
        "{FUNCTION_PREFIX}this page {how} a function from JavaScript data as a value, which 11ty shows as JavaScript's own text for it"
    ));
}
