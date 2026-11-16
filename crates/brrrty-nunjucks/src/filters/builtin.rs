//! Nunjucks 3.2.4's built-in filters, from `nunjucks/src/filters.js`. Each
//! function follows the JavaScript line by line, so that a value of any
//! type gives what Nunjucks gives, or the TypeError V8 gives.
//!
//! What Brrrty can't match exactly is refused: `random`, a `sort` whose
//! comparisons would depend on V8's sorting algorithm, a `dump` of a value
//! that contains itself, and a few other corners named where they are.

use std::cmp::Ordering;

use brrrty_js::Value;

use super::jslib::{
    AttrGetter, arg, call, copy_safeness, encode_uri_component, entries, fit, get, get_str,
    is_object, is_string, is_zero, json_stringify, lib_map, lt, macro_args_named, need_method,
    normalize, parse_float, parse_int, spaces, string_receiver, text, to_array,
};
use super::text as re;
use crate::error::ErrorKind;
use crate::jsops::{self, BinOp, Ops};
use crate::render::{R, RenderErr, Renderer};
use crate::value::{Obj, Val};

fn plain(message: &str) -> RenderErr {
    // A `TemplateError` thrown by a filter, as it reaches the caller.
    RenderErr::new(
        ErrorKind::Render,
        format!("Template render error: {message}"),
    )
}

/// Runs the built-in filter `name` on `argv`, the input first.
pub(super) fn run(r: &mut Renderer<'_>, name: &str, argv: &[Val]) -> R<Val> {
    let ops = r.ops();
    let item_limit = r.engine.item_limit;
    let a = |i: usize| arg(argv, i);
    match name {
        "abs" => Ok(Val::num(jsops::to_number(&a(0), ops)?.abs())),
        "batch" => batch(&a(0), &a(1), &a(2), ops, item_limit),
        "capitalize" => capitalize(a(0), ops),
        "center" => center(a(0), &a(1), ops),
        "default" | "d" => Ok(default(a(0), a(1), &a(2))),
        "dictsort" => dictsort(&a(0), &a(1), &a(2)),
        "dump" => Ok(match json_stringify(&a(0), &a(1), ops)? {
            Some(s) => Val::str(fit(s, ops)?),
            None => Val::undef(),
        }),
        "escape" | "e" => escape(a(0), ops),
        "safe" => safe(a(0), ops),
        "forceescape" => force_escape(a(0), ops),
        "first" => get(&a(0), &Val::num(0.0), ops),
        "groupby" => groupby(&a(0), &a(1), ops, item_limit),
        "indent" => indent(a(0), &a(1), &a(2), ops),
        "join" => join(&a(0), &a(1), &a(2), ops, item_limit),
        "last" => {
            let arr = a(0);
            let len = get_str(&arr, "length", ops)?;
            let i = jsops::binary(BinOp::Sub, &len, &Val::num(1.0), ops)?;
            get(&arr, &i, ops)
        }
        "length" => length(a(0), ops),
        "list" => list(a(0), ops),
        "lower" => {
            let s = normalize(a(0), "");
            call(&s, "str", "toLowerCase", &[], ops)
        }
        "upper" => {
            let s = normalize(a(0), "");
            call(&s, "str", "toUpperCase", &[], ops)
        }
        "nl2br" => nl2br(a(0)),
        "random" => random(&a(0), ops),
        "reject" => select_or_reject(false, &a(0), &a(1), &a(2), ops, item_limit),
        "select" => select_or_reject(true, &a(0), &a(1), &a(2), ops, item_limit),
        "rejectattr" => select_attr(false, &a(0), &a(1), ops),
        "selectattr" => select_attr(true, &a(0), &a(1), ops),
        "replace" => replace(a(0), &a(1), &a(2), &a(3), ops),
        "reverse" => reverse(a(0), ops, item_limit),
        "round" => round(&a(0), &a(1), &a(2), ops),
        "slice" => slice(&a(0), &a(1), &a(2), ops, item_limit),
        "sum" => sum(&a(0), &a(1), a(2), ops, item_limit),
        "sort" => sort(argv, ops, item_limit),
        "string" => string(a(0), ops),
        "striptags" => striptags(a(0), &a(1), ops),
        "title" => title(a(0), ops),
        "trim" => trim(a(0)),
        "truncate" => truncate(a(0), &a(1), &a(2), &a(3), ops),
        "urlencode" => urlencode(&a(0), ops),
        "urlize" => urlize(&a(0), &a(1), &a(2), ops),
        "wordcount" => wordcount(a(0)),
        "float" => {
            let s = jsops::to_string(&a(0), ops)?;
            let res = parse_float(&s);
            Ok(if res.is_nan() { a(1) } else { Val::num(res) })
        }
        "int" => {
            let args = macro_args_named(argv, &["value", "default", "base"]);
            let base = if args[2].is_undefined() {
                Val::num(10.0)
            } else {
                args[2].clone()
            };
            let s = jsops::to_string(&args[0], ops)?;
            let radix = to_int32(jsops::to_number(&base, ops)?);
            let res = parse_int(&s, radix)?;
            Ok(if res.is_nan() {
                args[1].clone()
            } else {
                Val::num(res)
            })
        }
        other => Err(jsops::unsupported(format!(
            "Brrrty doesn't have the Nunjucks filter `{other}` yet"
        ))),
    }
}

