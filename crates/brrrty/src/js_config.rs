//! `config.set_js_config("node")`: run the site's 11ty config in its own Node, against a
//! recording `eleventyConfig`, and apply what it recorded where the Rhai config's own
//! settings land. Rhai lines win over the JS config for the same setting.
//!
//! Plugins run against the recorder after the config function returns (11ty's order).

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
    run_mode: brrrty_node::RunMode,
    order_check: bool,
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
        brrrty_node::Worker::start_with(node, root, run_mode, order_check).map_err(|e| {
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
    config.js_config_file = Some(shown.display().to_string());
    add_filters(v, &shown, root, run_mode, config);
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

    for name in strings(val("bundles")) {
        if !config.js_bundles.contains(&name) {
            config.js_bundles.push(name);
        }
    }

    for name in strings(val("nativePlugins")) {
        match name.as_str() {
            "idattribute" => config.id_attribute = true,
            "htmlbase" => config.html_base = true,
            _ => {}
        }
    }

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

    let mut blocking: Vec<String> = Vec::new();
    if let Some(Value::Array(un)) = get(v, "unsupported") {
        for u in un.iter() {
            let method = text(get(u, "method")).unwrap_or_default();
            let at = text(get(u, "at")).unwrap_or_default();
            // What changes the output refuses every page, as `init`'s TODOs do.
            if crate::init::js_blocks(&method) {
                blocking.push(format!("{at}: eleventyConfig.{method}"));
            }
            let mut p = Problem::warning(
                codes::CONFIG_UNSUPPORTED,
                format!("{at}: eleventyConfig.{method} is not supported by Brrrty yet, so it was skipped"),
            )
            .with_help("pages that need it may differ from 11ty's, or refuse");
            let (file, line) = match at.rsplit_once(':') {
                Some((f, l)) => (Some(f.to_string()), l.parse::<usize>().ok()),
                None => (None, None),
            };
            config.js_unsupported.push(brrrty_core::JsUnsupported {
                method: method.trim_end_matches(" = ").to_string(),
                file,
                line,
            });
            if let Some((f, l)) = at.rsplit_once(':') {
                p = p.with_path(PathBuf::from(f));
                if let Ok(l) = l.parse::<usize>() {
                    p = p.at(l, 1);
                }
            }
            config.problems.push(p);
        }
    }
    if !blocking.is_empty() && config.unported.is_none() {
        config.unported = Some(
            Problem::error(
                codes::CONFIG_TODO,
                format!(
                    "the 11ty config uses what Brrrty can't run, and it changes what 11ty writes: {}",
                    blocking.join(", ")
                ),
            )
            .with_help("remove it, or port it by hand; until then every page is refused"),
        );
    }
    // The 11ty config ran: nothing is left unchecked against it.
    config
        .problems
        .retain(|p| p.code != brrrty_core::codes::CONFIG_UNVERIFIED);
    config.js_started = Some(brrrty_core::StartedJs(std::sync::Arc::new(
        std::sync::Mutex::new(Some(Box::new(worker))),
    )));
    Ok(())
}

/// What `this` is inside a config filter: 11ty's `page` (the template's own) and `eleventy`
/// (src/Engines/Nunjucks.js `wrapFilter` and Liquid.js `wrapFilter` both `augmentObject` them
/// onto `this`).
fn filter_this(eleventy: &Value) -> Value {
    let mut o = indexmap::IndexMap::new();
    if let Some((page, _)) = brrrty_js::functions::own_page() {
        o.insert("page".to_string(), page);
    }
    o.insert("eleventy".to_string(), eleventy.clone());
    Value::Object(std::sync::Arc::new(o))
}

/// The parts of a filter's `this` Brrrty can't give, per engine. Nunjucks's filter `this` is
/// its render `Context`: `ctx` (the template's variables) and `env` (the `Environment`).
/// LiquidJS's is its filter context: `context` and `liquid`.
const NUNJUCKS_THIS: &[(&str, &str)] = &[
    ("ctx", "Nunjucks's render context"),
    ("env", "the Nunjucks environment"),
];
const LIQUID_THIS: &[(&str, &str)] = &[
    ("context", "LiquidJS's variable scope"),
    ("liquid", "the LiquidJS engine"),
];

/// Turn the recorded filter and shortcode functions into ones that call Node.
fn add_filters(
    v: &Value,
    shown: &Path,
    root: &Path,
    run_mode: brrrty_node::RunMode,
    config: &mut ConfigOptions,
) {
    use brrrty_core::{FilterEngine, JsConfigFilter, JsConfigShortcode, JsLiquidShortcode};
    use brrrty_js::functions::{Settle, This};
    let Some(Value::Array(fns)) = get(v, "functions") else {
        return;
    };
    let eleventy = Value::from_json(&serde_json::json!({
        "version": "3.1.8",
        "generator": "Eleventy v3.1.8",
        "env": { "source": "cli", "runMode": run_mode.as_str(), "root": root.display().to_string() },
    }));
    let mut transforms: Vec<(String, brrrty_core::JsConfigTransform)> = Vec::new();
    for f in fns.iter() {
        if text(get(f, "kind")).as_deref() == Some("transform") {
            if let (Some(name), Some(Value::Function(h))) = (text(get(f, "name")), get(f, "fn")) {
                let stats =
                    std::sync::Arc::new(brrrty_core::JsFunctionStats::new("transform", &name));
                config.js_functions.push(stats.clone());
                let t = transform(*h, &name, shown, stats);
                // 11ty keeps transforms in an object: a name added again replaces the
                // function and keeps its place.
                match transforms.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1 = t,
                    None => transforms.push((name, t)),
                }
            }
            continue;
        }
        // `Some(paired)` for a shortcode, `None` for a filter.
        let (what, shortcode, kind) = match text(get(f, "kind")).as_deref() {
            Some("filter") => ("filter", None, "filter"),
            Some("shortcode") => ("shortcode", Some(false), "shortcode"),
            Some("pairedShortcode") => ("shortcode", Some(true), "pairedShortcode"),
            _ => continue,
        };
        let (Some(name), Some(Value::Function(h))) = (text(get(f, "name")), get(f, "fn")) else {
            continue;
        };
        let engine = match text(get(f, "engine")).as_deref() {
            Some("all") => FilterEngine::All,
            Some("nunjucks") => FilterEngine::Nunjucks,
            Some("liquid") => FilterEngine::Liquid,
            // `addJavaScriptFilter` and the like: for JavaScript templates only.
            _ => continue,
        };
        let is_async = matches!(get(f, "async"), Some(Value::Bool(true)));
        let stats = std::sync::Arc::new(brrrty_core::JsFunctionStats::new(kind, &name));
        config.js_functions.push(stats.clone());
        let make = |settle: Settle,
                    guard: &'static [(&'static str, &'static str)]|
         -> brrrty_liquid::ShortcodeFn {
            let (h, n, eleventy, shown, what, stats) = (
                *h,
                name.clone(),
                eleventy.clone(),
                shown.display().to_string(),
                what,
                stats.clone(),
            );
            std::sync::Arc::new(move |all: &[Value], _| {
                let Some(caller) = brrrty_js::functions::installed() else {
                    return Err(format!(
                        "{}the {what} `{n}` (from {shown}) can't run: Node isn't running",
                        brrrty_js::REFUSAL_PREFIX
                    ));
                };
                let this = filter_this(&eleventy);
                let started = std::time::Instant::now();
                let result = caller.call(h, This::Guarded(&this, guard), all, settle);
                stats.record(started.elapsed());
                result.map_err(|why| {
                    format!(
                        "{}the {what} `{n}` (from {shown}) failed: {why}",
                        brrrty_js::REFUSAL_PREFIX
                    )
                })
            })
        };
        // Liquid awaits a returned promise; Nunjucks takes what the function returns, unless
        // it was added as async.
        let njk = if is_async {
            Settle::Await
        } else {
            Settle::AsIs
        };
        // A filter's input is the first of the arguments the function gets.
        let as_filter = |run: brrrty_liquid::ShortcodeFn| -> brrrty_liquid::ArgsFilterFn {
            std::sync::Arc::new(move |input, args, ctx| {
                let mut all = Vec::with_capacity(args.len() + 1);
                all.push(input.clone());
                all.extend_from_slice(args);
                run(&all, ctx)
            })
        };
        if engine != FilterEngine::Liquid {
            let run = make(njk, NUNJUCKS_THIS);
            match shortcode {
                // Nunjucks calls a shortcode as a filter of its input and arguments.
                Some(paired) => config.js_shortcodes.push(JsConfigShortcode {
                    name: name.clone(),
                    paired,
                    engine: FilterEngine::Nunjucks,
                    run: as_filter(run),
                }),
                None => config.js_filters.push(JsConfigFilter {
                    name: name.clone(),
                    engine: FilterEngine::Nunjucks,
                    run: as_filter(run),
                }),
            }
        }
        if engine != FilterEngine::Nunjucks {
            let run = make(Settle::Await, LIQUID_THIS);
            match shortcode {
                Some(paired) => {
                    config
                        .js_liquid_shortcodes
                        .push(JsLiquidShortcode { name, paired, run })
                }
                None => config.js_filters.push(JsConfigFilter {
                    name,
                    engine: FilterEngine::Liquid,
                    run: as_filter(run),
                }),
            }
        }
    }
    // An object's integer-like keys come first, ascending, then the rest as added.
    let index = |n: &str| {
        n.parse::<u32>()
            .ok()
            .filter(|i| i.to_string() == n && *i != u32::MAX)
    };
    transforms.sort_by_key(|(n, _)| index(n).map_or((1, 0), |i| (0, i)));
    config
        .js_transforms
        .extend(transforms.into_iter().map(|(_, t)| t));
}

