//! Filters: the ones an embedder registers, and Nunjucks's own.
//!
//! Of Nunjucks's built-in filters, the escaping ones (`safe`, `escape`,
//! `e`, `forceescape`) are here, because autoescaping depends on them. The
//! rest of `filters.js` is refused by name until it is added.

use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;

use crate::engine::{Engine, FilterCtx, FilterFn};
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
    Safe,
    Escape,
    ForceEscape,
    User(FilterFn),
}

/// `env.getFilter(name)`.
pub(crate) fn lookup(engine: &Engine, name: &str) -> R<Filter> {
    if let Some(f) = engine.filters.get(name) {
        return Ok(Filter::User(f.clone()));
    }
    Ok(match name {
        "safe" => Filter::Safe,
        "escape" | "e" => Filter::Escape,
        "forceescape" => Filter::ForceEscape,
        _ if NUNJUCKS_FILTERS.contains(&name) => {
            return Err(RenderErr::new(
                ErrorKind::Unsupported,
                format!("Brrrty doesn't have the Nunjucks filter `{name}` yet"),
            ));
        }
        _ => {
            let err = error(format!("filter not found: {name}"));
            return Err(match closest(engine, name) {
                Some(c) => err.with_help(format!("did you mean `{c}`?")),
                None => err,
            });
        }
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
pub(crate) fn run(r: &mut Renderer<'_>, filter: &Filter, name: &str, args: &[Val]) -> R<Val> {
    let ops = r.ops();
    let input = args.first().cloned().unwrap_or_else(Val::undef);
    match filter {
        Filter::Safe => Ok(match input {
            Val::Safe(_) => input,
            v if v.is_nullish() => Val::safe(""),
            v => Val::safe(jsops::to_string_method(&v, ops)?),
        }),
        Filter::Escape => Ok(match input {
            Val::Safe(_) => input,
            v if v.is_nullish() => Val::safe(""),
            v => Val::safe(jsops::escape(&jsops::to_string_method(&v, ops)?)),
        }),
        Filter::ForceEscape => Ok(if input.is_nullish() {
            Val::safe("")
        } else {
            Val::safe(jsops::escape(&jsops::to_string_method(&input, ops)?))
        }),
        Filter::User(f) => {
            let mut values = Vec::with_capacity(args.len());
            for (i, a) in args.iter().enumerate() {
                let what = if i == 0 {
                    "its input".to_string()
                } else {
                    format!("argument {i}")
                };
                values.push(to_value(a, name, &what, 0)?);
            }
            let ctx = FilterCtx { timezone: ops.tz };
            let input = values.first().cloned().unwrap_or(Value::Undefined);
            let rest = values.get(1..).unwrap_or(&[]);
            let out = f(&input, rest, &ctx).map_err(|m| RenderErr::new(ErrorKind::Render, m))?;
            if let Value::String(s) = &out
                && s.len() > ops.limit
            {
                return Err(jsops::too_big(ops));
            }
            Ok(Val::V(out))
        }
    }
}

/// A render-time value as a plain [`Value`], for a filter written against
/// [`Value`]. A `SafeString` or a function has no such form.
fn to_value(v: &Val, filter: &str, what: &str, depth: usize) -> R<Value> {
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
                .map(|x| crate::render::grow(|| to_value(x, filter, what, depth + 1)))
                .collect::<R<Vec<_>>>()?,
        )),
        Val::Obj(o) => Value::Object(Arc::new(obj_to_map(o, filter, what, depth)?)),
        Val::Safe(_) => {
            return Err(RenderErr::new(
                ErrorKind::Unsupported,
                format!(
                    "filter `{filter}` would get a SafeString (from `safe`, `escape` or a macro) as {what}"
                ),
            )
            .with_help("Nunjucks hands a filter that object as it is, which Brrrty can't do yet; apply `safe` after this filter instead"));
        }
        Val::Func(_) => {
            return Err(RenderErr::new(
                ErrorKind::Unsupported,
                format!("filter `{filter}` would get a function as {what}"),
            ));
        }
    })
}

fn obj_to_map(o: &Obj, filter: &str, what: &str, depth: usize) -> R<IndexMap<String, Value>> {
    let mut map = IndexMap::new();
    for k in o.keys() {
        let v = o.get(&k).unwrap_or_else(Val::undef);
        let v = crate::render::grow(|| to_value(&v, filter, what, depth + 1))?;
        map.insert(k, v);
    }
    Ok(map)
}
