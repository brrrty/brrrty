//! Filters: Nunjucks's own (`filters.js`), and the ones an embedder
//! registers.
//!
//! Every filter in 3.2.4's `filters.js` is here, in `builtin.rs`. The ones
//! that use regular expressions are done by hand in `text.rs`, each exactly
//! as its fixed pattern behaves, so none waits for a regular-expression
//! engine. `jslib.rs` holds the JavaScript they lean on.

mod builtin;
mod jslib;
mod text;

use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::{FilterEnv, Value};
use indexmap::IndexMap;

use crate::context::Scope;
use crate::engine::{Engine, FilterFn};
use crate::error::ErrorKind;
use crate::jsops;
use crate::render::{R, RenderErr, Renderer, error};
use crate::value::{Obj, Val};

/// Every filter in Nunjucks 3.2.4's `filters.js`.
pub(crate) const NUNJUCKS_FILTERS: &[&str] = &[
    "abs",
    "batch",
    "capitalize",
    "center",
    "default",
    "dictsort",
    "dump",
    "escape",
    "safe",
    "first",
    "forceescape",
    "groupby",
    "indent",
    "join",
    "last",
    "length",
    "list",
    "lower",
    "nl2br",
    "random",
    "reject",
    "rejectattr",
    "select",
    "selectattr",
    "replace",
    "reverse",
    "round",
    "slice",
    "sum",
    "sort",
    "string",
    "striptags",
    "title",
    "trim",
    "truncate",
    "upper",
    "urlencode",
    "urlize",
    "wordcount",
    "float",
    "int",
    "d",
    "e",
];

#[derive(Clone)]
pub(crate) enum Filter {
    /// One of Nunjucks's own.
    Builtin(&'static str),
    User(FilterFn),
}

/// Whether `name` is one of Nunjucks's own filters.
pub(crate) fn is_builtin(name: &str) -> bool {
    NUNJUCKS_FILTERS.contains(&name)
}

/// `env.getFilter(name)`.
pub(crate) fn lookup(engine: &Engine, name: &str) -> R<Filter> {
    if let Some(f) = engine.filters.get(name) {
        return Ok(Filter::User(f.clone()));
    }
    if let Some(n) = NUNJUCKS_FILTERS.iter().find(|n| **n == name) {
        return Ok(Filter::Builtin(n));
    }
    let err = error(format!("filter not found: {name}"));
    Err(match closest(engine, name) {
        Some(c) => err.with_help(format!("did you mean `{c}`?")),
        None => err,
    })
}

fn closest<'a>(engine: &'a Engine, name: &str) -> Option<&'a str> {
    engine
        .filters
        .keys()
        .map(String::as_str)
        .chain(NUNJUCKS_FILTERS.iter().copied())
        .map(|k| (distance(name, k), k))
        .filter(|(d, _)| *d <= 2)
        .min()
        .map(|(_, k)| k)
}

/// Levenshtein distance.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Calls a filter. `args[0]` is the input.
pub(crate) fn run(
    r: &mut Renderer<'_>,
    filter: &Filter,
    name: &str,
    args: &[Val],
    scope: &Rc<Scope>,
) -> R<Val> {
    brrrty_js::functions::check_filter_args(
        name,
        args.iter().filter_map(|a| match a {
            Val::V(v) => Some(v),
            _ => None,
        }),
    );
    match filter {
        Filter::Builtin(n) => builtin::run(r, n, args),
        Filter::User(f) => run_user(r, f, name, args, scope),
    }
}

