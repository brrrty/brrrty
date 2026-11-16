//! The syntax tree, shaped like Nunjucks 3.2.4's `nodes.js`.
//!
//! Every node has a [`Span`]. Its line and column are the position
//! Nunjucks gives the same node, which is where Nunjucks reports errors
//! about it: usually the first token of the node, but for example a binary
//! operator takes the position of its left operand and a comparison the
//! position of its first operator, as in Nunjucks.
//!
//! Some constructs are stored the way Nunjucks builds them:
//!
//! - `{% raw %}` and `{% verbatim %}` are [`Node::Text`].
//! - `{% call %}` is an [`Node::Output`] of an [`Expr::Call`] whose last
//!   keyword argument is `caller`, an [`Expr::Caller`].
//! - `{% filter %}` is an [`Node::Output`] of an [`Expr::Filter`] whose
//!   first argument is an [`Expr::Capture`] of the body.
//!
//! `{% elif %}` chains are flattened into [`If::branches`].

/// Where a node is. `start` and `end` are byte offsets into the source;
/// `line` and `column` are 1-based and the column counts UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

/// A parsed template.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// Text, after whitespace control, printed as it is.
    Text(Text),
    /// `{{ expr }}`, `{% call %}` and `{% filter %}`.
    Output(Output),
    If(If),
    For(For),
    Set(Set),
    Block(Block),
    Extends(Extends),
    Include(Include),
    Macro(Macro),
    Import(Import),
    FromImport(FromImport),
    Switch(Switch),
}