/// `ToInt32`.
fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    (n.trunc().rem_euclid(4_294_967_296.0) as u64) as u32 as i32
}

// ---- escaping ----

pub(super) fn escape(input: Val, ops: Ops) -> R<Val> {
    Ok(match input {
        Val::Safe(_) => input,
        v if v.is_nullish() => Val::safe(""),
        v => Val::safe(jsops::escape(&jsops::to_string_method(&v, ops)?)),
    })
}

pub(super) fn safe(input: Val, ops: Ops) -> R<Val> {
    Ok(match input {
        Val::Safe(_) => input,
        v if v.is_nullish() => Val::safe(""),
        v => Val::safe(jsops::to_string_method(&v, ops)?),
    })
}

fn force_escape(input: Val, ops: Ops) -> R<Val> {
    Ok(if input.is_nullish() {
        Val::safe("")
    } else {
        Val::safe(jsops::escape(&jsops::to_string_method(&input, ops)?))
    })
}

// ---- strings ----

fn capitalize_text(lower: &str) -> String {
    let mut chars = lower.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    if first.len_utf16() == 2 {
        // `charAt(0)` is half of a pair, which `toUpperCase` leaves as it
        // is, and the pair is whole again after the `+`.
        return lower.to_string();
    }
    let mut out: String = first.to_uppercase().collect();
    out.push_str(chars.as_str());
    out
}

fn capitalize(input: Val, ops: Ops) -> R<Val> {
    let s = normalize(input, "");
    let lower = call(&s, "str", "toLowerCase", &[], ops)?;
    let lower = lower.as_str().unwrap_or_default().to_string();
    Ok(copy_safeness(&s, capitalize_text(&lower)))
}

fn center(input: Val, width: &Val, ops: Ops) -> R<Val> {
    let s = normalize(input, "");
    let width = if width.truthy() {
        width.clone()
    } else {
        Val::num(80.0)
    };
    let len = get_str(&s, "length", ops)?;
    if jsops::binary(BinOp::Ge, &len, &width, ops)?.truthy() {
        return Ok(s);
    }
    let n = jsops::to_number(&jsops::binary(BinOp::Sub, &width, &len, ops)?, ops)?;
    let pre = " ".repeat(super::jslib::count_f(n / 2.0 - n % 2.0, ops)?);
    let post = " ".repeat(super::jslib::count_f(n / 2.0, ops)?);
    let joined = jsops::add(&jsops::add(&Val::str(pre), &s, ops)?, &Val::str(post), ops)?;
    Ok(copy_safeness(
        &s,
        jsops::to_string(&joined, ops)?.to_string(),
    ))
}

fn default(val: Val, def: Val, bool_: &Val) -> Val {
    if bool_.truthy() {
        if val.truthy() { val } else { def }
    } else if val.is_undefined() {
        def
    } else {
        val
    }
}

fn indent(input: Val, width: &Val, indent_first: &Val, ops: Ops) -> R<Val> {
    let s = normalize(input, "");
    if matches!(&s, Val::V(Value::String(t)) if t.is_empty()) {
        return Ok(Val::str(""));
    }
    let width = if width.truthy() {
        width.clone()
    } else {
        Val::num(4.0)
    };
    need_method(&s, "str", "split")?;
    let lines = string_receiver(&s, "split")?.split('\n');
    let sp = spaces(&width, ops)?;
    let mut res = String::new();
    for (i, l) in lines.enumerate() {
        if i > 0 {
            res.push('\n');
        }
        if i == 0 && !indent_first.truthy() {
            res.push_str(l);
        } else {
            res.push_str(&sp);
            res.push_str(l);
        }
        if res.len() > ops.limit {
            return Err(jsops::too_big(ops));
        }
    }
    Ok(copy_safeness(&s, res))
}