/// A filter an embedder registered. It gets plain [`Value`]s, so a
/// template-made array or object is copied into one; a `SafeString` is
/// passed as its text, with the flags in [`FilterEnv`] saying so.
fn run_user(
    r: &mut Renderer<'_>,
    f: &FilterFn,
    name: &str,
    args: &[Val],
    scope: &Rc<Scope>,
) -> R<Val> {
    let ops = r.ops();
    // A shortcode: its result is a `SafeString` of the text, and its errors say so.
    let shortcode = name.strip_prefix(crate::parser::SHORTCODE_PREFIX);
    let paired = shortcode.is_some_and(|n| matches!(r.engine.shortcodes.get(n), Some(Some(_))));
    let name = shortcode.unwrap_or(name);
    let mut values = Vec::with_capacity(args.len());
    let mut safe = Vec::with_capacity(args.len());
    for (i, a) in args.iter().enumerate() {
        let what = if i == 0 {
            "its input".to_string()
        } else {
            format!("argument {i}")
        };
        safe.push(matches!(a, Val::Safe(_)));
        values.push(to_value(a, name, &what, 0, true)?);
    }
    let mut env = FilterEnv::new(ops.tz);
    env.input_safe = safe.first().copied().unwrap_or(false);
    env.args_safe = safe.get(1..).unwrap_or(&[]).to_vec();
    // The keyword arguments, taken apart from the last object argument.
    if let Some(Val::Obj(o)) = args.last()
        && args.len() > 1
        && o.has("__keywords")
    {
        for k in o.keys() {
            if k != "__keywords" {
                let v = o.get(&k).unwrap_or_else(Val::undef);
                env.kwargs
                    .push((k, to_value(&v, name, "a keyword argument", 0, false)?));
            }
        }
    }
    env.page = page(scope);
    let input = values.first().cloned().unwrap_or(Value::Undefined);
    let rest = values.get(1..).unwrap_or(&[]);
    let out = f(&input, rest, &env).map_err(|m| {
        // Eleventy wraps what a filter throws.
        if m.starts_with(brrrty_js::REFUSAL_PREFIX) {
            RenderErr::new(ErrorKind::Unsupported, m)
        } else if shortcode.is_some() {
            let kind = if paired {
                "paired shortcode"
            } else {
                "shortcode"
            };
            RenderErr::new(
                ErrorKind::Render,
                format!("Error with Nunjucks {kind} `{name}`: {m}"),
            )
        } else {
            RenderErr::new(
                ErrorKind::Render,
                format!("Error in Nunjucks Filter `{name}`: {m}"),
            )
        }
    })?;
    if let Value::String(s) = &out
        && s.len() > ops.limit
    {
        return Err(jsops::too_big(ops));
    }
    if shortcode.is_some() {
        // A plain one writes `"" + ret`; a paired one `new SafeString(ret)`, and for anything
        // but a string that is an empty SafeString, which prints `undefined`.
        if paired && !matches!(out, Value::String(_)) {
            return Ok(Val::safe("undefined".to_string()));
        }
        return Ok(Val::safe(out.to_js_string()));
    }
    if env.did_return_input() && env.input_safe {
        // `return input`: the `SafeString` itself.
        return Ok(args[0].clone());
    }
    Ok(Val::V(out))
}

/// `this.page`: Eleventy copies the template's data onto a filter's
/// `this`, so it is the context's `page`.
fn page(scope: &Rc<Scope>) -> Option<Value> {
    let vars = scope.tpl.context.vars.borrow();
    match vars.get_own("page") {
        Some(Val::V(v @ Value::Object(_))) => Some(v),
        _ => None,
    }
}

/// A render-time value as a plain [`Value`], for a filter written against
/// [`Value`]. A function has no such form, and neither has a `SafeString`
/// inside an array or object, where there's nowhere to say it is one; a
/// top-level `SafeString` goes as its text (`top`).
fn to_value(v: &Val, filter: &str, what: &str, depth: usize, top: bool) -> R<Value> {
    if depth > 1000 {
        return Err(RenderErr::new(
            ErrorKind::Limit,
            format!("filter `{filter}` would get a value nested more than 1000 levels deep"),
        ));
    }
    Ok(match v {
        Val::V(v) => v.clone(),
        Val::Arr(a) => Value::Array(Arc::new(
            a.borrow()
                .iter()
                .map(|x| crate::render::grow(|| to_value(x, filter, what, depth + 1, false)))
                .collect::<R<Vec<_>>>()?,
        )),
        Val::Obj(o) => Value::Object(Arc::new(obj_to_map(o, filter, what, depth)?)),
        Val::Safe(s) if top => Value::String(s.0.clone()),
        Val::Safe(_) => {
            return Err(RenderErr::new(
                ErrorKind::Unsupported,
                format!(
                    "filter `{filter}` would get a SafeString (from `safe`, `escape` or a macro) inside {what}"
                ),
            )
            .with_help("Brrrty can say a SafeString is one only for a filter's input or an argument; apply `safe` after this filter instead"));
        }
        Val::Func(_) => {
            return Err(RenderErr::new(
                ErrorKind::Unsupported,
                format!("filter `{filter}` would get a function as {what}"),
            ));
        }
        Val::Re(_) => {
            return Err(RenderErr::new(
                ErrorKind::Unsupported,
                format!("filter `{filter}` would get a regular expression as {what}"),
            ));
        }
    })
}

fn obj_to_map(o: &Obj, filter: &str, what: &str, depth: usize) -> R<IndexMap<String, Value>> {
    let mut map = IndexMap::new();
    for k in o.keys() {
        let v = o.get(&k).unwrap_or_else(Val::undef);
        let v = crate::render::grow(|| to_value(&v, filter, what, depth + 1, false))?;
        map.insert(k, v);
    }
    Ok(map)
}
