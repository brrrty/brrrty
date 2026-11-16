//! Array filters.

use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;

use super::{
    Args, Out, array, is_nil, js_string, same_value_zero, string, stringify, to_array, utf16_len,
};
use crate::ast::{Expr, FilteredValue};
use crate::context::{Context, Val};
use crate::error::Located;
use crate::lexer;
use crate::ops::{self, Cmp};
use crate::render::{R, RenderErr, Renderer};

fn undefined() -> Value {
    Value::Undefined
}

// ---- reading properties -----------------------------------------------------------

/// `context._getFromScope(item, path.split('.'), false)`.
fn read_path(item: &Value, path: &str) -> Value {
    let mut v = Val::V(item.clone());
    for key in path.split('.') {
        v = ops::read_property(&v, &Value::String(key.into()));
    }
    v.to_value()
}

/// A property filter's path, read as `readScopeValue` reads it.
fn scope_path(r: &Renderer<'_>, property: &Value) -> R<Option<Expr>> {
    let text = stringify(r, property);
    lexer::parse_scope_value(&text).map_err(located)
}

fn located(e: Located) -> RenderErr {
    RenderErr::new(e.kind, e.message)
}

/// `evalToken(token, context.spawn(item))`: the path read with `item` as
/// the only data.
fn eval_on(r: &mut Renderer<'_>, path: Option<&Expr>, item: &Value) -> R<Val> {
    let Some(path) = path else {
        return Ok(Val::undefined());
    };
    let mut sub = Renderer {
        engine: r.engine,
        template: r.template,
        loader: r.loader,
        ctx: Context::new(item),
    };
    sub.eval(path)
}

/// `new Value(stringify(exp), liquid)`: an expression with filters,
/// checked against `strictFilters` as soon as it is read.
fn parse_exp(r: &Renderer<'_>, exp: &Value) -> R<FilteredValue> {
    let text = stringify(r, exp);
    let fv = lexer::parse_filtered_value(&text).map_err(located)?;
    if r.engine.options.strict_filters
        && let Some(f) = fv
            .filters
            .iter()
            .find(|f| !r.engine.filters.contains_key(&f.name))
    {
        return Err(r.unknown_filter(&f.name));
    }
    Ok(fv)
}

/// Runs `exp` once for each item with `item_name` bound to it, until
/// `visit` says to stop.
fn each_exp(
    r: &mut Renderer<'_>,
    items: &[Value],
    item_name: &Value,
    exp: &FilteredValue,
    mut visit: impl FnMut(usize, &Value, Val) -> bool,
) -> R<()> {
    let name = js_string(r, item_name);
    for (i, item) in items.iter().enumerate() {
        let mut scope = std::collections::HashMap::new();
        scope.insert(name.clone(), Val::V(item.clone()));
        r.ctx.scopes.push(scope);
        let result = r.eval_filtered(exp);
        r.ctx.scopes.pop();
        if !visit(i, item, result?) {
            break;
        }
    }
    Ok(())
}

// ---- simple array filters ----------------------------------------------------------

pub(super) fn join(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    let sep = match Args(a).value(0) {
        s if is_nil(&s) => " ".to_string(),
        s => stringify(r, &s),
    };
    let parts: Vec<String> = items
        .iter()
        .map(|x| {
            if is_nil(x) {
                String::new()
            } else {
                js_string(r, x)
            }
        })
        .collect();
    let size = parts.iter().map(String::len).sum::<usize>()
        + sep.len().saturating_mul(parts.len().saturating_sub(1));
    r.check_bytes(size)?;
    Ok(string(parts.join(&sep)))
}

