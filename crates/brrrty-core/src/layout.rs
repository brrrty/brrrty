//! Layouts: the front matter `layout` key, resolved in `_includes` the way
//! 11ty 3.1.8's `TemplateLayoutPathResolver` and `TemplateLayout` do it.
//!
//! - A name is tried as written first, then with each template extension in
//!   11ty's default `templateFormats` order: `liquid`, `md`, `njk`, `html`,
//!   `11ty.js` (and `.cjs`, `.mjs`).
//! - A layout's front matter `layout` names the next layout. The chain's data
//!   is merged farthest first, so the nearest layout wins, and the page's own
//!   data wins over all of it.
//! - Layouts ignore `templateEngineOverride`: `.liquid` runs Liquid,
//!   `.njk` Nunjucks, `.html` the config's `htmlTemplateEngine`, and `.md`
//!   its `markdownTemplateEngine` and then Markdown.
//!
//! Where 11ty crashes or reports only the start of a cycle, Brrrty gives one
//! error naming the file: a cycle names every layout in it, and a folder, a
//! file with no template extension, or a `layout` key that is not a name in
//! a layout are errors at that layout.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brrrty_js::Value;

use crate::data::{json_text, key_line};
use crate::frontmatter::{Data, split_front_matter};
use crate::loader::lexical;
use crate::options::TemplateEngines;
use crate::problem::{Problem, codes};
use crate::render::body_line;
use crate::templates::{Compiled, Lang, Langs};

/// What runs on a layout's body.
#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// One template language.
    Template(Compiled),
    /// `.md`: its template language, if any, then Markdown.
    Markdown(Option<Compiled>),
    /// `.html` when `htmlTemplateEngine` is `false`: the body as written.
    Raw,
    /// A JavaScript layout (`.11ty.js`), loaded in the site's Node.
    Js(brrrty_js::runtime::TemplateId),
    /// A layout Brrrty can't render: one that doesn't parse. Its front
    /// matter still joins the page's data and names the next layout, so
    /// the page takes its place in the collections and its content can be
    /// read; writing the page is refused with this problem.
    Unrenderable(Problem),
}

/// A parsed layout file.
#[derive(Debug)]
pub(crate) struct LayoutFile {
    /// Relative to the project root.
    pub shown: PathBuf,
    /// Canonical, to tell layouts apart however they are named.
    pub real: PathBuf,
    pub kind: Kind,
    /// The text after the front matter.
    pub body: String,
    pub data: Data,
    /// Lines in the file before the body.
    pub body_line: usize,
    /// The file line of the front matter `layout` key.
    pub layout_line: Option<usize>,
}

#[derive(Debug, Clone)]
enum Failure {
    /// No file has the name. `tried` lists the paths looked at.
    Missing { tried: Vec<String> },
    /// The name leads outside the input folder.
    Outside,
    /// The file itself is the problem; reported once, at the file.
    File(Problem),
}

/// Why a page's layout chain could not be built.
#[derive(Debug, Clone)]
pub(crate) enum ChainError {
    /// A problem for this page alone.
    Page(Problem),
    /// A problem in a layout file, the same for every page that uses it.
    Layout(Problem),
}

/// Template extensions in 11ty's default `templateFormats` order.
const EXTENSIONS: &[&str] = &[
    "liquid", "md", "njk", "html", "11ty.js", "11ty.cjs", "11ty.mjs",
];

/// Resolves and parses layouts, each once per build.
pub(crate) struct Layouts<'a> {
    langs: &'a Langs<'a>,
    config: TemplateEngines,
    /// Canonical input folder.
    input: PathBuf,
    /// The input folder relative to the project root.
    input_shown: PathBuf,
    /// The layouts folder, relative to the input folder.
    dir: PathBuf,
    /// `addLayoutAlias` aliases.
    aliases: indexmap::IndexMap<String, String>,
    cache: Mutex<HashMap<PathBuf, Result<Arc<LayoutFile>, Failure>>>,
    /// JavaScript layouts loaded in Node, by canonical path: the instance
    /// and its data, or why it can't be used. `None` when JavaScript
    /// doesn't run.
    js: Option<JsLayouts>,
}