fn nl2br(input: Val) -> R<Val> {
    if input.is_nullish() {
        return Ok(Val::str(""));
    }
    need_method(&input, "str", "replace")?;
    let res = re::nl2br(string_receiver(&input, "replace")?);
    Ok(copy_safeness(&input, res))
}

fn trim(input: Val) -> R<Val> {
    need_method(&input, "str", "replace")?;
    let res = re::trim(string_receiver(&input, "replace")?).to_string();
    Ok(copy_safeness(&input, res))
}

fn title(input: Val, ops: Ops) -> R<Val> {
    let s = normalize(input, "");
    need_method(&s, "str", "split")?;
    let mut words = Vec::new();
    for w in string_receiver(&s, "split")?.split(' ') {
        // `capitalize(word)`: a plain string in, a plain string out.
        words.push(capitalize_text(&w.to_lowercase()));
    }
    Ok(copy_safeness(&s, fit(words.join(" "), ops)?))
}

fn truncate(input: Val, length: &Val, killwords: &Val, end: &Val, ops: Ops) -> R<Val> {
    let orig = input.clone();
    let mut input = normalize(input, "");
    let length = if length.truthy() {
        length.clone()
    } else {
        Val::num(255.0)
    };
    let len = get_str(&input, "length", ops)?;
    if jsops::binary(BinOp::Le, &len, &length, ops)?.truthy() {
        return Ok(input);
    }
    if killwords.truthy() {
        input = call(
            &input,
            "input",
            "substring",
            &[Val::num(0.0), length.clone()],
            ops,
        )?;
    } else {
        let mut idx = call(
            &input,
            "input",
            "lastIndexOf",
            &[Val::str(" "), length.clone()],
            ops,
        )?;
        if jsops::strict_eq(&idx, &Val::num(-1.0))? {
            idx = length.clone();
        }
        input = call(&input, "input", "substring", &[Val::num(0.0), idx], ops)?;
    }
    let end = if end.is_nullish() {
        Val::str("...")
    } else {
        end.clone()
    };
    let out = jsops::add(&input, &end, ops)?;
    Ok(copy_safeness(
        &orig,
        jsops::to_string(&out, ops)?.to_string(),
    ))
}

fn replace(input: Val, old: &Val, new_: &Val, max: &Val, ops: Ops) -> R<Val> {
    // `if (old instanceof RegExp) return str.replace(old, new_)`: a plain
    // string, even from a SafeString.
    if let Val::Re(_) = old {
        return match &input {
            Val::V(Value::String(_)) | Val::Safe(_) => {
                jsops::call_method(&input, "replace", &[old.clone(), new_.clone()], ops)
            }
            _ => Err(jsops::type_error("str.replace is not a function")),
        };
    }
    let original = input.clone();
    let max = if max.is_undefined() {
        Val::num(-1.0)
    } else {
        max.clone()
    };
    let old = match old {
        Val::V(Value::Number(n)) => Val::str(brrrty_js::js_number_to_string(*n)),
        v if is_string(v) => v.clone(),
        _ => return Ok(input),
    };
    let s = match input {
        Val::V(Value::Number(n)) => Val::str(brrrty_js::js_number_to_string(n)),
        v => v,
    };
    let Some(src) = text(&s) else {
        return Ok(s);
    };
    let old_text = old.as_str().unwrap_or_default();
    if old_text.is_empty() {
        // `new_ + str.split('').join(new_) + new_`: the string cut into
        // UTF-16 units.
        if src.chars().any(|c| c.len_utf16() == 2) {
            return Err(jsops::unsupported(
                "`replace` with an empty search on text with characters outside the Basic Multilingual Plane (such as emoji)",
            ));
        }
        let pieces: Vec<Val> = src.chars().map(|c| Val::str(c.to_string())).collect();
        let joined =
            jsops::call_method(&Val::arr(pieces), "join", std::slice::from_ref(new_), ops)?;
        let res = jsops::add(&jsops::add(new_, &joined, ops)?, new_, ops)?;
        return Ok(copy_safeness(
            &original,
            jsops::to_string(&res, ops)?.to_string(),
        ));
    }
    let hay = jsops::utf16(src);
    let needle = jsops::utf16(old_text);
    let find = |from: usize| -> Option<usize> {
        if needle.len() > hay.len() || from > hay.len() - needle.len() {
            return None;
        }
        (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == needle[..])
    };
    let mut next = find(0);
    if jsops::strict_eq(&max, &Val::num(0.0))? || next.is_none() {
        return Ok(s);
    }
    let mut res = String::new();
    let mut new_text: Option<String> = None;
    let mut pos = 0usize;
    let mut count = 0f64;
    while let Some(at) = next {
        if !(jsops::strict_eq(&max, &Val::num(-1.0))?
            || jsops::binary(BinOp::Lt, &Val::num(count), &max, ops)?.truthy())
        {
            break;
        }
        // `piece + new_` is the piece and `new_` as a primitive string.
        if new_text.is_none() {
            let p = jsops::to_primitive(new_, jsops::Hint::Default, ops)?;
            new_text = Some(jsops::prim_to_string(&p).to_string());
        }
        res.push_str(&jsops::from_utf16(&hay[pos..at])?);
        res.push_str(new_text.as_deref().unwrap_or_default());
        if res.len() > ops.limit {
            return Err(jsops::too_big(ops));
        }
        pos = at + needle.len();
        count += 1.0;
        next = find(pos);
    }
    if pos < hay.len() {
        res.push_str(&jsops::from_utf16(&hay[pos..])?);
    }
    Ok(copy_safeness(&original, fit(res, ops)?))
}

