//! Prism-compatible syntax highlighting for Brrrty, an Eleventy-compatible
//! static site generator: the grammars of Prism 1.30.0 as
//! @11ty/eleventy-plugin-syntaxhighlight 5.0.2 loads them ([`grammar`]),
//! and their regular expressions translated exactly to `fancy-regex`
//! ([`regex`]). A language with a regular expression that can't be
//! translated exactly is unsupported. [`tokenize`] ports Prism's tokenizer,
//! and [`highlight`] its `Prism.highlight` with the hooks the plugin's
//! components add ([`stringify`] writes the HTML).

pub mod grammar;
pub mod highlight;
pub mod regex;
pub mod stringify;
pub mod tokenize;

pub use highlight::{AllLoaded, Languages, highlight};
pub use tokenize::{Refusal, tokenize};