/// A transform from the JS config, called as 11ty's `TransformsUtil.runAll` does: with
/// `this = { inputPath, outputPath, url, page }` and `(content, outputPath)`, awaited.
fn transform(
    h: brrrty_js::FnHandle,
    name: &str,
    shown: &Path,
    stats: std::sync::Arc<brrrty_core::JsFunctionStats>,
) -> brrrty_core::JsConfigTransform {
    use brrrty_js::functions::{Settle, This};
    let (name, shown) = (name.to_string(), shown.display().to_string());
    let n = name.clone();
    let run = std::sync::Arc::new(move |content: &str, page: &Value| {
        let fail = |why: String| {
            format!(
                "{}the transform `{n}` (from {shown}) failed: {why}",
                brrrty_js::REFUSAL_PREFIX
            )
        };
        let Some(caller) = brrrty_js::functions::installed() else {
            return Err(fail("Node isn't running".to_string()));
        };
        let field = |k: &str| get(page, k).cloned().unwrap_or(Value::Undefined);
        let (input, output, url) = (field("inputPath"), field("outputPath"), field("url"));
        let mut o = indexmap::IndexMap::new();
        o.insert("inputPath".to_string(), input);
        o.insert("outputPath".to_string(), output.clone());
        o.insert("url".to_string(), url);
        o.insert("page".to_string(), page.clone());
        o.insert("baseHref".to_string(), Value::Undefined);
        let this = Value::Object(std::sync::Arc::new(o));
        let args = [Value::String(content.into()), output];
        let started = std::time::Instant::now();
        let called = caller.call(h, This::Value(&this), &args, Settle::Await);
        stats.record(started.elapsed());
        match called {
            Ok(Value::String(s)) => Ok(s.to_string()),
            Ok(other) => Err(fail(format!(
                "it gave {}, not text, and 11ty can't write that",
                other.to_js_string()
            ))),
            Err(why) => Err(fail(why)),
        }
    });
    brrrty_core::JsConfigTransform { name, run }
}
