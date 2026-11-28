//! 11ty's `RenderPlugin` (`src/Plugins/RenderPlugin.js`): `renderTemplate`, `renderFile` and
//! `renderContent` render a string or a file as a template inside another.
//!
//! The nested template sees only the data it is given, plus the caller's `page` and
//! `eleventy`: the plugin's `accessGlobalData` is off. The engines are borrowed from the build
//! while it renders pages, so the tags reach them through a [`Hook`] the build opens for that
//! long.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use brrrty_js::Value;
use indexmap::IndexMap;

use crate::options::TemplateEngines;
use crate::render::{Engines, Pipeline, engines_for};
use crate::templates::has_tags;

/// What a nested render is asked.
pub(crate) struct Request<'a> {
    /// `renderTemplate`'s language, or a file's override; `None` is the caller's own.
    pub lang: Option<&'a str>,
    pub data: Value,
    pub page: Option<Value>,
    pub eleventy: Option<Value>,
}

/// What the tags call: render a string, or a file.
pub(crate) trait Nest: Sync {
    fn string(&self, content: &str, req: Request<'_>) -> Result<String, String>;
    fn file(&self, path: &str, req: Request<'_>) -> Result<String, String>;
}

struct Raw(*const (dyn Nest + 'static));

// SAFETY: the pointee is `Sync`, and the pointer is only used under the lock that `Guard` takes
// before the pointee goes away.
unsafe impl Send for Raw {}
unsafe impl Sync for Raw {}

/// The build's nested renderer, once it has one.
#[derive(Clone, Default)]
pub struct Hook(Arc<RwLock<Option<Raw>>>);

/// Closes a [`Hook`] when dropped.
pub(crate) struct Guard(Hook);

impl Drop for Guard {
    fn drop(&mut self) {
        let mut slot = self.0.0.write().unwrap_or_else(|e| e.into_inner());
        *slot = None;
    }
}

impl Hook {
    /// Opens the hook on `nest` until the guard drops. `nest` must outlive the guard.
    pub(crate) fn open(&self, nest: &(dyn Nest + '_)) -> Guard {
        // SAFETY: only the lifetime is erased. The guard clears the pointer, waiting for any
        // call that holds it, before `nest` can end (the caller keeps the guard in a scope
        // inside `nest`'s).
        let ptr: *const (dyn Nest + '_) = nest;
        let ptr: *const (dyn Nest + 'static) = unsafe { std::mem::transmute(ptr) };
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = Some(Raw(ptr));
        Guard(self.clone())
    }

    fn with<T>(&self, f: impl FnOnce(&dyn Nest) -> Result<T, String>) -> Result<T, String> {
        let slot = self.0.read().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            // SAFETY: see `open`; the read lock is held for the call.
            Some(Raw(p)) => f(unsafe { &**p }),
            None => Err("Brrrty can't render a template here: no page is being built".to_string()),
        }
    }
}

/// The build's engines, as the tags use them.
pub(crate) struct Nested<'a> {
    pub pipeline: &'a Pipeline<'a>,
    pub root: &'a Path,
    pub engines: TemplateEngines,
}

impl Nested<'_> {
    fn run(&self, engines: Engines, source: &str, req: Request<'_>) -> Result<String, String> {
        // `renderShortcodeFn`: anything but a plain object is `{ _: data }`.
        let mut data: IndexMap<String, Value> = match req.data {
            Value::Object(o) => (*o).clone(),
            other => IndexMap::from([("_".to_string(), other)]),
        };
        data.insert("page".to_string(), req.page.unwrap_or(Value::Undefined));
        data.insert(
            "eleventy".to_string(),
            req.eleventy.unwrap_or(Value::Undefined),
        );
        let data = Value::Object(Arc::new(data));
        let langs = self.pipeline.langs;
        let template = |lang, source: &str| -> Result<String, String> {
            if !has_tags(lang, source) {
                return Ok(source.to_string());
            }
            let t = langs
                .compile(lang, "renderTemplate", None, source)
                .map_err(|e| e.message())?;
            Ok(langs.render(&t, &data).map_err(|e| e.message())?.output)
        };
        let markdown = |source: &str| -> Result<String, String> {
            self.pipeline
                .render_markdown(source, Path::new("renderTemplate"), None)
                .map_err(|p| p.message)
        };
        match engines {
            Engines::Raw => Ok(source.to_string()),
            Engines::Template(lang) => template(lang, source),
            Engines::Markdown { pre: Some(lang) } => markdown(&template(lang, source)?),
            Engines::Markdown { pre: None } => markdown(source),
            Engines::Js(_) => Err("11ty.js is not supported here".to_string()),
        }
    }
}

impl Nest for Nested<'_> {
    fn string(&self, content: &str, req: Request<'_>) -> Result<String, String> {
        // No language: the page's own `page.templateSyntax`.
        let syntax = match req.lang {
            Some(l) => l.to_string(),
            None => match &req.page {
                Some(Value::Object(p)) => match p.get("templateSyntax") {
                    Some(Value::String(s)) => s.to_string(),
                    _ => String::new(),
                },
                _ => String::new(),
            },
        };
        if syntax.trim().eq_ignore_ascii_case("11ty.js") {
            return Err("11ty.js is not yet supported as a template engine for `renderTemplate`. Use `renderFile` instead!".to_string());
        }
        let over = Value::String(syntax.as_str().into());
        let (engines, _) = engines_for("", Some(&over), self.engines).map_err(|p| p.message)?;
        self.run(engines, content, req)
    }

    fn file(&self, path: &str, req: Request<'_>) -> Result<String, String> {
        if path.is_empty() {
            return Err(
                "Missing file path argument passed to the `renderFile` shortcode.".to_string(),
            );
        }
        let full: PathBuf = self.root.join(path);
        if !full.is_file() {
            return Err(format!(
                "Could not find render plugin file for the `renderFile` shortcode, looking for: {path}"
            ));
        }
        let name = full.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let lower = name.to_lowercase();
        let ext = lower.rsplit('.').next().unwrap_or("");
        if lower.ends_with(".11ty.js")
            || lower.ends_with(".11ty.cjs")
            || lower.ends_with(".11ty.mjs")
        {
            return Err(format!(
                "Brrrty can't render a JavaScript template with `renderFile` yet: {path}"
            ));
        }
        let over = req.lang.map(|l| Value::String(l.into()));
        let engines = if over.is_some() {
            engines_for(ext, over.as_ref(), self.engines)
        } else if ["liquid", "njk", "md", "html"].contains(&ext) {
            engines_for(ext, None, self.engines)
        } else {
            return Err(format!(
                "Unknown engine for {path} (supported extensions: liquid, njk, md, html, 11ty.js)"
            ));
        }
        .map_err(|p| p.message)?
        .0;
        let source = std::fs::read_to_string(&full)
            .map_err(|e| format!("can't read {path} for the `renderFile` shortcode: {e}"))?;
        self.run(engines, &source, req)
    }
}

/// `renderShortcodeFn`'s reading of the arguments after the content: `(templateLang, data)`, or
/// `(data)` when the first isn't a string.
fn lang_and_data(args: &[Value]) -> (Option<String>, Value) {
    match args.first() {
        Some(Value::String(s)) => (
            (!s.is_empty()).then(|| s.to_string()),
            args.get(1)
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
        ),
        Some(other) => (None, other.clone()),
        None => (None, Value::Object(Default::default())),
    }
}

fn text(v: &Value) -> String {
    v.to_js_string()
}

fn do_string(
    hook: &Hook,
    content: &Value,
    args: &[Value],
    page: Option<Value>,
    eleventy: Option<Value>,
) -> Result<String, String> {
    let (lang, data) = lang_and_data(args);
    hook.with(|n| {
        n.string(
            &text(content),
            Request {
                lang: lang.as_deref(),
                data,
                page,
                eleventy,
            },
        )
    })
}

/// `renderFile(inputPath, data = {}, templateLang)`.
fn do_file(
    hook: &Hook,
    args: &[Value],
    page: Option<Value>,
    eleventy: Option<Value>,
) -> Result<String, String> {
    let path = args.first().map(text).unwrap_or_default();
    let data = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let lang = match args.get(2) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    };
    hook.with(|n| {
        n.file(
            &path,
            Request {
                lang: lang.as_deref(),
                data,
                page,
                eleventy,
            },
        )
    })
}

/// Adds the plugin's tags and filter to both engines.
pub fn register(
    liquid: &mut brrrty_liquid::Engine,
    nunjucks: &mut brrrty_nunjucks::Engine,
    hook: &Hook,
) {
    let h = hook.clone();
    liquid.register_raw_shortcode(
        "renderTemplate",
        Arc::new(move |body, args, ctx| {
            do_string(
                &h,
                &Value::String(body.into()),
                args,
                ctx.page.clone(),
                ctx.eleventy.clone(),
            )
        }),
    );
    let h = hook.clone();
    liquid.register_shortcode(
        "renderFile",
        false,
        Arc::new(move |args, ctx| {
            do_file(&h, args, ctx.page.clone(), ctx.eleventy.clone())
                .map(|s| Value::String(s.into()))
        }),
    );
    let h = hook.clone();
    liquid.register_args_filter(
        "renderContent",
        Arc::new(move |input, args, ctx| {
            do_string(&h, input, args, ctx.page.clone(), ctx.eleventy.clone())
                .map(|s| Value::String(s.into()))
        }),
    );

    let h = hook.clone();
    nunjucks.register_raw_shortcode(
        "renderTemplate",
        Arc::new(move |input, args, env| {
            do_string(&h, input, args, env.page.clone(), env.eleventy.clone())
                .map(|s| Value::String(s.into()))
        }),
    );
    let h = hook.clone();
    nunjucks.register_shortcode(
        "renderFile",
        false,
        Arc::new(move |input, args, env| {
            let mut all = vec![input.clone()];
            all.extend(args.iter().cloned());
            do_file(&h, &all, env.page.clone(), env.eleventy.clone())
                .map(|s| Value::String(s.into()))
        }),
    );
    let h = hook.clone();
    nunjucks.register_filter(
        "renderContent",
        Arc::new(move |input, args, env| {
            do_string(&h, input, args, env.page.clone(), env.eleventy.clone())
                .map(|s| Value::String(s.into()))
        }),
    );
}
