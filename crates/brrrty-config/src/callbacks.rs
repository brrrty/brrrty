//! `map`, `filter` and `reduce` on arrays, as Rhai's own do them, with
//! less work per call.
//!
//! Rhai's versions try up to three ways to call the function for every
//! item: with the item as `this`, then with the item as an argument, then
//! with the index too. Each way that doesn't fit the function's parameters
//! is a failed lookup that builds an error message, so `|acc, w, i|` in
//! `reduce` costs two failed lookups per item. An anonymous function's
//! parameters can't change between items, so the way that worked for the
//! first item is used for the rest. Any other function pointer (a named
//! function, which a native overload can answer by argument type, or a
//! curried one) still tries every way each time, as Rhai does.

use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, INT, NativeCallContext, Position};

type R<T> = Result<T, Box<EvalAltResult>>;

/// The ways [`Callback::call`] can call the function, in the order Rhai
/// tries them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    /// The item as `this`, then the arguments.
    This,
    /// The item moved in among the arguments.
    Moved,
    /// The item moved in, and the index after the arguments.
    WithIndex,
}

struct Callback<'a> {
    f: &'a FnPtr,
    caller: &'static str,
    /// The way that worked, for a function whose parameters are fixed.
    way: Option<Way>,
    fixed: bool,
}

impl<'a> Callback<'a> {
    fn new(f: &'a FnPtr, caller: &'static str) -> Callback<'a> {
        Callback {
            f,
            caller,
            way: None,
            fixed: f.is_anonymous() && !f.is_curried(),
        }
    }

    fn not_found(&self, err: &EvalAltResult) -> bool {
        matches!(err, EvalAltResult::ErrorFunctionNotFound(sig, ..) if sig.starts_with(self.f.fn_name()))
    }

    fn call_way(
        &self,
        ctx: &NativeCallContext,
        way: Way,
        item: &mut Dynamic,
        args: &[Dynamic],
        index: usize,
        at: usize,
    ) -> R<Dynamic> {
        match way {
            Way::This => self.f.call_raw(ctx, Some(item), args.to_vec()),
            Way::Moved => {
                let mut all = args.to_vec();
                all.insert(at, item.clone());
                self.f.call_raw(ctx, None, all)
            }
            Way::WithIndex => {
                let mut all = args.to_vec();
                all.push(Dynamic::from(INT::try_from(index).unwrap_or(INT::MAX)));
                all.insert(at, item.clone());
                self.f.call_raw(ctx, None, all)
            }
        }
    }

    /// Rhai's `call_raw_with_extra_args(caller, ctx, Some(item), args,
    /// [index], Some(at))`.
    fn call(
        &mut self,
        ctx: &NativeCallContext,
        item: &mut Dynamic,
        args: &[Dynamic],
        index: usize,
        at: usize,
    ) -> R<Dynamic> {
        let result = match self.way {
            Some(way) => self.call_way(ctx, way, item, args, index, at),
            None => {
                let mut tried = Way::This;
                let mut r = self.call_way(ctx, Way::This, item, args, index, at);
                for next in [Way::Moved, Way::WithIndex] {
                    match &r {
                        Err(e) if self.not_found(e) => {
                            tried = next;
                            r = self.call_way(ctx, next, item, args, index, at);
                        }
                        _ => break,
                    }
                }
                if self.fixed && r.is_ok() {
                    self.way = Some(tried);
                }
                r
            }
        };
        result.map_err(|err| {
            Box::new(EvalAltResult::ErrorInFunctionCall(
                self.caller.to_string(),
                ctx.call_source().unwrap_or("").to_string(),
                err,
                Position::NONE,
            ))
        })
    }
}

fn map(ctx: NativeCallContext, array: &mut Array, f: FnPtr) -> R<Array> {
    let mut cb = Callback::new(&f, "map");
    let mut out = Array::with_capacity(array.len());
    for (i, item) in array.iter_mut().enumerate() {
        out.push(cb.call(&ctx, item, &[], i, 0)?);
    }
    Ok(out)
}