fn striptags(input: Val, preserve: &Val, ops: Ops) -> R<Val> {
    let input = normalize(input, "");
    need_method(&input, "input", "replace")?;
    let stripped = re::remove_tags(string_receiver(&input, "replace")?);
    let trimmed = re::trim(&stripped);
    let res = if preserve.truthy() {
        re::preserve_linebreaks(trimmed)
    } else {
        re::squash_whitespace(trimmed)
    };
    Ok(copy_safeness(&input, fit(res, ops)?))
}

fn wordcount(input: Val) -> R<Val> {
    let s = normalize(input, "");
    // `str ? str.match(/\w+/g) : null`
    if !s.truthy() {
        return Ok(Val::null());
    }
    need_method(&s, "str", "match")?;
    let n = re::word_runs(string_receiver(&s, "match")?);
    Ok(if n == 0 {
        Val::null()
    } else {
        Val::num(n as f64)
    })
}

fn urlize(input: &Val, length: &Val, nofollow: &Val, ops: Ops) -> R<Val> {
    let length = match length {
        Val::V(Value::Number(n)) if n.is_nan() => Val::num(f64::INFINITY),
        v => v.clone(),
    };
    let no_follow = if matches!(nofollow, Val::V(Value::Bool(true))) {
        " rel=\"nofollow\""
    } else {
        ""
    };
    need_method(input, "str", "split")?;
    let src = string_receiver(input, "split")?;
    let mut out = String::new();
    for word in re::words_and_gaps(src) {
        let possible = re::strip_punctuation(word);
        let short = |ops: Ops| -> R<String> {
            let v = jsops::call_method(
                &Val::str(possible),
                "substr",
                &[Val::num(0.0), length.clone()],
                ops,
            )?;
            Ok(v.as_str().unwrap_or_default().to_string())
        };
        if word.chars().any(brrrty_js::is_js_whitespace) {
            out.push_str(word);
        } else if re::is_http(possible) {
            out.push_str(&format!(
                "<a href=\"{possible}\"{no_follow}>{}</a>",
                short(ops)?
            ));
        } else if re::is_www(possible) {
            out.push_str(&format!(
                "<a href=\"http://{possible}\"{no_follow}>{}</a>",
                short(ops)?
            ));
        } else if re::is_email(possible) {
            out.push_str(&format!("<a href=\"mailto:{possible}\">{possible}</a>"));
        } else if re::has_tld(possible) {
            out.push_str(&format!(
                "<a href=\"http://{possible}\"{no_follow}>{}</a>",
                short(ops)?
            ));
        } else {
            out.push_str(word);
        }
        if out.len() > ops.limit {
            return Err(jsops::too_big(ops));
        }
    }
    Ok(Val::str(out))
}

