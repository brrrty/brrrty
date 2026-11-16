//! Compiles a parsed template the way Nunjucks's `compiler.js` does, into
//! a tree the renderer runs.
//!
//! Nunjucks turns a template into JavaScript source and runs that. Three
//! things about that JavaScript decide what a template prints, and this
//! module reproduces them:
//!
//! - **Which names are JavaScript variables.** The compiler keeps its own
//!   frames: a loop variable, a macro argument, a macro defined inside a
//!   loop or an `{% import %}` target becomes a JavaScript variable that
//!   later code reads directly; every other name is looked up at run time
//!   in the runtime frames and the context. [`Binding`]s mirror that, and a
//!   name with a binding compiles to [`E::Var`].
//! - **JavaScript scoping.** Includes, imports, blocks and `extends` put
//!   the rest of their branch inside a callback, so a variable declared
//!   there is out of scope after the branch, and Nunjucks fails with a
//!   `ReferenceError`. A callback that would swallow a `switch` case or a
//!   loop's `else` is a JavaScript syntax error.
//! - **JavaScript precedence.** Operators are written out without
//!   parentheses, so `not a == b` runs as `(!a) == b` and `a * b % c` as
//!   `(a * b) % c`. Each expression is written out as tokens and re-read
//!   with JavaScript's precedence. A run of operators at one precedence
//!   becomes one flat [`E::Chain`], so a long comparison chain is never a
//!   deep tree.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{self, BinaryOp, CompareOp, Expr, Literal, Node, Span, UnaryOp};
use crate::error::{ErrorKind, Fail};
use crate::jsops::BinOp;

type CRes<T> = Result<T, Fail>;

/// A compiled template.
pub(crate) struct Program {
    /// The name errors show.
    pub name: String,
    /// Where `./` and `../` names resolve from.
    pub path: Option<String>,
    pub root: FnBody,
    pub blocks: Vec<BlockDef>,
}

pub(crate) struct FnBody {
    pub stmts: Vec<Stmt>,
    pub slots: usize,
}

pub(crate) struct BlockDef {
    pub name: String,
    pub body: FnBody,
    /// Where `super()`'s result goes, when the block uses it.
    pub super_slot: Option<usize>,
}

pub(crate) struct MacroDef {
    /// Positional arguments and their variable slots.
    pub args: Vec<(String, usize)>,
    /// Keyword arguments and their defaults.
    pub kwargs: Vec<(String, E)>,
    pub body: Vec<Stmt>,
    pub slots: usize,
    /// A `caller`: it keeps the frame it is called from.
    pub keep_frame: bool,
}

/// A JavaScript variable: `up` function scopes out, at `slot`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VarRef {
    pub up: usize,
    pub slot: usize,
}

pub(crate) enum Stmt {
    Text(String),
    Output {
        span: Span,
        expr: E,
    },
    If {
        span: Span,
        branches: Vec<(E, Vec<Stmt>)>,
        else_: Option<Vec<Stmt>>,
    },
    For(Box<ForStmt>),
    Set {
        span: Span,
        targets: Vec<(String, VarRef)>,
        value: SetValue,
    },
    Block {
        span: Span,
        name: String,
        in_block: bool,
    },
    Extends {
        span: Span,
        template: E,
    },
    Include {
        span: Span,
        template: E,
        ignore_missing: bool,
    },
    Macro {
        span: Span,
        name: String,
        def: Arc<MacroDef>,
        slot: usize,
        nested: bool,
    },
    Import {
        span: Span,
        template: E,
        with_context: bool,
        target: String,
        slot: usize,
        nested: bool,
    },
    FromImport {
        span: Span,
        template: E,
        with_context: bool,
        names: Vec<FromName>,
        nested: bool,
    },
    Switch {
        span: Span,
        expr: E,
        /// Each case's test, body, and whether a `break` follows it
        /// (Nunjucks writes one after a body that isn't empty).
        cases: Vec<(E, Vec<Stmt>, bool)>,
        default: Option<Vec<Stmt>>,
    },
}

pub(crate) struct FromName {
    pub name: String,
    pub alias: String,
    pub slot: usize,
}

pub(crate) enum SetValue {
    Expr(E),
    Capture(Vec<Stmt>),
}

pub(crate) struct ForStmt {
    pub span: Span,
    pub iter: E,
    pub targets: ForTargets,
    /// The loop's `len` variable, which keeps its value between runs of
    /// the loop in one function call.
    pub len_slot: usize,
    pub body: Vec<Stmt>,
    pub else_: Option<Vec<Stmt>>,
}

