//! Runs a compiled template, as the JavaScript Nunjucks generates for it
//! would.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use brrrty_js::{Loader, Value};

use crate::ast::Span;
use crate::compile::{E, Lit, Program, SetValue, Stmt};
use crate::context::{Context, CtxVars, Frame, Scope, TplEnv};
use crate::engine::Engine;
use crate::error::{ErrorKind, NunjucksError};
use crate::jsops::{self, BinOp, Ops};
use crate::value::{Func, Global, Obj, Val};

/// The most items a loop or `range()` may have.
pub(crate) const MAX_ITEMS: usize = 1_000_000;
/// The most templates (includes, imports, layouts) open at once.
pub(crate) const MAX_TEMPLATES: usize = 64;
/// The most macro calls open at once.
pub(crate) const MAX_CALLS: usize = 1000;

pub(crate) fn grow<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(256 * 1024, 2 * 1024 * 1024, f)
}

/// A render error. `Plain` errors take the position of the innermost node
/// they pass through.
#[derive(Debug)]
pub(crate) enum RenderErr {
    Plain {
        kind: ErrorKind,
        message: String,
        help: Option<String>,
    },
    Located(Box<NunjucksError>),
}

impl RenderErr {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> RenderErr {
        RenderErr::Plain {
            kind,
            message: message.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, text: impl Into<String>) -> RenderErr {
        if let RenderErr::Plain { help, .. } = &mut self {
            *help = Some(text.into());
        }
        self
    }

    pub fn locate(self, span: Option<Span>, template: &str) -> RenderErr {
        match self {
            RenderErr::Plain {
                kind,
                message,
                help,
            } => RenderErr::Located(Box::new(NunjucksError {
                kind,
                message,
                template: template.to_string(),
                line: span.map(|s| s.line),
                column: span.map(|s| s.column),
                help,
            })),
            located => located,
        }
    }

    pub fn into_error(self, template: &str) -> NunjucksError {
        match self.locate(None, template) {
            RenderErr::Located(e) => *e,
            RenderErr::Plain { .. } => unreachable!("located above"),
        }
    }
}

pub(crate) type R<T> = Result<T, RenderErr>;

/// A plain JavaScript `Error` with Nunjucks's message.
pub(crate) fn error(message: impl AsRef<str>) -> RenderErr {
    jsops::js_err("Error", message)
}

pub(crate) fn too_many(message: String) -> RenderErr {
    RenderErr::new(ErrorKind::Limit, message)
        .with_help("Brrrty stops loops and ranges so a mistake cannot use up all memory")
}

/// What a render made that may hold itself: macros close over the scope
/// that holds them, and arrays and objects can be made to contain
/// themselves. They are emptied when the render ends, so nothing leaks.
#[derive(Default)]
struct Gc {
    scopes: Vec<Weak<Scope>>,
    envs: Vec<Weak<TplEnv>>,
    arrays: Vec<Weak<RefCell<Vec<Val>>>>,
    objects: Vec<Weak<Obj>>,
    seen: HashSet<usize>,
}

thread_local! {
    static GC: RefCell<Vec<Gc>> = const { RefCell::new(Vec::new()) };
}

fn with_gc(f: impl FnOnce(&mut Gc)) {
    GC.with(|g| {
        if let Some(top) = g.borrow_mut().last_mut() {
            f(top);
        }
    });
}

/// Records that `v`, an array or object the template made, was changed.
pub(crate) fn note_mutation(v: &Val) {
    with_gc(|g| match v {
        Val::Arr(a) if g.seen.insert(Rc::as_ptr(a) as usize) => {
            g.arrays.push(Rc::downgrade(a));
        }
        Val::Obj(o) if g.seen.insert(Rc::as_ptr(o) as usize) => {
            g.objects.push(Rc::downgrade(o));
        }
        _ => {}
    });
}

fn note_scope(s: &Rc<Scope>) {
    with_gc(|g| {
        if g.seen.insert(Rc::as_ptr(s) as usize) {
            g.scopes.push(Rc::downgrade(s));
        }
    });
}

fn note_env(e: &Rc<TplEnv>) {
    with_gc(|g| g.envs.push(Rc::downgrade(e)));
}

impl Gc {
    fn sweep(self) {
        for e in self.envs.iter().filter_map(Weak::upgrade) {
            e.set_frame(Frame::new());
            *e.parent_template.borrow_mut() = None;
            e.context.vars.borrow_mut().own.clear();
            e.context.blocks.borrow_mut().clear();
        }
        for s in self.scopes.iter().filter_map(Weak::upgrade) {
            s.slots.borrow_mut().clear();
        }
        for a in self.arrays.iter().filter_map(Weak::upgrade) {
            a.borrow_mut().clear();
        }
        for o in self.objects.iter().filter_map(Weak::upgrade) {
            o.props.borrow_mut().clear();
        }
    }
}

/// A render's output, and the variables it read that were not defined, in
/// sorted order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub output: String,
    pub undefined: Vec<String>,
}