fn string(obj: Val, ops: Ops) -> R<Val> {
    if matches!(obj, Val::Safe(_)) {
        // `new SafeString(obj)` returns `obj` itself: it isn't a string.
        return Ok(obj);
    }
    if obj.is_nullish() {
        need_method(&obj, "target", "toString")?;
    }
    if let Val::V(Value::Object(o)) = &obj
        && o.contains_key("toString")
    {
        return Err(jsops::type_error("target.toString is not a function"));
    }
    Ok(Val::str(jsops::to_string(&obj, ops)?))
}

fn urlencode(obj: &Val, ops: Ops) -> R<Val> {
    if let Some(s) = obj.as_str() {
        return Ok(Val::str(encode_uri_component(s)));
    }
    let mut parts = Vec::new();
    if obj.is_array() {
        for item in obj.array_items().unwrap_or_default() {
            // `function (_ref) { var k = _ref[0], v = _ref[1]; ... }`
            let k = pair_part(&item, 0, ops)?;
            let enc_k = encode_part(&k, ops)?;
            let v = pair_part(&item, 1, ops)?;
            parts.push(format!("{enc_k}={}", encode_part(&v, ops)?));
        }
    } else {
        for (k, v) in entries(obj) {
            parts.push(format!(
                "{}={}",
                encode_uri_component(&k),
                encode_part(&Part::Value(v), ops)?
            ));
        }
    }
    Ok(Val::str(fit(parts.join("&"), ops)?))
}

/// What `item[i]` gave in `urlencode`.
enum Part {
    /// A UTF-16 unit of a string, which may be half a character.
    Unit(u16),
    Value(Val),
}

fn pair_part(item: &Val, i: usize, ops: Ops) -> R<Part> {
    if let Some(s) = item.as_str() {
        return Ok(match s.encode_utf16().nth(i) {
            Some(u) => Part::Unit(u),
            None => Part::Value(Val::undef()),
        });
    }
    Ok(Part::Value(get(item, &Val::num(i as f64), ops)?))
}

/// `encodeURIComponent(value)`, where a lone surrogate throws.
fn encode_part(part: &Part, ops: Ops) -> R<String> {
    match part {
        Part::Unit(u) => match String::from_utf16(&[*u]) {
            Ok(s) => Ok(encode_uri_component(&s)),
            Err(_) => Err(jsops::js_err("URIError", "URI malformed")),
        },
        Part::Value(v) => Ok(encode_uri_component(&jsops::to_string(v, ops)?)),
    }
}

// ---- numbers ----

fn round(val: &Val, precision: &Val, method: &Val, ops: Ops) -> R<Val> {
    let precision = if precision.truthy() {
        precision.clone()
    } else {
        Val::num(0.0)
    };
    let factor = jsops::pow(10.0, jsops::to_number(&precision, ops)?);
    let x = jsops::to_number(val, ops)? * factor;
    let rounded = match method.as_str() {
        Some("ceil") => x.ceil(),
        Some("floor") => x.floor(),
        _ => js_round(x),
    };
    Ok(Val::num(rounded / factor))
}

/// `Math.round`: halves go toward +infinity.
fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    if r == 0.0 && x < 0.0 { -0.0 } else { r }
}

// ---- lists and objects ----

fn batch(arr: &Val, linecount: &Val, fill: &Val, ops: Ops, limit: usize) -> R<Val> {
    let mut res = Vec::new();
    let mut tmp: Vec<Val> = Vec::new();
    let mut i = 0usize;
    loop {
        let len = get_str(arr, "length", ops)?;
        if !lt(&Val::num(i as f64), &len, ops)? {
            break;
        }
        if i >= limit {
            return Err(super::jslib::too_long("`batch`", limit));
        }
        let m = jsops::binary(BinOp::Mod, &Val::num(i as f64), linecount, ops)?;
        if is_zero(&m) && !tmp.is_empty() {
            res.push(Val::arr(std::mem::take(&mut tmp)));
        }
        tmp.push(get(arr, &Val::num(i as f64), ops)?);
        i += 1;
    }
    if !tmp.is_empty() {
        if fill.truthy() {
            let mut k = tmp.len();
            while lt(&Val::num(k as f64), linecount, ops)? {
                if k >= limit {
                    return Err(super::jslib::too_long("`batch`", limit));
                }
                tmp.push(fill.clone());
                k += 1;
            }
        }
        res.push(Val::arr(tmp));
    }
    Ok(Val::arr(res))
}

