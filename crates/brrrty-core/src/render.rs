//! The template pipeline: Liquid, then Markdown for `.md`, then the layout
//! chain, as 11ty 3.1.8 runs them with its default `markdownTemplateEngine`
//! and `htmlTemplateEngine` (both `liquid`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_js::Value as Json;
use brrrty_liquid::{Engine, ErrorKind, LiquidError, Template};
use brrrty_markdown::MarkdownRenderer;
use indexmap::IndexMap;

use crate::layout::{Kind, LayoutFile};
use crate::loader::FsLoader;
use crate::problem::{Problem, codes};

/// What runs on a page's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Engines {
    /// Written as is: `templateEngineOverride` of `html`, `""` or `false`.
    Raw,
    Liquid,
    /// Markdown, after Liquid when `liquid` is set.
    Markdown {
        liquid: bool,
    },
}

/// Template languages 11ty knows by default that Brrrty doesn't render.
const OTHER_ENGINES: &[&str] = &["njk", "11ty.js"];

fn override_error(message: String) -> Problem {
    Problem::error(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID, message)
        .with_help("use `liquid`, `md`, `html`, or `liquid,md`")
}

/// The engines for a page and its `page.templateSyntax`, from its file
/// extension (`ext`, lowercase) and the merged `templateEngineOverride`, as
/// 11ty's `TemplateRender` picks them.
pub(crate) fn engines_for(ext: &str, over: Option<&Json>) -> Result<(Engines, String), Problem> {
    let default = || match ext {
        "md" => (Engines::Markdown { liquid: true }, "liquid,md".to_string()),
        _ => (Engines::Liquid, "liquid".to_string()),
    };
    let Some(over) = over else {
        return Ok(default());
    };
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
    // 11ty lists the override's engines last first; a falsy override keeps
    // the default list, though the page is written as is.
    let syntax = if crate::js::js_truthy(over) {
        let mut list: Vec<&str> = engines.iter().map(String::as_str).collect();
        if markdown {
            list.push("md");
        }
        list.join(",")
    } else {
        default().1
    };
    let engine = match engines.first().map(String::as_str) {
        None if markdown => Engines::Markdown { liquid: false },
        None => Engines::Raw,
        Some("liquid") if markdown => Engines::Markdown { liquid: true },
        Some("liquid") => Engines::Liquid,
        Some(other) if OTHER_ENGINES.contains(&other) => {
            return Err(Problem::error(
                codes::TEMPLATE_ENGINE_UNSUPPORTED,
                format!(
                    "templateEngineOverride `{text}` uses {other}, which Brrrty doesn't support yet"
                ),
            )
            .with_help("Nunjucks support is coming soon. Until then, use `liquid`, `md`, `html` or `liquid,md`."));
        }
        Some(other) => {
            return Err(override_error(format!(
                "templateEngineOverride `{text}` names an unknown template language `{other}`"
            )));
        }
    };
    Ok((engine, syntax))
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
    pub body: &'a str,
    pub body_line: usize,
    pub engines: Engines,
    pub data: &'a IndexMap<String, Value>,
    pub layouts: &'a [Arc<LayoutFile>],
    /// A feed's template, which Brrrty renders itself.
    pub feed: Option<&'a crate::plugins::rss::FeedTemplate>,
}

/// A rendered page and the variables it read that nothing defined.
pub(crate) struct Output {
    pub html: String,
    pub undefined: BTreeSet<String>,
}

