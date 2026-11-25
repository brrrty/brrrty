//! The parsed form of a Liquid template.
//!
//! The tree mirrors what LiquidJS builds when it parses a template, so the
//! renderer can reproduce its behavior without re-reading the source. Text
//! nodes already have whitespace control applied. Every node keeps a [`Span`]
//! that points into the template source.

/// A location in the template source.
///
/// `start` and `end` are byte offsets. `line` and `column` are 1-based and
/// describe `start`. Columns count UTF-16 code units, as LiquidJS does.
///
/// Nodes parsed from inside a quoted partial name, such as the
/// `{{ name }}` in `{% include 'x-{{ name }}' %}`, all carry the span of
/// that quoted name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The whitespace-control dashes on a tag or output: `{%-` / `{{-` set
/// `left`, `-%}` / `-}}` set `right`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Trim {
    pub left: bool,
    pub right: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text(Text),
    Output(Output),
    Tag(Box<Tag>),
}

impl Node {
    pub fn span(&self) -> Span {
        match self {
            Node::Text(t) => t.span,
            Node::Output(o) => o.span,
            Node::Tag(t) => t.span,
        }
    }
}

/// Literal template text.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// The whole text run in the source, before trimming.
    pub span: Span,
    /// The text to output, after whitespace control.
    pub text: String,
}

/// `{{ value | filter }}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub span: Span,
    pub trim: Trim,
    pub value: FilteredValue,
}

/// A tag. For block tags, `span` covers only the opening tag.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub span: Span,
    pub trim: Trim,
    /// The tag name as written, such as `if` or `#`.
    pub name: String,
    pub kind: TagKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagKind {
    /// `if` / `elsif` / `else`. The first branch is the `if`.
    If {
        branches: Vec<Branch>,
        else_body: Option<Vec<Node>>,
    },
    /// `unless` / `elsif` / `else`. The first branch renders when its
    /// condition is falsy. Later branches come from `elsif` and render when
    /// their condition is truthy, as in LiquidJS.
    Unless {
        branches: Vec<Branch>,
        else_body: Vec<Node>,
    },
    Case {
        value: FilteredValue,
        whens: Vec<When>,
        else_body: Vec<Node>,
    },
    For(For),
    Tablerow(Tablerow),
    Break,
    Continue,
    Assign {
        name: String,
        value: FilteredValue,
    },
    Capture {
        name: String,
        body: Vec<Node>,
    },
    /// The name can be empty: `{% increment %}` is valid.
    Increment {
        name: String,
    },
    Decrement {
        name: String,
    },
    Cycle {
        group: Option<Expr>,
        candidates: Vec<FilteredValue>,
    },
    Include(Include),
    Render(Render),
    Layout(Layout),
    Block {
        name: String,
        body: Vec<Node>,
    },
    /// `{% raw %}`: the body text exactly as written.
    Raw {
        text: String,
    },
    Comment,
    /// `{% # ... %}`.
    InlineComment,
    Echo {
        value: Option<FilteredValue>,
    },
    /// `{% liquid %}`: one tag per line.
    Liquid {
        body: Vec<Node>,
    },
    /// A block tag the embedder added with
    /// [`crate::Engine::register_raw_block`]: its arguments and its body,
    /// each exactly as written.
    RawBlock {
        args: String,
        body: String,
    },
    /// A shortcode the embedder added with [`crate::Engine::register_shortcode`]: its
    /// arguments, and its body when paired.
    Shortcode {
        args: Vec<Expr>,
        body: Option<Vec<Node>>,
    },
}

/// One `if`, `elsif` or `unless` branch.
#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    /// The `if`, `elsif` or `unless` tag.
    pub span: Span,
    pub condition: FilteredValue,
    pub body: Vec<Node>,
}

/// One `when` branch of a `case`.
#[derive(Debug, Clone, PartialEq)]
pub struct When {
    pub span: Span,
    pub values: Vec<Expr>,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct For {
    pub variable: String,
    pub collection: Expr,
    /// The collection as written. LiquidJS keys `offset:continue` on it and
    /// shows it as `forloop.name`.
    pub collection_text: String,
    /// `limit`, `offset`, `reversed` and any other `name: value` pairs, in
    /// source order.
    pub args: Vec<HashArg>,
    pub body: Vec<Node>,
    pub else_body: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tablerow {
    /// Can be empty: LiquidJS does not check it.
    pub variable: String,
    pub collection: Expr,
    pub collection_text: String,
    pub args: Vec<HashArg>,
    pub body: Vec<Node>,
}

/// A `name: value` tag argument. A bare `name` has no value, and LiquidJS
/// treats it as `true`.
#[derive(Debug, Clone, PartialEq)]
pub struct HashArg {
    pub span: Span,
    pub name: String,
    pub value: Option<Expr>,
}

/// The partial named by `include`, `render` or `layout`.
#[derive(Debug, Clone, PartialEq)]
pub enum FilePath {
    /// `none`.
    None,
    /// A quoted name with no Liquid in it, after whitespace control.
    Name(String),
    /// A quoted name with Liquid in it, like `'x-{{ name }}'`. The
    /// renderer renders these nodes to get the name.
    Template(Vec<Node>),
    /// An unquoted name, evaluated as a value.
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Include {
    pub file: FilePath,
    /// `with value`.
    pub with: Option<Expr>,
    pub args: Vec<HashArg>,
}

/// `with value as alias` or `for value as alias` on `render`.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub value: Expr,
    /// The value as written. LiquidJS shows it as `forloop.name`.
    pub text: String,
    /// The `as` name. `Some("")` when `as` has no name after it.
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Render {
    pub file: FilePath,
    pub with: Option<Binding>,
    pub for_binding: Option<Binding>,
    pub args: Vec<HashArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub file: FilePath,
    pub args: Vec<HashArg>,
    /// Everything after the tag, to the end of the enclosing template or block.
    pub body: Vec<Node>,
}

/// An expression with filters, such as `a == b | upcase`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilteredValue {
    pub span: Span,
    /// The value as written, from where LiquidJS starts reading it. `cycle`
    /// uses this text to tell cycles apart.
    pub text: String,
    pub expr: Expr,
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub span: Span,
    pub name: String,
    pub args: Vec<FilterArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterArg {
    Positional(Expr),
    /// `name: value`. The name is the key as written, so `'k': v` has the
    /// name `'k'`. The value is missing for `name:` with nothing after it.
    Keyword {
        name: String,
        value: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal {
        span: Span,
        value: Literal,
    },
    /// A variable or property path. With no `root`, the first segment names
    /// a variable in scope. With a `root`, the segments read properties of a
    /// literal or range, as in `'abc'.size`.
    Path {
        span: Span,
        root: Option<Box<Expr>>,
        segments: Vec<Segment>,
    },
    /// `(start..end)`.
    Range {
        span: Span,
        start: Box<Expr>,
        end: Box<Expr>,
    },
    Binary {
        span: Span,
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Not {
        span: Span,
        operand: Box<Expr>,
    },
    /// An operand LiquidJS leaves out, as on the left of `{% if == b %}`.
    /// It evaluates to undefined.
    Missing {
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal { span, .. }
            | Expr::Path { span, .. }
            | Expr::Range { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Not { span, .. }
            | Expr::Missing { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    True,
    False,
    /// `nil` or `null`.
    Nil,
    Empty,
    Blank,
    Number(f64),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// `.name`, a leading `name`, or the empty key of `a[]`.
    Key { span: Span, name: String },
    /// `[expr]`.
    Index(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Contains,
    And,
    Or,
}