/// `readArrayElement(v, index)` for `first` and `last`.
fn element(v: &Value, last: bool) -> Value {
    match v {
        Value::Array(a) => if last { a.last() } else { a.first() }
            .cloned()
            .unwrap_or_else(undefined),
        Value::String(s) if !s.is_empty() => {
            let units: Vec<u16> = s.encode_utf16().collect();
            let unit = if last {
                units[units.len() - 1]
            } else {
                units[0]
            };
            string(String::from_utf16_lossy(&[unit]))
        }
        Value::Object(o) => {
            // An object with a numeric `length` reads like an array.
            match o.get("length") {
                Some(Value::Number(len)) => {
                    let index = if last { len - 1.0 } else { 0.0 };
                    if index >= 0.0 && index.fract() == 0.0 {
                        o.get(&ops::num(index)).cloned().unwrap_or_else(undefined)
                    } else {
                        undefined()
                    }
                }
                _ => string(""),
            }
        }
        _ => string(""),
    }
}

pub(super) fn first(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(element(&v.to_value(), false))
}

pub(super) fn last(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(element(&v.to_value(), true))
}

pub(super) fn reverse(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(array(items.iter().rev().cloned().collect()))
}

/// `(v && v.length) || 0`.
pub(super) fn size(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let len = match v.to_value() {
        Value::String(s) => Value::Number(utf16_len(&s) as f64),
        Value::Array(a) => Value::Number(a.len() as f64),
        Value::Object(o) => o.get("length").cloned().unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    };
    Ok(if len.js_truthy() {
        len
    } else {
        Value::Number(0.0)
    })
}

pub(super) fn map(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    let path = stringify(r, &Args(a).value(0));
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(array(items.iter().map(|x| read_path(x, &path)).collect()))
}

pub(super) fn sum(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    let property = Args(a).value(0);
    let path = property.js_truthy().then(|| stringify(r, &property));
    let mut total = 0.0;
    for item in items.iter() {
        let x = match &path {
            Some(p) => read_path(item, p),
            None => item.clone(),
        };
        let n = x.to_js_number();
        if !n.is_nan() {
            total += n;
        }
    }
    Ok(Value::Number(total))
}

pub(super) fn compact(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    Ok(array(
        items.iter().filter(|x| !is_nil(x)).cloned().collect(),
    ))
}

pub(super) fn concat(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let lhs = to_array(&v.to_value());
    let rhs = to_array(&Args(a).or(0, array(Vec::new())));
    r.check_bytes((lhs.len() + rhs.len()).saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(array(lhs.iter().chain(rhs.iter()).cloned().collect()))
}

pub(super) fn push(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let lhs = to_array(&v.to_value());
    r.check_bytes((lhs.len() + 1).saturating_mul(std::mem::size_of::<Value>()))?;
    let mut items = lhs.as_ref().clone();
    items.push(Args(a).value(0));
    Ok(array(items))
}

pub(super) fn unshift(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let lhs = to_array(&v.to_value());
    r.check_bytes((lhs.len() + 1).saturating_mul(std::mem::size_of::<Value>()))?;
    let mut items = Vec::with_capacity(lhs.len() + 1);
    items.push(Args(a).value(0));
    items.extend(lhs.iter().cloned());
    Ok(array(items))
}

pub(super) fn pop(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    Ok(array(items[..items.len().saturating_sub(1)].to_vec()))
}

pub(super) fn shift(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    Ok(array(items.iter().skip(1).cloned().collect()))
}

pub(super) fn slice(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let tz = r.tz();
    let v = v.to_value();
    if is_nil(&v) {
        return Ok(array(Vec::new()));
    }
    let is_array = matches!(v, Value::Array(_));
    let text = if is_array {
        String::new()
    } else {
        stringify(r, &v)
    };
    let chars: Vec<char> = text.chars().collect();
    let len = if is_array {
        match &v {
            Value::Array(items) => items.len(),
            _ => 0,
        }
    } else {
        chars.len()
    };
    let mut begin = a.value(0);
    let length = a.or(1, Value::Number(1.0));
    if begin.to_js_number() < 0.0 {
        begin = ops::js_add(
            &Value::Number(len as f64),
            &ops::to_primitive(&Val::V(begin), tz),
            tz,
        );
    }
    if begin.to_js_number() < 0.0 || length.to_js_number() < 0.0 {
        return Ok(if is_array {
            array(Vec::new())
        } else {
            string("")
        });
    }
    let length_prim = ops::to_primitive(&Val::V(length.clone()), tz);
    if let Value::Array(items) = &v {
        let end = ops::js_add(
            &ops::to_primitive(&Val::V(begin.clone()), tz),
            &length_prim,
            tz,
        );
        let (from, to) =
            ops::slice_bounds(items.len(), begin.to_js_number(), Some(end.to_js_number()));
        return Ok(array(items[from..to].to_vec()));
    }
    let start = advance(begin.to_js_number(), len);
    let end = start + advance(length.to_js_number(), len - start);
    Ok(string(chars[start..end].iter().collect::<String>()))
}

/// How many code points `advanceCodePoints` steps over, up to `max`.
fn advance(count: f64, max: usize) -> usize {
    if count.is_nan() || count <= 0.0 {
        0
    } else if count >= max as f64 {
        max
    } else {
        count.ceil() as usize
    }
}

pub(super) fn uniq(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let mut out: Vec<Value> = Vec::new();
    for item in items.iter() {
        if !out.iter().any(|seen| same_value_zero(seen, item)) {
            out.push(item.clone());
        }
    }
    Ok(array(out))
}

/// A random shuffle's start: not repeatable, as `Math.random()` is not.
fn random_below(n: usize, state: &mut u64) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % n as u64) as usize
}

