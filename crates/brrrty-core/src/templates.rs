//! The template languages Brrrty runs, Liquid and Nunjucks, each with its
//! engine and the loader 11ty gives it, behind one interface.
//!
//! Every Nunjucks compile and render runs inside [`crate::contain::catch`]:
//! a panic in the engine refuses the page instead of ending the build.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use brrrty_js::{Loader, Value};
use brrrty_liquid::LiquidError;
use brrrty_nunjucks::NunjucksError;

use crate::loader::FsLoader;
pub(crate) use crate::options::TemplateLanguage as Lang;
use crate::problem::{Problem, codes};

/// The help on a problem that is a bug in Brrrty's Nunjucks engine.
const NUNJUCKS_BUG_HELP: &str = "this is a bug in Brrrty's Nunjucks engine, not in your template. Please report the template at https://github.com/brrrty/brrrty/issues";

/// Both engines and their loaders.
pub(crate) struct Langs<'a> {
    pub liquid: &'a brrrty_liquid::Engine,
    /// `_includes`, then the input folder.
    pub liquid_loader: &'a FsLoader,
    pub nunjucks: &'a brrrty_nunjucks::Engine,
    /// `_includes`, then the project folder.
    pub nunjucks_loader: &'a FsLoader,
    /// The config added 11ty's syntax highlighting plugin for Nunjucks,
    /// whose paired `{% highlight %}` shortcode is refused.
    pub nunjucks_highlight: bool,
}

/// A template ready to render.
#[derive(Clone)]
pub(crate) enum Compiled {
    Liquid(brrrty_liquid::Template),
    Nunjucks(brrrty_nunjucks::Compiled),
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Compiled::Liquid(t) => f.debug_tuple("Liquid").field(t).finish(),
            Compiled::Nunjucks(c) => f.debug_tuple("Nunjucks").field(&c.name()).finish(),
        }
    }
}

/// An error from either engine.
#[derive(Debug)]
pub(crate) enum TplError {
    Liquid(LiquidError),
    Nunjucks(NunjucksError),
}

impl TplError {
    /// The name of the template the error is in, as the engine knows it.
    pub fn template(&self) -> &str {
        match self {
            TplError::Liquid(e) => &e.template,
            TplError::Nunjucks(e) => &e.template,
        }
    }
}

/// What a render gave.
pub(crate) struct Rendered {
    pub output: String,
    pub undefined: Vec<String>,
}

/// Whether `source` has anything `lang` would run: 11ty leaves a string
/// without its delimiters as written (`needsCompilation`).
pub(crate) fn has_tags(lang: Lang, source: &str) -> bool {
    source.contains("{{")
        || source.contains("{%")
        || (lang == Lang::Nunjucks && source.contains("{#"))
}

/// A panic in the Nunjucks engine, as one of its errors.
fn crashed(template: &str, why: String) -> NunjucksError {
    NunjucksError {
        kind: brrrty_nunjucks::ErrorKind::Render,
        message: format!("Brrrty's Nunjucks engine crashed: {why}"),
        template: template.to_string(),
        line: None,
        column: None,
        help: Some(NUNJUCKS_BUG_HELP.to_string()),
    }
}

