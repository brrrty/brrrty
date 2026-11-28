//! The template pipeline: the page's template language (Liquid or
//! Nunjucks), then Markdown for `.md`, then the layout chain, as 11ty 3.1.8
//! runs them. `.md` and `.html` run the config's `markdownTemplateEngine`
//! and `htmlTemplateEngine` (Liquid by default).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_js::Value as Json;
use brrrty_liquid::{ErrorKind, LiquidError};
use brrrty_markdown::MarkdownRenderer;
use indexmap::IndexMap;

use crate::layout::{Kind, LayoutFile};
use crate::options::TemplateEngines;
use crate::problem::{Problem, codes};
use crate::templates::{Compiled, Lang, Langs, TplError, has_tags};

/// What runs on a page's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Engines {
    /// Written as is: `templateEngineOverride` of `html`, `""` or `false`,
    /// or `.html` when `htmlTemplateEngine` is `false`.
    Raw,
    /// One template language.
    Template(Lang),
    /// Markdown, after `pre` when it is set.
    Markdown { pre: Option<Lang> },
    /// A JavaScript template (`.11ty.js`), loaded in the site's Node: its
    /// `render` makes the content.
    Js(brrrty_js::runtime::TemplateId),
}

/// Template languages 11ty knows by default that Brrrty doesn't render.
const OTHER_ENGINES: &[&str] = &["11ty.js"];

fn override_error(message: String) -> Problem {
    Problem::error(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID, message).with_help(
        "use `liquid`, `njk`, `md` or `html`, or `md` with one of the others, like `njk,md`",
    )
}

/// The engines a page runs without an override, and their
/// `page.templateSyntax`, from its extension (lowercase): 11ty's
/// `getEnginesList`.
fn default_engines(ext: &str, config: TemplateEngines) -> (Engines, String) {
    match ext {
        "md" => match config.markdown {
            Some(l) => (
                Engines::Markdown { pre: Some(l) },
                format!("{},md", l.name()),
            ),
            None => (Engines::Markdown { pre: None }, "md".to_string()),
        },
        "html" => match config.html {
            Some(l) => (Engines::Template(l), l.name().to_string()),
            None => (Engines::Raw, "html".to_string()),
        },
        "njk" => (Engines::Template(Lang::Nunjucks), "njk".to_string()),
        _ => (Engines::Template(Lang::Liquid), "liquid".to_string()),
    }
}

/// A `templateEngineOverride` taken apart as 11ty's `parseEngineOverrides`
/// does: whether it has `md`, and its one other language, if any.
fn parse_override(over: &Json) -> Result<(bool, Option<String>), Problem> {
    // `typeof (value || "") !== "string"` is an error in 11ty.
    let text = match over {
        Json::String(s) => s.as_ref(),
        v if !v.js_truthy() => "",
        v => {
            return Err(override_error(format!(
                "templateEngineOverride must be a string, found {}",
                crate::data::json_text(v)
            )));
        }
    };
    let mut markdown = false;
    let mut engines: Vec<String> = Vec::new();
    for name in text.split(',').map(|n| n.trim().to_lowercase()) {
        match name.as_str() {
            "" | "html" => {}
            "md" => markdown = true,
            _ if engines.contains(&name) => {}
            _ => engines.push(name),
        }
    }
    if engines.len() > 1 {
        return Err(override_error(format!(
            "templateEngineOverride `{text}` mixes template languages; 11ty allows one, plus `md`"
        )));
    }
    Ok((markdown, engines.pop()))
}

/// The language an override names, refused when Brrrty doesn't run it.
fn override_language(name: &str, over: &Json) -> Result<Lang, Problem> {
    let text = match over {
        Json::String(s) => s.to_string(),
        other => crate::data::json_text(other),
    };
    if let Some(lang) = Lang::from_name(name) {
        return Ok(lang);
    }
    if OTHER_ENGINES.contains(&name) {
        return Err(Problem::error(
            codes::TEMPLATE_ENGINE_UNSUPPORTED,
            format!("templateEngineOverride `{text}` uses {name}, which Brrrty doesn't support yet"),
        )
        .with_help("A JavaScript template is a `.11ty.js` file; 11ty doesn't run another template's text as JavaScript in a way Brrrty can follow. Use `liquid`, `njk`, `md` or `html`."));
    }
    Err(override_error(format!(
        "templateEngineOverride `{text}` names an unknown template language `{name}`"
    )))
}

