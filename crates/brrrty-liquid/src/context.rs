//! Render-time values and state: LiquidJS's drops, scopes and registers.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::{Value, js_number_to_string};

use crate::lexer::json_string;

/// A value during rendering. LiquidJS keeps `nil`, `empty`, `blank`,
/// `forloop` and `block` as drops, objects with their own comparison and
/// printing rules, until a tag or filter needs a plain value.
#[derive(Debug, Clone)]
pub(crate) enum Val {
    V(Value),
    /// `nil` and `null`: equal to null and undefined only, and never
    /// greater or less than anything.
    Nil,
    /// `empty`: equal to empty strings, arrays and objects, but not to
    /// another `empty`.
    Empty,
    /// `blank`: like `empty`, and also equal to `false`, null, undefined
    /// and whitespace-only strings.
    Blank,
    /// `forloop` or `tablerowloop`. Shared, so a copy made by `assign`
    /// sees the loop advance.
    Loop(Rc<LoopDrop>),
    /// `block` inside `{% block %}`.
    Block,
}

impl Val {
    pub fn undefined() -> Val {
        Val::V(Value::Undefined)
    }

    pub fn string(s: impl Into<Arc<str>>) -> Val {
        Val::V(Value::String(s.into()))
    }

    pub fn number(n: f64) -> Val {
        Val::V(Value::Number(n))
    }

    pub fn bool(b: bool) -> Val {
        Val::V(Value::Bool(b))
    }

    /// LiquidJS's `toValue`: a drop's `valueOf()`.
    pub fn to_value(&self) -> Value {
        match self {
            Val::V(v) => v.clone(),
            Val::Nil => Value::Null,
            Val::Empty | Val::Blank => Value::String("".into()),
            Val::Loop(d) => Value::String(d.json().into()),
            // BlockDrop's valueOf is Object.prototype.valueOf: the drop itself.
            Val::Block => Value::Object(Default::default()),
        }
    }

    /// Liquid truthiness: only `false`, null and undefined are falsy.
    pub fn truthy(&self) -> bool {
        self.to_value().liquid_truthy()
    }

    /// JavaScript truthiness of the value as it is, so every drop is
    /// truthy. This is what `||` sees in LiquidJS's tag code.
    pub fn js_truthy(&self) -> bool {
        match self {
            Val::V(v) => v.js_truthy(),
            _ => true,
        }
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Val::V(Value::Undefined))
    }

    /// Whether this is one of LiquidJS's comparable drops, which decide
    /// `==`, `<` and the rest themselves.
    pub fn is_comparable(&self) -> bool {
        matches!(self, Val::Nil | Val::Empty | Val::Blank)
    }

    /// `JSON.stringify(v)`, where `undefined` gives `None`.
    pub fn json(&self) -> Option<String> {
        match self {
            Val::V(v) => json(v),
            Val::Loop(d) => Some(d.json()),
            // Drops have no own enumerable properties worth printing.
            Val::Nil | Val::Empty | Val::Blank | Val::Block => Some("{}".into()),
        }
    }
}

/// LiquidJS's `ForloopDrop` and `TablerowloopDrop`.
#[derive(Debug)]
pub(crate) struct LoopDrop {
    pub i: Cell<usize>,
    pub length: usize,
    /// `variable-collection`, as written.
    pub name: String,
    /// `Some` for `tablerowloop`.
    pub cols: Option<Val>,
}

impl LoopDrop {
    pub fn new(length: usize, name: String, cols: Option<Val>) -> LoopDrop {
        LoopDrop {
            i: Cell::new(0),
            length,
            name,
            cols,
        }
    }

    pub fn next(&self) {
        self.i.set(self.i.get() + 1);
    }

    fn cols_number(&self) -> f64 {
        self.cols
            .as_ref()
            .map_or(f64::NAN, |c| c.to_value().to_js_number())
    }

    /// `Math.floor(i / cols) + 1`.
    pub fn row(&self) -> f64 {
        (self.i.get() as f64 / self.cols_number()).floor() + 1.0
    }

    /// `i % cols`.
    pub fn col0(&self) -> f64 {
        self.i.get() as f64 % self.cols_number()
    }

    /// A property, as `readProperty` finds it on the drop: own fields and
    /// methods, which it calls. Anything else is undefined.
    pub fn get(&self, key: &str) -> Val {
        let i = self.i.get() as f64;
        let len = self.length as f64;
        let tablerow = self.cols.is_some();
        match key {
            "i" => Val::number(i),
            "length" => Val::number(len),
            "name" => Val::string(self.name.as_str()),
            "index0" => Val::number(i),
            "index" => Val::number(i + 1.0),
            "first" => Val::bool(i == 0.0),
            "last" => Val::bool(i == len - 1.0),
            "rindex" => Val::number(len - i),
            "rindex0" => Val::number(len - i - 1.0),
            "next" => {
                self.next();
                Val::undefined()
            }
            "valueOf" => Val::string(self.json()),
            "cols" if tablerow => self.cols.clone().unwrap_or_else(Val::undefined),
            "row" if tablerow => Val::number(self.row()),
            "col0" if tablerow => Val::number(self.col0()),
            "col" if tablerow => Val::number(self.col0() + 1.0),
            "col_first" if tablerow => Val::bool(self.col0() == 0.0),
            "col_last" if tablerow => Val::bool(matches!(
                self.cols,
                Some(Val::V(Value::Number(c))) if self.col0() + 1.0 == c
            )),
            _ => Val::undefined(),
        }
    }