pub(super) fn sample(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let count = Args(a).or(0, Value::Number(1.0));
    let v = v.to_value();
    if is_nil(&v) {
        return Ok(array(Vec::new()));
    }
    let mut items: Vec<Value> = match &v {
        Value::Array(items) => items.as_ref().clone(),
        other => stringify(r, other)
            .chars()
            .map(|c| string(c.to_string()))
            .collect(),
    };
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64)
        | 1;
    for i in (1..items.len()).rev() {
        let j = random_below(i + 1, &mut state);
        items.swap(i, j);
    }
    if matches!(count, Value::Number(n) if n == 1.0) {
        return Ok(items.into_iter().next().unwrap_or(Value::Undefined));
    }
    let (from, to) = ops::slice_bounds(items.len(), 0.0, Some(count.to_js_number()));
    Ok(array(items[from..to].to_vec()))
}

// ---- sorting ---------------------------------------------------------------------------

/// `orderedCompare`: null and undefined last, otherwise `<` and `>`.
fn ordered_compare(a: &Value, b: &Value, tz: crate::Tz) -> i32 {
    match (is_nil(a), is_nil(b)) {
        (true, true) => return 0,
        (true, false) => return 1,
        (false, true) => return -1,
        _ => {}
    }
    let (x, y) = (Val::V(a.clone()), Val::V(b.clone()));
    if ops::compare(Cmp::Lt, &x, &y, tz) {
        -1
    } else if ops::compare(Cmp::Gt, &x, &y, tz) {
        1
    } else {
        0
    }
}

