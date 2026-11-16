//! `{% for %}`, as `compileFor` writes it.
//!
//! - The loop pushes a frame, then evaluates its iterable. A falsy iterable
//!   runs nothing.
//! - With one name, it counts `i` up while `i < arr.length` (read again
//!   each time, so pushing onto the array inside the loop extends it) and
//!   reads `arr[i]`: arrays, strings by UTF-16 unit, and any object with a
//!   `length`.
//! - With several names, an array's items are unpacked (`item[0]`,
//!   `item[1]`, ...); anything else is iterated as an object, by key and
//!   value. Only the object form sets the names in the frame by name, so
//!   only it is visible to an include.
//! - `loop.*` are set on the loop's frame each time round.
//! - `else` runs when the loop's `len` variable is falsy. That variable
//!   lives as long as the function, so a falsy iterable keeps the value an
//!   earlier run of the same loop left.

use std::rc::Rc;

use brrrty_js::Value;

use crate::compile::{ForStmt, ForTargets};
use crate::context::{Frame, Scope};
use crate::jsops::{self, BinOp};
use crate::render::{R, Renderer, too_many};
use crate::value::Val;

impl Renderer<'_> {
    pub(crate) fn for_loop(&mut self, f: &ForStmt, scope: &Rc<Scope>, out: &mut String) -> R<()> {
        let tpl = scope.tpl.clone();
        tpl.set_frame(tpl.frame().push(false));
        let arr = self.eval(&f.iter, scope)?;
        if arr.truthy() {
            let arr = self.iterable(arr)?;
            match &f.targets {
                ForTargets::One { name, slot } => {
                    let len = self.prop(&arr, "length")?;
                    scope.put(0, f.len_slot, len.clone());
                    let mut i = 0usize;
                    loop {
                        let cur = self.prop(&arr, "length")?;
                        let more = jsops::binary(BinOp::Lt, &Val::num(i as f64), &cur, self.ops())?;
                        if !more.truthy() {
                            break;
                        }
                        let v = jsops::raw_index(&arr, &Val::num(i as f64), self.ops())?;
                        scope.put(0, *slot, v.clone());
                        let frame = tpl.frame();
                        frame.set(name, v, false)?;
                        self.loop_bindings(&frame, i, &len)?;
                        self.exec(&f.body, scope, out)?;
                        i += 1;
                        check_count(i, self.engine.item_limit)?;
                    }
                }
                ForTargets::Many { names, slots } if arr.is_array() => {
                    let len = self.prop(&arr, "length")?;
                    scope.put(0, f.len_slot, len.clone());
                    let mut i = 0usize;
                    loop {
                        let cur = self.prop(&arr, "length")?;
                        let more = jsops::binary(BinOp::Lt, &Val::num(i as f64), &cur, self.ops())?;
                        if !more.truthy() {
                            break;
                        }
                        let item = jsops::raw_index(&arr, &Val::num(i as f64), self.ops())?;
                        for (u, slot) in slots.iter().enumerate() {
                            let v = jsops::raw_index(&item, &Val::num(u as f64), self.ops())?;
                            scope.put(0, *slot, v);
                        }
                        let frame = tpl.frame();
                        self.loop_bindings(&frame, i, &len)?;
                        self.exec(&f.body, scope, out)?;
                        i += 1;
                        check_count(i, self.engine.item_limit)?;
                    }
                    let _ = names;
                }
                ForTargets::Many { names, slots } => {
                    let pairs = self.object_entries(&arr)?;
                    let len = Val::num(pairs.len() as f64);
                    scope.put(0, f.len_slot, len.clone());
                    for (i, (k, v)) in pairs.into_iter().enumerate() {
                        let k = Val::str(k);
                        scope.put(0, slots[0], k.clone());
                        scope.put(0, slots[1], v.clone());
                        let frame = tpl.frame();
                        frame.set(&names[0], k, false)?;
                        frame.set(&names[1], v, false)?;
                        self.loop_bindings(&frame, i, &len)?;
                        self.exec(&f.body, scope, out)?;
                        check_count(i + 1, self.engine.item_limit)?;
                    }
                }
            }
        }
        if let Some(else_) = &f.else_
            && !scope.get(0, f.len_slot).truthy()
        {
            self.exec(else_, scope, out)?;
        }
        tpl.set_frame(tpl.frame().pop());
        Ok(())
    }

    /// `arr.length` and other plain reads.
    fn prop(&self, v: &Val, key: &str) -> R<Val> {
        jsops::raw_index(v, &Val::str(key), self.ops())
    }

    /// `runtime.fromIterator`: a `SafeString` becomes its characters.
    fn iterable(&self, v: Val) -> R<Val> {
        match &v {
            Val::Safe(s) => Ok(Val::arr(
                s.0.chars().map(|c| Val::str(c.to_string())).collect(),
            )),
            Val::Func(_) => Err(jsops::unsupported("Brrrty doesn't loop over a function")),
            Val::Re(_) => Err(jsops::unsupported(
                "Brrrty doesn't loop over a regular expression",
            )),
            _ => Ok(v),
        }
    }

    /// The keys and values `for (k in obj)` visits.
    fn object_entries(&self, v: &Val) -> R<Vec<(String, Val)>> {
        Ok(match v {
            Val::V(Value::Object(o)) => Value::js_entries(o)
                .into_iter()
                .map(|(k, v)| (k.clone(), Val::V(v.clone())))
                .collect(),
            Val::Obj(o) => o
                .keys()
                .into_iter()
                .map(|k| {
                    let v = o.get(&k).unwrap_or_else(Val::undef);
                    (k, v)
                })
                .collect(),
            Val::V(Value::String(s)) => {
                let units: Vec<u16> = s.encode_utf16().collect();
                let mut out = Vec::with_capacity(units.len());
                for (i, u) in units.iter().enumerate() {
                    out.push((
                        i.to_string(),
                        Val::str(jsops::from_utf16(std::slice::from_ref(u))?),
                    ));
                }
                out
            }
            _ => Vec::new(),
        })
    }

    /// `_emitLoopBindings`.
    fn loop_bindings(&self, frame: &Rc<Frame>, i: usize, len: &Val) -> R<()> {
        let ops = self.ops();
        let i = Val::num(i as f64);
        let one = Val::num(1.0);
        let index = jsops::binary(BinOp::Add, &i, &one, ops)?;
        let revindex = jsops::binary(BinOp::Sub, len, &i, ops)?;
        let revindex0 = jsops::binary(BinOp::Sub, &revindex, &one, ops)?;
        let first = jsops::binary(BinOp::StrictEq, &i, &Val::num(0.0), ops)?;
        let len_less = jsops::binary(BinOp::Sub, len, &one, ops)?;
        let last = jsops::binary(BinOp::StrictEq, &i, &len_less, ops)?;
        frame.set("loop.index", index, false)?;
        frame.set("loop.index0", i, false)?;
        frame.set("loop.revindex", revindex, false)?;
        frame.set("loop.revindex0", revindex0, false)?;
        frame.set("loop.first", first, false)?;
        frame.set("loop.last", last, false)?;
        frame.set("loop.length", len.clone(), false)?;
        Ok(())
    }
}

fn check_count(n: usize, limit: usize) -> R<()> {
    if n > limit {
        return Err(too_many(format!("a loop ran more than {limit} times")));
    }
    Ok(())
}
