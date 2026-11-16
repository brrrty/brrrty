//! Render-time values: what a compiled Nunjucks template holds in its
//! JavaScript variables.
//!
//! Data from the build stays a [`Value`] and is never changed. What a
//! template makes itself is its own kind of value, because JavaScript gives
//! those identity and lets the template change them: arrays a template
//! builds (`[1, 2]`, the result of `.split()`), objects it builds
//! (`{a: 1}`, `loop`, an imported namespace, a `cycler()`), Nunjucks's
//! `SafeString`, and functions (macros, `caller`, the globals and the
//! methods `memberLookup` wraps).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;

use crate::compile::MacroDef;
use crate::context::Scope;

#[derive(Clone)]
pub(crate) enum Val {
    /// A primitive, or data from the build (arrays, objects and dates the
    /// template may read but not change).
    V(Value),
    /// Nunjucks's `SafeString`: an object wrapping a string that output
    /// does not escape.
    Safe(Rc<Safe>),
    /// An array the template made.
    Arr(Rc<RefCell<Vec<Val>>>),
    /// An object the template made.
    Obj(Rc<Obj>),
    Func(Rc<Func>),
    /// A `RegExp` from an `r/.../` literal: each evaluation makes a new one.
    Re(Rc<Re>),
}

/// A `RegExp` object.
pub(crate) struct Re {
    pub lit: Arc<RegexLit>,
    /// `lastIndex` may no longer be 0: a global `test()` matched.
    pub moved: std::cell::Cell<bool>,
}

/// An `r/.../` literal, compiled once with the template.
pub(crate) struct RegexLit {
    /// `RegExp.prototype.source`.
    pub source: String,
    /// `RegExp.prototype.flags`: the flags in the order `gimy`.
    pub flags: String,
    /// The pattern on the `regex` crate, or why Brrrty can't match it
    /// exactly: refused when the template matches with it.
    re: Result<Arc<brrrty_jsregex::matcher::JsRegex>, String>,
}

impl std::fmt::Debug for RegexLit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_js_string())
    }
}

impl RegexLit {
    /// `new RegExp(body, flags)` from the literal's text. `Err` when
    /// JavaScript rejects the pattern, which Nunjucks does as it parses,
    /// or when it is a form Brrrty doesn't read.
    pub fn new(body: &str, flags: &str) -> Result<RegexLit, String> {
        brrrty_jsregex::ast::parse(body, false)?;
        let re = brrrty_jsregex::matcher::compile(body, flags);
        let flags: String = "gimy".chars().filter(|c| flags.contains(*c)).collect();
        // `EscapeRegExpPattern`: line terminators are escaped; a `/` can't
        // be in the body unescaped.
        let mut source = String::new();
        for c in body.chars() {
            match c {
                '\n' => source.push_str("\\n"),
                '\r' => source.push_str("\\r"),
                '\u{2028}' => source.push_str("\\u2028"),
                '\u{2029}' => source.push_str("\\u2029"),
                c => source.push(c),
            }
        }
        if source.is_empty() {
            source.push_str("(?:)");
        }
        Ok(RegexLit { source, flags, re })
    }

    /// The pattern to match with, or why Brrrty can't match it exactly.
    pub fn matcher(&self) -> Result<&brrrty_jsregex::matcher::JsRegex, String> {
        match &self.re {
            Ok(re) => Ok(re),
            Err(why) => Err(format!(
                "Brrrty can't match with `{}` exactly: {why}",
                self.to_js_string()
            )),
        }
    }

    /// `RegExp.prototype.toString`.
    #[must_use]
    pub fn to_js_string(&self) -> String {
        format!("/{}/{}", self.source, self.flags)
    }
}

pub(crate) struct Safe(pub Arc<str>);

/// A plain JavaScript object the template made. Keys keep insertion order;
/// [`Obj::keys`] gives JavaScript's own-key order.
#[derive(Default)]
pub(crate) struct Obj {
    pub props: RefCell<IndexMap<String, Val>>,
}

impl Obj {
    pub fn new(props: IndexMap<String, Val>) -> Rc<Obj> {
        Rc::new(Obj {
            props: RefCell::new(props),
        })
    }

    pub fn get(&self, key: &str) -> Option<Val> {
        self.props.borrow().get(key).cloned()
    }

    pub fn has(&self, key: &str) -> bool {
        self.props.borrow().contains_key(key)
    }

    pub fn set(&self, key: &str, v: Val) {
        self.props.borrow_mut().insert(key.to_string(), v);
    }

    /// The own keys in JavaScript's order: array indexes first, in numeric
    /// order, then the rest in insertion order.
    pub fn keys(&self) -> Vec<String> {
        js_key_order(self.props.borrow().keys().cloned())
    }
}

