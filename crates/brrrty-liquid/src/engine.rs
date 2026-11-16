//! The public engine: options, the filter registry, and parse and render
//! entry points.

use std::collections::HashMap;
use std::sync::Arc;

use brrrty_js::Value;

use crate::ast::FilteredValue;
use crate::context::Val;
use crate::error::{ErrorKind, ParseErr};
use crate::parser::distance;
use crate::render::{R, Renderer};
use crate::{LiquidError, Template};

/// The time zone dates are printed and formatted in. LiquidJS uses the
/// process's local zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    /// The machine's local zone, which honors `TZ`.
    Local,
    Utc,
}

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Reject unknown filters, as 11ty configures LiquidJS. Templates from
    /// [`Engine::parse`] are checked when they are parsed. Others are
    /// checked when a filter is used.
    pub strict_filters: bool,
    pub timezone: Tz,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions {
            strict_filters: true,
            timezone: Tz::Local,
        }
    }
}

/// What a filter can see of the engine.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FilterCtx {
    pub timezone: Tz,
}

/// A filter: `(input, positional arguments, keyword arguments, context)`.
///
/// Keyword names are as written, so `'k': v` arrives as `'k'`. A keyword
/// with nothing after its colon has the value `Undefined`. An `Err` becomes
/// a render error at the tag or output that used the filter.
pub type FilterFn = Arc<
    dyn Fn(&Value, &[Value], &[(String, Value)], &FilterCtx) -> Result<Value, String> + Send + Sync,
>;

/// Finds partials by name. Returns the resolved name and the source.
pub trait Loader: Send + Sync {
    fn load(&self, name: &str) -> Option<(String, String)>;
}

/// A registered filter: one the embedder added, or one of the standard set.
#[derive(Clone)]
pub(crate) enum Filter {
    User(FilterFn),
    /// A standard filter. It gets the renderer, for the time zone, the
    /// size limit and the filters that evaluate expressions, and every
    /// argument as written: a keyword argument arrives as a `[name, value]`
    /// array, as LiquidJS passes it.
    Builtin(Builtin),
}

pub(crate) type Builtin = fn(&mut Renderer<'_>, &Val, &[Val]) -> R<Value>;

pub struct Engine {
    pub(crate) options: EngineOptions,
    pub(crate) filters: HashMap<String, Filter>,
    /// Bytes a render may write in all, captures included.
    pub(crate) output_limit: usize,
}

/// The most a render may write: 512 MiB.
const OUTPUT_LIMIT: usize = 512 * 1024 * 1024;

impl Engine {
    pub fn new(opts: EngineOptions) -> Engine {
        let mut filters = HashMap::new();
        crate::filters::register(&mut filters);
        Engine {
            options: opts,
            filters,
            output_limit: OUTPUT_LIMIT,
        }
    }

    pub fn options(&self) -> &EngineOptions {
        &self.options
    }

    /// Adds or replaces the filter `name`. A new filter replaces a standard
    /// one of the same name.
    pub fn register_filter(&mut self, name: &str, f: FilterFn) {
        self.filters.insert(name.to_string(), Filter::User(f));
    }

    /// Parses `source` and, with `strict_filters`, rejects any filter that
    /// is not registered. The check runs where LiquidJS runs it, so errors
    /// come in source order with LiquidJS's message and position.
    pub fn parse(&self, template_name: &str, source: &str) -> Result<Template, LiquidError> {
        if !self.options.strict_filters {
            return crate::parse(template_name, source);
        }
        let check = |value: &FilteredValue| -> Result<(), ParseErr> {
            for f in &value.filters {
                if !self.filters.contains_key(&f.name) {
                    let (message, help) = self.unknown_filter(&f.name);
                    return Err(ParseErr::Plain {
                        kind: ErrorKind::UnknownFilter,
                        message,
                        help,
                    });
                }
            }
            Ok(())
        };
        crate::parse_checked(template_name, source, Some(&check))
    }

    /// Renders `t` with `data` as its variables.
    pub fn render(
        &self,
        t: &Template,
        data: &Value,
        loader: &dyn Loader,
    ) -> Result<String, LiquidError> {
        Renderer::new(self, &t.name, data, loader).render(&t.nodes)
    }

    /// LiquidJS's message for an unknown filter, and a hint naming the
    /// closest registered filter.
    pub(crate) fn unknown_filter(&self, name: &str) -> (String, Option<String>) {
        let closest = self
            .filters
            .keys()
            .map(|k| (distance(name, k), k.as_str()))
            .filter(|(d, _)| *d <= 2)
            .min();
        (
            format!("undefined filter: {name}"),
            closest.map(|(_, k)| format!("did you mean `{k}`?")),
        )
    }
}