fn utf16_cmp(a: &str, b: &str) -> i32 {
    match a.encode_utf16().cmp(b.encode_utf16()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// `caseInsensitiveCompare`.
fn case_insensitive_compare(a: &Value, b: &Value, tz: crate::Tz) -> i32 {
    match (is_nil(a), is_nil(b)) {
        (true, true) => return 0,
        (true, false) => return 1,
        (false, true) => return -1,
        _ => {}
    }
    utf16_cmp(
        &ops::to_js_string(a, tz).to_lowercase(),
        &ops::to_js_string(b, tz).to_lowercase(),
    )
}

/// `Array.prototype.sort` with `cmp`, stable like V8's.
///
/// Up to 63 items this is V8's own algorithm (a run, then binary insertion),
/// so even a comparator that is not consistent, like `orderedCompare` on a
/// mix of numbers and words, gives V8's order. Longer arrays use a plain
/// stable merge sort, which agrees with V8 whenever the comparator is
/// consistent.
fn js_sort<T: Clone>(items: &mut Vec<T>, cmp: &dyn Fn(&T, &T) -> i32) {
    let n = items.len();
    if n < 2 {
        return;
    }
    if n >= 64 {
        *items = merge_sort(std::mem::take(items), cmp);
        return;
    }
    // CountAndMakeRun(0, n)
    let mut run = 2;
    let descending = cmp(&items[1], &items[0]) < 0;
    let mut previous = 1;
    for idx in 2..n {
        let order = cmp(&items[idx], &items[previous]);
        if descending {
            if order >= 0 {
                break;
            }
        } else if order < 0 {
            break;
        }
        previous = idx;
        run += 1;
    }
    if descending {
        items[..run].reverse();
    }
    // BinaryInsertionSort(0, run, n)
    for start in run..n {
        let (mut left, mut right) = (0, start);
        let pivot = items[start].clone();
        while left < right {
            let mid = left + ((right - left) >> 1);
            if cmp(&pivot, &items[mid]) < 0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        for p in (left + 1..=start).rev() {
            items[p] = items[p - 1].clone();
        }
        items[left] = pivot;
    }
}

fn merge_sort<T: Clone>(items: Vec<T>, cmp: &dyn Fn(&T, &T) -> i32) -> Vec<T> {
    if items.len() < 2 {
        return items;
    }
    let right = items[items.len() / 2..].to_vec();
    let left = items[..items.len() / 2].to_vec();
    let (left, right) = (merge_sort(left, cmp), merge_sort(right, cmp));
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if cmp(&right[j], &left[i]) < 0 {
            out.push(right[j].clone());
            j += 1;
        } else {
            out.push(left[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    out
}

fn sort_by(
    r: &mut Renderer<'_>,
    v: &Val,
    a: &[Val],
    cmp: fn(&Value, &Value, crate::Tz) -> i32,
) -> Out {
    let items = to_array(&v.to_value());
    let property = Args(a).value(0);
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let path = property.js_truthy().then(|| stringify(r, &property));
    let tz = r.tz();
    let mut keyed: Vec<(Value, Value)> = items
        .iter()
        .map(|item| {
            let key = match &path {
                Some(p) => read_path(item, p),
                None => item.clone(),
            };
            (item.clone(), key)
        })
        .collect();
    js_sort(&mut keyed, &|x, y| cmp(&x.1, &y.1, tz));
    Ok(array(keyed.into_iter().map(|(item, _)| item).collect()))
}

pub(super) fn sort(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    sort_by(r, v, a, ordered_compare)
}

pub(super) fn sort_natural(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    sort_by(r, v, a, case_insensitive_compare)
}

// ---- searching --------------------------------------------------------------------------

/// `matches(value, expected)`.
fn matches(value: &Val, expected: Option<&Val>) -> bool {
    match expected {
        None | Some(Val::V(Value::Undefined)) => value.truthy(),
        Some(e) => ops::equals(value, e),
    }
}

/// The property filters: `where`, `reject`, `find`, `find_index`, `has`.
/// Calls `visit(index, item, matched)` for each item until it says stop.
fn each_match(
    r: &mut Renderer<'_>,
    v: &Val,
    a: &[Val],
    mut visit: impl FnMut(usize, &Value, bool) -> bool,
) -> R<()> {
    let items = to_array(&v.to_value());
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let property = Args(a).value(0);
    let path = scope_path(r, &property)?;
    for (i, item) in items.iter().enumerate() {
        let value = eval_on(r, path.as_ref(), item)?;
        let matched = matches(&value, a.get(1));
        if !visit(i, item, matched) {
            break;
        }
    }
    Ok(())
}

fn filter(r: &mut Renderer<'_>, v: &Val, a: &[Val], include: bool) -> Out {
    let mut out = Vec::new();
    each_match(r, v, a, |_, item, matched| {
        if matched == include {
            out.push(item.clone());
        }
        true
    })?;
    Ok(array(out))
}

pub(super) fn where_(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    filter(r, v, a, true)
}

pub(super) fn reject(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    filter(r, v, a, false)
}

/// The first match, if any: `[index, item]`.
fn search(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> R<Option<(usize, Value)>> {
    let mut found = None;
    each_match(r, v, a, |i, item, matched| {
        if matched {
            found = Some((i, item.clone()));
        }
        !matched
    })?;
    Ok(found)
}

pub(super) fn has(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(Value::Bool(search(r, v, a)?.is_some()))
}

pub(super) fn find_index(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(search(r, v, a)?.map_or(Value::Undefined, |(i, _)| Value::Number(i as f64)))
}

pub(super) fn find(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(search(r, v, a)?.map_or(Value::Undefined, |(_, item)| item))
}

// ---- the expression filters --------------------------------------------------------------

/// `filter_exp`: items for which the expression is exactly `include`.
fn filter_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val], include: bool) -> Out {
    let a = Args(a);
    let exp = parse_exp(r, &a.value(1))?;
    let items = to_array(&v.to_value());
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let mut out = Vec::new();
    each_exp(r, &items, &a.value(0), &exp, |_, item, value| {
        if matches!(value, Val::V(Value::Bool(b)) if b == include) {
            out.push(item.clone());
        }
        true
    })?;
    Ok(array(out))
}

pub(super) fn where_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    filter_exp(r, v, a, true)
}

pub(super) fn reject_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    filter_exp(r, v, a, false)
}

/// `search_exp`: the first item whose expression is JavaScript-truthy.
fn search_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> R<Option<(usize, Value)>> {
    let a = Args(a);
    let exp = parse_exp(r, &a.value(1))?;
    let items = to_array(&v.to_value());
    let mut found = None;
    each_exp(r, &items, &a.value(0), &exp, |i, item, value| {
        if value.js_truthy() {
            found = Some((i, item.clone()));
            return false;
        }
        true
    })?;
    Ok(found)
}

pub(super) fn has_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(Value::Bool(search_exp(r, v, a)?.is_some()))
}

pub(super) fn find_index_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(search_exp(r, v, a)?.map_or(Value::Undefined, |(i, _)| Value::Number(i as f64)))
}

pub(super) fn find_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(search_exp(r, v, a)?.map_or(Value::Undefined, |(_, item)| item))
}

