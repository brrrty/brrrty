//! A Liquid template engine that behaves like LiquidJS 10.30.1 as 11ty 3.1.8
//! configures it.
//!
//! [`parse`] turns a template into a [`Template`]. Parse errors carry the
//! LiquidJS message and the line and column LiquidJS reports, plus a hint
//! where one helps.

pub mod ast;
mod error;
mod lexer;
mod parser;

pub use ast::Template;
pub use error::{ErrorKind, LiquidError};

/// Parses `source`. `template_name` names the template in errors.
pub fn parse(template_name: &str, source: &str) -> Result<Template, LiquidError> {
    let mut parser = parser::Parser::new();
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
