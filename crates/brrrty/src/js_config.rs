//! `config.set_js_config("node")`: run the site's 11ty config in its own Node, against a
//! recording `eleventyConfig`, and apply what it recorded where the Rhai config's own
//! settings land. Rhai lines win over the JS config for the same setting.
//!
//! Recorded functions (filters, shortcodes, transforms, plugins) are not callable yet: pages
//! that use them refuse as they do with the config off.

use std::path::{Path, PathBuf};

use brrrty_config::DirOptions;
use brrrty_core::{ConfigOptions, PassthroughEntry, Problem, codes};
use brrrty_js::Value;
use brrrty_js::runtime::{JsResult, JsRuntime};

/// 11ty 3.1.8's config file lookup order (`src/TemplateConfig.js`): `.eleventy.js` first.
const CONFIG_FILES: &[&str] = &[
    ".eleventy.js",
    "eleventy.config.js",
    "eleventy.config.mjs",
    "eleventy.config.cjs",
];

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key),
        _ => None,
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn text(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn problem(message: String, file: Option<&Path>, line: Option<u32>) -> Problem {
    let mut p = Problem::error(codes::CONFIG_ERROR, message);
    if let Some(f) = file {
        p = p.with_path(f);
    }
    if let Some(l) = line {
        p = p.at(l as usize, 1);
    }
    p
}

/// Start Node, load the 11ty config in `root` and apply it to `config` and `dir`.
pub fn apply(
    root: &Path,
    node: &brrrty_node::NodeInfo,
    config: &mut ConfigOptions,
    dir: &mut DirOptions,
) -> Result<(), Vec<Problem>> {
    let Some(file) = CONFIG_FILES
        .iter()
        .map(|f| root.join(f))
        .find(|f| f.is_file())
    else {
        config.problems.push(
            Problem::warning(
                codes::CONFIG_UNSUPPORTED,
                "config.set_js_config(\"node\") is on, but there is no 11ty config file here",
            )
            .with_help(format!("looked for {}", CONFIG_FILES.join(", "))),
        );
        return Ok(());
    };
    let shown = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
    let mut worker =
        brrrty_node::Worker::start(node, root, brrrty_node::RunMode::Build).map_err(|e| {
            vec![problem(
                format!(
                    "Brrrty could not start Node to load {}: {e}",
                    shown.display()
                ),
                None,
                None,
            )]
        })?;
    let loaded = match worker.load_config(&file) {
        JsResult::Ok(d) => d,
        JsResult::Thrown(e) => {
            return Err(vec![problem(
                format!("{} threw: {}", shown.display(), e.message),
                e.file.as_deref().map(|f| f.strip_prefix(root).unwrap_or(f)),
                e.line,
            )]);
        }
        JsResult::Refused(r) => {
            return Err(vec![problem(
                format!("{} could not be loaded: {}", shown.display(), r.why),
                Some(&shown),
                None,
            )]);
        }
        JsResult::Failed(f) => {
            return Err(vec![problem(
                format!(
                    "Node stopped while loading {}: {}",
                    shown.display(),
                    f.message
                ),
                Some(&shown),
                None,
            )]);
        }
    };
    let v = &loaded.value;
    let values = get(v, "values");
    let val = |k: &str| values.and_then(|x| get(x, k));

    // Passthrough copy, then ignores: the Rhai config's come after, as its lines run after.
    let mut passthrough = Vec::new();
    if let Some(Value::Array(a)) = val("passthroughCopy") {
        for c in a.iter() {
            let Some(from) = text(get(c, "input")) else {
                continue;
            };
            if matches!(get(c, "options"), Some(Value::Object(o)) if !o.is_empty()) {
                config.problems.push(
                    Problem::warning(
                        codes::CONFIG_UNSUPPORTED,
                        format!(
                            "addPassthroughCopy(\"{from}\") options (like `filter` or `expand`) are not supported by Brrrty yet, so they were ignored"
                        ),
                    )
                    .with_path(shown.clone()),
                );
            }
            match get(c, "output") {
                Some(Value::String(to)) => passthrough.push(PassthroughEntry {
                    from,
                    to: Some(to.to_string()),
                }),
                Some(Value::Bool(false)) => {}
                _ => passthrough.push(PassthroughEntry { from, to: None }),
            }
        }
    }
    passthrough.append(&mut config.passthrough);
    config.passthrough = passthrough;
    let mut ignores = strings(val("ignores"));
    ignores.retain(|i| !config.ignores.contains(i));
    ignores.append(&mut config.ignores);
    config.ignores = ignores;

    if let Some(Value::Object(o)) = val("globalData") {
        for (k, x) in o.iter() {
            config
                .global_data
                .entry(k.clone())
                .or_insert_with(|| x.clone());
        }
    }

    // Template formats: a Rhai `set_template_formats` (or returned list) wins; otherwise the JS
    // set (or the defaults) plus both sides' additions.
    let fmt_err = |m: String| vec![problem(m, Some(&shown), None)];
    let js_returned = get(v, "returned");
    let normalize = |what: &str, v: &Value| -> Result<Vec<String>, Vec<Problem>> {
        brrrty_config::normalize_template_formats(what, strings(Some(v))).map_err(fmt_err)
    };
    if !config.template_formats_set {
        let set = match val("templateFormats") {
            Some(f) => Some(normalize("`setTemplateFormats`", f)?),
            None => match js_returned.and_then(|r| get(r, "templateFormats")) {
                Some(f) => Some(normalize("`templateFormats`", f)?),
                None => None,
            },
        };
        let mut added = Vec::new();
        if let Some(Value::Array(a)) = val("templateFormatsAdded") {
            for x in a.iter() {
                added.extend(normalize("`addTemplateFormats`", x)?);
            }
        }
        added.extend(config.added_template_formats.iter().cloned());
        let formats = match set {
            Some(f) => Some(f),
            None if !added.is_empty() => Some(
                ["liquid", "md", "njk", "html", "11ty.js"]
                    .map(String::from)
                    .to_vec(),
            ),
            None => None,
        }
        .map(|mut f| {
            for a in added {
                if !f.contains(&a) {
                    f.push(a);
                }
            }
            f
        });
        config.template_formats = formats.filter(|f| !f.is_empty());
    }

    // Template engines from the returned object, unless the Rhai config changed them.
    if config.template_engines == brrrty_core::TemplateEngines::default() {
        for (key, slot) in [("markdownTemplateEngine", 0), ("htmlTemplateEngine", 1)] {
            let Some(e) = js_returned.and_then(|r| get(r, key)) else {
                continue;
            };
            let lang = match e {
                Value::Bool(false) => None,
                Value::String(n) => match brrrty_core::TemplateLanguage::from_name(n) {
                    Some(l) => Some(l),
                    None => {
                        return Err(vec![problem(
                            format!(
                                "`{key}` is `{n}`, which Brrrty doesn't run there (use \"liquid\", \"njk\" or false)"
                            ),
                            Some(&shown),
                            None,
                        )]);
                    }
                },
                _ => {
                    return Err(vec![problem(
                        format!("`{key}` should be a template language, like \"njk\", or false"),
                        Some(&shown),
                        None,
                    )]);
                }
            };
            if slot == 0 {
                config.template_engines.markdown = lang;
            } else {
                config.template_engines.html = lang;
            }
        }
    }
    // Every other returned key is skipped loudly.
    if let Some(Value::Object(o)) = js_returned {
        for (k, _) in o.iter() {
            if !matches!(
                k.as_str(),
                "dir" | "templateFormats" | "markdownTemplateEngine" | "htmlTemplateEngine"
            ) {
                let why = if k == "dataTemplateEngine" {
                    "11ty 3 doesn't read it"
                } else {
                    "Brrrty doesn't support it yet"
                };
                config.problems.push(
                    Problem::warning(
                        codes::CONFIG_UNSUPPORTED,
                        format!("the config returns `{k}`, which was skipped: {why}"),
                    )
                    .with_path(shown.clone()),
                );
            }
        }
    }

    // `dir`: the setters, then what the config function returned.
    let rhai = dir.clone();
    for source in [val("dir"), get(v, "returned").and_then(|r| get(r, "dir"))] {
        let Some(source) = source else { continue };
        let theirs = DirOptions {
            input: text(get(source, "input")),
            output: text(get(source, "output")),
            includes: text(get(source, "includes")),
            data: text(get(source, "data")),
            layouts: text(get(source, "layouts")),
        };
        // Rhai wins: only fill what it left unset (the later JS source wins among JS).
        if rhai.input.is_none() && theirs.input.is_some() {
            dir.input = theirs.input;
        }
        if rhai.output.is_none() && theirs.output.is_some() {
            dir.output = theirs.output;
        }
        if rhai.includes.is_none() && theirs.includes.is_some() {
            dir.includes = theirs.includes;
        }
        if rhai.data.is_none() && theirs.data.is_some() {
            dir.data = theirs.data;
        }
        if rhai.layouts.is_none() && theirs.layouts.is_some() {
            dir.layouts = theirs.layouts;
        }
    }
    config.includes = dir.includes.clone();
    config.layouts = dir.layouts.clone();
    config.data_folder = dir.data.clone();

    if let Some(Value::Array(un)) = get(v, "unsupported") {
        for u in un.iter() {
            let method = text(get(u, "method")).unwrap_or_default();
            let at = text(get(u, "at")).unwrap_or_default();
            let mut p = Problem::warning(
                codes::CONFIG_UNSUPPORTED,
                format!("{at}: eleventyConfig.{method} is not supported by Brrrty yet, so it was skipped"),
            )
            .with_help("pages that need it may differ from 11ty's, or refuse");
            if let Some((f, l)) = at.rsplit_once(':') {
                p = p.with_path(PathBuf::from(f));
                if let Ok(l) = l.parse::<usize>() {
                    p = p.at(l, 1);
                }
            }
            config.problems.push(p);
        }
    }
    Ok(())
}