/// JavaScript layouts loaded in Node, by canonical path: each instance's id
/// and its data, or why it can't be used.
pub(crate) type JsLayouts =
    HashMap<PathBuf, Result<(brrrty_js::runtime::TemplateId, Data), Problem>>;

/// The help for a JavaScript template or layout while JavaScript is off.
pub(crate) const JS_OFF_HELP: &str = "JavaScript templates run in your site's own Node: add `config.set_js_data(\"node\");` to brrrty.config.rhai, or build with --js-data=node. Liquid, Nunjucks, Markdown and HTML work without it.";

/// The JavaScript spelling of a front matter value, for messages.
fn shown_value(v: &Value) -> String {
    json_text(v)
}

impl<'a> Layouts<'a> {
    pub fn new(
        langs: &'a Langs<'a>,
        config: TemplateEngines,
        input: PathBuf,
        input_shown: PathBuf,
        dir: PathBuf,
        aliases: indexmap::IndexMap<String, String>,
    ) -> Layouts<'a> {
        Layouts {
            langs,
            config,
            input,
            input_shown,
            dir,
            aliases,
            cache: Mutex::new(HashMap::new()),
            js: None,
        }
    }

    /// Every JavaScript layout file in the layouts folder (`.11ty.js`,
    /// `.cjs`, `.mjs`), in path order: they load with the data, before any
    /// page asks for one.
    pub fn js_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![self.input.join(&self.dir)];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                match e.file_type() {
                    Ok(t) if t.is_dir() => stack.push(path),
                    Ok(_) => {
                        let name = e.file_name();
                        if crate::page::without_js_extension(&name.to_string_lossy()).is_some() {
                            out.push(path);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        out.sort();
        out
    }

    /// The file `name` resolves to, as [`Layouts::chain`] would find it,
    /// without parsing or caching it: `None` when there is none.
    pub fn find_file(&self, name: &str) -> Option<PathBuf> {
        let name = self.aliases.get(name).map_or(name, String::as_str);
        let rel = lexical(&format!("{}/{name}", self.dir.display()))?;
        let mut candidates = vec![rel.clone()];
        for ext in EXTENSIONS {
            let mut with = rel.as_os_str().to_os_string();
            with.push(format!(".{ext}"));
            candidates.push(PathBuf::from(with));
        }
        candidates
            .into_iter()
            .map(|c| self.input.join(c))
            .find(|c| c.exists())
            .filter(|c| c.is_file())
    }

    /// The JavaScript layouts as loaded, by canonical path.
    pub fn set_js(&mut self, js: JsLayouts) {
        self.js = Some(js);
    }

    /// The layouts folder, relative to the project root.
    pub fn dir_shown(&self) -> PathBuf {
        self.input_shown.join(&self.dir)
    }

    /// The layout chain for a page whose front matter has `layout: name`,
    /// nearest layout first. `page` is the page's root-relative path and
    /// `page_text` its whole source, for the line of its `layout` key.
    pub fn chain(
        &self,
        name: &str,
        page: &Path,
        page_text: &str,
    ) -> Result<Vec<Arc<LayoutFile>>, ChainError> {
        // Each layout with the name its referrer used for it.
        let mut chain: Vec<(Arc<LayoutFile>, String)> = Vec::new();
        let mut name = name.to_string();
        loop {
            let layout = match self.resolve(&name) {
                Ok(l) => l,
                Err(Failure::File(p)) => return Err(ChainError::Layout(p)),
                Err(failure) => {
                    let (code, message, help) = match failure {
                        Failure::Missing { tried } => (
                            codes::LAYOUT_MISSING,
                            format!("layout `{name}` not found"),
                            format!(
                                "layouts are looked up in {}; tried: {}",
                                self.dir_shown().display(),
                                tried.join(", ")
                            ),
                        ),
                        _ => (
                            codes::LAYOUT_INVALID,
                            format!("layout `{name}` is outside the input folder"),
                            "keep layouts inside the input folder, usually in _includes"
                                .to_string(),
                        ),
                    };
                    let p = Problem::error(code, message).with_help(help);
                    return Err(match chain.last() {
                        None => ChainError::Page(at_line(
                            p.with_path(page),
                            key_line(page_text, "layout"),
                        )),
                        Some((from, _)) => {
                            ChainError::Layout(at_line(p.with_path(&from.shown), from.layout_line))
                        }
                    });
                }
            };
            if let Some(start) = chain.iter().position(|(l, _)| l.real == layout.real) {
                return Err(ChainError::Layout(cycle(&chain[start..], &name)));
            }
            let next = layout.data.get("layout").cloned();
            chain.push((layout.clone(), name));
            match next {
                None => break,
                Some(Value::String(s)) if !s.is_empty() => name = s.to_string(),
                Some(other) => {
                    return Err(ChainError::Layout(at_line(
                        Problem::error(
                            codes::LAYOUT_INVALID,
                            format!(
                                "`layout: {}` in a layout must name another layout",
                                shown_value(&other)
                            ),
                        )
                        .with_path(&layout.shown)
                        .with_help("remove the `layout` line to end the layout chain here"),
                        layout.layout_line,
                    )));
                }
            }
        }
        Ok(chain.into_iter().map(|(l, _)| l).collect())
    }

    fn resolve(&self, name: &str) -> Result<Arc<LayoutFile>, Failure> {
        // An alias stands for a file's name; names are relative to the
        // layouts folder, which is inside the input folder.
        let name = self.aliases.get(name).map_or(name, String::as_str);
        let rel = lexical(&format!("{}/{name}", self.dir.display())).ok_or(Failure::Outside)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = cache.get(&rel) {
            return found.clone();
        }
        // Parsing while holding the lock keeps every layout parsed once;
        // layouts are few and small.
        let result = self.load(&rel);
        cache.insert(rel, result.clone());
        result
    }

    fn load(&self, rel: &Path) -> Result<Arc<LayoutFile>, Failure> {
        let mut tried = Vec::new();
        let mut candidates = vec![rel.to_path_buf()];
        for ext in EXTENSIONS {
            let mut with = rel.as_os_str().to_os_string();
            with.push(format!(".{ext}"));
            candidates.push(PathBuf::from(with));
        }
        let Some(found) = candidates.into_iter().find(|c| {
            tried.push(self.input_shown.join(c).display().to_string());
            // 11ty's check counts folders too.
            self.input.join(c).exists()
        }) else {
            return Err(Failure::Missing { tried });
        };
        let shown = self.input_shown.join(&found);
        let file = |code, message: String, help: &str| {
            Failure::File(
                Problem::error(code, message)
                    .with_path(&shown)
                    .with_help(help),
            )
        };
        let full = self.input.join(&found);
        if full.is_dir() {
            return Err(file(
                codes::LAYOUT_INVALID,
                format!("layout {} is a folder", shown.display()),
                "name a layout file, like `layout: name` for _includes/name.liquid",
            ));
        }
        let real = full.canonicalize().map_err(|e| {
            file(
                codes::LAYOUT_INVALID,
                format!("cannot read layout: {e}"),
                "check the file's permissions",
            )
        })?;
        if !real.starts_with(&self.input) {
            return Err(file(
                codes::LAYOUT_INVALID,
                format!(
                    "layout {} links to a file outside the input folder",
                    shown.display()
                ),
                "keep layouts inside the input folder, usually in _includes",
            ));
        }
        let lower = found.to_string_lossy().to_lowercase();
        let language = if lower.ends_with(".liquid") {
            Language::Template(Some(Lang::Liquid))
        } else if lower.ends_with(".njk") {
            Language::Template(Some(Lang::Nunjucks))
        } else if lower.ends_with(".html") {
            Language::Template(self.config.html)
        } else if lower.ends_with(".md") {
            Language::Markdown(self.config.markdown)
        } else if [".11ty.js", ".11ty.cjs", ".11ty.mjs"]
            .iter()
            .any(|x| lower.ends_with(x))
        {
            return match self.js.as_ref().and_then(|js| js.get(&real)) {
                Some(Ok((id, data))) => Ok(Arc::new(LayoutFile {
                    shown,
                    real,
                    kind: Kind::Js(*id),
                    body: String::new(),
                    data: data.clone(),
                    body_line: 0,
                    layout_line: None,
                })),
                Some(Err(p)) => Err(Failure::File(p.clone())),
                None if self.js.is_some() => Err(file(
                    codes::LAYOUT_UNSUPPORTED,
                    "this JavaScript layout is named in a way Brrrty didn't follow before the pages loaded, so it wasn't loaded".to_string(),
                    "Name it with `layout` in front matter, a data file or a layout's data. This is a gap in Brrrty; please report it.",
                )),
                None => Err(file(
                    codes::LAYOUT_UNSUPPORTED,
                    "this JavaScript layout doesn't run while JavaScript is off".to_string(),
                    JS_OFF_HELP,
                )),
            };
        } else {
            return Err(file(
                codes::LAYOUT_INVALID,
                format!("layout {} has no template file extension", shown.display()),
                "rename it to end in .liquid, .njk, .md or .html",
            ));
        };
        let text = fs::read(&real)
            .map_err(|e| {
                file(
                    codes::TEMPLATE_UNREADABLE,
                    format!("cannot read layout: {e}"),
                    "check the file's permissions",
                )
            })
            .and_then(|b| {
                String::from_utf8(b).map_err(|_| {
                    file(
                        codes::TEMPLATE_NOT_UTF8,
                        "layout is not valid UTF-8".to_string(),
                        "save the file as UTF-8",
                    )
                })
            })?;
        let (data, body) =
            split_front_matter(&text).map_err(|p| Failure::File(p.with_path(&shown)))?;
        let body_line = body_line(&text, body);
        // A layout that doesn't parse still has its data: it only refuses
        // writing its pages.
        let compile = |lang: Lang| {
            self.langs
                .compile(lang, &shown.display().to_string(), Some(&real), body)
                .map_err(|e| {
                    Kind::Unrenderable(self.langs.problem(&e, shown.clone(), body_line, None))
                })
        };
        let kind = match language {
            Language::Template(Some(lang)) => compile(lang).map_or_else(|k| k, Kind::Template),
            Language::Template(None) => Kind::Raw,
            Language::Markdown(Some(lang)) => {
                compile(lang).map_or_else(|k| k, |t| Kind::Markdown(Some(t)))
            }
            Language::Markdown(None) => Kind::Markdown(None),
        };
        Ok(Arc::new(LayoutFile {
            layout_line: key_line(&text, "layout"),
            shown,
            real,
            kind,
            body: body.to_string(),
            data,
            body_line,
        }))
    }
}

/// What runs on a layout file: a template language (`None` for `.html`
/// with no `htmlTemplateEngine`), or Markdown after one.
enum Language {
    Template(Option<Lang>),
    Markdown(Option<Lang>),
}

impl LayoutFile {
    /// Why Brrrty can't render this layout, if it can't.
    pub fn unrenderable(&self) -> Option<&Problem> {
        match &self.kind {
            Kind::Unrenderable(p) => Some(p),
            _ => None,
        }
    }
}

fn at_line(p: Problem, line: Option<usize>) -> Problem {
    match line {
        Some(l) => p.at(l, 1),
        None => p,
    }
}

/// The error for a cycle: `members` are the layouts in it, each with the
/// name it was reached by, and `back` is the name the last one uses for the
/// first. The cycle is shown from its first file in path order, so every
/// page that runs into it gets the same error.
fn cycle(members: &[(Arc<LayoutFile>, String)], back: &str) -> Problem {
    let k = members.len();
    // How each member is named by the one before it in the cycle.
    let names: Vec<&str> = (0..k)
        .map(|j| if j == 0 { back } else { members[j].1.as_str() })
        .collect();
    let start = (0..k)
        .min_by(|a, b| members[*a].0.shown.cmp(&members[*b].0.shown))
        .unwrap_or(0);
    let order: Vec<usize> = (0..k).map(|j| (start + j) % k).collect();
    let mut chain: Vec<&str> = order.iter().map(|&j| names[j]).collect();
    chain.push(names[start]);
    let last = &members[order[k - 1]].0;
    at_line(
        Problem::error(
            codes::LAYOUT_CYCLE,
            format!("layout cycle: {}", chain.join(" -> ")),
        )
        .with_path(&last.shown)
        .with_help("remove or change one `layout` line in the cycle"),
        last.layout_line,
    )
}
