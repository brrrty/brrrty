//! `config.add_plugin("rss", #{ ... })`: the options of
//! `@11ty/eleventy-plugin-rss`'s `feedPlugin`, read the way
//! `getFeedTemplate` reads them, into the feed's virtual template.

use std::sync::Arc;

use brrrty_core::frontmatter::Data;
use brrrty_core::plugins::rss::{FeedKind, FeedOptions, FeedSort, FeedTemplate};
use brrrty_js::{Tz, Value};
use indexmap::IndexMap;

use crate::convert::deep_copy;

/// The options a script may give, and the names 11ty gives them.
const OPTIONS: [(&str, &str); 8] = [
    ("type", "type"),
    ("output_path", "outputPath"),
    ("input_path", "inputPath"),
    ("collection", "collection"),
    ("metadata", "metadata"),
    ("template_data", "templateData"),
    ("stylesheet", "stylesheet"),
    ("script", "script"),
];

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(Arc::new(
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
    ))
}

/// `getFeedTemplate`'s defaults.
fn defaults() -> Value {
    obj(vec![
        ("type", s("atom")),
        (
            "collection",
            obj(vec![
                ("name", Value::Bool(false)),
                ("limit", Value::Number(0.0)),
                ("sort", s("auto")),
            ]),
        ),
        ("outputPath", s("/feed.xml")),
        ("templateData", obj(vec![])),
        (
            "metadata",
            obj(vec![
                ("title", s("Blog Title")),
                (
                    "subtitle",
                    s("This is a longer description about your blog."),
                ),
                ("language", s("")),
                ("base", s("https://example.com/")),
                (
                    "author",
                    obj(vec![("name", s("Your Name")), ("email", s(""))]),
                ),
            ]),
        ),
    ])
}

fn get(v: &Value, key: &str) -> Value {
    v.get(key)
}

/// A JavaScript `typeof`.
fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Undefined => "undefined",
        Value::Null | Value::Array(_) | Value::Object(_) | Value::Date(_) => "object",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
    }
}

/// 11ty's `slugify` filter.
fn slugify(v: &Value) -> String {
    let f = brrrty_js::eleventy_filters()
        .into_iter()
        .find(|(name, _)| *name == "slugify")
        .map(|(_, f)| f)
        .expect("11ty has a slugify filter");
    match f(v, &[], &brrrty_js::FilterEnv::new(Tz::Local)) {
        Ok(Value::String(s)) => s.to_string(),
        Ok(other) => other.to_js_string(),
        Err(_) => String::new(),
    }
}

/// A name Nunjucks reads after `collections.` (the RSS template's
/// spelling): an identifier that isn't a literal.
fn nunjucks_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$');
    first_ok
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !["true", "false", "none", "null"].contains(&name)
}

/// Text the plugin writes into its template as it is: text that would be
/// a Nunjucks tag is refused.
fn template_safe(text: &str) -> bool {
    !(text.contains("{{") || text.contains("{%") || text.contains("{#"))
}

