//! Nunjucks's runtime scopes: `Frame` (runtime.js), `Context`
//! (environment.js), and the JavaScript function scopes a compiled
//! template's code runs in.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::Value;

use crate::compile::Program;
use crate::error::ErrorKind;
use crate::render::{R, RenderErr};
use crate::value::{Obj, Val};

/// Nunjucks's `Frame`: one level of `{% set %}`, loop and macro variables.
pub(crate) struct Frame {
    vars: RefCell<HashMap<String, Val>>,
    pub parent: Option<Rc<Frame>>,
    pub top_level: Cell<bool>,
    /// Writes (`set`) never go past this frame to its parent.
    isolate_writes: bool,
}

impl Frame {
    pub fn new() -> Rc<Frame> {
        Rc::new(Frame {
            vars: RefCell::new(HashMap::new()),
            parent: None,
            top_level: Cell::new(false),
            isolate_writes: false,
        })
    }

    pub fn push(self: &Rc<Frame>, isolate_writes: bool) -> Rc<Frame> {
        Rc::new(Frame {
            vars: RefCell::new(HashMap::new()),
            parent: Some(self.clone()),
            top_level: Cell::new(false),
            isolate_writes,
        })
    }

    /// `frame.pop()`: the parent. Nunjucks never pops the outermost frame.
    pub fn pop(self: &Rc<Frame>) -> Rc<Frame> {
        self.parent.clone().unwrap_or_else(Frame::new)
    }

    /// `frame.lookup(name)`: the innermost frame whose value for `name` is
    /// not `undefined`.
    pub fn lookup(&self, name: &str) -> Val {
        let mut f = Some(self);
        while let Some(frame) = f {
            if let Some(v) = frame.vars.borrow().get(name)
                && !v.is_undefined()
            {
                return v.clone();
            }
            f = frame.parent.as_deref();
        }
        Val::undef()
    }

    /// `frame.resolve(name, forWrite)`. Only this frame's `isolate_writes`
    /// counts, as in Nunjucks.
    fn resolve(self: &Rc<Frame>, name: &str, for_write: bool) -> Option<Rc<Frame>> {
        let has = |f: &Frame| f.vars.borrow().get(name).is_some_and(|v| !v.is_undefined());
        if has(self) {
            return Some(self.clone());
        }
        if for_write && self.isolate_writes {
            return None;
        }
        let mut f = self.parent.clone();
        while let Some(frame) = f {
            if has(&frame) {
                return Some(frame);
            }
            f = frame.parent.clone();
        }
        None
    }

    /// `frame.set(name, val, resolveUp)`. A dotted name (`loop.index`)
    /// makes the objects on the way, as Nunjucks does.
    pub fn set(self: &Rc<Frame>, name: &str, val: Val, resolve_up: bool) -> R<()> {
        let parts: Vec<&str> = name.split('.').collect();
        if resolve_up && let Some(frame) = self.resolve(parts[0], true) {
            return frame.set(name, val, false);
        }
        if parts.len() == 1 {
            self.vars.borrow_mut().insert(name.to_string(), val);
            return Ok(());
        }
        // `if (!obj[id]) obj[id] = {}` for the first part, in this frame's
        // variables, then on through the objects.
        let first = parts[0];
        let mut obj = {
            let mut vars = self.vars.borrow_mut();
            let cur = vars.get(first).cloned().unwrap_or_else(Val::undef);
            if cur.truthy() {
                cur
            } else {
                let o = Val::Obj(Rc::new(Obj::default()));
                vars.insert(first.to_string(), o.clone());
                o
            }
        };
        for (i, part) in parts.iter().enumerate().skip(1) {
            let last = i == parts.len() - 1;
            let target = match &obj {
                Val::Obj(o) => o.clone(),
                other => return Err(cannot_set(other, part)),
            };
            if last {
                target.set(part, val);
                crate::render::note_mutation(&obj);
                return Ok(());
            }
            let cur = target.get(part).unwrap_or_else(Val::undef);
            obj = if cur.truthy() {
                cur
            } else {
                let o = Val::Obj(Rc::new(Obj::default()));
                target.set(part, o.clone());
                o
            };
        }
        Ok(())
    }
}

/// Setting property `prop` on something that isn't an object the template
/// made: JavaScript's strict-mode error for a primitive, a refusal for data
/// and the rest.
fn cannot_set(on: &Val, prop: &str) -> RenderErr {
    let shown = match on {
        Val::V(Value::Number(n)) => Some(("number", brrrty_js::js_number_to_string(*n))),
        Val::V(Value::String(s)) => Some(("string", s.to_string())),
        Val::V(Value::Bool(b)) => Some(("boolean", b.to_string())),
        _ => None,
    };
    match shown {
        Some((kind, text)) => RenderErr::new(
            ErrorKind::Render,
            format!("TypeError: Cannot create property '{prop}' on {kind} '{text}'"),
        ),
        None => RenderErr::new(
            ErrorKind::Unsupported,
            format!(
                "setting `{prop}` here would change a value that isn't a plain object the template made"
            ),
        )
        .with_help(
            "Nunjucks would add the property to that value (for a loop variable named `loop`, to each item); Brrrty doesn't change data, so rename the variable",
        ),
    }
}