pub(crate) enum ForTargets {
    One {
        name: String,
        slot: usize,
    },
    /// `for a, b in ...`. Over an array, each item is unpacked into the
    /// slots; over an object, the first two are the key and the value.
    Many {
        names: Vec<String>,
        slots: Vec<usize>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum Lit {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Arc<str>),
}

/// An expression as the compiled JavaScript evaluates it.
pub(crate) enum E {
    Lit(Lit),
    Var(VarRef),
    /// `runtime.contextOrFrameLookup(context, frame, name)`.
    Lookup(String),
    /// A JavaScript variable read where it is out of scope.
    OutOfScope(String),
    /// `(a, b)`.
    Comma(Vec<E>),
    Array(Vec<E>),
    Dict(Vec<(String, E)>),
    Member {
        span: Span,
        target: Box<E>,
        key: Box<E>,
    },
    Call {
        span: Span,
        callee: Box<E>,
        /// How Nunjucks names the callee in errors.
        name: String,
        args: Vec<E>,
        kwargs: Option<Vec<(String, E)>>,
    },
    Filter {
        span: Span,
        name: String,
        args: Vec<E>,
        kwargs: Option<Vec<(String, E)>>,
    },
    /// `env.getTest(name).call(context, operand, arg)`.
    Test {
        span: Span,
        name: String,
        operand: Box<E>,
        arg: Option<Box<E>>,
    },
    In {
        span: Span,
        lhs: Box<E>,
        rhs: Box<E>,
    },
    /// `Math.floor(e)`.
    Floor(Box<E>),
    /// `Math.pow(a, b)`.
    Pow(Box<E>, Box<E>),
    Cond {
        cond: Box<E>,
        then: Box<E>,
        else_: Option<Box<E>>,
    },
    Caller(Arc<MacroDef>),
    Capture(Vec<Stmt>),
    Not(Box<E>),
    Neg(Box<E>),
    Pos(Box<E>),
    /// Left-associative operators of one precedence, applied in order.
    Chain {
        first: Box<E>,
        rest: Vec<(BinOp, E)>,
    },
    And(Vec<E>),
    Or(Vec<E>),
}

/// A name the compiler made a JavaScript variable for.
#[derive(Debug, Clone, Copy)]
struct Binding {
    fn_level: usize,
    slot: usize,
    /// The JavaScript scope (function or callback) it is declared in.
    scope: u32,
}

struct CFrame {
    vars: HashMap<String, Binding>,
    parent: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Function,
    Callback,
    /// A body Nunjucks wraps with `_withScopedSyntax`: callbacks opened in
    /// it close at its end.
    Region,
    /// A `switch` case body, where a callback is a syntax error.
    Case,
    /// A `switch` default or a loop's `else`, where a callback is a syntax
    /// error.
    Tail,
}

/// The longest comparison chain Brrrty runs. Nunjucks's generated
/// JavaScript fails near 5,600 (`RangeError: Maximum call stack size
/// exceeded` in V8's parser, with Node's default stack).
const MAX_COMPARISONS: usize = 4096;

/// The key the compiler binds `super()`'s result under. Not a valid name.
const SUPER: &str = "\0super";

struct Compiler {
    frames: Vec<CFrame>,
    cur: usize,
    /// Slots used by each function being compiled, innermost last.
    fns: Vec<usize>,
    open: Vec<(u32, ScopeKind)>,
    next_scope: u32,
    in_block: bool,
}

pub(crate) fn compile(name: &str, path: Option<&str>, nodes: &[Node]) -> CRes<Program> {
    let mut c = Compiler {
        frames: Vec::new(),
        cur: 0,
        fns: Vec::new(),
        open: Vec::new(),
        next_scope: 0,
        in_block: false,
    };
    let mut blocks = Vec::new();
    collect_blocks(nodes, false, &mut blocks);

    let root: FnBody = c.function(None, |c| c.nodes(nodes))?.into();
    c.in_block = true;
    let mut defs: Vec<BlockDef> = Vec::new();
    for (block, top) in blocks {
        if defs.iter().any(|d| d.name == block.name) {
            return Err(Fail::js(
                "Error",
                format!("Block \"{}\" defined more than once.", block.name),
            ));
        }
        let mut super_slot = None;
        let body = c
            .function(None, |c| {
                if top && block.body.iter().any(node_has_super) {
                    // `liftSuper` puts a `Super` node first; its callback
                    // holds the result for the rest of the block.
                    c.open_callback()?;
                    let slot = c.new_slot();
                    c.bind(SUPER, slot);
                    super_slot = Some(slot);
                }
                c.nodes(&block.body)
            })?
            .into();
        defs.push(BlockDef {
            name: block.name.clone(),
            body,
            super_slot,
        });
    }
    Ok(Program {
        name: name.to_string(),
        path: path.map(str::to_string),
        root,
        blocks: defs,
    })
}

/// Every `{% block %}`, in Nunjucks's `findAll` order, and whether it is
/// inside no other block.
fn collect_blocks<'a>(nodes: &'a [Node], inside: bool, out: &mut Vec<(&'a ast::Block, bool)>) {
    for n in nodes {
        crate::render::grow(|| collect_node(n, inside, out));
    }
}

fn collect_node<'a>(n: &'a Node, inside: bool, out: &mut Vec<(&'a ast::Block, bool)>) {
    match n {
        Node::Block(b) => {
            out.push((b, !inside));
            collect_blocks(&b.body, true, out);
        }
        Node::Text(_)
        | Node::Extends(_)
        | Node::Include(_)
        | Node::Import(_)
        | Node::FromImport(_) => {}
        Node::Output(o) => collect_expr(&o.expr, inside, out),
        Node::If(i) => {
            for b in &i.branches {
                collect_expr(&b.cond, inside, out);
                collect_blocks(&b.body, inside, out);
            }
            if let Some(e) = &i.else_body {
                collect_blocks(e, inside, out);
            }
        }
        Node::For(f) => {
            collect_expr(&f.iter, inside, out);
            collect_blocks(&f.body, inside, out);
            if let Some(e) = &f.else_body {
                collect_blocks(e, inside, out);
            }
        }
        Node::Set(s) => match &s.value {
            ast::SetValue::Expr(e) => collect_expr(e, inside, out),
            ast::SetValue::Capture(c) => collect_blocks(&c.body, inside, out),
        },
        Node::Macro(m) => {
            for a in &m.args.positional {
                collect_expr(a, inside, out);
            }
            for p in &m.args.keyword {
                collect_expr(&p.value, inside, out);
            }
            collect_blocks(&m.body, inside, out);
        }
        Node::Switch(s) => {
            collect_expr(&s.expr, inside, out);
            for c in &s.cases {
                collect_expr(&c.cond, inside, out);
                collect_blocks(&c.body, inside, out);
            }
            if let Some(d) = &s.default {
                collect_blocks(d, inside, out);
            }
        }
    }
}

/// Blocks inside expressions: in a `{% call %}` body or a `{% filter %}`
/// body.
fn collect_expr<'a>(e: &'a Expr, inside: bool, out: &mut Vec<(&'a ast::Block, bool)>) {
    for_each_child(e, &mut |child| match child {
        Expr::Caller { body, .. } => collect_blocks(body, inside, out),
        Expr::Capture(c) => collect_blocks(&c.body, inside, out),
        _ => {}
    });
}

/// Calls `f` on `e` and every expression inside it.
fn for_each_child<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    crate::render::grow(|| {
        f(e);
        let args = |a: &'a ast::Args, f: &mut dyn FnMut(&'a Expr)| {
            for x in &a.positional {
                for_each_child(x, f);
            }
            for p in &a.keyword {
                for_each_child(&p.key, f);
                for_each_child(&p.value, f);
            }
        };
        match e {
            Expr::Group { items, .. } | Expr::Array { items, .. } => {
                for x in items {
                    for_each_child(x, f);
                }
            }
            Expr::Dict { pairs, .. } => {
                for p in pairs {
                    for_each_child(&p.key, f);
                    for_each_child(&p.value, f);
                }
            }
            Expr::Lookup { target, key, .. } => {
                for_each_child(target, f);
                for_each_child(key, f);
            }
            Expr::Call {
                callee, args: a, ..
            } => {
                for_each_child(callee, f);
                args(a, f);
            }
            Expr::Filter { args: a, .. } | Expr::Caller { args: a, .. } => args(a, f),
            Expr::InlineIf {
                cond, body, else_, ..
            } => {
                for_each_child(cond, f);
                for_each_child(body, f);
                if let Some(e) = else_ {
                    for_each_child(e, f);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                for_each_child(lhs, f);
                for_each_child(rhs, f);
            }
            Expr::Unary { operand, .. } => for_each_child(operand, f),
            Expr::Compare { expr, ops, .. } => {
                for_each_child(expr, f);
                for op in ops {
                    for_each_child(&op.expr, f);
                }
            }
            Expr::Literal { .. }
            | Expr::Symbol { .. }
            | Expr::Capture(_)
            | Expr::Missing { .. } => {}
        }
    });
}

