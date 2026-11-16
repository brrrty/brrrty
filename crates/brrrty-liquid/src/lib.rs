//! A Liquid template engine that behaves like LiquidJS 10.30.1 as 11ty 3.1.8
//! configures it.
//!
//! [`parse`] turns a template into a [`Template`]. Parse errors carry the
//! LiquidJS message and the line and column LiquidJS reports, plus a hint
//! where one helps. An [`Engine`] holds the filters and options, checks
//! filters as it parses ([`Engine::parse`]) and renders templates
//! ([`Engine::render`]).

pub mod ast;
mod context;
mod engine;
mod error;
mod filters;
mod lexer;
mod ops;
mod parser;
mod render;
mod tags;

pub use ast::Template;
pub use engine::{
    ArgsFilterFn, Engine, EngineOptions, FilterCtx, FilterFn, LoadError, Loader, Rendered, Tz,
};
pub use error::{ErrorKind, LiquidError};

/// Parses `source`. `template_name` names the template in errors.
///
/// This checks syntax only. [`Engine::parse`] also rejects unknown filters
/// when `strict_filters` is on.
pub fn parse(template_name: &str, source: &str) -> Result<Template, LiquidError> {
    parse_checked(template_name, source, None)
}

pub(crate) fn parse_checked(
    template_name: &str,
    source: &str,
    check: parser::Check<'_>,
) -> Result<Template, LiquidError> {
    let mut parser = parser::Parser::new(check);
    match parser.parse_source(source) {
        Ok(nodes) => Ok(Template {
            name: template_name.to_string(),
            nodes,
        }),
        Err(e) => {
            let (line, column) = lexer::LineIndex::new(source).position(e.offset);
            Err(LiquidError {
                kind: e.kind,
                message: e.message,
                template: template_name.to_string(),
                line,
                column,
                help: e.help,
            })
        }
    }
}