pub(crate) struct Pipeline<'a> {
    pub engine: &'a Engine,
    pub loader: &'a FsLoader,
    pub markdown: &'a dyn MarkdownRenderer,
    /// 11ty's syntax highlighting plugin, when the config added it and it
    /// highlights Markdown.
    pub highlight: Option<&'a crate::plugins::syntaxhighlight::SyntaxHighlight>,
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
        let page_liquid = |undefined: &mut BTreeSet<String>| -> Result<String, Problem> {
            // 11ty skips the engine when the body has no Liquid in it.
            if !job.body.contains("{{") && !job.body.contains("{%") {
                return Ok(job.body.to_string());
            }
            let name = job.shown.display().to_string();
            let t = self
                .engine
                .parse(&name, job.body)
                .map_err(|e| self.problem(&e, job))?;
            let data = Value::Object(Arc::new(job.data.clone()));
            self.liquid(&t, &data, undefined, job)
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
            });
        }
        let html = match job.engines {
            Engines::Raw => job.body.to_string(),
            Engines::Liquid => page_liquid(&mut undefined)?,
            Engines::Markdown { liquid: true } => {
                let source = page_liquid(&mut undefined)?;
                // A line in the page is only known when Liquid changed nothing.
                let line = (source == job.body).then_some(job.body_line);
                self.render_markdown(&source, job.shown, line)?
            }
            Engines::Markdown { liquid: false } => {
                self.render_markdown(job.body, job.shown, Some(job.body_line))?
            }
        };
        Ok(Output { html, undefined })
    }

    /// The page's layout chain around `content`, its rendered content.
    pub fn layouts(&self, job: &Job<'_>, content: &str) -> Result<Output, Problem> {
        let mut undefined = BTreeSet::new();
        let mut content = content.to_string();
        for layout in job.layouts {
            // 11ty renders a layout with `{ content, ...data }`.
            let mut with_content = IndexMap::with_capacity(job.data.len() + 1);
            with_content.insert("content".to_string(), Value::String(content.into()));
            for (k, v) in job.data {
                with_content.insert(k.clone(), v.clone());
            }
            let layout_data = Value::Object(Arc::new(with_content));
            content = match &layout.kind {
                Kind::Liquid(t) => self.liquid(t, &layout_data, &mut undefined, job)?,
                Kind::Markdown(t) => {
                    let out = self.liquid(t, &layout_data, &mut undefined, job)?;
                    self.render_markdown(&out, &layout.shown, None)?
                }
                // Pages with such a layout are refused before they get here.
                Kind::Unrenderable(p) => return Err(p.clone()),
            };
        }
        Ok(Output {
            html: content,
            undefined,
        })
    }

    /// Renders Markdown, turning a panic in the Markdown library into a Problem for this page.
    /// `line` is the line `source` starts after in `file`, when it is the file's own text.
    fn render_markdown(
        &self,
        source: &str,
        file: &Path,
        line: Option<usize>,
    ) -> Result<String, Problem> {
        let render = || {
            let Some(plugin) = self.highlight else {
                return Ok(self.markdown.render(source));
            };
            let mut page = plugin.page();
            let mut failed = None;
            let html = self.markdown.render_highlighted(source, &mut |fence| {
                if failed.is_some() {
                    return None;
                }
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
            });
            match failed {
                Some(p) => Err(p),
                None => Ok(html),
            }
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
    }

    fn liquid(
        &self,
        t: &Template,
        data: &Value,
        undefined: &mut BTreeSet<String>,
        job: &Job<'_>,
    ) -> Result<String, Problem> {
        let r = self
            .engine
            .render_tracked(t, data, self.loader)
            .map_err(|e| self.problem(&e, job))?;
        undefined.extend(r.undefined);
        Ok(r.output)
    }

    /// Where an error came from: the page, one of its layouts, or a partial
    /// the loader found (named by its canonical path).
    fn problem(&self, e: &LiquidError, job: &Job<'_>) -> Problem {
        let template = Path::new(&e.template);
        let (path, offset) = if template == job.shown {
            (job.shown.to_path_buf(), job.body_line)
        } else if let Some(l) = job.layouts.iter().find(|l| l.shown == template) {
            (l.shown.clone(), l.body_line)
        } else if template.is_absolute() {
            (
                self.loader
                    .shown(template)
                    .unwrap_or_else(|| template.to_path_buf()),
                0,
            )
        } else {
            (template.to_path_buf(), 0)
        };
        liquid_problem(e, path, offset, Some(job.shown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engines(
        ext: &str,
        over: Option<serde_json::Value>,
    ) -> Result<(Engines, String), &'static str> {
        engines_for(ext, over.map(|j| Json::from_json(&j)).as_ref()).map_err(|p| p.code)
    }

    /// Probed: `page.templateSyntax` and output for each override.
    #[test]
    fn overrides_like_11ty() {
        use Engines::*;
        let ok = |e, s: &str| Ok((e, s.to_string()));
        assert_eq!(
            engines("md", None),
            ok(Markdown { liquid: true }, "liquid,md")
        );
        assert_eq!(engines("html", None), ok(Liquid, "liquid"));
        assert_eq!(engines("liquid", None), ok(Liquid, "liquid"));
        assert_eq!(
            engines("md", Some(json!("md"))),
            ok(Markdown { liquid: false }, "md")
        );
        assert_eq!(engines("md", Some(json!("liquid"))), ok(Liquid, "liquid"));
        assert_eq!(
            engines("html", Some(json!("liquid,md"))),
            ok(Markdown { liquid: true }, "liquid,md")
        );
        assert_eq!(
            engines("liquid", Some(json!("md,liquid"))),
            ok(Markdown { liquid: true }, "liquid,md")
        );
        assert_eq!(engines("md", Some(json!("html"))), ok(Raw, ""));
        assert_eq!(engines("md", Some(json!(" HTML , "))), ok(Raw, ""));
        assert_eq!(engines("liquid", Some(json!(false))), ok(Raw, "liquid"));
        assert_eq!(engines("liquid", Some(json!(null))), ok(Raw, "liquid"));
        assert_eq!(engines("liquid", Some(json!(""))), ok(Raw, "liquid"));
        assert_eq!(
            engines("md", Some(json!("njk"))),
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

    #[test]
    fn body_lines() {
        assert_eq!(body_line("---\na: 1\n---\nbody", "body"), 3);
        assert_eq!(body_line("\u{feff}---\n---\nb", "b"), 2);
        assert_eq!(body_line("plain", "plain"), 0);
    }
}