fn is_super_call(e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. } if matches!(&**callee,
        Expr::Symbol { name, .. } if name == "super")
        || matches!(&**callee, Expr::Literal { value: Literal::String(s), .. } if s == "super"))
}

/// Whether a `super()` call is anywhere under `n`, nested blocks included.
fn node_has_super(n: &Node) -> bool {
    let nodes_have = |ns: &[Node]| ns.iter().any(node_has_super);
    let expr_has = |e: &Expr| {
        let mut found = false;
        for_each_child(e, &mut |x| {
            found |= is_super_call(x)
                || matches!(x, Expr::Caller { body, .. } if body.iter().any(node_has_super))
                || matches!(x, Expr::Capture(c) if c.body.iter().any(node_has_super));
        });
        found
    };
    crate::render::grow(|| match n {
        Node::Text(_) => false,
        Node::Output(o) => expr_has(&o.expr),
        Node::If(i) => {
            i.branches
                .iter()
                .any(|b| expr_has(&b.cond) || nodes_have(&b.body))
                || i.else_body.as_deref().is_some_and(nodes_have)
        }
        Node::For(f) => {
            expr_has(&f.iter)
                || f.targets.iter().any(expr_has)
                || nodes_have(&f.body)
                || f.else_body.as_deref().is_some_and(nodes_have)
        }
        Node::Set(s) => {
            s.targets.iter().any(expr_has)
                || match &s.value {
                    ast::SetValue::Expr(e) => expr_has(e),
                    ast::SetValue::Capture(c) => nodes_have(&c.body),
                }
        }
        Node::Block(b) => nodes_have(&b.body),
        Node::Extends(e) => expr_has(&e.template),
        Node::Include(i) => expr_has(&i.template),
        Node::Macro(m) => {
            m.args.positional.iter().any(expr_has)
                || m.args.keyword.iter().any(|p| expr_has(&p.value))
                || nodes_have(&m.body)
        }
        Node::Import(i) => expr_has(&i.template),
        Node::FromImport(f) => expr_has(&f.template),
        Node::Switch(s) => {
            expr_has(&s.expr)
                || s.cases
                    .iter()
                    .any(|c| expr_has(&c.cond) || nodes_have(&c.body))
                || s.default.as_deref().is_some_and(nodes_have)
        }
    })
}

fn at(span: Span) -> Option<(usize, usize)> {
    Some((span.line, span.column))
}

fn refuse(message: impl Into<String>, span: Span) -> Fail {
    Fail {
        kind: ErrorKind::Unsupported,
        message: message.into(),
        at: at(span),
        help: None,
    }
}

fn null_typename() -> Fail {
    Fail::null_read("typename")
}

/// Nunjucks's node type name, for `assertType` errors.
fn typename(e: &Expr) -> &'static str {
    match e {
        Expr::Literal { .. } => "Literal",
        Expr::Symbol { .. } => "Symbol",
        Expr::Group { .. } => "Group",
        Expr::Array { .. } => "Array",
        Expr::Dict { .. } => "Dict",
        Expr::Lookup { .. } => "LookupVal",
        Expr::Call { .. } => "FunCall",
        Expr::Filter { .. } => "Filter",
        Expr::InlineIf { .. } => "InlineIf",
        Expr::Binary { op, .. } => match op {
            BinaryOp::Or => "Or",
            BinaryOp::And => "And",
            BinaryOp::In => "In",
            BinaryOp::Is => "Is",
            BinaryOp::Add => "Add",
            BinaryOp::Concat => "Concat",
            BinaryOp::Sub => "Sub",
            BinaryOp::Mul => "Mul",
            BinaryOp::Div => "Div",
            BinaryOp::FloorDiv => "FloorDiv",
            BinaryOp::Mod => "Mod",
            BinaryOp::Pow => "Pow",
        },
        Expr::Unary { op, .. } => match op {
            UnaryOp::Not => "Not",
            UnaryOp::Neg => "Neg",
            UnaryOp::Pos => "Pos",
        },
        Expr::Compare { .. } => "Compare",
        Expr::Caller { .. } => "Caller",
        Expr::Capture(_) => "Capture",
        Expr::Missing { .. } => "null",
    }
}

