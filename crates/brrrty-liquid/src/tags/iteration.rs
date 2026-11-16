//! `for`, `tablerow` and `cycle`.

use std::collections::HashMap;
use std::rc::Rc;

use brrrty_js::Value;

use crate::ast::{Expr, FilteredValue, For, Tablerow};
use crate::context::{LoopDrop, Val};
use crate::ops::{self, num};
use crate::render::{MAX_ITEMS, R, Renderer, too_many};

/// The items `collection` loops over, refusing more than the limit.
fn items(r: &mut Renderer<'_>, collection: &Expr, tag: &str) -> R<Vec<Value>> {
    let v = r.eval(collection)?;
    let len = ops::enumerable_len(&v);
    if len > MAX_ITEMS {
        return Err(too_many(format!(
            "the {tag} loop has {len} items, more than the limit of {MAX_ITEMS}"
        )));
    }
    Ok(ops::to_enumerable(&v))
}

/// A hash value, if the hash has the key and it is not undefined.
fn arg<'v>(hash: &'v [(String, Val)], key: &str) -> Option<&'v Val> {
    hash.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .filter(|v| !v.is_undefined())
}

/// `array.slice(start, end)`, with JavaScript's coercion of the bounds.
fn slice(items: Vec<Value>, start: &Val, end: Option<&Val>) -> Vec<Value> {
    let n = |v: &Val| v.to_value().to_js_number();
    let (from, to) = ops::slice_bounds(items.len(), n(start), end.map(n));
    items.into_iter().skip(from).take(to - from).collect()
}

pub(super) fn for_tag(r: &mut Renderer<'_>, f: &For, out: &mut String) -> R<()> {
    let mut items = items(r, &f.collection, "for")?;
    if items.is_empty() {
        return r.render_nodes(&f.else_body, out);
    }
    let tz = r.tz();
    // `offset: continue` reads the register through a `continue` variable.
    // A first loop sees `{}`, as LiquidJS's `getRegister(key, {})` gives.
    let key = format!("continue-{}-{}", f.variable, f.collection_text);
    let start = r
        .ctx
        .continues
        .entry(key.clone())
        .or_insert_with(|| Value::Object(Default::default()))
        .clone();
    r.ctx
        .scopes
        .push(HashMap::from([("continue".to_string(), Val::V(start))]));
    let hash = r.eval_hash(&f.args);
    r.ctx.scopes.pop();
    let hash = hash?;

    // LiquidJS applies these in this order, whatever order they are written in.
    let offset = arg(&hash, "offset");
    if let Some(o) = offset {
        items = slice(items, o, None);
    }
    if let Some(l) = arg(&hash, "limit") {
        items = slice(items, &Val::number(0.0), Some(l));
    }
    if arg(&hash, "reversed").is_some() {
        items.reverse();
    }
    // `(hash['offset'] || 0) + collection.length`.
    let base = match offset {
        Some(o) if o.js_truthy() => ops::to_primitive(o, tz),
        _ => Value::Number(0.0),
    };
    let next = ops::js_add(&base, &Value::Number(items.len() as f64), tz);
    r.ctx.continues.insert(key, next);

    let drop = Rc::new(LoopDrop::new(
        items.len(),
        format!("{}-{}", f.variable, f.collection_text),
        None,
    ));
    r.ctx.scopes.push(HashMap::from([(
        "forloop".to_string(),
        Val::Loop(drop.clone()),
    )]));
    let top = r.ctx.scopes.len() - 1;
    let mut result = Ok(());
    for item in items {
        r.ctx.scopes[top].insert(f.variable.clone(), Val::V(item));
        r.ctx.clear_flags();
        result = r.render_nodes(&f.body, out);
        if result.is_err() || r.ctx.break_called {
            break;
        }
        drop.next();
    }
    r.ctx.clear_flags();
    r.ctx.scopes.pop();
    result
}

/// `tablerow`. It neither resets nor obeys `break` and `continue`, so a
/// flag set in one cell cuts every later cell short after its first node,
/// as in LiquidJS.
pub(super) fn tablerow(r: &mut Renderer<'_>, t: &Tablerow, out: &mut String) -> R<()> {
    let items = items(r, &t.collection, "tablerow")?;
    let tz = r.tz();
    let hash = r.eval_hash(&t.args)?;
    let raw = |k: &str| hash.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    // `args.offset || 0` and `offset + limit`, with JavaScript's `+`.
    let offset = match raw("offset") {
        Some(o) if o.js_truthy() => o.clone(),
        _ => Val::number(0.0),
    };
    let limit = match arg(&hash, "limit") {
        Some(l) => l.clone(),
        None => Val::number(items.len() as f64),
    };
    let end = Val::V(ops::js_add(
        &ops::to_primitive(&offset, tz),
        &ops::to_primitive(&limit, tz),
        tz,
    ));
    let items = slice(items, &offset, Some(&end));
    let cols = match raw("cols") {
        Some(c) if c.js_truthy() => c.clone(),
        _ => Val::number(items.len() as f64),
    };

    let len = items.len();
    let drop = Rc::new(LoopDrop::new(
        len,
        format!("{}-{}", t.variable, t.collection_text),
        Some(cols),
    ));
    r.ctx.scopes.push(HashMap::from([(
        "tablerowloop".to_string(),
        Val::Loop(drop.clone()),
    )]));
    let top = r.ctx.scopes.len() - 1;
    let result = (|| {
        for item in items {
            r.ctx.scopes[top].insert(t.variable.clone(), Val::V(item));
            if drop.col0() == 0.0 {
                let row = drop.row();
                if row != 1.0 {
                    r.write(out, "</tr>")?;
                }
                r.write(out, &format!("<tr class=\"row{}\">", num(row)))?;
            }
            r.write(
                out,
                &format!("<td class=\"col{}\">", num(drop.col0() + 1.0)),
            )?;
            r.render_nodes(&t.body, out)?;
            r.write(out, "</td>")?;
            drop.next();
        }
        if len > 0 {
            r.write(out, "</tr>")?;
        }
        Ok(())
    })();
    r.ctx.scopes.pop();
    result
}

/// `cycle` prints the next candidate of its group. Groups are keyed by the
/// group's value and the candidates as written, so `'a','b'` and
/// `'a', 'b'` are different cycles.
pub(super) fn cycle(
    r: &mut Renderer<'_>,
    group: Option<&Expr>,
    candidates: &[FilteredValue],
    out: &mut String,
) -> R<()> {
    let group = match group {
        Some(g) => r.eval(g)?,
        None => Val::undefined(),
    };
    let texts: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
    let fingerprint = format!(
        "cycle:{}:{}",
        ops::template_string(&group, r.tz()),
        texts.join(",")
    );
    let idx = *r.ctx.cycles.get(&fingerprint).unwrap_or(&0);
    let candidate = &candidates[idx];
    r.ctx
        .cycles
        .insert(fingerprint, (idx + 1) % candidates.len());
    let v = r.eval_filtered(candidate)?.to_value();
    // A tag's falsy result is not written.
    if v.js_truthy() {
        r.write_value(out, &Val::V(v))?;
    }
    Ok(())
}
