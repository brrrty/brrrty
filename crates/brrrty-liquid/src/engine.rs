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

pub use brrrty_js::Tz;

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

impl FilterCtx {
    /// The context a filter sees when Liquid's time zone is `timezone`,
    /// for calling a filter outside a render.
    pub fn new(timezone: Tz) -> Self {
        Self { timezone }
    }
}

/// A filter: `(input, positional arguments, keyword arguments, context)`.
///
/// Keyword names are as written, so `'k': v` arrives as `'k'`. A keyword
/// with nothing after its colon has the value `Undefined`. An `Err` becomes
/// a render error at the tag or output that used the filter.
pub type FilterFn = Arc<
    dyn Fn(&Value, &[Value], &[(String, Value)], &FilterCtx) -> Result<Value, String> + Send + Sync,
>;

/// A filter written the way 11ty passes arguments to a JavaScript filter:
/// `(input, arguments in the order written, context)`. A keyword argument
/// keeps its place and arrives as a two-item array `[name, value]`, where
/// `name` is as written (`'k'` keeps its quotes) and a keyword with
/// nothing after its colon has the value `undefined`. An `Err` becomes a
/// render error at the tag or output that used the filter.
pub type ArgsFilterFn =
    Arc<dyn Fn(&Value, &[Value], &FilterCtx) -> Result<Value, String> + Send + Sync>;

/// Why a [`Loader`] could not supply a partial.
///
/// `io` is `None` when no root has the file: the partial is missing, and
/// `tried` lists every path looked at. `io` is `Some` when a file was found
/// but could not be used: it can't be read, isn't UTF-8, or resolves
/// outside the loader's roots.
#[derive(Debug, Default)]
pub struct LoadError {
    pub tried: Vec<String>,
    pub io: Option<std::io::Error>,
}

/// Finds partials by name. Returns the resolved name and the source.
///
/// The engine does the name handling LiquidJS does: it appends `.liquid`
/// when the name has no extension (Node's `path.extname` rule), then calls
/// `load` with that file name, as written otherwise (`sub/page.md`,
/// `./x.liquid`). The loader only maps a file name to a file: it tries each
/// of its roots in order (11ty's are the includes folder, then the input
/// folder), refuses names that leave a root, and returns the path it found
/// and the source. Partials are told apart by that path, so it should be
/// canonical: a cycle is then found however the name is spelled.
pub trait Loader: Send + Sync {
    fn load(&self, name: &str) -> Result<(String, String), LoadError>;
}

/// A render's output, and the variables it read that were not defined, in
/// sorted order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub output: String,
    pub undefined: Vec<String>,
}

/// A registered filter: one the embedder added, or one of the standard set.
#[derive(Clone)]
pub(crate) enum Filter {
    User(FilterFn),
    /// A filter that takes keyword arguments in place, as 11ty's JS
    /// filters do.
    Args(ArgsFilterFn),
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

    /// Adds or replaces the filter `name` with one that gets its arguments
    /// as 11ty gives them to a JavaScript filter (see [`ArgsFilterFn`]).
    pub fn register_args_filter(&mut self, name: &str, f: ArgsFilterFn) {
        self.filters.insert(name.to_string(), Filter::Args(f));
    }

    /// Adds an engine-neutral filter, such as one of Eleventy's. Keyword
    /// arguments are not passed to it. It sees the engine's time zone.
    pub fn register_filter_impl(&mut self, name: &str, f: brrrty_js::FilterImpl) {
        self.register_filter(
            name,
            Arc::new(move |input, args, _kwargs, ctx| {
                f(input, args, &brrrty_js::FilterEnv::new(ctx.timezone))
            }),
        );
    }

    /// Adds Eleventy's own filters: `slug`, `slugify`, `url` and `log`.
    /// `slugify` replaces the standard Liquid filter of that name, as it
    /// does in Eleventy.
    pub fn register_eleventy_filters(&mut self) {
        for (name, f) in brrrty_js::eleventy_filters() {
            self.register_filter_impl(name, f);
        }
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

    /// Renders `t` like [`Engine::render`], and also reports the variables
    /// it looked up that neither the template nor `data` defines. Lookups
    /// inside `{% render %}` partials are not counted: those partials never
    /// see the data.
    pub fn render_tracked(
        &self,
        t: &Template,
        data: &Value,
        loader: &dyn Loader,
    ) -> Result<Rendered, LiquidError> {
        Renderer::new(self, &t.name, data, loader).render_tracked(&t.nodes)
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