/// The name a `{% set %}` or a `{% macro %}` gives: a symbol or a quoted
/// string, as `node.value` is.
fn name_of(e: &Expr) -> Option<&str> {
    match e {
        Expr::Symbol { name, .. } => Some(name),
        Expr::Literal {
            value: Literal::String(s),
            ..
        } => Some(s),
        _ => None,
    }
}

/// A token of the JavaScript Nunjucks writes for an expression.
enum Tok {
    Atom(E),
    Prefix(UnaryOp),
    Bin(Op),
}

#[derive(Clone, Copy)]
enum Op {
    Or,
    And,
    B(BinOp),
}

impl Op {
    fn precedence(self) -> u8 {
        match self {
            Op::Or => 3,
            Op::And => 4,
            Op::B(b) => b.precedence(),
        }
    }
}

const LEVELS: [u8; 6] = [3, 4, 8, 9, 11, 12];

/// Reads tokens back with JavaScript's precedence.
struct Reader {
    toks: std::vec::IntoIter<Tok>,
    peeked: Option<Tok>,
}

impl Reader {
    fn peek(&mut self) -> Option<&Tok> {
        if self.peeked.is_none() {
            self.peeked = self.toks.next();
        }
        self.peeked.as_ref()
    }

    fn next(&mut self) -> Option<Tok> {
        self.peek();
        self.peeked.take()
    }

    fn level(&mut self, lvl: usize) -> E {
        if lvl == LEVELS.len() {
            return self.unary();
        }
        let first = self.level(lvl + 1);
        let mut rest: Vec<(Op, E)> = Vec::new();
        while let Some(Tok::Bin(op)) = self.peek() {
            let op = *op;
            if op.precedence() != LEVELS[lvl] {
                break;
            }
            self.next();
            rest.push((op, self.level(lvl + 1)));
        }
        if rest.is_empty() {
            return first;
        }
        match rest[0].0 {
            Op::Or => E::Or(
                std::iter::once(first)
                    .chain(rest.into_iter().map(|r| r.1))
                    .collect(),
            ),
            Op::And => E::And(
                std::iter::once(first)
                    .chain(rest.into_iter().map(|r| r.1))
                    .collect(),
            ),
            Op::B(_) => E::Chain {
                first: Box::new(first),
                rest: rest
                    .into_iter()
                    .map(|(op, e)| match op {
                        Op::B(b) => (b, e),
                        _ => unreachable!("one level holds one kind of operator"),
                    })
                    .collect(),
            },
        }
    }

    fn unary(&mut self) -> E {
        let mut prefixes = Vec::new();
        let atom = loop {
            match self.next() {
                Some(Tok::Prefix(op)) => prefixes.push(op),
                Some(Tok::Atom(e)) => break e,
                // The writer always ends a run of prefixes with an atom.
                _ => break E::Lit(Lit::Undefined),
            }
        };
        prefixes.into_iter().rev().fold(atom, |e, op| match op {
            UnaryOp::Not => E::Not(Box::new(e)),
            UnaryOp::Neg => E::Neg(Box::new(e)),
            UnaryOp::Pos => E::Pos(Box::new(e)),
        })
    }
}

fn read(toks: Vec<Tok>) -> E {
    let mut r = Reader {
        toks: toks.into_iter(),
        peeked: None,
    };
    r.level(0)
}

impl Compiler {
    // ---- scopes and frames ----

    fn new_scope(&mut self, kind: ScopeKind) -> u32 {
        let id = self.next_scope;
        self.next_scope += 1;
        self.open.push((id, kind));
        id
    }

    /// Closes the scope `id` and every callback opened inside it.
    fn close_scope(&mut self, id: u32) {
        while let Some((top, _)) = self.open.pop() {
            if top == id {
                break;
            }
        }
    }

    /// Runs `f` in a region: the callbacks it opens close at its end.
    fn region<T>(&mut self, kind: ScopeKind, f: impl FnOnce(&mut Self) -> CRes<T>) -> CRes<T> {
        let id = self.new_scope(kind);
        let r = f(self);
        self.close_scope(id);
        r
    }

    /// Opens a callback that lasts to the end of the enclosing region.
    fn open_callback(&mut self) -> CRes<()> {
        let enclosing = self
            .open
            .iter()
            .rev()
            .find(|(_, k)| *k != ScopeKind::Callback)
            .map(|(_, k)| *k);
        match enclosing {
            Some(ScopeKind::Case) => Err(Fail::js("SyntaxError", "Illegal break statement")),
            Some(ScopeKind::Tail) => Err(Fail::js("SyntaxError", "missing ) after argument list")),
            _ => {
                self.new_scope(ScopeKind::Callback);
                Ok(())
            }
        }
    }

    /// The scope a `var` declared here belongs to.
    fn decl_scope(&self) -> u32 {
        self.open
            .iter()
            .rev()
            .find(|(_, k)| matches!(k, ScopeKind::Function | ScopeKind::Callback))
            .map_or(0, |(id, _)| *id)
    }

    fn in_scope(&self, b: &Binding) -> bool {
        self.open.iter().any(|(id, _)| *id == b.scope)
    }

    fn push_frame(&mut self, linked: bool) -> usize {
        let saved = self.cur;
        self.frames.push(CFrame {
            vars: HashMap::new(),
            parent: linked.then_some(self.cur),
        });
        self.cur = self.frames.len() - 1;
        saved
    }

    fn has_parent(&self) -> bool {
        self.frames[self.cur].parent.is_some()
    }

    fn lookup(&self, name: &str) -> Option<Binding> {
        let mut f = Some(self.cur);
        while let Some(i) = f {
            if let Some(b) = self.frames[i].vars.get(name) {
                return Some(*b);
            }
            f = self.frames[i].parent;
        }
        None
    }

    fn new_slot(&mut self) -> usize {
        let n = self.fns.last_mut().expect("inside a function");
        *n += 1;
        *n - 1
    }

    fn bind(&mut self, name: &str, slot: usize) {
        let b = Binding {
            fn_level: self.fns.len() - 1,
            slot,
            scope: self.decl_scope(),
        };
        self.frames[self.cur].vars.insert(name.to_string(), b);
    }