pub(crate) struct Renderer<'a> {
    pub engine: &'a Engine,
    pub loader: &'a dyn Loader,
    written: usize,
    pub templates: usize,
    pub calls: usize,
    /// Loaded templates by the name they were loaded under.
    pub cache: HashMap<String, Arc<Program>>,
    undefined: BTreeSet<String>,
}

impl<'a> Renderer<'a> {
    pub fn new(engine: &'a Engine, loader: &'a dyn Loader) -> Renderer<'a> {
        Renderer {
            engine,
            loader,
            written: 0,
            templates: 0,
            calls: 0,
            cache: HashMap::new(),
            undefined: BTreeSet::new(),
        }
    }

    /// `Template.render(data)`.
    pub fn render(
        mut self,
        program: &Arc<Program>,
        data: &Value,
    ) -> Result<Rendered, NunjucksError> {
        brrrty_js::traps::clear();
        GC.with(|g| g.borrow_mut().push(Gc::default()));
        let base = match data {
            Value::Object(_) => data.clone(),
            _ => Value::Object(Default::default()),
        };
        let result = self.render_program(program, CtxVars::new(base), None);
        if let Some(gc) = GC.with(|g| g.borrow_mut().pop()) {
            gc.sweep();
        }
        let traps = brrrty_js::traps::take();
        match result {
            Ok(output) => {
                let mut undefined = self.undefined;
                undefined.extend(traps);
                Ok(Rendered {
                    output,
                    undefined: undefined.into_iter().collect(),
                })
            }
            Err(e) => Err(e.into_error(&program.name)),
        }
    }

    pub fn ops(&self) -> Ops {
        Ops {
            tz: self.engine.options.timezone,
            limit: self.engine.output_limit,
        }
    }

    // ---- templates ----

    pub fn enter_template(&mut self, name: &str) -> R<()> {
        self.templates += 1;
        if self.templates > MAX_TEMPLATES {
            return Err(RenderErr::new(
                ErrorKind::Limit,
                format!("more than {MAX_TEMPLATES} templates are open at once, the last being \"{name}\""),
            )
            .with_help("look for an include, import or extends that leads back to itself"));
        }
        Ok(())
    }

    /// `Template.render(ctx, parentFrame)`.
    pub fn render_program(
        &mut self,
        program: &Arc<Program>,
        vars: CtxVars,
        parent: Option<Rc<Frame>>,
    ) -> R<String> {
        self.enter_template(&program.name)?;
        let context = Context::new(vars, program);
        let frame = match parent {
            Some(p) => p.push(true),
            None => Frame::new(),
        };
        frame.top_level.set(true);
        let out = self.run_root(program, context, frame);
        self.templates -= 1;
        out
    }

    /// `Template.getExported(ctx, parentFrame)`.
    pub fn exported(
        &mut self,
        program: &Arc<Program>,
        vars: Option<CtxVars>,
        parent: Option<Rc<Frame>>,
    ) -> R<Rc<Obj>> {
        self.enter_template(&program.name)?;
        let frame = match parent {
            Some(p) => p.push(false),
            None => Frame::new(),
        };
        frame.top_level.set(true);
        let context = Context::new(vars.unwrap_or_else(CtxVars::empty), program);
        let r = self.run_root(program, context.clone(), frame);
        self.templates -= 1;
        r?;
        Ok(context.exported())
    }

    /// `rootRenderFunc`, then each parent template's, for `extends`.
    fn run_root(
        &mut self,
        program: &Arc<Program>,
        context: Rc<Context>,
        frame: Rc<Frame>,
    ) -> R<String> {
        let mut program = program.clone();
        let mut frame = frame;
        let mut parents = 0;
        let result = loop {
            let tpl = Rc::new(TplEnv {
                program: program.clone(),
                context: context.clone(),
                frame: RefCell::new(frame),
                parent_template: RefCell::new(None),
            });
            note_env(&tpl);
            let scope = Scope::new(program.root.slots, None, tpl.clone());
            let mut out = String::new();
            if let Err(e) = self.exec(&program.root.stmts, &scope, &mut out) {
                break Err(e);
            }
            let parent = tpl.parent_template.borrow_mut().take();
            match parent {
                Some(p) => {
                    if let Err(e) = self.enter_template(&p.name) {
                        break Err(e);
                    }
                    parents += 1;
                    frame = tpl.frame();
                    program = p;
                }
                None => break Ok(out),
            }
        };
        self.templates -= parents;
        result
    }

    // ---- statements ----

    pub fn exec(&mut self, stmts: &[Stmt], scope: &Rc<Scope>, out: &mut String) -> R<()> {
        for s in stmts {
            grow(|| self.stmt(s, scope, out))
                .map_err(|e| e.locate(stmt_span(s), &scope.tpl.program.name))?;
        }
        Ok(())
    }

    /// Runs `stmts` into a new buffer.
    pub fn capture(&mut self, stmts: &[Stmt], scope: &Rc<Scope>) -> R<String> {
        let mut out = String::new();
        self.exec(stmts, scope, &mut out)?;
        Ok(out)
    }

    fn stmt(&mut self, s: &Stmt, scope: &Rc<Scope>, out: &mut String) -> R<()> {
        match s {
            Stmt::Text(t) => self.write(out, t),
            Stmt::Output { expr, .. } => {
                let v = self.eval(expr, scope)?;
                let s = jsops::suppress(&v, self.engine.options.autoescape, self.ops())?;
                self.write(out, &s)
            }
            Stmt::If {
                branches, else_, ..
            } => {
                for (cond, body) in branches {
                    if self.eval(cond, scope)?.truthy() {
                        return self.exec(body, scope, out);
                    }
                }
                match else_ {
                    Some(body) => self.exec(body, scope, out),
                    None => Ok(()),
                }
            }
            Stmt::For(f) => self.for_loop(f, scope, out),
            Stmt::Set { targets, value, .. } => {
                let v = match value {
                    SetValue::Expr(e) => self.eval(e, scope)?,
                    SetValue::Capture(body) => Val::str(self.capture(body, scope)?),
                };
                for (_, r) in targets {
                    scope.put(r.up, r.slot, v.clone());
                }
                let tpl = &scope.tpl;
                for (name, _) in targets {
                    let frame = tpl.frame();
                    frame.set(name, v.clone(), true)?;
                    if frame.top_level.get() {
                        tpl.context.set_variable(name, v.clone());
                        if !name.starts_with('_') {
                            tpl.context.add_export(name);
                        }
                    }
                }
                Ok(())
            }
            Stmt::Switch {
                expr,
                cases,
                default,
                ..
            } => self.switch(expr, cases, default.as_deref(), scope, out),
            Stmt::Block { name, in_block, .. } => {
                let tpl = &scope.tpl;
                if !*in_block && tpl.parent_template.borrow().is_some() {
                    return Ok(());
                }
                let first = tpl
                    .context
                    .blocks
                    .borrow()
                    .get(name)
                    .and_then(|l| l.first().cloned());
                let Some(block) = first else {
                    return Err(error(format!("unknown block \"{name}\"")));
                };
                let s = self.invoke_block(&block, &tpl.context, tpl.frame())?;
                self.write(out, &s)
            }
            Stmt::Extends { template, .. } => self.extends(template, scope),
            Stmt::Include {
                template,
                ignore_missing,
                ..
            } => self.include(template, *ignore_missing, scope, out),
            Stmt::Macro {
                name,
                def,
                slot,
                nested,
                ..
            } => {
                note_scope(scope);
                let f = Val::func(Func::Macro {
                    def: def.clone(),
                    scope: scope.clone(),
                });
                scope.put(0, *slot, f.clone());
                let tpl = &scope.tpl;
                if *nested {
                    tpl.frame().set(name, f, false)?;
                } else {
                    if !name.starts_with('_') {
                        tpl.context.add_export(name);
                    }
                    tpl.context.set_variable(name, f);
                }
                Ok(())
            }
            Stmt::Import {
                template,
                with_context,
                target,
                slot,
                nested,
                ..
            } => {
                let ns = Val::Obj(self.import(template, *with_context, scope)?);
                scope.put(0, *slot, ns.clone());
                self.bind_import(scope, *nested, target, ns)
            }
            Stmt::FromImport {
                template,
                with_context,
                names,
                nested,
                ..
            } => {
                let ns = self.import(template, *with_context, scope)?;
                for n in names {
                    let Some(v) = ns.get(&n.name) else {
                        return Err(error(format!("cannot import '{}'", n.name)));
                    };
                    scope.put(0, n.slot, v.clone());
                    self.bind_import(scope, *nested, &n.alias, v)?;
                }
                Ok(())
            }
        }
    }

    fn bind_import(&mut self, scope: &Rc<Scope>, nested: bool, name: &str, v: Val) -> R<()> {
        if nested {
            scope.tpl.frame().set(name, v, false)
        } else {
            scope.tpl.context.set_variable(name, v);
            Ok(())
        }
    }

    fn switch(
        &mut self,
        expr: &E,
        cases: &[(E, Vec<Stmt>, bool)],
        default: Option<&[Stmt]>,
        scope: &Rc<Scope>,
        out: &mut String,
    ) -> R<()> {
        let v = self.eval(expr, scope)?;
        let mut start = None;
        for (i, (cond, _, _)) in cases.iter().enumerate() {
            let c = self.eval(cond, scope)?;
            if jsops::strict_eq(&v, &c)? {
                start = Some(i);
                break;
            }
        }
        if let Some(i) = start {
            for (_, body, breaks) in &cases[i..] {
                self.exec(body, scope, out)?;
                if *breaks {
                    return Ok(());
                }
            }
        }
        if let Some(d) = default {
            self.exec(d, scope, out)?;
        }
        Ok(())
    }

    /// Appends `s`, keeping the whole render under the output limit.
    pub fn write(&mut self, out: &mut String, s: &str) -> R<()> {
        let limit = self.engine.output_limit;
        if self.written + s.len() > limit {
            return Err(RenderErr::new(
                ErrorKind::Limit,
                format!("the render wrote more than {} MiB", limit / (1024 * 1024)),
            )
            .with_help("look for a loop, macro or capture that keeps doubling its output"));
        }
        self.written += s.len();
        out.push_str(s);
        Ok(())
    }

    // ---- expressions ----

    pub fn eval(&mut self, e: &E, scope: &Rc<Scope>) -> R<Val> {
        grow(|| self.eval_inner(e, scope))
    }

    fn eval_inner(&mut self, e: &E, scope: &Rc<Scope>) -> R<Val> {
        let ops = self.ops();
        let at = |span: &Span| {
            let name = scope.tpl.program.name.clone();
            let span = *span;
            move |e: RenderErr| e.locate(Some(span), &name)
        };
        Ok(match e {
            E::Lit(l) => match l {
                Lit::Undefined => Val::undef(),
                Lit::Null => Val::null(),
                Lit::Bool(b) => Val::bool(*b),
                Lit::Num(n) => Val::num(*n),
                Lit::Str(s) => Val::str(s.clone()),
            },
            E::Var(r) => scope.get(r.up, r.slot),
            E::Lookup(name) => self.lookup(name, scope)?,
            E::OutOfScope(name) => {
                // Nunjucks fails with `ReferenceError: t_N is not defined`,
                // naming a variable of its generated code.
                return Err(jsops::unsupported(format!(
                    "ReferenceError: `{name}` is out of scope here"
                ))
                .with_help("Nunjucks keeps a name from `{% import %}` or `{% from %}`, or a macro defined after an include, only inside the branch that declared it"));
            }
            E::Comma(items) => {
                let mut last = Val::undef();
                for i in items {
                    last = self.eval(i, scope)?;
                }
                last
            }
            E::Array(items) => {
                let mut v = Vec::with_capacity(items.len());
                for i in items {
                    v.push(self.eval(i, scope)?);
                }
                Val::arr(v)
            }
            E::Dict(pairs) => {
                let mut v = Vec::with_capacity(pairs.len());
                for (k, x) in pairs {
                    v.push((k.clone(), self.eval(x, scope)?));
                }
                jsops::new_object(v)
            }
            E::Member { span, target, key } => {
                let t = self.eval(target, scope)?;
                let k = self.eval(key, scope)?;
                jsops::member(&t, &k, ops).map_err(at(span))?
            }
            E::Call {
                span,
                callee,
                name,
                args,
                kwargs,
            } => {
                let f = self.eval(callee, scope)?;
                let argv = self.eval_args(args, kwargs.as_deref(), scope)?;
                self.call_wrap(&f, name, argv).map_err(at(span))?
            }
            E::Filter {
                span,
                name,
                args,
                kwargs,
            } => self
                .filter(name, args, kwargs.as_deref(), scope)
                .map_err(at(span))?,
            E::Test {
                span,
                name,
                operand,
                arg,
            } => {
                let test = crate::tests::lookup(name).map_err(at(span))?;
                let v = self.eval(operand, scope)?;
                let a = match arg {
                    Some(a) => Some(self.eval(a, scope)?),
                    None => None,
                };
                crate::tests::run(test, &v, a.as_ref(), ops).map_err(at(span))?
            }
            E::In { span, lhs, rhs } => {
                let l = self.eval(lhs, scope)?;
                let r = self.eval(rhs, scope)?;
                Val::bool(jsops::in_operator(&l, &r, ops).map_err(at(span))?)
            }
            E::Floor(inner) => {
                let v = self.eval(inner, scope)?;
                Val::num(jsops::to_number(&v, ops)?.floor())
            }
            E::Pow(a, b) => {
                let x = self.eval(a, scope)?;
                let y = self.eval(b, scope)?;
                let x = jsops::to_number(&x, ops)?;
                let y = jsops::to_number(&y, ops)?;
                Val::num(jsops::pow(x, y))
            }
            E::Cond { cond, then, else_ } => {
                if self.eval(cond, scope)?.truthy() {
                    self.eval(then, scope)?
                } else {
                    match else_ {
                        Some(e) => self.eval(e, scope)?,
                        None => Val::str(""),
                    }
                }
            }
            E::Caller(def) => {
                note_scope(scope);
                Val::func(Func::Macro {
                    def: def.clone(),
                    scope: scope.clone(),
                })
            }
            E::Capture(body) => Val::str(self.capture(body, scope)?),
            E::Not(x) => Val::bool(!self.eval(x, scope)?.truthy()),
            E::Neg(x) => {
                let v = self.eval(x, scope)?;
                Val::num(-jsops::to_number(&v, ops)?)
            }
            E::Pos(x) => {
                let v = self.eval(x, scope)?;
                Val::num(jsops::to_number(&v, ops)?)
            }
            E::Chain { first, rest } => {
                let mut acc = self.eval(first, scope)?;
                for (op, x) in rest {
                    let r = self.eval(x, scope)?;
                    acc = binop(*op, &acc, &r, ops)?;
                }
                acc
            }
            E::And(items) => {
                let mut v = Val::undef();
                for (i, x) in items.iter().enumerate() {
                    v = self.eval(x, scope)?;
                    if i + 1 < items.len() && !v.truthy() {
                        break;
                    }
                }
                v
            }
            E::Or(items) => {
                let mut v = Val::undef();
                for (i, x) in items.iter().enumerate() {
                    v = self.eval(x, scope)?;
                    if i + 1 < items.len() && v.truthy() {
                        break;
                    }
                }
                v
            }
        })
    }

    /// Positional arguments, then the keyword arguments as one object, as
    /// `runtime.makeKeywordArgs` builds it.
    pub fn eval_args(
        &mut self,
        args: &[E],
        kwargs: Option<&[(String, E)]>,
        scope: &Rc<Scope>,
    ) -> R<Vec<Val>> {
        let mut out = Vec::with_capacity(args.len() + 1);
        for a in args {
            out.push(self.eval(a, scope)?);
        }
        if let Some(kw) = kwargs {
            let mut pairs = Vec::with_capacity(kw.len() + 1);
            for (k, e) in kw {
                pairs.push((k.clone(), self.eval(e, scope)?));
            }
            pairs.push(("__keywords".to_string(), Val::bool(true)));
            out.push(jsops::new_object(pairs));
        }
        Ok(out)
    }

    /// `runtime.contextOrFrameLookup(context, frame, name)`.
    pub fn lookup(&mut self, name: &str, scope: &Rc<Scope>) -> R<Val> {
        let tpl = &scope.tpl;
        let v = tpl.frame().lookup(name);
        if !v.is_undefined() {
            return Ok(v);
        }
        let vars = tpl.context.vars.borrow();
        let proto = jsops::OBJECT_PROTO.contains(&name) || name == "__proto__";
        let in_ctx = vars.has_own(name) || proto;
        if !in_ctx && let Some(g) = self.global(name) {
            return Ok(g);
        }
        match vars.get_own(name) {
            Some(v) => {
                if v.is_undefined() {
                    self.undefined.insert(name.to_string());
                }
                Ok(v)
            }
            None if proto => Err(jsops::unsupported(format!(
                "`{name}` is not defined; in Nunjucks it names a built-in JavaScript function"
            ))
            .with_help("define the variable in the data, or rename it")),
            None => {
                self.undefined.insert(name.to_string());
                Ok(Val::undef())
            }
        }
    }

    fn global(&self, name: &str) -> Option<Val> {
        let g = match name {
            "range" => Global::Range,
            "cycler" => Global::Cycler,
            "joiner" => Global::Joiner,
            _ => return self.engine.globals.get(name).cloned().map(Val::V),
        };
        if let Some(v) = self.engine.globals.get(name) {
            return Some(Val::V(v.clone()));
        }
        Some(Val::func(Func::Global(g)))
    }
}

