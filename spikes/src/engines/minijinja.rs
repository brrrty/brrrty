//! Adapter for the `minijinja` crate (3.0.5), public API only.
//!
//! Settings, chosen to match what 11ty's Nunjucks (`{ dev: true }`, otherwise nunjucks
//! defaults) does with the reference cases:
//! - `UndefinedBehavior::Chainable`: nunjucks prints undefined as empty, iterates it as empty
//!   and lets `a.b.c` chain through undefined. `Lenient` (the default) fails on `a.b` when `a`
//!   is undefined; `Strict`/`SemiStrict` fail on printing. Only `Chainable` matches.
//! - Auto escape always on (`AutoEscape::Html` for every template name): nunjucks'
//!   `autoescape` option defaults to true for all templates. The crate's default only escapes
//!   names ending in .html/.htm/.xml, and `render_str` uses the name `<string>`.
//! - `keep_trailing_newline`: nunjucks keeps a template's final newline, the crate strips it.
//!
//! Includes come from the case through `Environment::set_loader`, by exact name (nunjucks does
//! not append an extension either). `slug`, `slugify`, `url` and `log` are registered as
//! 11ty's Nunjucks filters. They are simplified approximations (see `liquid_crate`).

use crate::case::Case;
use crate::engines::liquid_crate::{slugify_modern, slugify_npm, url_filter};
use crate::score::Engine;
use minijinja::syntax::SyntaxConfig;
use minijinja::value::{Serde, Value};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

type IncludesKey = Vec<(String, String)>;

/// Environments are built once per distinct include set; cases without includes share one.
#[derive(Default)]
pub struct MiniJinja {
    envs: Mutex<HashMap<IncludesKey, Arc<Environment<'static>>>>,
}

impl MiniJinja {
    pub fn new() -> Self {
        Self::default()
    }

    fn env_for(
        &self,
        includes: &BTreeMap<String, String>,
    ) -> Result<Arc<Environment<'static>>, String> {
        let key: IncludesKey = includes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut cache = self.envs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(env) = cache.get(&key) {
            return Ok(env.clone());
        }
        let env = Arc::new(build_env(includes)?);
        cache.insert(key, env.clone());
        Ok(env)
    }
}

fn build_env(includes: &BTreeMap<String, String>) -> Result<Environment<'static>, String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    let syntax = SyntaxConfig::builder()
        .keep_trailing_newline(true)
        .build()
        .map_err(|e| e.to_string())?;
    env.set_syntax(syntax);
    let files = includes.clone();
    env.set_loader(move |name| Ok(files.get(name).cloned()));
    env.add_filter("slug", |s: String| slugify_npm(&s));
    env.add_filter("slugify", |s: String| slugify_modern(&s));
    env.add_filter("url", |s: String| url_filter(&s));
    env.add_filter("log", |v: Value| v);
    Ok(env)
}

/// The error text plus the line number when the crate reports one.
fn describe(e: &minijinja::Error) -> String {
    let mut msg = format!("{e:#}");
    if let Some(line) = e.line() {
        msg.push_str(&format!(" [line {line}]"));
    }
    msg
}

impl Engine for MiniJinja {
    fn name(&self) -> String {
        "minijinja-3.0.5".into()
    }

    fn render(&self, case: &Case) -> Result<String, String> {
        let env = self.env_for(&case.includes)?;
        env.render_str(&case.template, Value::from(Serde(&case.data)))
            .map_err(|e| describe(&e))
    }

    fn parse(&self, source: &str) -> Result<(), String> {
        let env = self.env_for(&BTreeMap::new())?;
        env.template_from_str(source)
            .map(|_| ())
            .map_err(|e| describe(&e))
    }
}