/// The engines for a page and its `page.templateSyntax`, from its file
/// extension (`ext`, lowercase), the merged `templateEngineOverride` and
/// the config, as 11ty's `TemplateRender` picks them.
pub(crate) fn engines_for(
    ext: &str,
    over: Option<&Json>,
    config: TemplateEngines,
) -> Result<(Engines, String), Problem> {
    let Some(over) = over else {
        return Ok(default_engines(ext, config));
    };
    let (markdown, other) = parse_override(over)?;
    let lang = other
        .as_deref()
        .map(|name| override_language(name, over))
        .transpose()?;
    // 11ty lists the override's engines last first; a falsy override keeps
    // the default list, though the page is written as is.
    let syntax = if crate::js::js_truthy(over) {
        let mut list: Vec<&str> = lang.iter().map(|l| l.name()).collect();
        if markdown {
            list.push("md");
        }
        list.join(",")
    } else {
        default_engines(ext, config).1
    };
    let engine = match (lang, markdown) {
        (pre, true) => Engines::Markdown { pre },
        (Some(l), false) => Engines::Template(l),
        (None, false) => Engines::Raw,
    };
    Ok((engine, syntax))
}

/// How a JavaScript read of `collections` that can't be built is refused.
pub(crate) const GAP_TRAP: &str = "reads `collections`, but ";

/// The keys a JavaScript template or layout can't read on this page: the
/// withheld ones, and `collections` when it can't be built (a marker in the
/// data, so it is still listed as 11ty lists it).
fn js_traps(job: &Job<'_>) -> Vec<(String, String)> {
    let mut traps = job.traps.to_vec();
    if let Some(why) = job.gap {
        traps.push(("collections".to_string(), format!("{GAP_TRAP}{why}")));
    }
    traps
}

/// Render the JavaScript template or layout `id` with `data` in the site's
/// Node (shown as `shown` in problems).
fn js_render(
    id: brrrty_js::runtime::TemplateId,
    data: &Value,
    first: Option<&Value>,
    traps: &[(String, String)],
    shown: &Path,
) -> Result<Value, Problem> {
    let Some(calls) = crate::data::js_calls() else {
        return Err(Problem::error(
            codes::JS_WORKER_FAILED,
            "this JavaScript template can't render: JavaScript stopped earlier in this build",
        )
        .with_path(shown)
        .following("Fix the problem reported above."));
    };
    calls.render(id, data, first, traps, shown)
}

/// What renders a page's permalink and computed values, which 11ty
/// compiles without Markdown (`bypassMarkdown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DataEngine {
    /// The language, or `None` when strings are used as written.
    pub lang: Option<Lang>,
    /// The language whose delimiters 11ty looks for first, by the page's
    /// extension: `.liquid` and `.njk` check their own, `.md` and `.html`
    /// always compile.
    check: Option<Lang>,
    /// The language 11ty parses computed values with to order them
    /// (`parseForSymbols` of the page's preprocessor engine), or `None`
    /// when that engine has none.
    pub symbols: Option<Lang>,
}

impl DataEngine {
    /// The engine of a JavaScript template: 11ty's JavaScript engine makes
    /// a template of a string that gives it back as written, so permalink
    /// and computed strings aren't compiled.
    pub fn raw() -> DataEngine {
        DataEngine {
            lang: None,
            check: None,
            symbols: None,
        }
    }

    /// The engine for a page with extension `ext` (lowercase).
    ///
    /// With an override, 11ty's `setEngineOverride` picks it: nothing for
    /// `html` or a falsy value, and for one with `md` the config's
    /// `markdownTemplateEngine` (probed: the override's own language only
    /// runs on the body). Without, the extension does.
    pub fn new(
        ext: &str,
        over: Option<&Json>,
        config: TemplateEngines,
    ) -> Result<DataEngine, Problem> {
        let by_ext = |ext: &str| match ext {
            "md" => config.markdown,
            "html" => config.html,
            "njk" => Some(Lang::Nunjucks),
            _ => Some(Lang::Liquid),
        };
        let lang = match over {
            None => by_ext(ext),
            Some(over) => match parse_override(over)? {
                (true, _) => config.markdown,
                (false, Some(name)) => Some(override_language(&name, over)?),
                (false, None) => None,
            },
        };
        let check = match ext {
            "md" | "html" => None,
            "njk" => Some(Lang::Nunjucks),
            _ => Some(Lang::Liquid),
        };
        Ok(DataEngine {
            lang,
            check,
            symbols: by_ext(ext),
        })
    }