/// A context's variables: the data, and what the template set on top.
#[derive(Clone)]
pub(crate) struct CtxVars {
    pub base: Value,
    pub own: HashMap<String, Val>,
}

impl CtxVars {
    pub fn new(base: Value) -> CtxVars {
        CtxVars {
            base,
            own: HashMap::new(),
        }
    }

    pub fn empty() -> CtxVars {
        CtxVars::new(Value::Object(Default::default()))
    }

    /// An own property: set by the template, or in the data.
    pub fn get_own(&self, name: &str) -> Option<Val> {
        if let Some(v) = self.own.get(name) {
            return Some(v.clone());
        }
        match &self.base {
            Value::Object(o) => {
                brrrty_js::traps::read_top(&self.base, name, o.get(name).cloned()).map(Val::V)
            }
            _ => None,
        }
    }

    pub fn has_own(&self, name: &str) -> bool {
        self.own.contains_key(name)
            || matches!(&self.base, Value::Object(o) if o.contains_key(name))
    }
}

/// Which template's block function, by its index in that template.
#[derive(Clone)]
pub(crate) struct BlockRef {
    pub program: Arc<Program>,
    pub index: usize,
}

impl BlockRef {
    pub fn is(&self, other: &BlockRef) -> bool {
        Arc::ptr_eq(&self.program, &other.program) && self.index == other.index
    }
}

/// Nunjucks's `Context`.
pub(crate) struct Context {
    pub vars: RefCell<CtxVars>,
    pub blocks: RefCell<HashMap<String, Vec<BlockRef>>>,
    pub exported: RefCell<Vec<String>>,
}

impl Context {
    /// `new Context(ctx, blocks)`: a copy of `vars` and the template's own
    /// blocks.
    pub fn new(vars: CtxVars, program: &Arc<Program>) -> Rc<Context> {
        let mut blocks = HashMap::new();
        for (index, b) in program.blocks.iter().enumerate() {
            blocks.insert(
                b.name.clone(),
                vec![BlockRef {
                    program: program.clone(),
                    index,
                }],
            );
        }
        Rc::new(Context {
            vars: RefCell::new(vars),
            blocks: RefCell::new(blocks),
            exported: RefCell::new(Vec::new()),
        })
    }

    pub fn set_variable(&self, name: &str, v: Val) {
        self.vars.borrow_mut().own.insert(name.to_string(), v);
    }

    pub fn add_export(&self, name: &str) {
        self.exported.borrow_mut().push(name.to_string());
    }

    pub fn add_block(&self, name: &str, b: BlockRef) {
        self.blocks
            .borrow_mut()
            .entry(name.to_string())
            .or_default()
            .push(b);
    }

    /// `getExported()`: each exported name with its value in the context.
    pub fn exported(&self) -> Rc<Obj> {
        let vars = self.vars.borrow();
        let obj = Obj::default();
        for name in self.exported.borrow().iter() {
            obj.set(name, vars.get_own(name).unwrap_or_else(Val::undef));
        }
        Rc::new(obj)
    }
}

/// The state of one run of a template's root or block function, which the
/// macros and callers defined inside it share: `frame` and `context` are
/// that function's variables, which a macro reassigns while it runs.
pub(crate) struct TplEnv {
    pub program: Arc<Program>,
    pub context: Rc<Context>,
    pub frame: RefCell<Rc<Frame>>,
    /// The root function's `parentTemplate`.
    pub parent_template: RefCell<Option<Arc<Program>>>,
}

impl TplEnv {
    pub fn frame(&self) -> Rc<Frame> {
        self.frame.borrow().clone()
    }

    pub fn set_frame(&self, f: Rc<Frame>) {
        *self.frame.borrow_mut() = f;
    }
}

/// One run of a JavaScript function in the compiled template (the root, a
/// block, a macro or a caller): its `var`s, and the scope it closes over.
pub(crate) struct Scope {
    pub slots: RefCell<Vec<Val>>,
    pub parent: Option<Rc<Scope>>,
    pub tpl: Rc<TplEnv>,
}

#[cfg(test)]
thread_local! {
    /// Scopes alive on this thread, for the leak test.
    pub(crate) static LIVE_SCOPES: Cell<isize> = const { Cell::new(0) };
}

#[cfg(test)]
impl Drop for Scope {
    fn drop(&mut self) {
        LIVE_SCOPES.with(|n| n.set(n.get() - 1));
    }
}

impl Scope {
    pub fn new(slots: usize, parent: Option<Rc<Scope>>, tpl: Rc<TplEnv>) -> Rc<Scope> {
        #[cfg(test)]
        LIVE_SCOPES.with(|n| n.set(n.get() + 1));
        Rc::new(Scope {
            slots: RefCell::new(vec![Val::undef(); slots]),
            parent,
            tpl,
        })
    }

    fn up(self: &Rc<Scope>, up: usize) -> Rc<Scope> {
        let mut s = self.clone();
        for _ in 0..up {
            s = s
                .parent
                .clone()
                .expect("a variable's scope encloses its use");
        }
        s
    }

    pub fn get(self: &Rc<Scope>, up: usize, slot: usize) -> Val {
        self.up(up).slots.borrow()[slot].clone()
    }

    pub fn put(self: &Rc<Scope>, up: usize, slot: usize, v: Val) {
        self.up(up).slots.borrow_mut()[slot] = v;
    }
}
