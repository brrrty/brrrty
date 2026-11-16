//! Nunjucks's tests (`tests.js`), for `x is name`.

use brrrty_js::Value;

use crate::jsops::{self, BinOp, Ops};
use crate::render::{R, error};
use crate::value::Val;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Test {
    Callable,
    Defined,
    DivisibleBy,
    Escaped,
    EqualTo,
    Even,
    Falsy,
    Ge,
    Gt,
    Le,
    Lt,
    Lower,
    Ne,
    Null,
    Number,
    Odd,
    String,
    Truthy,
    Undefined,
    Upper,
    Iterable,
    Mapping,
}

/// `env.getTest(name)`.
pub(crate) fn lookup(name: &str) -> R<Test> {
    Ok(match name {
        "callable" => Test::Callable,
        "defined" => Test::Defined,
        "divisibleby" => Test::DivisibleBy,
        "escaped" => Test::Escaped,
        "equalto" | "eq" | "sameas" => Test::EqualTo,
        "even" => Test::Even,
        "falsy" => Test::Falsy,
        "ge" => Test::Ge,
        "greaterthan" | "gt" => Test::Gt,
        "le" => Test::Le,
        "lessthan" | "lt" => Test::Lt,
        "lower" => Test::Lower,
        "ne" => Test::Ne,
        "null" => Test::Null,
        "number" => Test::Number,
        "odd" => Test::Odd,
        "string" => Test::String,
        "truthy" => Test::Truthy,
        "undefined" => Test::Undefined,
        "upper" => Test::Upper,
        "iterable" => Test::Iterable,
        "mapping" => Test::Mapping,
        _ => return Err(error(format!("test not found: {name}"))),
    })
}

/// Runs a test on `value`, with `arg` as its second argument.
pub(crate) fn run(test: Test, value: &Val, arg: Option<&Val>, ops: Ops) -> R<Val> {
    let undef = Val::undef();
    let two = arg.unwrap_or(&undef);
    let op = |o: BinOp, a: &Val, b: &Val| jsops::binary(o, a, b, ops);
    let rem_is = |n: f64, d: &Val| -> R<Val> {
        let r = op(BinOp::Mod, value, d)?;
        op(BinOp::StrictEq, &r, &Val::num(n))
    };
    Ok(match test {
        Test::Callable => Val::bool(matches!(value, Val::Func(_))),
        Test::Defined => Val::bool(!value.is_undefined()),
        Test::DivisibleBy => rem_is(0.0, two)?,
        Test::Escaped => Val::bool(matches!(value, Val::Safe(_))),
        Test::EqualTo => op(BinOp::StrictEq, value, two)?,
        Test::Even => rem_is(0.0, &Val::num(2.0))?,
        Test::Odd => rem_is(1.0, &Val::num(2.0))?,
        Test::Falsy => Val::bool(!value.truthy()),
        Test::Truthy => Val::bool(value.truthy()),
        Test::Ge => op(BinOp::Ge, value, two)?,
        Test::Gt => op(BinOp::Gt, value, two)?,
        Test::Le => op(BinOp::Le, value, two)?,
        Test::Lt => op(BinOp::Lt, value, two)?,
        Test::Ne => op(BinOp::StrictNe, value, two)?,
        Test::Null => Val::bool(matches!(value, Val::V(Value::Null))),
        Test::Undefined => Val::bool(value.is_undefined()),
        Test::Number => Val::bool(value.type_of() == "number"),
        Test::String => Val::bool(value.type_of() == "string"),
        Test::Lower | Test::Upper => {
            let method = if test == Test::Lower {
                "toLowerCase"
            } else {
                "toUpperCase"
            };
            match value {
                Val::V(Value::String(s)) => {
                    let changed = if test == Test::Lower {
                        s.to_lowercase()
                    } else {
                        s.to_uppercase()
                    };
                    Val::bool(*changed == **s)
                }
                // A string compared with the SafeString object itself.
                Val::Safe(_) => Val::bool(false),
                Val::V(Value::Undefined | Value::Null) => {
                    let what = if value.is_undefined() {
                        "undefined"
                    } else {
                        "null"
                    };
                    return Err(jsops::type_error(format!(
                        "Cannot read properties of {what} (reading '{method}')"
                    )));
                }
                _ => {
                    return Err(jsops::type_error(format!(
                        "value.{method} is not a function"
                    )));
                }
            }
        }
        Test::Iterable => match value {
            Val::V(Value::Undefined | Value::Null) => {
                let what = if value.is_undefined() {
                    "undefined"
                } else {
                    "object null"
                };
                return Err(jsops::type_error(format!(
                    "{what} is not iterable (cannot read property Symbol(Symbol.iterator))"
                )));
            }
            _ => Val::bool(matches!(
                value,
                Val::V(Value::String(_) | Value::Array(_)) | Val::Safe(_) | Val::Arr(_)
            )),
        },
        Test::Mapping => Val::bool(
            value.type_of() == "object"
                && !value.is_array()
                && !matches!(value, Val::V(Value::Null)),
        ),
    })
}