fn slice(arr: &Val, slices: &Val, fill: &Val, ops: Ops, limit: usize) -> R<Val> {
    let len = jsops::to_number(&get_str(arr, "length", ops)?, ops)?;
    let n = jsops::to_number(slices, ops)?;
    let slice_length = (len / n).floor();
    let extra = len % n;
    let mut res = Vec::new();
    let mut offset = 0.0;
    let mut i = 0usize;
    while (i as f64) < n {
        if i >= limit {
            return Err(super::jslib::too_long("`slice`", limit));
        }
        let fi = i as f64;
        let start = offset + fi * slice_length;
        if fi < extra {
            offset += 1.0;
        }
        let end = offset + (fi + 1.0) * slice_length;
        let curr = call(arr, "arr", "slice", &[Val::num(start), Val::num(end)], ops)?;
        let curr = if fill.truthy() && fi >= extra {
            need_method(&curr, "currSlice", "push")?;
            let mut items = curr.array_items().unwrap_or_default();
            items.push(fill.clone());
            Val::arr(items)
        } else {
            curr
        };
        res.push(curr);
        i += 1;
    }
    Ok(Val::arr(res))
}

fn sum(arr: &Val, attr: &Val, start: Val, ops: Ops, limit: usize) -> R<Val> {
    let start = if start.is_undefined() {
        Val::num(0.0)
    } else {
        start
    };
    let arr = if attr.truthy() {
        Val::arr(lib_map(arr, ops, limit, |v| get(&v, attr, ops))?)
    } else {
        arr.clone()
    };
    need_method(&arr, "arr", "reduce")?;
    let mut acc = Val::num(0.0);
    let mut text: Option<String> = None;
    for item in arr.array_items().unwrap_or_default() {
        match &mut text {
            // Once the sum is a string, each `+` appends the item's text.
            Some(t) => {
                let p = jsops::to_primitive(&item, jsops::Hint::Default, ops)?;
                t.push_str(&jsops::prim_to_string(&p));
                if t.len() > ops.limit {
                    return Err(jsops::too_big(ops));
                }
            }
            None => {
                acc = jsops::add(&acc, &item, ops)?;
                if let Some(s) = acc.as_str() {
                    text = Some(s.to_string());
                }
            }
        }
    }
    if let Some(t) = text {
        acc = Val::str(t);
    }
    jsops::add(&start, &acc, ops)
}

fn join(arr: &Val, del: &Val, attr: &Val, ops: Ops, limit: usize) -> R<Val> {
    let del = if del.truthy() {
        del.clone()
    } else {
        Val::str("")
    };
    let arr = if attr.truthy() {
        Val::arr(lib_map(arr, ops, limit, |v| get(&v, attr, ops))?)
    } else {
        arr.clone()
    };
    call(&arr, "arr", "join", &[del], ops)
}

fn list(val: Val, ops: Ops) -> R<Val> {
    if let Some(s) = val.as_str() {
        let mut out = Vec::new();
        for u in s.encode_utf16() {
            out.push(Val::str(jsops::from_utf16(&[u])?));
        }
        return Ok(Val::arr(out));
    }
    if is_object(&val) {
        let _ = ops;
        return Ok(Val::arr(
            entries(&val)
                .into_iter()
                .map(|(k, v)| {
                    jsops::new_object(vec![
                        ("key".to_string(), Val::str(k)),
                        ("value".to_string(), v),
                    ])
                })
                .collect(),
        ));
    }
    if val.is_array() {
        return Ok(val);
    }
    Err(plain("list filter: type not iterable"))
}

fn reverse(val: Val, ops: Ops, limit: usize) -> R<Val> {
    if let Some(s) = val.as_str() {
        let mut units: Vec<u16> = s.encode_utf16().collect();
        units.reverse();
        return Ok(Val::str(jsops::from_utf16(&units)?));
    }
    let mut items = lib_map(&val, ops, limit, Ok)?;
    items.reverse();
    Ok(Val::arr(items))
}

fn length(val: Val, ops: Ops) -> R<Val> {
    let value = normalize(val, "");
    if is_object(&value) && !matches!(value, Val::Safe(_)) {
        let n = match &value {
            Val::Obj(o) => o.props.borrow().len(),
            Val::V(Value::Object(o)) => o.len(),
            _ => 0,
        };
        return Ok(Val::num(n as f64));
    }
    get_str(&value, "length", ops)
}