    fn var_ref(&self, b: &Binding) -> VarRef {
        VarRef {
            up: self.fns.len() - 1 - b.fn_level,
            slot: b.slot,
        }
    }

    /// Compiles a JavaScript function's body. `linked`: `None` for a fresh
    /// compiler frame (the root, a block, a macro), `Some(_)` to push one
    /// onto the current frame (a caller, which sees the frames around it).
    fn function<T>(
        &mut self,
        linked: Option<bool>,
        f: impl FnOnce(&mut Self) -> CRes<T>,
    ) -> CRes<FnBodyOf<T>> {
        let saved = self.push_frame(linked.is_some());
        self.fns.push(0);
        let id = self.new_scope(ScopeKind::Function);
        let r = f(self);
        self.close_scope(id);
        let slots = self.fns.pop().unwrap_or(0);
        self.cur = saved;
        Ok(FnBodyOf { value: r?, slots })
    }

    // ---- statements ----

    fn nodes(&mut self, nodes: &[Node]) -> CRes<FnBody> {
        let stmts = self.stmts(nodes)?;
        Ok(FnBody { stmts, slots: 0 })
    }

    fn stmts(&mut self, nodes: &[Node]) -> CRes<Vec<Stmt>> {
        let mut out = Vec::with_capacity(nodes.len());
        for n in nodes {
            if let Some(s) = crate::render::grow(|| self.node(n))? {
                out.push(s);
            }
        }
        Ok(out)
    }

    fn body(&mut self, kind: ScopeKind, nodes: &[Node]) -> CRes<Vec<Stmt>> {
        self.region(kind, |c| c.stmts(nodes))
    }

    fn node(&mut self, n: &Node) -> CRes<Option<Stmt>> {
        Ok(Some(match n {
            Node::Text(t) if t.text.is_empty() => return Ok(None),
            Node::Text(t) => Stmt::Text(t.text.clone()),
            Node::Output(o) => Stmt::Output {
                span: o.span,
                expr: self.expr(&o.expr)?,
            },
            Node::If(i) => {
                if i.is_async {
                    return Err(async_refused("ifAsync", i.span));
                }
                let mut branches = Vec::new();
                for b in &i.branches {
                    let cond = self.expr(&b.cond)?;
                    branches.push((cond, self.body(ScopeKind::Region, &b.body)?));
                }
                let else_ = match &i.else_body {
                    Some(e) => Some(self.body(ScopeKind::Region, e)?),
                    None => None,
                };
                Stmt::If {
                    span: i.span,
                    branches,
                    else_,
                }
            }
            Node::For(f) => Stmt::For(Box::new(self.for_loop(f)?)),
            Node::Set(s) => self.set(s)?,
            Node::Block(b) => {
                let s = Stmt::Block {
                    span: b.span,
                    name: b.name.clone(),
                    in_block: self.in_block,
                };
                self.open_callback()?;
                s
            }
            Node::Extends(e) => {
                if self.in_block {
                    return Err(refuse(
                        "Brrrty doesn't run `{% extends %}` inside a block: Nunjucks sets a JavaScript global there",
                        e.span,
                    ));
                }
                let template = self.expr(&e.template)?;
                self.open_callback()?;
                Stmt::Extends {
                    span: e.span,
                    template,
                }
            }
            Node::Include(i) => {
                let template = self.expr(&i.template)?;
                self.open_callback()?;
                Stmt::Include {
                    span: i.span,
                    template,
                    ignore_missing: i.ignore_missing,
                }
            }
            Node::Macro(m) => {
                let Some(name) = name_of(&m.name) else {
                    return Err(refuse("a macro's name must be a name", m.span));
                };
                let def = self.macro_def(&m.args, &m.body, false)?;
                let slot = self.new_slot();
                self.bind(name, slot);
                Stmt::Macro {
                    span: m.span,
                    name: name.to_string(),
                    def,
                    slot,
                    nested: self.has_parent(),
                }
            }
            Node::Import(i) => {
                let Expr::Symbol { name: target, .. } = &i.target else {
                    return Err(refuse("an `{% import %}` must be `as` a name", i.span));
                };
                let template = self.expr(&i.template)?;
                self.open_callback()?;
                self.open_callback()?;
                let slot = self.new_slot();
                self.bind(target, slot);
                Stmt::Import {
                    span: i.span,
                    template,
                    with_context: i.with_context == Some(true),
                    target: target.clone(),
                    slot,
                    nested: self.has_parent(),
                }
            }
            Node::FromImport(f) => {
                let template = self.expr(&f.template)?;
                self.open_callback()?;
                self.open_callback()?;
                let mut names = Vec::new();
                for n in &f.names {
                    let Some(name) = name_of(&n.name) else {
                        return Err(refuse("an imported name must be a name", n.span));
                    };
                    let alias = match &n.alias {
                        None => name,
                        Some(Expr::Symbol { name, .. }) => name,
                        Some(_) => return Err(refuse("an import's alias must be a name", n.span)),
                    };
                    let slot = self.new_slot();
                    self.bind(alias, slot);
                    names.push(FromName {
                        name: name.to_string(),
                        alias: alias.to_string(),
                        slot,
                    });
                }
                Stmt::FromImport {
                    span: f.span,
                    template,
                    with_context: f.with_context == Some(true),
                    names,
                    nested: self.has_parent(),
                }
            }
            Node::Switch(s) => {
                let expr = self.expr(&s.expr)?;
                let mut cases = Vec::new();
                for c in &s.cases {
                    let cond = self.expr(&c.cond)?;
                    let body = self.body(ScopeKind::Case, &c.body)?;
                    cases.push((cond, body, !c.body.is_empty()));
                }
                let default = match &s.default {
                    Some(d) => Some(self.body(ScopeKind::Tail, d)?),
                    None => None,
                };
                Stmt::Switch {
                    span: s.span,
                    expr,
                    cases,
                    default,
                }
            }
        }))
    }

