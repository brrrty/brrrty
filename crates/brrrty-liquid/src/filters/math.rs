//! Math filters: `argumentsToNumber(Math.fn)` and friends.

use brrrty_js::{Value, js_number_to_string, js_string_to_number};

use super::{Args, Out, to_number};
use crate::context::Val;
use crate::render::Renderer;

/// The input and every argument as numbers, as `argumentsToNumber` makes
/// them.
fn numbers(input: &Val, argv: &[Val]) -> Vec<f64> {
    std::iter::once(input.to_value())
        .chain(argv.iter().map(Val::to_value))
        .map(|v| to_number(&v))
        .collect()
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

/// `undefined` as a number, which is `NaN`.
fn at(nums: &[f64], i: usize) -> f64 {
    nums.get(i).copied().unwrap_or(f64::NAN)
}

pub(super) fn abs(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(n(at(&numbers(v, a), 0).abs()))
}

/// `Math.max(...args)`: every argument counts, not just the first.
pub(super) fn at_least(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(n(numbers(v, a)
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max)))
}

pub(super) fn at_most(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(n(numbers(v, a).into_iter().fold(f64::INFINITY, f64::min)))
}

pub(super) fn ceil(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(n(at(&numbers(v, a), 0).ceil()))
}

pub(super) fn floor(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    Ok(n(at(&numbers(v, a), 0).floor()))
}

pub(super) fn divided_by(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let x = numbers(v, a);
    let (dividend, divisor) = (at(&x, 0), at(&x, 1));
    // `integerArithmetic = false` unless the third argument is a nonzero number.
    let integer = x.get(2).is_some_and(|i| *i != 0.0);
    Ok(n(if integer {
        (dividend / divisor).floor()
    } else {
        dividend / divisor
    }))
}

pub(super) fn minus(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let x = numbers(v, a);
    Ok(n(at(&x, 0) - at(&x, 1)))
}

pub(super) fn plus(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let x = numbers(v, a);
    Ok(n(at(&x, 0) + at(&x, 1)))
}

pub(super) fn times(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let x = numbers(v, a);
    Ok(n(at(&x, 0) * at(&x, 1)))
}

/// `((v % arg) + arg) % arg`.
pub(super) fn modulo(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let x = numbers(v, a);
    let (v, m) = (at(&x, 0), at(&x, 1));
    Ok(n(((v % m) + m) % m))
}

/// `shiftDecimal`: moves the decimal point through the number's text, so
/// `1.005` rounds to two places the way its digits say, not the way its
/// binary value does.
fn shift_decimal(x: f64, exponent: f64) -> f64 {
    let text = js_number_to_string(x);
    let (mantissa, power) = text.split_once('e').unwrap_or((&text, "0"));
    let power = js_string_to_number(power);
    js_string_to_number(&format!(
        "{mantissa}e{}",
        js_number_to_string(power + exponent)
    ))
}

/// `Math.round`: halves go up.
fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

pub(super) fn round(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let value = to_number(&v.to_value());
    let places = to_number(&Args(a).or(0, Value::Number(0.0)));
    let scaled = shift_decimal(value.abs(), places);
    // 2^53 - 1. Written as a negation so NaN returns the input.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(scaled <= 9_007_199_254_740_991.0) {
        return Ok(n(value));
    }
    let rounded = shift_decimal(js_round(scaled), -places);
    Ok(n(if value < 0.0 { -rounded } else { rounded }))
}