// ---- grouping ---------------------------------------------------------------------------------

/// A `Map` from key to items, in insertion order.
struct Groups(Vec<(Value, Vec<Value>)>);

impl Groups {
    fn add(&mut self, key: Value, item: Value) {
        match self.0.iter_mut().find(|(k, _)| same_value_zero(k, &key)) {
            Some((_, items)) => items.push(item),
            None => self.0.push((key, vec![item])),
        }
    }

    fn into_value(self) -> Value {
        array(
            self.0
                .into_iter()
                .map(|(name, items)| {
                    let mut o = IndexMap::new();
                    o.insert("name".to_string(), name);
                    o.insert("items".to_string(), array(items));
                    Value::Object(Arc::new(o))
                })
                .collect(),
        )
    }
}

pub(super) fn group_by(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let items = ops::to_enumerable(v);
    let path = scope_path(r, &Args(a).value(0))?;
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let mut groups = Groups(Vec::new());
    for item in items {
        let key = eval_on(r, path.as_ref(), &item)?.to_value();
        groups.add(key, item);
    }
    Ok(groups.into_value())
}

pub(super) fn group_by_exp(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let exp = parse_exp(r, &a.value(1))?;
    let items = ops::to_enumerable(v);
    r.check_bytes(items.len().saturating_mul(std::mem::size_of::<Value>()))?;
    let mut groups = Groups(Vec::new());
    each_exp(r, &items, &a.value(0), &exp, |_, item, key| {
        groups.add(key.to_value(), item.clone());
        true
    })?;
    Ok(groups.into_value())
}
