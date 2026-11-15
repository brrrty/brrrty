//! Adapter for the `liquid` crate (0.26.11), public API only.
//!
//! `slug`, `slugify` and `url` are 11ty filters, registered as custom filters through the
//! crate's documented `ParseFilter` extension point. They are simplified approximations
//! (no transliteration table), so their fidelity is part of what the score measures.
//! `log` is registered as an identity filter; 11ty prints to the console and returns its input.

use crate::case::Case;
use crate::score::Engine;
use liquid::ParserBuilder;
use liquid::partials::{EagerCompiler, InMemorySource};
use liquid_core::{
    Display_filter, Filter, FilterReflection, ParseFilter, Result as LResult, Runtime, Value,
    ValueView,
};

pub struct LiquidCrate;

fn slugify_npm(input: &str) -> String {
    // Mirrors the npm `slugify` package with `{ lower: true }`: map a few symbols to words,
    // drop characters outside its allowed set, join whitespace with '-'.
    let mut mapped = String::new();
    for c in input.chars() {
        match c {
            '$' => mapped.push_str("dollar"),
            '%' => mapped.push_str("percent"),
            '&' => mapped.push_str("and"),
            '<' => mapped.push_str("less"),
            '>' => mapped.push_str("greater"),
            '|' => mapped.push_str("or"),
            c if c.is_ascii_alphanumeric() || c.is_whitespace() => mapped.push(c),
            c if "_*+~.()'\"!-:@".contains(c) => mapped.push(c),
            _ => {}
        }
    }
    let mut out = String::new();
    for word in mapped.split_whitespace() {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(word);
    }
    out.to_lowercase()
}

fn slugify_modern(input: &str) -> String {
    // Approximates @sindresorhus/slugify: lowercase, "&" becomes "and", runs of
    // non-alphanumerics become one '-'. Non-ASCII letters are dropped (no transliteration).
    let lowered = input.replace('&', " and ").to_lowercase();
    let mut out = String::new();
    let mut pending = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            if pending && !out.is_empty() {
                out.push('-');
            }
            pending = false;
            out.push(c);
        } else {
            pending = true;
        }
    }
    out
}

fn url_filter(input: &str) -> String {
    if input.is_empty() {
        ".".to_string()
    } else {
        input.to_string()
    }
}

macro_rules! simple_filter {
    ($parser:ident, $parsed:ident, $name:literal, $f:expr) => {
        #[derive(Clone, ParseFilter, FilterReflection)]
        #[filter(
                    name = $name,
                    description = "11ty filter approximation",
                    parsed($parsed)
                )]
        pub struct $parser;

        #[derive(Debug, Default, Display_filter)]
        #[name = $name]
        struct $parsed;

        impl Filter for $parsed {
            fn evaluate(&self, input: &dyn ValueView, _runtime: &dyn Runtime) -> LResult<Value> {
                let f: fn(&str) -> String = $f;
                Ok(Value::scalar(f(&input.to_kstr())))
            }
        }
    };
}

simple_filter!(Slug, SlugFilter, "slug", slugify_npm);
simple_filter!(Slugify, SlugifyFilter, "slugify", slugify_modern);
simple_filter!(Url, UrlFilter, "url", url_filter);
simple_filter!(Log, LogFilter, "log", |s| s.to_string());

type Partials = EagerCompiler<InMemorySource>;

fn builder() -> ParserBuilder {
    ParserBuilder::with_stdlib()
        .filter(Slug)
        .filter(Slugify)
        .filter(Url)
        .filter(Log)
}

impl Engine for LiquidCrate {
    fn name(&self) -> String {
        "liquid-0.26.11".into()
    }

    fn render(&self, case: &Case) -> Result<String, String> {
        let mut partials = Partials::empty();
        for (name, content) in &case.includes {
            partials.add(name.clone(), content.clone());
        }
        let parser = builder()
            .partials(partials)
            .build()
            .map_err(|e| e.to_string())?;
        let template = parser.parse(&case.template).map_err(|e| e.to_string())?;
        let globals = liquid::to_object(&case.data).map_err(|e| e.to_string())?;
        template.render(&globals).map_err(|e| e.to_string())
    }

    fn parse(&self, source: &str) -> Result<(), String> {
        let parser = builder().build().map_err(|e| e.to_string())?;
        parser.parse(source).map(|_| ()).map_err(|e| e.to_string())
    }
}