    /// Whether 11ty compiles `s` (`needsCompilation`). A string with none
    /// of the language's delimiters renders as written either way.
    ///
    /// 11ty checks with the page's own engine until a string compiles, and
    /// with the override's after. They only disagree on a `.liquid` page
    /// whose strings run as Nunjucks, for a string whose only tag is a
    /// Nunjucks comment: with more than one string to render (`repeated`),
    /// which way it goes depends on the order 11ty renders them, so it is
    /// refused.
    pub fn compiles(&self, s: &str, repeated: bool) -> Result<bool, Problem> {
        let Some(lang) = self.lang else {
            return Ok(false);
        };
        let ours = has_tags(lang, s);
        let first = self.check.is_none_or(|c| has_tags(c, s));
        if ours && !first && repeated {
            return Err(Problem::error(
                codes::COMPUTED_UNSUPPORTED,
                "a value whose only Nunjucks tag is a comment (`{#`), on a `.liquid` page whose `templateEngineOverride` runs Nunjucks: 11ty compiles it or not depending on the order it renders the page's values",
            )
            .with_help("Remove the comment, or add `{{ \"\" }}` to the value so it is always compiled."));
        }
        Ok(ours && first)
    }
}

/// How many lines of `text` come before `body`, its tail.
pub(crate) fn body_line(text: &str, body: &str) -> usize {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text[..text.len() - body.len()].matches('\n').count()
}

/// A Liquid error as a problem in `path`, whose body starts after
/// `offset` lines. `page` names the page being rendered when it is another
/// file.
pub(crate) fn liquid_problem(
    e: &LiquidError,
    path: PathBuf,
    offset: usize,
    page: Option<&Path>,
) -> Problem {
    let code = match e.kind {
        ErrorKind::UnknownFilter => codes::LIQUID_UNKNOWN_FILTER,
        ErrorKind::Render => codes::LIQUID_RENDER,
        ErrorKind::Limit => codes::LIQUID_LIMIT,
        // Only the syntax highlighting plugin's `{% highlight %}` refuses
        // or fails.
        ErrorKind::Unsupported => codes::SYNTAX_HIGHLIGHT_UNSUPPORTED,
        ErrorKind::TagFailed => codes::SYNTAX_HIGHLIGHT_FAILED,
        ErrorKind::Syntax | ErrorKind::UnknownTag | ErrorKind::Unclosed | ErrorKind::TooDeep => {
            codes::LIQUID_SYNTAX
        }
    };
    let message = match page {
        Some(page) if page != path => {
            format!("{} while rendering {}", e.message, page.display())
        }
        _ => e.message.clone(),
    };
    let mut p = Problem::error(code, message)
        .with_path(path)
        .at(e.line + offset, e.column);
    if let Some(help) = &e.help {
        p = p.with_help(help.clone());
    }
    p
}

/// A page ready to render.
pub(crate) struct Job<'a> {
    /// Relative to the project root.
    pub shown: &'a Path,
    /// The canonical file, which Nunjucks resolves `./` includes against;
    /// `None` for a virtual template.
    pub real: Option<&'a Path>,
    pub body: &'a str,
    pub body_line: usize,
    pub engines: Engines,
    pub data: &'a IndexMap<String, Value>,
    pub layouts: &'a [Arc<LayoutFile>],
    /// A feed's template, which Brrrty renders itself.
    pub feed: Option<&'a crate::plugins::rss::FeedTemplate>,
    /// For a JavaScript template: the `page` of its first page, which 11ty
    /// keeps as `this.page`.
    pub first: Option<&'a Value>,
    /// Keys the page's data lacks because Brrrty can't supply them, and
    /// why: a JavaScript template or layout reading one is refused.
    pub traps: &'a [(String, String)],
    /// Why the page has no `collections`, when the collections can't be
    /// built: a JavaScript template or layout reading it is refused.
    pub gap: Option<&'a str>,
}