    fn for_loop(&mut self, f: &ast::For) -> CRes<ForStmt> {
        if f.kind != ast::ForKind::For {
            let tag = if f.kind == ast::ForKind::AsyncEach {
                "asyncEach"
            } else {
                "asyncAll"
            };
            return Err(async_refused(tag, f.span));
        }
        let saved = self.push_frame(true);
        let r = (|| {
            let iter = self.expr(&f.iter)?;
            let len_slot = self.new_slot();
            let mut names = Vec::new();
            for t in &f.targets {
                let Expr::Symbol { name, .. } = t else {
                    return Err(refuse(
                        "Brrrty runs `for` only with plain names before `in`",
                        f.span,
                    ));
                };
                if names.contains(name) {
                    return Err(refuse(format!("the loop names `{name}` twice"), f.span));
                }
                names.push(name.clone());
            }
            let targets = if names.len() == 1 {
                let slot = self.new_slot();
                self.bind(&names[0], slot);
                ForTargets::One {
                    name: names.remove(0),
                    slot,
                }
            } else {
                let slots: Vec<usize> = names.iter().map(|_| self.new_slot()).collect();
                for (n, s) in names.iter().zip(&slots) {
                    self.bind(n, *s);
                }
                ForTargets::Many { names, slots }
            };
            let body = self.body(ScopeKind::Region, &f.body)?;
            let else_ = match &f.else_body {
                Some(e) => Some(self.body(ScopeKind::Tail, e)?),
                None => None,
            };
            Ok(ForStmt {
                span: f.span,
                iter,
                targets,
                len_slot,
                body,
                else_,
            })
        })();
        self.cur = saved;
        r
    }

    fn set(&mut self, s: &ast::Set) -> CRes<Stmt> {
        let mut names = Vec::new();
        for t in &s.targets {
            let Some(name) = name_of(t) else {
                return Err(refuse(
                    "Brrrty runs `{% set %}` only with plain names",
                    s.span,
                ));
            };
            names.push(name.to_string());
        }
        let mut targets = Vec::new();
        for name in names {
            let r = match self.lookup(&name) {
                Some(b) if self.in_scope(&b) => self.var_ref(&b),
                Some(_) => {
                    return Err(refuse(
                        format!(
                            "`{name}` was declared in a branch that has ended; Nunjucks would set a JavaScript global here"
                        ),
                        s.span,
                    ));
                }
                None => VarRef {
                    up: 0,
                    slot: self.new_slot(),
                },
            };
            targets.push((name, r));
        }
        let value = match &s.value {
            ast::SetValue::Expr(e) => SetValue::Expr(self.expr(e)?),
            ast::SetValue::Capture(c) => SetValue::Capture(self.capture(&c.body)?),
        };
        Ok(Stmt::Set {
            span: s.span,
            targets,
            value,
        })
    }

    /// A capture body: an immediately called JavaScript function, run in
    /// the current frames.
    fn capture(&mut self, body: &[Node]) -> CRes<Vec<Stmt>> {
        let id = self.new_scope(ScopeKind::Function);
        let r = self.body(ScopeKind::Region, body);
        self.close_scope(id);
        r
    }

    fn macro_def(
        &mut self,
        args: &ast::Args,
        body: &[Node],
        keep_frame: bool,
    ) -> CRes<Arc<MacroDef>> {
        let mut positional = Vec::new();
        let kw = &args.keyword[..];
        let last_dict =
            args.keyword.is_empty() && matches!(args.positional.last(), Some(Expr::Dict { .. }));
        if last_dict {
            return Err(refuse(
                "a macro's last argument can't be a `{...}` literal",
                args.positional.last().map(Expr::span).unwrap_or_default(),
            ));
        }
        for a in &args.positional {
            match a {
                Expr::Symbol { name, .. } => positional.push(name.clone()),
                Expr::Missing { .. } => return Err(null_typename()),
                other => {
                    return Err(Fail::template(
                        format!("assertType: invalid type: {}", typename(other)),
                        at(other.span()),
                    ));
                }
            }
        }
        let mut kwargs_names = Vec::new();
        for p in kw {
            let Some(name) = name_of(&p.key) else {
                return Err(refuse("a macro's keyword argument must be a name", p.span));
            };
            kwargs_names.push(name.to_string());
        }
        let def = self.function(keep_frame.then_some(true), |c| {
            let mut args = Vec::new();
            for name in &positional {
                let slot = c.new_slot();
                c.bind(name, slot);
                args.push((name.clone(), slot));
            }
            let mut kwargs = Vec::new();
            for (name, p) in kwargs_names.iter().zip(kw) {
                kwargs.push((name.clone(), c.expr(&p.value)?));
            }
            let body = c.body(ScopeKind::Region, body)?;
            Ok((args, kwargs, body))
        })?;
        let (args, kwargs, body) = def.value;
        Ok(Arc::new(MacroDef {
            args,
            kwargs,
            body,
            slots: def.slots,
            keep_frame,
        }))
    }

    // ---- expressions ----

    fn expr(&mut self, e: &Expr) -> CRes<E> {
        let mut toks = Vec::new();
        self.emit(e, &mut toks)?;
        Ok(read(toks))
    }

    fn boxed(&mut self, e: &Expr) -> CRes<Box<E>> {
        Ok(Box::new(self.expr(e)?))
    }

    fn atom(&mut self, e: E, out: &mut Vec<Tok>) -> CRes<()> {
        out.push(Tok::Atom(e));
        Ok(())
    }

    fn emit(&mut self, e: &Expr, out: &mut Vec<Tok>) -> CRes<()> {
        crate::render::grow(|| self.emit_inner(e, out))
    }