impl Node {
    pub fn span(&self) -> Span {
        match self {
            Node::Text(n) => n.span,
            Node::Output(n) => n.span,
            Node::If(n) => n.span,
            Node::For(n) => n.span,
            Node::Set(n) => n.span,
            Node::Block(n) => n.span,
            Node::Extends(n) => n.span,
            Node::Include(n) => n.span,
            Node::Macro(n) => n.span,
            Node::Import(n) => n.span,
            Node::FromImport(n) => n.span,
            Node::Switch(n) => n.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub span: Span,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub span: Span,
    pub expr: Expr,
}

/// `{% if %}`, with each `{% elif %}` or `{% elseif %}` as another branch.
#[derive(Debug, Clone, PartialEq)]
pub struct If {
    /// Where the `if` tag name is.
    pub span: Span,
    /// `{% ifAsync %}`, which Nunjucks runs like `if` when no filter is
    /// asynchronous.
    pub is_async: bool,
    pub branches: Vec<Branch>,
    pub else_body: Option<Vec<Node>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    /// Where the `if`, `elif` or `elseif` tag name is.
    pub span: Span,
    pub cond: Expr,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForKind {
    For,
    AsyncEach,
    AsyncAll,
}

#[derive(Debug, Clone, PartialEq)]
pub struct For {
    pub span: Span,
    pub kind: ForKind,
    /// The loop variables. One is a plain loop variable. More than one
    /// (`for k, v in ...`) unpack each item. The first is always an
    /// [`Expr::Symbol`]; Nunjucks checks nothing about the others.
    pub targets: Vec<Expr>,
    pub iter: Expr,
    pub body: Vec<Node>,
    pub else_body: Option<Vec<Node>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub span: Span,
    pub targets: Vec<Expr>,
    pub value: SetValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetValue {
    /// `{% set x = expr %}`
    Expr(Expr),
    /// `{% set x %}...{% endset %}`
    Capture(Capture),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub span: Span,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    pub name: String,
    pub name_span: Span,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Extends {
    pub span: Span,
    pub template: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Include {
    pub span: Span,
    pub template: Expr,
    pub ignore_missing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Macro {
    pub span: Span,
    /// Any expression without a postfix; Nunjucks checks nothing here.
    pub name: Expr,
    pub args: Args,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    pub span: Span,
    pub template: Expr,
    pub target: Expr,
    /// `with context` is `Some(true)`, `without context` `Some(false)`.
    pub with_context: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FromImport {
    pub span: Span,
    pub template: Expr,
    pub names: Vec<ImportName>,
    /// Only the last name's `with context` or `without context` counts,
    /// as in Nunjucks.
    pub with_context: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportName {
    pub span: Span,
    pub name: Expr,
    pub alias: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Switch {
    pub span: Span,
    pub expr: Expr,
    pub cases: Vec<Case>,
    pub default: Option<Vec<Node>>,
}

/// One `{% case %}`. Nunjucks gives cases no position.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub cond: Expr,
    pub body: Vec<Node>,
}

/// Call arguments or a macro signature: the positional arguments, then
/// the `name=value` pairs, which Nunjucks gathers at the end whatever
/// order they were written in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Args {
    pub positional: Vec<Expr>,
    pub keyword: Vec<Pair>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub span: Span,
    pub key: Expr,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    String(String),
    Number(f64),
    Bool(bool),
    /// `none` or `null`.
    Null,
    /// `r/pattern/flags`. The pattern is not checked here.
    Regex {
        pattern: String,
        flags: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    In,
    Is,
    Add,
    /// `~`
    Concat,
    Sub,
    Mul,
    Div,
    /// `//`
    FloorDiv,
    Mod,
    /// `**`
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
    Pos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    StrictEq,
    Ne,
    StrictNe,
    Lt,
    Gt,
    Le,
    Ge,
}

impl CompareOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CompareOp::Eq => "==",
            CompareOp::StrictEq => "===",
            CompareOp::Ne => "!=",
            CompareOp::StrictNe => "!==",
            CompareOp::Lt => "<",
            CompareOp::Gt => ">",
            CompareOp::Le => "<=",
            CompareOp::Ge => ">=",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompareOperand {
    /// Where the operator is.
    pub span: Span,
    pub op: CompareOp,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal {
        span: Span,
        value: Literal,
    },
    Symbol {
        span: Span,
        name: String,
    },
    /// `(a)`, or `(a, b)`, which Nunjucks compiles to a JavaScript comma
    /// expression.
    Group {
        span: Span,
        items: Vec<Expr>,
    },
    Array {
        span: Span,
        items: Vec<Expr>,
    },
    Dict {
        span: Span,
        pairs: Vec<Pair>,
    },
    /// `target.key` (`key` is a string literal) or `target[key]`.
    Lookup {
        span: Span,
        target: Box<Expr>,
        key: Box<Expr>,
    },
    Call {
        span: Span,
        callee: Box<Expr>,
        args: Args,
    },
    /// `input | name(args)`: the input is the first positional argument.
    /// A dotted name such as `a.b` is kept whole, as Nunjucks does.
    Filter {
        span: Span,
        name: String,
        args: Args,
    },
    /// `body if cond else else_`
    InlineIf {
        span: Span,
        cond: Box<Expr>,
        body: Box<Expr>,
        else_: Option<Box<Expr>>,
    },
    Binary {
        span: Span,
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        span: Span,
        op: UnaryOp,
        operand: Box<Expr>,
    },
    /// `expr op a op b ...`
    Compare {
        span: Span,
        expr: Box<Expr>,
        ops: Vec<CompareOperand>,
    },
    /// The `caller` a `{% call %}` block passes to the macro.
    Caller {
        span: Span,
        args: Args,
        body: Vec<Node>,
    },
    /// The body of a `{% filter %}` block, as the filter's input.
    Capture(Capture),
    /// Where Nunjucks's parser leaves `null`, such as the callee of
    /// `{{ ,(1) }}` or the key of `{{ a[] }}`. Nunjucks fails on these
    /// when it compiles the template.
    Missing {
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal { span, .. }
            | Expr::Symbol { span, .. }
            | Expr::Group { span, .. }
            | Expr::Array { span, .. }
            | Expr::Dict { span, .. }
            | Expr::Lookup { span, .. }
            | Expr::Call { span, .. }
            | Expr::Filter { span, .. }
            | Expr::InlineIf { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Compare { span, .. }
            | Expr::Caller { span, .. }
            | Expr::Missing { span } => *span,
            Expr::Capture(c) => c.span,
        }
    }
}