fn filter(ctx: NativeCallContext, array: &mut Array, f: FnPtr) -> R<Array> {
    let mut cb = Callback::new(&f, "filter");
    let mut out = Array::new();
    for (i, item) in array.iter_mut().enumerate() {
        if cb.call(&ctx, item, &[], i, 0)?.as_bool().unwrap_or(false) {
            out.push(item.clone());
        }
    }
    Ok(out)
}

fn reduce(ctx: NativeCallContext, array: &mut Array, f: FnPtr, initial: Dynamic) -> R<Dynamic> {
    let mut cb = Callback::new(&f, "reduce");
    let mut acc = initial;
    for (i, item) in array.iter_mut().enumerate() {
        acc = cb.call(&ctx, item, &[acc], i, 1)?;
    }
    Ok(acc)
}

pub(crate) fn register(e: &mut Engine) {
    e.register_fn("map", map)
        .register_fn("filter", filter)
        .register_fn(
            "reduce",
            |ctx: NativeCallContext, a: &mut Array, f: FnPtr| reduce(ctx, a, f, Dynamic::UNIT),
        )
        .register_fn("reduce", reduce);
}

#[cfg(test)]
mod tests {
    use rhai::Engine;

    /// Rhai's own `map`, `filter` and `reduce` and these give the same
    /// result, or the same error, for every way a function can take its
    /// arguments.
    #[test]
    fn same_as_rhais_own() {
        let mut ours = Engine::new();
        super::register(&mut ours);
        let rhai = Engine::new();
        let scripts = [
            "[1, 2, 3].map(|x| x * 2)",
            "[1, 2, 3].map(|x, i| x * 10 + i)",
            "[1, 2, 3].map(|| this * 3)",
            "[1, 2, 3].filter(|x| x > 1)",
            "[1, 2, 3].filter(|x, i| i != 1)",
            "[1, 2, 3].filter(|| this != 2)",
            "[1, 2, 3].filter(|x| x)",
            "[1, 2, 3].reduce(|a, x| (a ?? 0) + x)",
            "[1, 2, 3].reduce(|a, x, i| (a ?? 0) + x * i)",
            "[1, 2, 3].reduce(|a| (a ?? 0) + this)",
            "[1, 2, 3].reduce(|a, x| a + x, 10)",
            "[\"a\", \"b\", \"c\"].reduce(|acc, w, i| if i == 0 { w } else { acc + \" \" + w }, \"\")",
            "[].reduce(|a, x| a + x, 5)",
            "[].map(|x| x)",
            "fn dbl(x) { x * 2 } [1, 2].map(Fn(\"dbl\"))",
            "fn dbl(x) { x * 2 } [1, 2].map(\"dbl\")",
            "fn add(a, x, i) { (a ?? 0) + x + i } [1, 2].reduce(Fn(\"add\"))",
            "let f = Fn(\"add3\").curry(100); fn add3(c, a, x) { c + (a ?? 0) + x } [1, 2].reduce(f)",
            "[1, 2, 3].map(|x, y, z| x)",
            "[1, 2, 3].reduce(|a, x| x.nope())",
            "[1, 2, 3].map(|x| if x == 2 { throw \"two\" } else { x })",
            "let a = [1, 2]; a.map(|| { this = 5; this }); a",
        ];
        for s in scripts {
            let want = rhai.eval::<rhai::Dynamic>(s).map(|v| format!("{v:?}"));
            let got = ours.eval::<rhai::Dynamic>(s).map(|v| format!("{v:?}"));
            match (want, got) {
                (Ok(w), Ok(g)) => assert_eq!(g, w, "{s}"),
                (Err(w), Err(g)) => assert_eq!(g.to_string(), w.to_string(), "{s}"),
                (w, g) => panic!("{s}: Rhai gave {w:?}, ours gave {g:?}"),
            }
        }
    }
}