/// A rendered page and the variables it read that nothing defined.
pub(crate) struct Output {
    pub html: String,
    pub undefined: BTreeSet<String>,
    /// What a JavaScript template or layout gave, as it gave it, when that
    /// isn't text: `undefined` (11ty writes no file), a number, an array.
    pub js: Option<Value>,
}

pub(crate) struct Pipeline<'a> {
    pub langs: &'a Langs<'a>,
    pub markdown: &'a dyn MarkdownRenderer,
    /// 11ty's syntax highlighting plugin, when the config added it and it
    /// highlights Markdown.
    pub highlight: Option<&'a crate::plugins::syntaxhighlight::SyntaxHighlight>,
    /// Renderer rules on 11ty's library (`amendLibrary("md", ...)`).
    pub rules: Option<&'a (dyn brrrty_markdown::RenderRules + Send + Sync)>,
}

/// The help on a page whose code Brrrty can't highlight exactly.
pub(crate) const HIGHLIGHT_HELP: &str = "Brrrty refuses this page rather than write it differently from 11ty; the rest of the site still builds";

/// The help on a page where the syntax highlighting plugin throws.
pub(crate) const HIGHLIGHT_THROWN_HELP: &str =
    "11ty stops the whole build here. Fix the code's language, or the plugin's options";

/// A fenced code block the syntax highlighting plugin can't highlight here.
fn highlight_problem(
    e: crate::plugins::syntaxhighlight::Error,
    language: &str,
    file: &Path,
    line: Option<usize>,
) -> Problem {
    use crate::plugins::syntaxhighlight::Error;
    let p = match e {
        Error::Unsupported(why) => Problem::error(
            codes::SYNTAX_HIGHLIGHT_UNSUPPORTED,
            format!("the fenced code block in `{language}` can't be highlighted the way 11ty highlights it: {why}"),
        )
        .with_help(HIGHLIGHT_HELP),
        Error::Thrown(why) => Problem::error(
            codes::SYNTAX_HIGHLIGHT_FAILED,
            format!("the syntax highlighting plugin fails on the fenced code block in `{language}`: {why}"),
        )
        .with_help(HIGHLIGHT_THROWN_HELP),
    }
    .with_path(file);
    match line {
        Some(l) => p.at(l, 1),
        None => p,
    }
}