fn random(arr: &Val, ops: Ops) -> R<Val> {
    let len = jsops::to_number(&get_str(arr, "length", ops)?, ops)?;
    if len.is_nan() {
        return get_str(arr, "NaN", ops);
    }
    if (0.0..=1.0).contains(&len) {
        // `Math.floor(Math.random() * len)` is 0 whatever `random` gives.
        return get(arr, &Val::num(0.0), ops);
    }
    Err(jsops::unsupported(
        "the `random` filter picks at random, so its output can't be the same as Nunjucks's",
    )
    .with_help("pick the item in the data, or with a Rhai filter that is deterministic"))
}

fn select_or_reject(
    expected: bool,
    arr: &Val,
    test_name: &Val,
    second: &Val,
    ops: Ops,
    limit: usize,
) -> R<Val> {
    let test_name = if test_name.is_undefined() {
        Val::str("truthy")
    } else {
        test_name.clone()
    };
    let key = jsops::property_key(&test_name, ops)?;
    let test = crate::tests::lookup(&key)?;
    let mut out = Vec::new();
    for item in to_array(arr, ops, limit)? {
        let result = crate::tests::run(test, &item, Some(second), ops)?;
        if jsops::strict_eq(&result, &Val::bool(expected))? {
            out.push(item);
        }
    }
    Ok(Val::arr(out))
}

fn select_attr(expected: bool, arr: &Val, attr: &Val, ops: Ops) -> R<Val> {
    need_method(arr, "arr", "filter")?;
    let mut out = Vec::new();
    for item in arr.array_items().unwrap_or_default() {
        let truthy = get(&item, attr, ops)?.truthy();
        if truthy == expected {
            out.push(item);
        }
    }
    Ok(Val::arr(out))
}

fn groupby(arr: &Val, attr: &Val, ops: Ops, limit: usize) -> R<Val> {
    if matches!(attr, Val::Func(_)) {
        return Err(jsops::unsupported(
            "`groupby` with a function instead of an attribute name",
        ));
    }
    let getter = AttrGetter::new(attr, ops)?;
    let result = Obj::new(Default::default());
    let mut i = 0usize;
    loop {
        let len = get_str(arr, "length", ops)?;
        if !lt(&Val::num(i as f64), &len, ops)? {
            break;
        }
        if i >= limit {
            return Err(super::jslib::too_long("`groupby`", limit));
        }
        let value = get(arr, &Val::num(i as f64), ops)?;
        let key = getter.get(&value, ops)?;
        let key = jsops::property_key(&key, ops)?;
        if jsops::OBJECT_PROTO.contains(&&*key) || &*key == "__proto__" {
            return Err(jsops::type_error(
                "(result[key] || result[key]).push is not a function",
            ));
        }
        match result.get(&key) {
            Some(Val::Arr(group)) => {
                group.borrow_mut().push(value);
                crate::render::note_mutation(&Val::Arr(group));
            }
            _ => {
                let group = Val::arr(vec![value]);
                crate::render::note_mutation(&group);
                result.set(&key, group);
            }
        }
        i += 1;
    }
    let out = Val::Obj(result);
    crate::render::note_mutation(&out);
    Ok(out)
}

// ---- sorting ----

/// What a sort key is, when comparing keys is a total order that any
/// stable sort gives the same answer for.
enum Keys {
    Numbers(Vec<f64>),
    Strings(Vec<String>),
}

/// Reads `keys` as one kind, or `None`. Numbers with NaN, `undefined`,
/// mixed kinds and objects make the comparisons depend on V8's TimSort
/// (they aren't a consistent order), which Brrrty doesn't reproduce.
/// With `dates`, a date counts as the number of milliseconds `<` and `>`
/// compare it by; `===` would compare two dates by identity, so `dictsort`
/// doesn't take them.
fn uniform(keys: &[Val], dates: bool) -> Option<Keys> {
    if dates
        && keys.iter().any(|k| matches!(k, Val::V(Value::Date(_))))
        && keys.iter().all(|k| {
            matches!(k, Val::V(Value::Date(_)))
                || matches!(k, Val::V(Value::Number(n)) if !n.is_nan())
        })
    {
        return Some(Keys::Numbers(
            keys.iter()
                .map(|k| match k {
                    Val::V(Value::Date(d)) => d.timestamp_millis() as f64,
                    Val::V(Value::Number(n)) => *n,
                    _ => 0.0,
                })
                .collect(),
        ));
    }
    if keys
        .iter()
        .all(|k| matches!(k, Val::V(Value::Number(n)) if !n.is_nan()))
    {
        return Some(Keys::Numbers(
            keys.iter()
                .map(|k| match k {
                    Val::V(Value::Number(n)) => *n,
                    _ => 0.0,
                })
                .collect(),
        ));
    }
    if keys.iter().all(is_string) {
        return Some(Keys::Strings(
            keys.iter()
                .map(|k| k.as_str().unwrap_or_default().to_string())
                .collect(),
        ));
    }
    None
}

