//! `config.add_plugin("syntaxhighlight", #{ ... })`: the options of
//! `@11ty/eleventy-plugin-syntaxhighlight` 5.0.2, in snake_case.

use brrrty_core::plugins::syntaxhighlight::{Attributes, Options, SyntaxHighlight};
use brrrty_js::Value;
use rhai::{Dynamic, FnPtr, Map};

use crate::convert::to_value;
use crate::object::JsObject;

const CALL: &str = "add_plugin(\"syntaxhighlight\", ...)";

/// The options a script may give, and the names 11ty gives them.
const OPTIONS: [(&str, &str); 8] = [
    ("line_separator", "lineSeparator"),
    ("error_on_invalid_language", "errorOnInvalidLanguage"),
    ("always_wrap_line_highlights", "alwaysWrapLineHighlights"),
    ("pre_attributes", "preAttributes"),
    ("code_attributes", "codeAttributes"),
    ("trim", "trim"),
    ("languages", "languages"),
    ("template_formats", "templateFormats"),
];

fn boolean(name: &str, v: &Dynamic) -> Result<bool, String> {
    v.as_bool()
        .map_err(|t| format!("{CALL}: `{name}` must be true or false, not a {t}"))
}

/// `preAttributes` or `codeAttributes`: an `object()` (or a map with one
/// key, whose order can't matter), or a falsy value for none.
fn attributes(name: &str, v: &Dynamic) -> Result<Attributes, String> {
    if v.is::<FnPtr>() {
        return Err(format!(
            "{CALL}: `{name}` is a function, which Brrrty can't call the way the plugin does; give the attributes themselves"
        ));
    }
    let entries = |value: Value| -> Result<Attributes, String> {
        let Value::Object(map) = value else {
            unreachable!("an object converts to an object")
        };
        Ok(Some(
            map.iter()
                .map(|(k, x)| (k.clone(), x.clone()))
                .collect::<Vec<_>>(),
        ))
    };
    let functions = |has_function: bool| {
        if has_function {
            Err(format!(
                "{CALL}: `{name}` has a function, which Brrrty can't call the way the plugin does; give each attribute a string or a number"
            ))
        } else {
            Ok(())
        }
    };
    if let Some(m) = v.read_lock::<Map>() {
        functions(m.values().any(|x| x.is::<FnPtr>()))?;
        if m.len() > 1 {
            return Err(format!(
                "{CALL}: `{name}` is a `#{{...}}` map, which sorts its keys, so the attributes wouldn't be in the order written; build it with `object()`"
            ));
        }
    } else if let Some(o) = v.read_lock::<JsObject>() {
        // Only an object a script built can hold a function.
        if let JsObject::Owned(m) = &*o {
            functions(m.values().any(|x| x.is::<FnPtr>()))?;
        }
    } else {
        let value = to_value(v).map_err(|why| format!("{CALL}: `{name}`: {why}"))?;
        if !value.js_truthy() {
            return Ok(None);
        }
        return Err(match value {
            Value::String(_) => format!(
                "{CALL}: `{name}` must be an object; 11ty's plugin throws \"Syntax highlighter plugin custom attributes on <pre> and <code> must be an object.\""
            ),
            _ => format!("{CALL}: `{name}` must be an `object()` of attributes"),
        });
    }
    entries(to_value(v).map_err(|why| format!("{CALL}: `{name}`: {why}"))?)
}

fn strings(name: &str, v: &Dynamic) -> Result<Vec<String>, String> {
    let bad = || format!("{CALL}: `{name}` must be a list of strings");
    let Some(list) = v.read_lock::<rhai::Array>() else {
        return Err(bad());
    };
    list.iter()
        .map(|x| x.clone().into_string().map_err(|_| bad()))
        .collect()
}

/// Read one `add_plugin("syntaxhighlight", options)` call. `Err` is the
/// message for the config problem.
pub fn syntax_highlight(options: Option<&Map>) -> Result<SyntaxHighlight, String> {
    let mut o = Options::default();
    for (key, v) in options.into_iter().flatten() {
        match key.as_str() {
            "line_separator" => {
                o.line_separator = v
                    .clone()
                    .into_string()
                    .map_err(|_| format!("{CALL}: `line_separator` must be a string"))?;
            }
            "error_on_invalid_language" => o.error_on_invalid_language = boolean(key, v)?,
            "always_wrap_line_highlights" => o.always_wrap_line_highlights = boolean(key, v)?,
            "pre_attributes" => o.pre_attributes = attributes(key, v)?,
            "code_attributes" => o.code_attributes = attributes(key, v)?,
            "trim" => o.trim = boolean(key, v)?,
            "languages" => o.languages = strings(key, v)?,
            "template_formats" => {
                o.template_formats = Some(match v.clone().into_string() {
                    Ok(s) => vec![s],
                    Err(_) => strings(key, v)?,
                });
            }
            "init" => {
                return Err(format!(
                    "{CALL}: `init` gets Prism itself to change, which Brrrty can't reproduce"
                ));
            }
            other => {
                let known: Vec<String> = OPTIONS.iter().map(|(r, _)| format!("`{r}`")).collect();
                let hint = OPTIONS
                    .iter()
                    .find(|(_, js)| *js == other)
                    .map(|(r, _)| format!(" (`{other}` is `{r}` here)"))
                    .unwrap_or_default();
                return Err(format!(
                    "{CALL}: Brrrty doesn't support the option `{other}`{hint}; it supports {}",
                    known.join(", ")
                ));
            }
        }
    }
    SyntaxHighlight::new(o)
}