fn binop(op: BinOp, a: &Val, b: &Val, ops: Ops) -> R<Val> {
    jsops::binary(op, a, b, ops)
}

fn stmt_span(s: &Stmt) -> Option<Span> {
    Some(match s {
        Stmt::Text(_) => return None,
        Stmt::Output { span, .. }
        | Stmt::If { span, .. }
        | Stmt::Set { span, .. }
        | Stmt::Block { span, .. }
        | Stmt::Extends { span, .. }
        | Stmt::Include { span, .. }
        | Stmt::Macro { span, .. }
        | Stmt::Import { span, .. }
        | Stmt::FromImport { span, .. }
        | Stmt::Switch { span, .. } => *span,
        Stmt::For(f) => f.span,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use brrrty_js::{LoadError, Loader, Tz, Value};

    use crate::engine::{Engine, EngineOptions};
    use crate::error::ErrorKind;

    struct Map(BTreeMap<String, String>);

    impl Loader for Map {
        fn load(&self, name: &str) -> Result<(String, String), LoadError> {
            self.0
                .get(name)
                .map(|s| (name.to_string(), s.clone()))
                .ok_or_else(Default::default)
        }
    }

    fn engine() -> Engine {
        Engine::new(EngineOptions {
            timezone: Tz::Utc,
            autoescape: true,
        })
    }

    fn render(
        e: &Engine,
        src: &str,
        includes: &[(&str, &str)],
    ) -> Result<String, crate::NunjucksError> {
        let map = Map(includes
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect());
        let t = e.compile("t", src)?;
        e.render(&t, &Value::Object(Default::default()), &map)
    }

    fn kind(r: Result<String, crate::NunjucksError>) -> ErrorKind {
        r.expect_err("an error").kind
    }

    #[test]
    fn output_over_the_limit_is_an_error() {
        let mut e = engine();
        e.output_limit = 1000;
        assert_eq!(render(&e, &"x".repeat(1000), &[]).unwrap().len(), 1000);
        assert_eq!(kind(render(&e, &"x".repeat(1001), &[])), ErrorKind::Limit);
        // Captures and macros count too: these double a string without
        // printing it.
        let doubling = "{% set s = 'x' %}{% for i in range(64) %}{% set s %}{{ s }}{{ s }}{% endset %}{% endfor %}";
        assert_eq!(kind(render(&e, doubling, &[])), ErrorKind::Limit);
        let concat = "{% set s = 'x' %}{% for i in range(64) %}{% set s = s ~ s %}{% endfor %}";
        assert_eq!(kind(render(&e, concat, &[])), ErrorKind::Limit);
        let repeat = "{{ 'x'.repeat(5000) }}";
        assert_eq!(kind(render(&e, repeat, &[])), ErrorKind::Limit);
        let pushing =
            "{% set a = [1] %}{% for i in range(64) %}{% set a = a.concat(a) %}{% endfor %}";
        assert_eq!(kind(render(&e, pushing, &[])), ErrorKind::Limit);
    }

    #[test]
    fn the_real_limit_is_512_mib() {
        assert_eq!(engine().output_limit, 512 * 1024 * 1024);
    }

    #[test]
    fn loops_and_ranges_stop() {
        let mut e = engine();
        assert_eq!(e.item_limit, 1_000_000);
        e.item_limit = 1000;
        assert_eq!(render(&e, "{{ range(1000) }}", &[]).unwrap().len(), 3889);
        assert_eq!(kind(render(&e, "{{ range(1001) }}", &[])), ErrorKind::Limit);
        assert_eq!(
            kind(render(&e, "{{ range(0, 1, 0.0000000001) }}", &[])),
            ErrorKind::Limit
        );
        // A loop that pushes onto what it walks never ends in Nunjucks.
        let forever = "{% set a = [1] %}{% for x in a %}{{ a.push(x) and '' }}{% endfor %}";
        assert_eq!(kind(render(&e, forever, &[])), ErrorKind::Limit);
    }

    #[test]
    fn recursion_stops() {
        small_stack(recursion);
    }

    fn recursion() {
        let e = engine();
        let m = "{% macro m(n) %}{{ m(n + 1) }}{% endmacro %}{{ m(0) }}";
        assert_eq!(kind(render(&e, m, &[])), ErrorKind::Limit);
        let caller =
            "{% macro m() %}{{ caller() }}{% endmacro %}{% call m() %}{{ caller() }}{% endcall %}";
        assert_eq!(kind(render(&e, caller, &[])), ErrorKind::Limit);
        let inc = render(
            &e,
            "{% include 'a' %}",
            &[("a", "{% include 'b' %}"), ("b", "{% include 'a' %}")],
        );
        assert_eq!(kind(inc), ErrorKind::Limit);
        let ext = render(
            &e,
            "{% extends 'a' %}",
            &[("a", "{% extends 'b' %}"), ("b", "{% extends 'a' %}")],
        );
        assert_eq!(kind(ext), ErrorKind::Limit);
        let imp = render(
            &e,
            "{% import 'a' as x %}",
            &[("a", "{% import 'a' as y %}")],
        );
        assert_eq!(kind(imp), ErrorKind::Limit);
        // A legitimate recursion well inside the limit works.
        let ok = "{% macro m(n) %}{% if n > 0 %}{{ m(n - 1) }}{% endif %}{{ n % 10 }}{% endmacro %}{{ m(500) }}";
        assert_eq!(render(&e, ok, &[]).unwrap().len(), 501);
    }

    /// Runs `f` on a thread with a small stack, as a build's workers have.
    fn small_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn long_chains_and_deep_nesting_do_not_overflow() {
        small_stack(|| {
            let e = engine();
            let n = 100_000;
            // A comparison chain is flat, so its length costs no stack.
            // Nunjucks fails near 5,600 comparisons; Brrrty runs 4,096 and
            // refuses longer ones. node: a chain of 4,097 ones joined by
            // " < " is true, and by " == " true.
            let chain = format!("{{{{ {} }}}}", vec!["1"; 4097].join(" < "));
            assert_eq!(render(&e, &chain, &[]).unwrap(), "true");
            let odd = format!("{{{{ {} }}}}", vec!["1"; 4096].join(" < "));
            assert_eq!(render(&e, &odd, &[]).unwrap(), "false");
            let eq = format!("{{{{ {} }}}}", vec!["1"; 4097].join(" == "));
            assert_eq!(render(&e, &eq, &[]).unwrap(), "true");
            let long = format!("{{{{ {} }}}}", vec!["1"; n].join(" < "));
            assert_eq!(kind(render(&e, &long, &[])), ErrorKind::Limit);
            // As deep as the parser allows, in every kind of nesting.
            let d = 250;
            let shapes = [
                format!("{{{{ {}1{} }}}}", "(".repeat(d), ")".repeat(d)),
                format!("{{{{ {}1{} }}}}", "[".repeat(d), "]".repeat(d)),
                format!("{{{{ {}1 }}}}", "not ".repeat(d)),
                format!("{{{{ 1{} }}}}", " + 1".repeat(d)),
                format!("{{{{ {}1{} }}}}", "{a: ".repeat(d), "}".repeat(d)),
                format!("{}x{}", "{% if true %}".repeat(d), "{% endif %}".repeat(d)),
                format!(
                    "{}x{}",
                    "{% for i in [1] %}".repeat(d),
                    "{% endfor %}".repeat(d)
                ),
                format!(
                    "{}x{}",
                    "{% macro m() %}".repeat(d),
                    "{% endmacro %}".repeat(d)
                ),
                format!("{}x{}", "{% set c %}".repeat(d), "{% endset %}".repeat(d)),
            ];
            for s in &shapes {
                let _ = render(&e, s, &[]);
            }
        });
    }

    #[test]
    fn a_render_frees_what_holds_itself() {
        use crate::context::LIVE_SCOPES;
        let e = engine();
        let before = LIVE_SCOPES.with(|n| n.get());
        let src = "{% macro m(x) %}{{ x }}{% endmacro %}{% for i in [1] %}{% macro n() %}{{ m(i) }}{% endmacro %}{{ n() }}{% endfor %}\
                   {% set a = [] %}{{ a.push(a) }}{% set o = cycler(a) %}{{ a.push(o) }}{{ o.next() and '' }}\
                   {% call m() %}{% macro k() %}{% endmacro %}{% endcall %}{% import 'i' as ns %}{{ ns.f() }}";
        let out = render(&e, src, &[("i", "{% macro f() %}F{% endmacro %}")]).unwrap();
        // node: a macro made in a loop can't see the loop variable.
        assert_eq!(out, "12F");
        assert_eq!(LIVE_SCOPES.with(|n| n.get()), before);
        // And after an error part way through.
        let _ = render(&e, "{% macro m() %}{{ m }}{% endmacro %}{{ m() }}", &[]);
        assert_eq!(LIVE_SCOPES.with(|n| n.get()), before);
    }

    #[test]
    fn tracked_renders_list_the_undefined_variables() {
        let e = engine();
        let t = e
            .compile("t", "{{ a }}{{ b.c }}{% set d = 1 %}{{ d }}{% for x in [1] %}{{ x }}{{ z }}{% endfor %}{{ range(1) }}{{ m }}")
            .unwrap();
        let data = Value::from_json(&serde_json::json!({"b": {}, "m": null}));
        let r = e.render_tracked(&t, &data, &Map(BTreeMap::new())).unwrap();
        assert_eq!(r.output, "110");
        assert_eq!(r.undefined, ["a", "z"]);
    }

    #[test]
    fn hostile_templates_do_not_panic() {
        let e = engine();
        let pieces = [
            "{{", "}}", "{%", "%}", "a", "1", "(", ")", "[", "]", "{", "}", "|", "safe", "~", "+",
            "-", "not", "in", "is", "if", "else", "for", "x", "endfor", "endif", "set", "=",
            "macro", "endmacro", "call", "endcall", "caller", "(", ",", "\"s\"", "block",
            "endblock", "super", "include", "import", "as", "from", "extends", "loop", ".",
            "length", "range", "<", "==", "**", "//", "%", "r/x/", "switch", "case",
        ];
        let mut seed: u64 = 7;
        for _ in 0..20_000 {
            let mut s = String::new();
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let n = (seed >> 33) % 24;
            for _ in 0..n {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s.push_str(pieces[((seed >> 33) % pieces.len() as u64) as usize]);
                s.push(' ');
            }
            let _ = render(&e, &s, &[("x", "{{ x }}"), ("\"s\"", "s")]);
        }
    }
}
