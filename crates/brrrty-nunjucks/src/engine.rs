//! The public engine: options, filters, globals, and compile and render
//! entry points.

use std::collections::HashMap;
use std::sync::Arc;

use brrrty_js::{Loader, Tz, Value};

use crate::compile::Program;
use crate::error::NunjucksError;
use crate::render::{Rendered, Renderer};

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// The zone a date is printed in, as `Date.prototype.toString` prints
    /// it in the process's zone.
    pub timezone: Tz,
    /// Nunjucks's `autoescape`, which 11ty leaves on.
    pub autoescape: bool,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions {
            timezone: Tz::Local,
            autoescape: true,
        }
    }
}

/// A filter: `(input, arguments, what the filter can see)`. It is the
/// shared [`brrrty_js::FilterFn`], so one filter can serve Liquid and
/// Nunjucks.
///
/// The arguments are the ones written, in order. Keyword arguments arrive
/// as a last object argument that also has `__keywords: true`, as
/// Nunjucks passes them to a JavaScript filter, and again, taken apart,
/// in [`FilterEnv::kwargs`]. A `SafeString` input or argument arrives as
/// its text, with [`FilterEnv::input_safe`] or [`FilterEnv::args_safe`]
/// set. An `Err` becomes a render error where the filter was used.
pub type FilterFn = brrrty_js::FilterFn;

/// A compiled template, ready to render.
#[derive(Clone)]
pub struct Compiled {
    pub(crate) program: Arc<Program>,
}

impl Compiled {
    /// The name errors show.
    pub fn name(&self) -> &str {
        &self.program.name
    }
}

pub struct Engine {
    pub(crate) options: EngineOptions,
    pub(crate) filters: HashMap<String, FilterFn>,
    pub(crate) globals: HashMap<String, Value>,
    /// Bytes a render may write in all, captures and macros included.
    pub(crate) output_limit: usize,
    /// The most items a loop or `range()` may have.
    pub(crate) item_limit: usize,
    /// The most loop iterations a render may run in all, nested loops,
    /// includes and macros together.
    pub(crate) step_limit: usize,
}

/// The most a render may write: 512 MiB.
const OUTPUT_LIMIT: usize = 512 * 1024 * 1024;

impl Engine {
    pub fn new(options: EngineOptions) -> Engine {
        Engine {
            options,
            filters: HashMap::new(),
            globals: HashMap::new(),
            output_limit: OUTPUT_LIMIT,
            item_limit: crate::render::MAX_ITEMS,
            step_limit: crate::render::MAX_STEPS,
        }
    }

    pub fn options(&self) -> &EngineOptions {
        &self.options
    }

    /// Adds or replaces the filter `name`. It takes the place of a
    /// Nunjucks filter of the same name, as `addFilter` does.
    pub fn register_filter(&mut self, name: &str, f: FilterFn) {
        self.filters.insert(name.to_string(), f);
    }

    /// Adds an engine-neutral filter, such as one of Eleventy's.
    pub fn register_filter_impl(&mut self, name: &str, f: brrrty_js::FilterImpl) {
        self.register_filter(name, Arc::new(f));
    }

    /// Adds Eleventy's own filters that Nunjucks can run: `slug`,
    /// `slugify`, `url` and `log`.
    ///
    /// `slug` and `slugify` take an options object as a second argument,
    /// which a Nunjucks call can write (`{{ t | slugify({lower: false}) }}`,
    /// or `lower=false`). Brrrty's versions have no options, so a call that
    /// passes an object is refused.
    ///
    /// Eleventy's other filters are not added to either engine:
    /// `getCollectionItemIndex`, `getCollectionItem`,
    /// `getPreviousCollectionItem`, `getNextCollectionItem`,
    /// `renderTransforms` (async) and `inputPathToUrl`.
    pub fn register_eleventy_filters(&mut self) {
        for (name, f) in brrrty_js::eleventy_filters() {
            if name == "slug" || name == "slugify" {
                self.register_filter(
                    name,
                    Arc::new(move |input, args, env| {
                        if args.iter().any(|a| matches!(a, Value::Object(_))) {
                            return Err(format!(
                                "Brrrty doesn't support options for the `{name}` filter yet"
                            ));
                        }
                        f(input, args, env)
                    }),
                );
            } else {
                self.register_filter_impl(name, f);
            }
        }
    }

    /// Adds a global variable, as `addGlobal` does. The context's own
    /// variables hide it.
    pub fn add_global(&mut self, name: &str, value: Value) {
        self.globals.insert(name.to_string(), value);
    }

    /// Whether a template can use the filter `name`: one added with
    /// [`Engine::register_filter`] or one of Nunjucks's own.
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.contains_key(name) || crate::filters::is_builtin(name)
    }

    /// Parses and compiles `source`. `name` names the template in errors.
    pub fn compile(&self, name: &str, source: &str) -> Result<Compiled, NunjucksError> {
        Ok(Compiled {
            program: self.compile_program(name, None, source)?,
        })
    }

    /// Like [`Engine::compile`], for a template at `path`: names starting
    /// `./` or `../` in it resolve against `path`, as Nunjucks resolves
    /// them against a template's path. Give an absolute path, so the
    /// resolved names are absolute too.
    pub fn compile_at(
        &self,
        name: &str,
        path: &str,
        source: &str,
    ) -> Result<Compiled, NunjucksError> {
        Ok(Compiled {
            program: self.compile_program(name, Some(path), source)?,
        })
    }

    pub(crate) fn compile_program(
        &self,
        name: &str,
        path: Option<&str>,
        source: &str,
    ) -> Result<Arc<Program>, NunjucksError> {
        let t = crate::parse(name, source)?;
        let program = crate::compile::compile(name, path, &t.nodes).map_err(|e| e.named(name))?;
        Ok(Arc::new(program))
    }

    /// Renders `t` with `data` as its context. Templates it includes,
    /// imports or extends come from `loader`.
    pub fn render(
        &self,
        t: &Compiled,
        data: &Value,
        loader: &dyn Loader,
    ) -> Result<String, NunjucksError> {
        self.render_tracked(t, data, loader).map(|r| r.output)
    }

    /// Renders `t` like [`Engine::render`], and also reports the variables
    /// it looked up that neither the template nor `data` defines.
    pub fn render_tracked(
        &self,
        t: &Compiled,
        data: &Value,
        loader: &dyn Loader,
    ) -> Result<Rendered, NunjucksError> {
        Renderer::new(self, loader).render(&t.program, data)
    }
}