    fn emit_inner(&mut self, e: &Expr, out: &mut Vec<Tok>) -> CRes<()> {
        match e {
            Expr::Literal { span, value } => {
                let lit = match value {
                    Literal::String(s) => Lit::Str(s.as_str().into()),
                    Literal::Number(n) => Lit::Num(*n),
                    Literal::Bool(b) => Lit::Bool(*b),
                    Literal::Null => Lit::Null,
                    Literal::Regex { .. } => {
                        return Err(refuse(
                            "Brrrty doesn't run Nunjucks regular expressions (`r/.../`) yet",
                            *span,
                        )
                        .with_help("a later Brrrty release runs them; until then, use a string method or a filter"));
                    }
                };
                self.atom(E::Lit(lit), out)
            }
            Expr::Symbol { name, .. } => {
                let v = self.symbol(name);
                self.atom(v, out)
            }
            Expr::Group { items, .. } => {
                let v = match items.len() {
                    0 => return Err(Fail::js("SyntaxError", "Unexpected token ')'")),
                    1 => self.expr(&items[0])?,
                    _ => E::Comma(items.iter().map(|i| self.expr(i)).collect::<CRes<_>>()?),
                };
                self.atom(v, out)
            }
            Expr::Array { items, .. } => {
                let v = E::Array(items.iter().map(|i| self.expr(i)).collect::<CRes<_>>()?);
                self.atom(v, out)
            }
            Expr::Dict { pairs, .. } => {
                let mut v = Vec::new();
                for p in pairs {
                    let key = self.dict_key(&p.key)?;
                    v.push((key, self.expr(&p.value)?));
                }
                self.atom(E::Dict(v), out)
            }
            Expr::Lookup { span, target, key } => {
                if matches!(**key, Expr::Missing { .. }) || matches!(**target, Expr::Missing { .. })
                {
                    return Err(null_typename());
                }
                let v = E::Member {
                    span: *span,
                    target: self.boxed(target)?,
                    key: self.boxed(key)?,
                };
                self.atom(v, out)
            }
            Expr::Call { span, callee, args } => {
                if matches!(**callee, Expr::Missing { .. }) {
                    return Err(null_typename());
                }
                if self.in_block && is_super_call(e) {
                    // `liftSuper` made this a read of the block's result.
                    let v = match self.lookup(SUPER) {
                        Some(b) if self.in_scope(&b) => E::Var(self.var_ref(&b)),
                        _ => E::Lit(Lit::Undefined),
                    };
                    return self.atom(v, out);
                }
                let name = node_name(callee)?;
                let callee = self.boxed(callee)?;
                let (args, kwargs) = self.args(args)?;
                self.atom(
                    E::Call {
                        span: *span,
                        callee,
                        name,
                        args,
                        kwargs,
                    },
                    out,
                )
            }
            Expr::Filter { span, name, args } => {
                let (args, kwargs) = self.args(args)?;
                self.atom(
                    E::Filter {
                        span: *span,
                        name: name.clone(),
                        args,
                        kwargs,
                    },
                    out,
                )
            }
            Expr::InlineIf {
                cond, body, else_, ..
            } => {
                let cond = self.boxed(cond)?;
                let then = self.boxed(body)?;
                let else_ = match else_ {
                    Some(e) => Some(self.boxed(e)?),
                    None => None,
                };
                self.atom(E::Cond { cond, then, else_ }, out)
            }
            Expr::Binary { span, op, lhs, rhs } => {
                let simple = |op: Op| Tok::Bin(op);
                match op {
                    BinaryOp::Or
                    | BinaryOp::And
                    | BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod => {
                        let tok = match op {
                            BinaryOp::Or => Op::Or,
                            BinaryOp::And => Op::And,
                            BinaryOp::Add => Op::B(BinOp::Add),
                            BinaryOp::Sub => Op::B(BinOp::Sub),
                            BinaryOp::Mul => Op::B(BinOp::Mul),
                            BinaryOp::Div => Op::B(BinOp::Div),
                            _ => Op::B(BinOp::Mod),
                        };
                        self.emit(lhs, out)?;
                        out.push(simple(tok));
                        self.emit(rhs, out)
                    }
                    BinaryOp::Concat => {
                        self.emit(lhs, out)?;
                        out.push(Tok::Bin(Op::B(BinOp::Add)));
                        out.push(Tok::Atom(E::Lit(Lit::Str("".into()))));
                        out.push(Tok::Bin(Op::B(BinOp::Add)));
                        self.emit(rhs, out)
                    }
                    BinaryOp::FloorDiv => {
                        let mut inner = Vec::new();
                        self.emit(lhs, &mut inner)?;
                        inner.push(Tok::Bin(Op::B(BinOp::Div)));
                        self.emit(rhs, &mut inner)?;
                        self.atom(E::Floor(Box::new(read(inner))), out)
                    }
                    BinaryOp::Pow => {
                        let v = E::Pow(self.boxed(lhs)?, self.boxed(rhs)?);
                        self.atom(v, out)
                    }
                    BinaryOp::In => {
                        let v = E::In {
                            span: *span,
                            lhs: self.boxed(lhs)?,
                            rhs: self.boxed(rhs)?,
                        };
                        self.atom(v, out)
                    }
                    BinaryOp::Is => {
                        let (name, arg) = self.test_of(rhs)?;
                        let v = E::Test {
                            span: *span,
                            name,
                            operand: self.boxed(lhs)?,
                            arg,
                        };
                        out.push(Tok::Atom(v));
                        out.push(Tok::Bin(Op::B(BinOp::StrictEq)));
                        self.atom(E::Lit(Lit::Bool(true)), out)
                    }
                }
            }
            Expr::Unary { op, operand, .. } => {
                let doubled = matches!(
                    (op, &**operand),
                    (
                        UnaryOp::Neg,
                        Expr::Unary {
                            op: UnaryOp::Neg,
                            ..
                        }
                    ) | (
                        UnaryOp::Pos,
                        Expr::Unary {
                            op: UnaryOp::Pos,
                            ..
                        }
                    )
                );
                if doubled {
                    return Err(Fail::js(
                        "SyntaxError",
                        "Invalid left-hand side expression in prefix operation",
                    ));
                }
                out.push(Tok::Prefix(*op));
                self.emit(operand, out)
            }
            Expr::Compare { span, expr, ops } => {
                if ops.len() > MAX_COMPARISONS {
                    return Err(Fail {
                        kind: ErrorKind::Limit,
                        message: format!(
                            "a chain of {} comparisons is longer than Brrrty runs ({MAX_COMPARISONS})",
                            ops.len()
                        ),
                        at: at(*span),
                        help: Some("Nunjucks fails on chains of about 5,600 comparisons or more, when V8 runs out of stack".into()),
                    });
                }
                self.emit(expr, out)?;
                for o in ops {
                    let op = match o.op {
                        CompareOp::Eq => BinOp::Eq,
                        CompareOp::StrictEq => BinOp::StrictEq,
                        CompareOp::Ne => BinOp::Ne,
                        CompareOp::StrictNe => BinOp::StrictNe,
                        CompareOp::Lt => BinOp::Lt,
                        CompareOp::Gt => BinOp::Gt,
                        CompareOp::Le => BinOp::Le,
                        CompareOp::Ge => BinOp::Ge,
                    };
                    out.push(Tok::Bin(Op::B(op)));
                    self.emit(&o.expr, out)?;
                }
                Ok(())
            }
            Expr::Caller { args, body, .. } => {
                let def = self.macro_def(args, body, true)?;
                self.atom(E::Caller(def), out)
            }
            Expr::Capture(c) => {
                let body = self.capture(&c.body)?;
                self.atom(E::Capture(body), out)
            }
            Expr::Missing { .. } => Err(null_typename()),
        }
    }

