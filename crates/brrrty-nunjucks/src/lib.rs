//! A Nunjucks template engine that behaves like Nunjucks 3.2.4 as 11ty
//! 3.1.8 configures it: `new Environment(loader, { dev: true })`, so
//! `trimBlocks`, `lstripBlocks` and custom delimiters are off.
//!
//! [`parse`] turns a template into a [`Template`]. Parse errors carry the
//! Nunjucks message and the line and column Nunjucks reports. An
//! [`Engine`] compiles templates ([`Engine::compile`]) and renders them
//! ([`Engine::render`]), with JavaScript's semantics for what a template
//! computes and Nunjucks's autoescaping.

pub mod ast;
mod compile;
mod context;
mod engine;
mod error;
mod filters;
mod jsops;
mod lexer;
mod parser;
mod render;
mod tags;
mod tests;
mod value;

pub use ast::Template;
pub use brrrty_js::{LoadError, Loader, Tz};
pub use engine::{Compiled, Engine, EngineOptions, FilterCtx, FilterFn};
pub use error::{ErrorKind, NunjucksError};
pub use render::Rendered;

/// Parses `source`. `template_name` names the template in errors.
///
/// This is Nunjucks's parser. The checks Nunjucks makes when it compiles
/// the parsed template, such as for unknown filters, come later.
pub fn parse(template_name: &str, source: &str) -> Result<Template, NunjucksError> {
    match parser::parse(source) {
        Ok(nodes) => Ok(Template {
            name: template_name.to_string(),
            nodes,
        }),
        Err(e) => Err(e.named(template_name)),
    }
}