impl Langs<'_> {
    /// Compiles `source`, named `name` in errors. `path` is the template's
    /// canonical file, which Nunjucks resolves `./` names against.
    pub fn compile(
        &self,
        lang: Lang,
        name: &str,
        path: Option<&Path>,
        source: &str,
    ) -> Result<Compiled, TplError> {
        match lang {
            Lang::Liquid => self
                .liquid
                .parse(name, source)
                .map(Compiled::Liquid)
                .map_err(TplError::Liquid),
            Lang::Nunjucks => {
                let compiled = crate::contain::catch(|| match path {
                    Some(p) => self
                        .nunjucks
                        .compile_at(name, &p.display().to_string(), source),
                    None => self.nunjucks.compile(name, source),
                });
                match compiled {
                    Ok(r) => r.map(Compiled::Nunjucks).map_err(TplError::Nunjucks),
                    Err(why) => Err(TplError::Nunjucks(crashed(name, why))),
                }
            }
        }
    }

    /// Renders `t` with `data`.
    pub fn render(&self, t: &Compiled, data: &Value) -> Result<Rendered, TplError> {
        match t {
            Compiled::Liquid(t) => self
                .liquid
                .render_tracked(t, data, self.liquid_loader)
                .map(|r| Rendered {
                    output: r.output,
                    undefined: r.undefined,
                })
                .map_err(TplError::Liquid),
            Compiled::Nunjucks(c) => {
                let r = crate::contain::catch(|| {
                    self.nunjucks.render_tracked(c, data, self.nunjucks_loader)
                });
                match r {
                    Ok(Ok(r)) => Ok(Rendered {
                        output: r.output,
                        undefined: r.undefined,
                    }),
                    Ok(Err(e)) => Err(TplError::Nunjucks(e)),
                    Err(why) => Err(TplError::Nunjucks(crashed(c.name(), why))),
                }
            }
        }
    }

    /// Compiles and renders a string that has no file, such as a
    /// permalink, adding what it read undefined to `undefined`.
    pub fn render_string(
        &self,
        lang: Lang,
        name: &str,
        source: &str,
        data: &Value,
        undefined: &mut BTreeSet<String>,
    ) -> Result<String, TplError> {
        let t = self.compile(lang, name, None, source)?;
        let r = self.render(&t, data)?;
        undefined.extend(r.undefined);
        Ok(r.output)
    }

    /// Where a template an engine named lives, relative to the project
    /// root, when the loader found it: Liquid names partials by their
    /// canonical path, Nunjucks by the name the template wrote.
    pub fn shown(&self, e: &TplError) -> Option<PathBuf> {
        let name = Path::new(e.template());
        let loader = match e {
            TplError::Liquid(_) => self.liquid_loader,
            TplError::Nunjucks(_) => self.nunjucks_loader,
        };
        if name.is_absolute()
            && let Some(shown) = loader.shown(name)
        {
            return Some(shown);
        }
        match e {
            TplError::Nunjucks(_) => {
                let (real, _) = loader.load(e.template()).ok()?;
                loader.shown(Path::new(&real))
            }
            TplError::Liquid(_) => None,
        }
    }

    /// The error as a problem in `path`, whose text starts after `offset`
    /// lines. `page` names the page being rendered when it is another file.
    pub fn problem(
        &self,
        e: &TplError,
        path: PathBuf,
        offset: usize,
        page: Option<&Path>,
    ) -> Problem {
        let p = match e {
            TplError::Liquid(e) => crate::render::liquid_problem(e, path, offset, page),
            TplError::Nunjucks(e) => nunjucks_problem(e, path, offset, page),
        };
        highlight_tag_problem(p, self.nunjucks_highlight)
    }
}

/// A Nunjucks error as a problem in `path`, whose text starts after
/// `offset` lines.
pub(crate) fn nunjucks_problem(
    e: &NunjucksError,
    path: PathBuf,
    offset: usize,
    page: Option<&Path>,
) -> Problem {
    use brrrty_nunjucks::ErrorKind;
    let code = match e.kind {
        ErrorKind::Syntax | ErrorKind::TooDeep => codes::NUNJUCKS_SYNTAX,
        ErrorKind::Unsupported => codes::NUNJUCKS_UNSUPPORTED,
        ErrorKind::Limit => codes::NUNJUCKS_LIMIT,
        _ => codes::NUNJUCKS_RENDER,
    };
    let message = match page {
        Some(page) if page != path => {
            format!("{} while rendering {}", e.message, page.display())
        }
        _ => e.message.clone(),
    };
    let mut p = Problem::error(code, message).with_path(path);
    if let Some(line) = e.line {
        p = p.at(line + offset, e.column.unwrap_or(1));
    }
    if let Some(help) = &e.help {
        p = p.with_help(help.clone());
    }
    p
}

/// A `{% highlight %}` in a Nunjucks template, in a site whose syntax
/// highlighting plugin adds that paired shortcode to Nunjucks: said as
/// such, not as an unknown tag. Liquid's tag, and Markdown's fences, are
/// highlighted.
pub(crate) fn highlight_tag_problem(p: Problem, nunjucks_highlight: bool) -> Problem {
    if nunjucks_highlight
        && p.code == codes::NUNJUCKS_SYNTAX
        && p.message
            .strip_prefix("unknown block tag: highlight")
            .is_some_and(|rest| {
                !rest.starts_with(|c: char| c == '_' || c == '$' || c.is_alphanumeric())
            })
    {
        Problem {
            code: codes::SYNTAX_HIGHLIGHT_UNSUPPORTED,
            message: "this page uses `{% highlight %}` in Nunjucks, the syntax highlighting plugin's paired shortcode, which Brrrty doesn't run yet".to_string(),
            help: Some(format!(
                "Brrrty highlights Markdown code fences and Liquid's `{{% highlight %}}`. {}",
                crate::render::HIGHLIGHT_HELP
            )),
            ..p
        }
    } else {
        p
    }
}