impl Pipeline<'_> {
    /// The page's content without its layouts: what 11ty calls
    /// `templateContent`, and what a collection item's `content` holds.
    pub fn content(&self, job: &Job<'_>) -> Result<Output, Problem> {
        let mut undefined = BTreeSet::new();
        let page = |lang: Lang, undefined: &mut BTreeSet<String>| -> Result<String, Problem> {
            // Without its delimiters, a body renders as written.
            if !has_tags(lang, job.body) {
                return Ok(job.body.to_string());
            }
            let name = job.shown.display().to_string();
            let t = self
                .langs
                .compile(lang, &name, job.real, job.body)
                .map_err(|e| self.problem(&e, job))?;
            let data = Value::Object(Arc::new(job.data.clone()));
            self.render(&t, &data, undefined, job)
        };
        if let Some(feed) = job.feed {
            let r = crate::plugins::rss::feed::render(feed, job.data).map_err(|why| {
                Problem::error(codes::FEED_FAILED, format!("this feed can't be built: {why}"))
                    .with_path(job.shown)
                    .with_help("11ty stops the whole build here. Fix the feed's options in brrrty.config.rhai, or the data it reads.")
            })?;
            return Ok(Output {
                html: r.text,
                undefined: r.undefined,
                js: None,
            });
        }
        let html = match job.engines {
            Engines::Js(id) => {
                let data = Value::Object(Arc::new(job.data.clone()));
                return match js_render(id, &data, job.first, &js_traps(job), job.shown)? {
                    Value::String(s) => Ok(Output {
                        html: s.to_string(),
                        undefined,
                        js: None,
                    }),
                    other => Ok(Output {
                        html: String::new(),
                        undefined,
                        js: Some(other),
                    }),
                };
            }
            Engines::Raw => job.body.to_string(),
            Engines::Template(lang) => page(lang, &mut undefined)?,
            Engines::Markdown { pre: Some(lang) } => {
                let source = page(lang, &mut undefined)?;
                // A line in the page is only known when the template
                // language changed nothing.
                let line = (source == job.body).then_some(job.body_line);
                self.render_markdown(&source, job.shown, line)?
            }
            Engines::Markdown { pre: None } => {
                self.render_markdown(job.body, job.shown, Some(job.body_line))?
            }
        };
        Ok(Output {
            html,
            undefined,
            js: None,
        })
    }

    /// The page's layout chain around `content`, its rendered content (text,
    /// or what a JavaScript template gave). `js` in the result is what the
    /// last layout gave when that isn't text.
    pub fn layouts(&self, job: &Job<'_>, content: &Value) -> Result<Output, Problem> {
        let mut undefined = BTreeSet::new();
        let mut content = content.clone();
        for layout in job.layouts {
            // 11ty renders a layout with `{ content, ...data }`.
            let mut with_content = IndexMap::with_capacity(job.data.len() + 1);
            with_content.insert("content".to_string(), content);
            for (k, v) in job.data {
                with_content.insert(k.clone(), v.clone());
            }
            let layout_data = Value::Object(Arc::new(with_content));
            let text = |s: String| Value::String(s.into());
            content = match &layout.kind {
                Kind::Template(t) => text(self.render(t, &layout_data, &mut undefined, job)?),
                Kind::Markdown(pre) => {
                    let out = match pre {
                        Some(t) => self.render(t, &layout_data, &mut undefined, job)?,
                        None => layout.body.clone(),
                    };
                    text(self.render_markdown(&out, &layout.shown, None)?)
                }
                Kind::Raw => text(layout.body.clone()),
                Kind::Js(id) => js_render(*id, &layout_data, None, &js_traps(job), &layout.shown)?,
                // Pages with such a layout are refused before they get here.
                Kind::Unrenderable(p) => return Err(p.clone()),
            };
        }
        Ok(match content {
            Value::String(s) => Output {
                html: s.to_string(),
                undefined,
                js: None,
            },
            other => Output {
                html: String::new(),
                undefined,
                js: Some(other),
            },
        })
    }

    /// Renders Markdown, turning a panic in the Markdown library into a Problem for this page.
    /// `line` is the line `source` starts after in `file`, when it is the file's own text.
    pub(crate) fn render_markdown(
        &self,
        source: &str,
        file: &Path,
        line: Option<usize>,
    ) -> Result<String, Problem> {
        // `Err(Ok(why))` is a construct the Markdown library refuses; `Err(Err(p))` a highlight
        // problem or a renderer rule that failed.
        let rule_failed = |why: String| {
            Problem::error(codes::MARKDOWN_RULE_FAILED, why)
                .with_path(file)
                .with_help("fix the markdown-it renderer rule named above in your config (`config.amend_library(\"md\", ...)`)")
        };
        let with_rules = |highlight: Option<&mut brrrty_markdown::Highlight<'_>>| {
            let rules = self.rules.expect("called with rules");
            self.markdown
                .try_render_with_rules(source, highlight, rules)
                .map_err(|e| match e {
                    brrrty_markdown::RenderError::Refused(why) => Ok(why),
                    brrrty_markdown::RenderError::Rule(why) => Err(rule_failed(why)),
                })
        };
        let render = || -> Result<String, Result<String, Problem>> {
            let Some(plugin) = self.highlight else {
                if self.rules.is_some() {
                    return with_rules(None);
                }
                return self.markdown.try_render(source).map_err(Ok);
            };
            let mut page = plugin.page();
            let mut failed = None;
            let mut highlight = |fence: &brrrty_markdown::Fence<'_>| {
                if failed.is_some() {
                    return None;
                }
                page.at(file, line.map_or(0, |l| l + fence.line));
                match page.markdown(fence.content, fence.language) {
                    Ok(html) => Some(html),
                    Err(e) => {
                        let language = fence.language.split('/').next().unwrap_or("");
                        failed = Some(highlight_problem(
                            e,
                            language,
                            file,
                            line.map(|l| l + fence.line),
                        ));
                        None
                    }
                }
            };
            let html = if self.rules.is_some() {
                with_rules(Some(&mut highlight))
            } else {
                self.markdown
                    .try_render_highlighted(source, &mut highlight)
                    .map_err(Ok)
            };
            match failed {
                Some(p) => Err(Err(p)),
                None => html,
            }
        };
        let refused = |why: String| {
            Problem::error(
                codes::MARKDOWN_RENDER_FAILED,
                format!("Brrrty can't render this Markdown exactly yet: {why}"),
            )
            .with_path(file)
            .with_help("rewrite the construct named above (or put it in a code block) until Brrrty supports it; markdown-it would render it differently from what Brrrty would write")
        };
        crate::contain::catch(render)
            .map_err(|why| {
                Problem::error(
                    codes::MARKDOWN_RENDER_FAILED,
                    format!("the Markdown renderer failed on this file: {why}"),
                )
                .with_path(file)
                .with_help(
                    "this is a bug in Brrrty's Markdown library. Changing the Markdown near the construct it chokes on (for example, putting `~~strikethrough~~` or emphasis on one line, or a blank line before a list) usually works around it. Please report the file at https://github.com/brrrty/brrrty/issues",
                )
            })?
            .map_err(|e| match e {
                Ok(why) => refused(why),
                Err(p) => p,
            })
    }

    fn render(
        &self,
        t: &Compiled,
        data: &Value,
        undefined: &mut BTreeSet<String>,
        job: &Job<'_>,
    ) -> Result<String, Problem> {
        let r = self
            .langs
            .render(t, data)
            .map_err(|e| self.problem(&e, job))?;
        undefined.extend(r.undefined);
        Ok(r.output)
    }

    /// Where an error came from: the page, one of its layouts, or a
    /// template the loader found.
    fn problem(&self, e: &TplError, job: &Job<'_>) -> Problem {
        let template = Path::new(e.template());
        let (path, offset) = if template == job.shown {
            (job.shown.to_path_buf(), job.body_line)
        } else if let Some(l) = job.layouts.iter().find(|l| l.shown == template) {
            (l.shown.clone(), l.body_line)
        } else {
            (
                self.langs
                    .shown(e)
                    .unwrap_or_else(|| template.to_path_buf()),
                0,
            )
        };
        self.langs.problem(e, path, offset, Some(job.shown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LIQUID: Lang = Lang::Liquid;
    const NJK: Lang = Lang::Nunjucks;

    fn engines_with(
        ext: &str,
        over: Option<serde_json::Value>,
        config: TemplateEngines,
    ) -> Result<(Engines, String), &'static str> {
        engines_for(ext, over.map(|j| Json::from_json(&j)).as_ref(), config).map_err(|p| p.code)
    }

    fn engines(
        ext: &str,
        over: Option<serde_json::Value>,
    ) -> Result<(Engines, String), &'static str> {
        engines_with(ext, over, TemplateEngines::default())
    }

    /// Probed: `page.templateSyntax` and output for each override.
    #[test]
    fn overrides_like_11ty() {
        use Engines::*;
        let ok = |e, s: &str| Ok((e, s.to_string()));
        assert_eq!(
            engines("md", None),
            ok(Markdown { pre: Some(LIQUID) }, "liquid,md")
        );
        assert_eq!(engines("html", None), ok(Template(LIQUID), "liquid"));
        assert_eq!(engines("liquid", None), ok(Template(LIQUID), "liquid"));
        assert_eq!(engines("njk", None), ok(Template(NJK), "njk"));
        assert_eq!(
            engines("md", Some(json!("md"))),
            ok(Markdown { pre: None }, "md")
        );
        assert_eq!(
            engines("md", Some(json!("liquid"))),
            ok(Template(LIQUID), "liquid")
        );
        assert_eq!(
            engines("html", Some(json!("liquid,md"))),
            ok(Markdown { pre: Some(LIQUID) }, "liquid,md")
        );
        assert_eq!(
            engines("liquid", Some(json!("md,liquid"))),
            ok(Markdown { pre: Some(LIQUID) }, "liquid,md")
        );
        assert_eq!(engines("md", Some(json!("njk"))), ok(Template(NJK), "njk"));
        assert_eq!(
            engines("liquid", Some(json!("njk,md"))),
            ok(Markdown { pre: Some(NJK) }, "njk,md")
        );
        assert_eq!(
            engines("njk", Some(json!("md"))),
            ok(Markdown { pre: None }, "md")
        );
        assert_eq!(engines("md", Some(json!("html"))), ok(Raw, ""));
        assert_eq!(engines("md", Some(json!(" HTML , "))), ok(Raw, ""));
        assert_eq!(engines("liquid", Some(json!(false))), ok(Raw, "liquid"));
        assert_eq!(engines("njk", Some(json!(false))), ok(Raw, "njk"));
        assert_eq!(engines("liquid", Some(json!(null))), ok(Raw, "liquid"));
        assert_eq!(engines("liquid", Some(json!(""))), ok(Raw, "liquid"));
        assert_eq!(
            engines("md", Some(json!("11ty.js"))),
            Err(codes::TEMPLATE_ENGINE_UNSUPPORTED)
        );
        assert_eq!(
            engines("md", Some(json!("liquid,njk"))),
            Err(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID)
        );
        assert_eq!(
            engines("md", Some(json!(5))),
            Err(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID)
        );
        assert_eq!(
            engines("md", Some(json!("pug"))),
            Err(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID)
        );
    }

    /// Probed: `markdownTemplateEngine` and `htmlTemplateEngine` of `njk`
    /// and `false`.
    #[test]
    fn config_engines_like_11ty() {
        use Engines::*;
        let ok = |e, s: &str| Ok((e, s.to_string()));
        let njk = TemplateEngines {
            markdown: Some(NJK),
            html: Some(NJK),
        };
        let off = TemplateEngines {
            markdown: None,
            html: None,
        };
        assert_eq!(
            engines_with("md", None, njk),
            ok(Markdown { pre: Some(NJK) }, "njk,md")
        );
        assert_eq!(engines_with("html", None, njk), ok(Template(NJK), "njk"));
        assert_eq!(
            engines_with("liquid", None, njk),
            ok(Template(LIQUID), "liquid")
        );
        assert_eq!(
            engines_with("md", None, off),
            ok(Markdown { pre: None }, "md")
        );
        assert_eq!(engines_with("html", None, off), ok(Raw, "html"));
        assert_eq!(engines_with("md", Some(json!(false)), off), ok(Raw, "md"));
    }

    fn data(ext: &str, over: Option<serde_json::Value>, config: TemplateEngines) -> DataEngine {
        DataEngine::new(ext, over.map(|j| Json::from_json(&j)).as_ref(), config).unwrap()
    }

    /// Probed: which language runs a page's permalink, and whether it is
    /// compiled at all.
    #[test]
    fn permalinks_compile_like_11ty() {
        let njk = TemplateEngines {
            markdown: Some(NJK),
            html: Some(NJK),
        };
        // An override with `md` runs the config's Markdown language.
        let d = data("md", Some(json!("liquid,md")), njk);
        assert_eq!(d.lang, Some(NJK));
        assert_eq!(d.compiles("/{# x #}/", true), Ok(true));
        assert_eq!(data("md", Some(json!("md")), njk).lang, Some(NJK));
        assert_eq!(data("njk", Some(json!("liquid")), njk).lang, Some(LIQUID));
        assert_eq!(data("njk", Some(json!("html")), njk).lang, None);
        assert_eq!(data("html", None, njk).lang, Some(NJK));
        assert_eq!(
            data("html", None, TemplateEngines::default()).lang,
            Some(LIQUID)
        );
        // `.liquid` checks for Liquid's delimiters first.
        let d = data("liquid", Some(json!("njk,md")), njk);
        assert_eq!(d.lang, Some(NJK));
        assert_eq!(d.compiles("/d{# x #}/", false), Ok(false));
        assert!(d.compiles("/d{# x #}/", true).is_err());
        assert_eq!(d.compiles("/{{ x }}{# x #}/", true), Ok(true));
        let d = data("njk", None, njk);
        assert_eq!(d.compiles("/{# x #}/", true), Ok(true));
        assert_eq!(d.compiles("/x/", true), Ok(false));
        // Symbols come from the extension's language, not the override's.
        assert_eq!(data("md", Some(json!("liquid,md")), njk).symbols, Some(NJK));
        assert_eq!(
            data("liquid", Some(json!("njk")), njk).symbols,
            Some(LIQUID)
        );
    }

    #[test]
    fn body_lines() {
        assert_eq!(body_line("---\na: 1\n---\nbody", "body"), 3);
        assert_eq!(body_line("\u{feff}---\n---\nb", "b"), 2);
        assert_eq!(body_line("plain", "plain"), 0);
    }
}
