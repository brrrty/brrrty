//! Prism-compatible syntax highlighting for Brrrty, an Eleventy-compatible
//! static site generator: the grammars of Prism 1.30.0 as
//! @11ty/eleventy-plugin-syntaxhighlight 5.0.2 loads them ([`grammar`]),
//! and their regular expressions translated exactly to `fancy-regex`
//! ([`regex`]). A language with a regular expression that can't be
//! translated exactly is unsupported.

pub mod grammar;
pub mod regex;