/// Orders keys as `Object.keys` does.
pub(crate) fn js_key_order(keys: impl Iterator<Item = String>) -> Vec<String> {
    let mut index: Vec<(u32, String)> = Vec::new();
    let mut rest = Vec::new();
    for k in keys {
        match brrrty_js::array_index(&k) {
            Some(i) => index.push((i, k)),
            None => rest.push(k),
        }
    }
    index.sort_by_key(|(i, _)| *i);
    index.into_iter().map(|(_, k)| k).chain(rest).collect()
}

/// The globals Nunjucks defines (`globals.js`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Global {
    Range,
    Cycler,
    Joiner,
}

pub(crate) enum Func {
    /// A macro, or the `caller` of a `{% call %}` block, with the scope it
    /// closes over.
    Macro {
        def: Arc<MacroDef>,
        scope: Rc<Scope>,
    },
    Global(Global),
    /// What `joiner()` returns.
    Joiner {
        sep: Val,
        first: Cell<bool>,
    },
    /// A `cycler()`'s `next`, which sets `current` on the object it is
    /// called on.
    CyclerNext {
        items: Rc<Vec<Val>>,
        index: Rc<Cell<i64>>,
    },
    /// A `cycler()`'s `reset`.
    CyclerReset {
        index: Rc<Cell<i64>>,
    },
    /// The function `runtime.memberLookup` returns for a property that is a
    /// function: it calls `this[name]` with `this` as the receiver.
    Method {
        this: Val,
        name: Arc<str>,
    },
}

impl Val {
    pub fn undef() -> Val {
        Val::V(Value::Undefined)
    }

    pub fn null() -> Val {
        Val::V(Value::Null)
    }

    pub fn num(n: f64) -> Val {
        Val::V(Value::Number(n))
    }

    pub fn bool(b: bool) -> Val {
        Val::V(Value::Bool(b))
    }

    pub fn str(s: impl Into<Arc<str>>) -> Val {
        Val::V(Value::String(s.into()))
    }

    pub fn safe(s: impl Into<Arc<str>>) -> Val {
        Val::Safe(Rc::new(Safe(s.into())))
    }

    pub fn arr(items: Vec<Val>) -> Val {
        Val::Arr(Rc::new(RefCell::new(items)))
    }

    pub fn func(f: Func) -> Val {
        Val::Func(Rc::new(f))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Val::V(Value::Undefined))
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Val::V(Value::Undefined | Value::Null))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::V(Value::String(s)) => Some(s),
            _ => None,
        }
    }

    /// JavaScript truthiness. Every object is truthy, an empty `SafeString`
    /// included.
    pub fn truthy(&self) -> bool {
        match self {
            Val::V(v) => v.js_truthy(),
            _ => true,
        }
    }

    /// `typeof`.
    pub fn type_of(&self) -> &'static str {
        match self {
            Val::V(Value::Undefined) => "undefined",
            Val::V(Value::Bool(_)) => "boolean",
            Val::V(Value::Number(_)) => "number",
            Val::V(Value::String(_)) => "string",
            Val::Func(_) => "function",
            _ => "object",
        }
    }

    /// `Array.isArray`.
    pub fn is_array(&self) -> bool {
        matches!(self, Val::Arr(_) | Val::V(Value::Array(_)))
    }

    /// The items of an array, data or the template's own.
    pub fn array_items(&self) -> Option<Vec<Val>> {
        match self {
            Val::Arr(a) => Some(a.borrow().clone()),
            Val::V(Value::Array(a)) => Some(a.iter().cloned().map(Val::V).collect()),
            _ => None,
        }
    }

    /// `===` on objects: whether `self` and `other` are the same object.
    pub fn same_object(&self, other: &Val) -> bool {
        match (self, other) {
            (Val::Safe(a), Val::Safe(b)) => Rc::ptr_eq(a, b),
            (Val::Arr(a), Val::Arr(b)) => Rc::ptr_eq(a, b),
            (Val::Obj(a), Val::Obj(b)) => Rc::ptr_eq(a, b),
            (Val::Func(a), Val::Func(b)) => Rc::ptr_eq(a, b),
            (Val::Re(a), Val::Re(b)) => Rc::ptr_eq(a, b),
            (Val::V(Value::Array(a)), Val::V(Value::Array(b))) => Arc::ptr_eq(a, b),
            (Val::V(Value::Object(a)), Val::V(Value::Object(b))) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl From<Value> for Val {
    fn from(v: Value) -> Val {
        Val::V(v)
    }
}