/// Read one `add_plugin("rss", options)` call into the feed's virtual
/// template. `Err` is the message for the config problem.
pub fn feed(options: &Value) -> Result<FeedOptions, String> {
    let call = "add_plugin(\"rss\", ...)";
    let Value::Object(given) = options else {
        return Err(format!("{call}: the options must be a map"));
    };
    // The options under the names 11ty reads.
    let mut js: IndexMap<String, Value> = IndexMap::new();
    for (k, v) in given.iter() {
        match OPTIONS.iter().find(|(rhai, _)| rhai == k) {
            Some((_, name)) => {
                js.insert(name.to_string(), v.clone());
            }
            None => {
                let known: Vec<String> = OPTIONS.iter().map(|(r, _)| format!("`{r}`")).collect();
                return Err(format!(
                    "{call}: Brrrty doesn't support the option `{k}`; it supports {}",
                    known.join(", ")
                ));
            }
        }
    }
    let raw = Value::Object(Arc::new(js));
    // `options.metadata.title` names the template, before the defaults.
    let title = get(&get(&raw, "metadata"), "title");
    let suffix = if title.js_truthy() {
        format!("-{}", slugify(&title))
    } else {
        String::new()
    };
    let raw_type = get(&raw, "type");
    let o = deep_copy(Some(&defaults()), &raw);

    let collection = get(&o, "collection");
    let name = get(&collection, "name");
    if !name.js_truthy() {
        return Err(
            "Missing `collection.name` option in feedPlugin from @11ty/eleventy-plugin-rss.".into(),
        );
    }
    let Value::String(name) = name else {
        return Err(format!(
            "Only string is supported in `collection.name` option in feedPlugin from @11ty/eleventy-plugin-rss. Received: {}",
            type_of(&name)
        ));
    };
    let sort = match get(&collection, "sort") {
        Value::String(x) if &*x == "auto" => FeedSort::Auto,
        Value::String(x) if &*x == "ascending" => FeedSort::Ascending,
        Value::String(x) if &*x == "descending" => FeedSort::Descending,
        other => {
            return Err(format!(
                "Invalid `collection.sort` option in feedPlugin from @11ty/eleventy-plugin-rss. Expected \"auto\", \"ascending\", or \"descending\", received: {}",
                other.to_js_string()
            ));
        }
    };
    let kind = match get(&o, "type") {
        Value::String(t) => FeedKind::from_name(&t),
        _ => None,
    }
    .ok_or_else(|| {
        format!(
            "Missing or invalid feed type. Received: {}",
            get(&o, "type").to_js_string()
        )
    })?;
    let limit = match get(&collection, "limit") {
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 => n as i64,
        other => {
            return Err(format!(
                "{call}: `collection.limit` is {}; Brrrty supports a whole number (0 for no limit)",
                other.to_js_string()
            ));
        }
    };
    let bad_name = name
        .chars()
        .any(|c| c.is_control() || matches!(c, '\'' | '\\' | '{' | '}' | '%' | '#'));
    if bad_name || (kind == FeedKind::Rss && !nunjucks_name(&name)) {
        return Err(format!(
            "{call}: the collection name `{name}` would change the meaning of the plugin's Nunjucks template, which Brrrty doesn't follow; use a name of letters, digits, `_` and `$`"
        ));
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        match get(&o, key) {
            v if !v.js_truthy() => Ok(None),
            Value::String(t) if template_safe(&t) => Ok(Some(t.to_string())),
            Value::String(_) => Err(format!(
                "{call}: `{key}` has Nunjucks tags in it, which the plugin would run; Brrrty doesn't"
            )),
            other => Err(format!(
                "{call}: `{key}` is {}; it should be a URL",
                other.to_js_string()
            )),
        }
    };
    let stylesheet = text("stylesheet")?;
    let script = text("script")?;
    let output_path = match get(&o, "outputPath") {
        Value::String(p) => p.to_string(),
        other => {
            return Err(format!(
                "{call}: `output_path` is {}; it should be the feed's path, like \"/feed.xml\"",
                other.to_js_string()
            ));
        }
    };

    let input_path = match get(&raw, "inputPath") {
        v if !v.js_truthy() => {
            let ty = if raw_type.js_truthy() {
                raw_type.to_js_string()
            } else {
                "atom".to_string()
            };
            format!("eleventy-plugin-feed{suffix}-{ty}.njk")
        }
        Value::String(p) => p.to_string(),
        other => {
            return Err(format!(
                "{call}: `input_path` is {}; it should be a path in the input folder",
                other.to_js_string()
            ));
        }
    };
    let input_path = input_path.replace('\\', "/");
    let input_path = input_path.trim_start_matches("./").to_string();
    if !input_path.ends_with(".njk")
        || input_path.starts_with('/')
        || input_path.split('/').any(|p| p == ".." || p.is_empty())
        || input_path.starts_with("_data/")
    {
        return Err(format!(
            "{call}: `input_path` is `{input_path}`; Brrrty supports a `.njk` path inside the input folder, outside the includes, layouts and `_data` folders"
        ));
    }

    let mut data = Data::new();
    if let Value::Object(t) = get(&o, "templateData") {
        for (k, v) in t.iter() {
            data.insert(k.clone(), v.clone());
        }
    }
    data.insert("permalink".into(), Value::String(output_path.into()));
    if &*name == "all" {
        data.insert("eleventyExcludeFromCollections".into(), Value::Bool(true));
        data.insert("eleventyImport".into(), obj(vec![]));
    } else {
        data.insert(
            "eleventyExcludeFromCollections".into(),
            Value::Array(Arc::new(vec![Value::String(name.clone())])),
        );
        data.insert(
            "eleventyImport".into(),
            obj(vec![(
                "collections",
                Value::Array(Arc::new(vec![Value::String(name.clone())])),
            )]),
        );
    }
    data.insert("layout".into(), Value::Bool(false));
    data.insert("metadata".into(), get(&o, "metadata"));
    Ok(FeedOptions {
        input_path,
        data,
        template: Arc::new(FeedTemplate {
            kind,
            collection: name.to_string(),
            limit,
            sort,
            stylesheet,
            script,
        }),
    })
}