    /// `JSON.stringify(this)`, which is also its `valueOf()`.
    pub fn json(&self) -> String {
        let mut out = format!(
            "{{\"i\":{},\"length\":{},\"name\":{}",
            self.i.get(),
            self.length,
            json_str(&self.name)
        );
        if let Some(cols) = &self.cols
            && let Some(c) = cols.json()
        {
            out.push_str(",\"cols\":");
            out.push_str(&c);
        }
        out.push('}');
        out
    }
}

fn json_str(s: &str) -> String {
    json_string(&s.encode_utf16().collect::<Vec<_>>())
}

/// `JSON.stringify(v)` without spacing. `None` for undefined, and for a
/// value holding a collection item, which can't be read whole.
pub fn json(v: &Value) -> Option<String> {
    Some(match v {
        Value::Undefined => return None,
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) if n.is_finite() => js_number_to_string(*n),
        Value::Number(_) => "null".into(),
        Value::String(s) => json_str(s),
        Value::Array(a) => {
            let items: Vec<String> = a
                .iter()
                .map(|x| json(x).unwrap_or_else(|| "null".into()))
                .collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(o) => {
            // A trap (a collection item) refuses the page: stop there.
            let items: Vec<String> = Value::js_entries_or_trap(o)?
                .into_iter()
                .filter_map(|(k, v)| Some(format!("{}:{}", json_str(k), json(v)?)))
                .collect();
            format!("{{{}}}", items.join(","))
        }
        Value::Date(d) => format!(
            "\"{}\"",
            d.with_timezone(&chrono::Utc)
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        ),
    })
}

/// Rendering state: LiquidJS's `Context`.
pub(crate) struct Context<'a> {
    /// `scopes[0]` is where `assign` and `capture` write. Later scopes are
    /// pushed by loops and blocks.
    pub scopes: Vec<HashMap<String, Val>>,
    /// The render's data: LiquidJS's `environments`.
    pub data: &'a Value,
    /// What `increment` and `decrement` wrote into the data. LiquidJS
    /// writes into the data object itself; this keeps the caller's data
    /// unchanged.
    pub env: HashMap<String, Value>,
    /// `cycle` positions by fingerprint.
    pub cycles: HashMap<String, usize>,
    /// `offset: continue` positions by `continue-variable-collection`.
    pub continues: HashMap<String, Value>,
    pub break_called: bool,
    pub continue_called: bool,
    /// Bytes written so far, captures included.
    pub written: usize,
    /// Variables looked up that no scope and no data has.
    pub undefined: RefCell<BTreeSet<String>>,
}

impl<'a> Context<'a> {
    pub fn new(data: &'a Value) -> Context<'a> {
        Context {
            scopes: vec![HashMap::new()],
            data,
            env: HashMap::new(),
            cycles: HashMap::new(),
            continues: HashMap::new(),
            break_called: false,
            continue_called: false,
            written: 0,
            undefined: RefCell::new(BTreeSet::new()),
        }
    }

    /// A variable, as LiquidJS's `findScope` and `readProperty` find it:
    /// the innermost scope that has the key, then the data, then the empty
    /// `globals` object, whose `size` is 0.
    pub fn variable(&self, key: &Value) -> Val {
        let k = key.to_js_string();
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(&k) {
                return v.clone();
            }
        }
        if let Some(v) = self.data_get(&k) {
            // A marker for data Brrrty doesn't have yet counts as undefined.
            if matches!(v, Value::Undefined) {
                self.undefined.borrow_mut().insert(k);
            }
            return Val::V(v);
        }
        // Recorded even for `size`, so a data key of that name can't be
        // shadowed without the caller knowing.
        let is_size = matches!(key, Value::String(s) if &**s == "size");
        self.undefined.borrow_mut().insert(k);
        if is_size {
            return Val::number(0.0);
        }
        Val::undefined()
    }

    /// An own property of the data, after `increment` and `decrement`.
    pub fn data_get(&self, key: &str) -> Option<Value> {
        if let Some(v) = self.env.get(key) {
            return Some(v.clone());
        }
        match self.data {
            // A sub-evaluation over a collection item reads its content.
            Value::Object(o) => brrrty_js::traps::read_top(self.data, key, o.get(key).cloned()),
            // A filter can use a string or an array as the data, as
            // LiquidJS's `Object(value)` does: `length` and indexes are
            // its own properties.
            Value::Null | Value::Undefined => None,
            other => crate::ops::own_property(other, key),
        }
    }

    pub fn clear_flags(&mut self) {
        self.break_called = false;
        self.continue_called = false;
    }

    pub fn interrupted(&self) -> bool {
        self.break_called || self.continue_called
    }
}