    fn symbol(&self, name: &str) -> E {
        match self.lookup(name) {
            Some(b) if self.in_scope(&b) => E::Var(self.var_ref(&b)),
            Some(_) => E::OutOfScope(name.to_string()),
            None => E::Lookup(name.to_string()),
        }
    }

    fn dict_key(&self, key: &Expr) -> CRes<String> {
        let k = match key {
            Expr::Symbol { name, .. } => name.clone(),
            Expr::Literal {
                value: Literal::String(s),
                ..
            } => s.clone(),
            Expr::Missing { .. } => return Err(null_typename()),
            other => {
                return Err(Fail::template(
                    "compilePair: Dict keys must be strings or names",
                    at(other.span()),
                ));
            }
        };
        if k == "__proto__" {
            return Err(refuse(
                "Brrrty doesn't make an object with a `__proto__` key",
                key.span(),
            ));
        }
        Ok(k)
    }

    #[allow(clippy::type_complexity)]
    fn args(&mut self, a: &ast::Args) -> CRes<(Vec<E>, Option<Vec<(String, E)>>)> {
        let mut args = Vec::new();
        for x in &a.positional {
            args.push(self.expr(x)?);
        }
        let kwargs = if a.keyword.is_empty() {
            None
        } else {
            let mut kw = Vec::new();
            for p in &a.keyword {
                let key = self.dict_key(&p.key)?;
                kw.push((key, self.expr(&p.value)?));
            }
            Some(kw)
        };
        Ok((args, kwargs))
    }

    /// The test an `is` names, as `compileIs` reads it from its right side,
    /// and the test's argument.
    fn test_of(&mut self, rhs: &Expr) -> CRes<(String, Option<Box<E>>)> {
        let lit_name = |v: &Literal| match v {
            Literal::String(s) => s.clone(),
            Literal::Number(n) => brrrty_js::js_number_to_string(*n),
            Literal::Bool(b) => b.to_string(),
            Literal::Null => "null".into(),
            Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
        };
        match rhs {
            Expr::Symbol { name, .. } => Ok((name.clone(), None)),
            Expr::Literal { value, .. } => Ok((lit_name(value), None)),
            Expr::Call { callee, args, span } => {
                let name = match &**callee {
                    Expr::Symbol { name, .. } => name.clone(),
                    Expr::Literal { value, .. } => lit_name(value),
                    _ => "undefined".into(),
                };
                if !args.keyword.is_empty() || args.positional.len() > 1 {
                    return Err(refuse(
                        "Brrrty runs an `is` test with at most one argument",
                        *span,
                    ));
                }
                let arg = match args.positional.first() {
                    Some(a) => Some(self.boxed(a)?),
                    None => None,
                };
                Ok((name, arg))
            }
            Expr::Filter { span, .. } => Err(refuse(
                "Brrrty doesn't run an `is` test whose name is followed by a filter",
                *span,
            )),
            Expr::Missing { .. } => Err(null_typename()),
            _ => Ok(("undefined".into(), None)),
        }
    }
}

/// A function body and the slots it needs.
struct FnBodyOf<T> {
    value: T,
    slots: usize,
}

impl FnBodyOf<FnBody> {
    fn into_body(self) -> FnBody {
        FnBody {
            stmts: self.value.stmts,
            slots: self.slots,
        }
    }
}

/// `compiler._getNodeName`: how a call's callee is named in errors.
fn node_name(e: &Expr) -> CRes<String> {
    Ok(match e {
        Expr::Symbol { name, .. } => name.clone(),
        Expr::Call { callee, .. } => format!("the return value of ({})", node_name(callee)?),
        Expr::Lookup { target, key, .. } => {
            format!("{}[\"{}\"]", node_name(target)?, node_name(key)?)
        }
        Expr::Literal { value, .. } => match value {
            Literal::String(s) => s.clone(),
            Literal::Number(n) => brrrty_js::js_number_to_string(*n),
            Literal::Bool(b) => b.to_string(),
            Literal::Null => return Err(Fail::null_read("toString")),
            Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
        },
        _ => "--expression--".into(),
    })
}

fn async_refused(tag: &str, span: Span) -> Fail {
    refuse(format!("Brrrty doesn't run `{{% {tag} %}}`"), span)
        .with_help("without asynchronous filters it does the same as `if` or `for`; use those")
}

// The root and block bodies come back through `function` with their slot
// counts; this keeps `compile` readable.
impl From<FnBodyOf<FnBody>> for FnBody {
    fn from(b: FnBodyOf<FnBody>) -> FnBody {
        b.into_body()
    }
}