fn order_of(keys: &Keys) -> impl Fn(usize, usize) -> Ordering + '_ {
    move |a, b| match keys {
        Keys::Numbers(n) => n[a].partial_cmp(&n[b]).unwrap_or(Ordering::Equal),
        Keys::Strings(s) => s[a].encode_utf16().cmp(s[b].encode_utf16()),
    }
}

fn refuse_sort(what: &str) -> RenderErr {
    jsops::unsupported(format!(
        "`{what}` of values that aren't all numbers or all strings: the order would depend on how V8 sorts"
    ))
    .with_help("make the values one type, or sort in the data")
}

fn sort(argv: &[Val], ops: Ops, limit: usize) -> R<Val> {
    let args = macro_args_named(argv, &["value", "reverse", "case_sensitive", "attribute"]);
    let (arr, reversed, case_sens, attr) = (&args[0], args[1].truthy(), args[2].truthy(), &args[3]);
    let all = lib_map(arr, ops, limit, Ok)?;
    // V8 puts `undefined` items last without asking the comparator.
    let (items, undefined): (Vec<Val>, Vec<Val>) = all.into_iter().partition(|v| !v.is_undefined());
    if items.len() < 2 {
        return Ok(Val::arr(items.into_iter().chain(undefined).collect()));
    }
    let getter = AttrGetter::new(attr, ops)?;
    let mut keys = Vec::with_capacity(items.len());
    for item in &items {
        keys.push(if attr.truthy() {
            getter.get(item, ops)?
        } else {
            item.clone()
        });
    }
    let Some(mut uni) = uniform(&keys, true) else {
        return Err(refuse_sort("sort"));
    };
    if let Keys::Strings(s) = &mut uni
        && !case_sens
    {
        for k in s.iter_mut() {
            *k = k.to_lowercase();
        }
    }
    let cmp = order_of(&uni);
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        let o = cmp(a, b);
        if reversed { o.reverse() } else { o }
    });
    Ok(Val::arr(
        order
            .into_iter()
            .map(|i| items[i].clone())
            .chain(undefined)
            .collect(),
    ))
}

fn dictsort(val: &Val, case_sensitive: &Val, by: &Val) -> R<Val> {
    if !is_object(val) {
        return Err(plain("dictsort filter: val must be an object"));
    }
    if matches!(val, Val::Safe(_)) {
        return Err(jsops::unsupported(
            "`dictsort` of a SafeString (Nunjucks lists its `val`, `length`, `valueOf` and `toString`)",
        ));
    }
    let pairs = entries(val);
    let si = match by {
        Val::V(Value::Undefined) => 0,
        v if v.as_str() == Some("key") => 0,
        v if v.as_str() == Some("value") => 1,
        _ => {
            return Err(plain(
                "dictsort filter: You can only sort by either key or value",
            ));
        }
    };
    let out = |order: Vec<usize>| {
        Val::arr(
            order
                .into_iter()
                .map(|i| Val::arr(vec![Val::str(pairs[i].0.clone()), pairs[i].1.clone()]))
                .collect(),
        )
    };
    if pairs.len() < 2 {
        return Ok(out((0..pairs.len()).collect()));
    }
    let keys: Vec<Val> = pairs
        .iter()
        .map(|(k, v)| {
            if si == 0 {
                Val::str(k.clone())
            } else {
                v.clone()
            }
        })
        .collect();
    let Some(mut uni) = uniform(&keys, false) else {
        return Err(refuse_sort("dictsort"));
    };
    if let Keys::Strings(s) = &mut uni
        && !case_sensitive.truthy()
    {
        for k in s.iter_mut() {
            *k = k.to_uppercase();
        }
    }
    let cmp = order_of(&uni);
    let mut order: Vec<usize> = (0..pairs.len()).collect();
    order.sort_by(|&a, &b| cmp(a, b));
    Ok(out(order))
}
