//! Prism-compatible syntax highlighting for Brrrty, an Eleventy-compatible
//! static site generator: the grammars of Prism 1.30.0 as
//! @11ty/eleventy-plugin-syntaxhighlight 5.0.2 loads them ([`grammar`]),
//! and their regular expressions translated exactly to `fancy-regex`
//! ([`regex`]). A language with a regular expression that can't be
//! translated exactly is unsupported. [`tokenize`] ports Prism's tokenizer,
//! and [`highlight`] its `Prism.highlight` with the hooks the plugin's
//! components add ([`stringify`] writes the HTML).
//!
//! The grammars and the test probes are made by the Node scripts in the
//! repository's `crates/brrrty-prism/tools`; its README says how to move
//! to a new Prism.

pub mod grammar;
pub mod highlight;
pub use brrrty_jsregex::backtrack as regex;
pub mod stringify;
pub mod tokenize;

pub use highlight::{AllLoaded, Languages, highlight};
pub use tokenize::{Refusal, tokenize};
