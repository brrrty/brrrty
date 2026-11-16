//! JavaScript regular expressions, for Brrrty, an Eleventy-compatible static
//! site generator: what Node's syntax means, unit by unit.
//!
//! - [`lex`] reads a pattern into tokens that say how each part was
//!   written, with the web-compatibility grammar (ECMA-262 Annex B) that
//!   Node uses without the `u` flag.
//! - [`ast`] parses those tokens into a tree of UTF-16 units and checks
//!   where a backtracking engine's answer can differ from JavaScript's.
//! - [`classes`] has JavaScript's `\d`, `\s`, `\w`, line terminators and the
//!   case folding of the `i` flag; [`set`] has sets of UTF-16 units.
//!
//! `brrrty-config` uses the lexer and classes to translate script patterns
//! to the `regex` crate, and `brrrty-prism` uses the tree to translate
//! Prism's grammars to `fancy-regex`.

pub mod ast;
mod canon_table;
pub mod classes;
pub mod lex;
pub mod set;
