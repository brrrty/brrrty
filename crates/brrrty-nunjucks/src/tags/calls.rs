//! Calling things: `runtime.callWrap`, macros (`runtime.makeMacro` and the
//! function `_compileMacro` writes), the globals, and filters.

use std::cell::Cell;
use std::rc::Rc;

use crate::compile::{E, MacroDef};
use crate::context::{Frame, Scope};
use crate::error::ErrorKind;
use crate::jsops::{self, BinOp};
use crate::render::{MAX_CALLS, R, RenderErr, Renderer, error, too_many};
use crate::value::{Func, Global, Obj, Val};

impl Renderer<'_> {
    /// `runtime.callWrap(obj, name, context, args)`.
    pub(crate) fn call_wrap(&mut self, f: &Val, name: &str, args: Vec<Val>) -> R<Val> {
        if !f.truthy() {
            return Err(error(format!(
                "Unable to call `{name}`, which is undefined or falsey"
            )));
        }
        let Val::Func(func) = f else {
            return Err(error(format!(
                "Unable to call `{name}`, which is not a function"
            )));
        };
        self.call(func, None, args)
    }

    /// Calls `f` with `this` as its receiver (`None` for the context).
    pub(crate) fn call(&mut self, f: &Rc<Func>, this: Option<&Val>, args: Vec<Val>) -> R<Val> {
        match &**f {
            Func::Macro { def, scope } => self.call_macro(def, scope, args),
            Func::Global(Global::Range) => self.range(&args),
            Func::Global(Global::Cycler) => Ok(cycler(args)),
            Func::Global(Global::Joiner) => {
                let sep = args.into_iter().next().unwrap_or_else(Val::undef);
                let sep = if sep.truthy() { sep } else { Val::str(",") };
                Ok(Val::func(Func::Joiner {
                    sep,
                    first: Cell::new(true),
                }))
            }
            Func::Joiner { sep, first } => Ok(if first.replace(false) {
                Val::str("")
            } else {
                sep.clone()
            }),
            Func::CyclerNext { items, index } => {
                let mut i = index.get() + 1;
                if i >= items.len() as i64 {
                    i = 0;
                }
                index.set(i);
                let current = items.get(i as usize).cloned().unwrap_or_else(Val::undef);
                set_current(this, current.clone())?;
                Ok(current)
            }
            Func::CyclerReset { index } => {
                index.set(-1);
                set_current(this, Val::null())?;
                Ok(Val::undef())
            }
            Func::Method { this, name } => {
                let own = match this {
                    Val::Obj(o) => o.get(name),
                    _ => None,
                };
                match own {
                    Some(Val::Func(g)) => self.call(&g, Some(this), args),
                    Some(_) => Err(jsops::type_error("obj[val].apply is not a function")),
                    None => jsops::call_method(this, name, &args, self.ops()),
                }
            }
        }
    }

    /// A macro or caller: `makeMacro`'s argument handling, then the body
    /// in a new frame.
    fn call_macro(&mut self, def: &MacroDef, closure: &Rc<Scope>, args: Vec<Val>) -> R<Val> {
        self.calls += 1;
        if self.calls > MAX_CALLS {
            self.calls -= 1;
            return Err(RenderErr::new(
                ErrorKind::Limit,
                format!("macros are nested more than {MAX_CALLS} calls deep"),
            )
            .with_help("look for a macro that calls itself without stopping"));
        }
        let r = self.macro_body(def, closure, args);
        self.calls -= 1;
        r
    }

    fn macro_body(&mut self, def: &MacroDef, closure: &Rc<Scope>, mut args: Vec<Val>) -> R<Val> {
        let names = &def.args;
        let is_kw = |v: &Val| matches!(v, Val::Obj(o) if o.has("__keywords"));
        let kw_passed = args.last().is_some_and(is_kw);
        let count = args.len() - usize::from(kw_passed);
        let kwargs: Rc<Obj> = match args.last() {
            Some(Val::Obj(o)) if kw_passed => o.clone(),
            _ => Rc::new(Obj::default()),
        };
        let mut positional: Vec<Val>;
        let kwargs = if count > names.len() {
            positional = args[..names.len()].to_vec();
            for (i, v) in args[names.len()..count].iter().enumerate() {
                if let Some((k, _)) = def.kwargs.get(i) {
                    kwargs.set(k, v.clone());
                }
            }
            Some(kwargs)
        } else if count < names.len() {
            positional = args[..count].to_vec();
            for (name, _) in &names[count..] {
                positional.push(kwargs.get(name).unwrap_or_else(Val::undef));
                kwargs.props.borrow_mut().shift_remove(name);
            }
            Some(kwargs)
        } else {
            // `func.apply(this, macroArgs)`: the keyword object, if any,
            // is the `kwargs` parameter.
            positional = std::mem::take(&mut args);
            let kw = positional.get(names.len()).cloned();
            positional.truncate(names.len());
            match kw {
                Some(Val::Obj(o)) => Some(o),
                Some(v) if v.truthy() => {
                    return Err(jsops::unsupported(
                        "Brrrty can't pass this value as a macro's keyword arguments",
                    ));
                }
                _ => None,
            }
        };
        let kwargs = kwargs.unwrap_or_default();

        let scope = Scope::new(def.slots, Some(closure.clone()), closure.tpl.clone());
        let tpl = &closure.tpl;
        let caller_frame = tpl.frame();
        tpl.set_frame(if def.keep_frame {
            caller_frame.push(true)
        } else {
            Frame::new()
        });
        let frame = tpl.frame();
        if let Some(c) = kwargs.get("caller") {
            frame.set("caller", c, false)?;
        }
        for ((name, slot), v) in names.iter().zip(positional) {
            frame.set(name, v.clone(), false)?;
            scope.put(0, *slot, v);
        }
        for (name, default) in &def.kwargs {
            let v = match kwargs.get(name) {
                Some(v) => v,
                None => self.eval(default, &scope)?,
            };
            tpl.frame().set(name, v, false)?;
        }
        let out = self.capture(&def.body, &scope)?;
        tpl.set_frame(if def.keep_frame {
            tpl.frame().pop()
        } else {
            caller_frame
        });
        Ok(Val::safe(out))
    }

    /// The `range` global.
    fn range(&mut self, args: &[Val]) -> R<Val> {
        let ops = self.ops();
        let arg = |i: usize| args.get(i).cloned().unwrap_or_else(Val::undef);
        let (mut start, mut stop, mut step) = (arg(0), arg(1), arg(2));
        if stop.is_undefined() {
            stop = start;
            start = Val::num(0.0);
            step = Val::num(1.0);
        } else if !step.truthy() {
            step = Val::num(1.0);
        }
        let up = jsops::binary(BinOp::Gt, &step, &Val::num(0.0), ops)?.truthy();
        let test = if up { BinOp::Lt } else { BinOp::Gt };
        let mut out = Vec::new();
        let mut i = start;
        while jsops::binary(test, &i, &stop, ops)?.truthy() {
            let limit = self.engine.item_limit;
            if out.len() >= limit {
                return Err(too_many(format!(
                    "range() would make more than {limit} items"
                )));
            }
            out.push(i.clone());
            i = jsops::binary(BinOp::Add, &i, &step, ops)?;
        }
        Ok(Val::arr(out))
    }

    /// `env.getFilter(name).call(context, input, ...args)`.
    pub(crate) fn filter(
        &mut self,
        name: &str,
        args: &[E],
        kwargs: Option<&[(String, E)]>,
        scope: &Rc<Scope>,
    ) -> R<Val> {
        let filter = crate::filters::lookup(self.engine, name)?;
        let argv = self.eval_args(args, kwargs, scope)?;
        crate::filters::run(self, &filter, name, &argv, scope)
    }
}

/// `this.current = value` in a cycler's methods.
fn set_current(this: Option<&Val>, v: Val) -> R<()> {
    match this {
        Some(Val::Obj(o)) => {
            o.set("current", v);
            crate::render::note_mutation(&Val::Obj(o.clone()));
            Ok(())
        }
        _ => Err(jsops::unsupported(
            "Brrrty runs a cycler's methods only on the cycler itself",
        )),
    }
}

/// The `cycler` global's object: `{current, reset, next}`.
fn cycler(items: Vec<Val>) -> Val {
    let index = Rc::new(Cell::new(-1));
    let items = Rc::new(items);
    jsops::new_object(vec![
        ("current".into(), Val::null()),
        (
            "reset".into(),
            Val::func(Func::CyclerReset {
                index: index.clone(),
            }),
        ),
        ("next".into(), Val::func(Func::CyclerNext { items, index })),
    ])
}
