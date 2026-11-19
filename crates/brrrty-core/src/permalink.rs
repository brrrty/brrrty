//! Permalink resolution: where a page is written and what its URL is.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use brrrty_js::Value;

use crate::frontmatter::Data;
use crate::problem::{Problem, codes};
use crate::render::DataEngine;
use crate::templates::{Langs, TplError};

/// The outcome of resolving a page's permalink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permalink {
    /// Write the page to `output_rel` (relative to the output folder), served at `url`.
    Write { url: String, output_rel: PathBuf },
    /// `permalink: false`: write nothing.
    Skip,
}

/// Resolve the URL and output path for a page from its `permalink` as
/// written, with no template rendered. [`Locator`] renders one first.
///
/// `rel_to_input` is the source file path relative to the input directory.
pub fn resolve_permalink(rel_to_input: &Path, front_matter: &Data) -> Result<Permalink, Problem> {
    resolve_value(
        rel_to_input,
        front_matter.get("permalink"),
        front_matter,
        None,
    )
}

/// Resolve a rendered `permalink` value. `data` is only read for
/// `eleventyAllowMissingExtension`. `subdir` is a generated page's number,
/// the folder 11ty adds to a default permalink.
fn resolve_value(
    rel: &Path,
    permalink: Option<&Value>,
    data: &Data,
    subdir: Option<usize>,
) -> Result<Permalink, Problem> {
    let default_permalink = |rel: &Path| default_permalink(rel, subdir);
    match permalink {
        None => Ok(default_permalink(rel)),
        Some(Value::Bool(false)) => Ok(Permalink::Skip),
        Some(Value::Bool(true)) => Err(Problem::error(
            codes::PERMALINK_INVALID,
            "`permalink: true` is not valid",
        )
        .with_path(rel)
        .with_help("Did you mean `permalink: false`?")),
        Some(Value::Null) => Ok(default_permalink(rel)),
        Some(Value::String(s)) if s.is_empty() => Ok(default_permalink(rel)),
        Some(Value::String(s)) => explicit_permalink(rel, s, data),
        // A map: `build` is the path; with none, nothing is written (probed).
        Some(Value::Object(o)) if o.is_empty() => Ok(default_permalink(rel)),
        Some(Value::Object(o)) => match o.get("build") {
            None => Ok(Permalink::Skip),
            Some(Value::String(s)) if s.is_empty() => {
                Err(empty_permalink("`permalink.build`", rel))
            }
            Some(Value::String(s)) => explicit_permalink(rel, s, data),
            Some(_) => Err(map_unsupported(rel)),
        },
        Some(other) => Err(Problem::error(
            codes::PERMALINK_INVALID,
            format!(
                "`permalink` must be a string or `false`, found {}",
                kind(other)
            ),
        )
        .with_path(rel)),
    }
}

/// 11ty crashes writing a permalink that renders to nothing (probed).
fn empty_permalink(what: &str, rel: &Path) -> Problem {
    Problem::error(
        codes::PERMALINK_INVALID,
        format!("{what} rendered to an empty string"),
    )
    .with_path(rel)
    .with_help("11ty stops the whole build here. To skip writing a page, use `permalink: false`.")
}

fn map_unsupported(rel: &Path) -> Problem {
    Problem::error(
        codes::PERMALINK_UNSUPPORTED,
        "This `permalink` map isn't supported: only `build` and other keys set to a string work",
    )
    .with_path(rel)
    .with_help("Write `permalink` as a string, or a map whose values are strings. 11ty itself writes a file named `false` for `build: false`; use `permalink: false` to skip a page.")
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a map",
        Value::Undefined => "undefined",
        Value::Date(_) => "a date",
        Value::Function(_) => "a function",
    }
}

fn build(segments: &[String], trailing_slash: bool) -> Permalink {
    let mut url = String::from("/");
    url.push_str(&segments.join("/"));
    if trailing_slash && !segments.is_empty() {
        url.push('/');
    }
    // 11ty's `normalizePathToUrl`: a URL never ends in `index.html` or a
    // bare `index` (probed: `permalink: /a/index.html` has the URL `/a/`).
    for tail in ["/index.html", "/index"] {
        if let Some(stem) = url.strip_suffix(tail) {
            url = format!("{stem}/");
            break;
        }
    }
    let mut output_rel: PathBuf = segments.iter().collect();
    if trailing_slash {
        output_rel.push("index.html");
    }
    Permalink::Write { url, output_rel }
}

fn default_permalink(rel: &Path, subdir: Option<usize>) -> Permalink {
    let mut segments: Vec<String> = rel
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    // 11ty's `_hasDuplicateFolder`: `posts/posts.md` is the folder's index.
    // `.11ty.js` comes off whole (11ty's `removeTemplateExtension`).
    let stem = rel.file_name().map(|n| {
        let n = n.to_string_lossy();
        match crate::page::without_js_extension(&n) {
            Some(base) => base.to_string(),
            None => rel
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    });
    if let Some(stem) = stem
        && stem != "index"
        && segments.last() != Some(&stem)
    {
        segments.push(stem);
    }
    // Pagination's extra folder goes before `index.html`.
    if let Some(n) = subdir {
        segments.push(n.to_string());
    }
    build(&segments, true)
}

fn explicit_permalink(rel: &Path, raw: &str, front_matter: &Data) -> Result<Permalink, Problem> {
    let is_drive = |seg: &str| {
        let b = seg.as_bytes();
        b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    };
    if raw.contains('\\') || raw.split('/').any(is_drive) {
        return Err(Problem::error(
            codes::PERMALINK_INVALID,
            format!("Permalink `{raw}` contains a backslash or drive letter"),
        )
        .with_path(rel)
        .with_help(
            "Permalinks use forward slashes and can't contain backslashes or drive letters.",
        ));
    }
    let last = raw.rsplit('/').next().unwrap_or("");
    let mut trailing_slash = last.is_empty() || last == "." || last == "..";
    // Resolve like URL normalization, clamped at the root.
    let mut segments: Vec<String> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s.to_string()),
        }
    }
    if segments.is_empty() {
        trailing_slash = true;
    }
    // 11ty checks this flag with JavaScript truthiness, so `"yes"` and `1` count too.
    let allow_missing = front_matter
        .get("eleventyAllowMissingExtension")
        .is_some_and(crate::js::js_truthy);
    if !trailing_slash
        && !allow_missing
        && !matches!(
            segments[segments.len() - 1].as_str(),
            "_redirects" | ".htaccess" | "_headers"
        )
        && Path::new(&segments[segments.len() - 1])
            .extension()
            .is_none()
    {
        return Err(Problem::error(
            codes::PERMALINK_NO_EXTENSION,
            format!("Permalink `{raw}` has no file extension"),
        )
        .with_path(rel)
        .with_help(
            "Add a file extension (like /dir/noext.html), end it with / to write an index.html, or set `eleventyAllowMissingExtension: true` to write the file as named.",
        ));
    }
    Ok(build(&segments, trailing_slash))
}

/// Where a page is written, and what `page.url` and `page.outputPath` hold.
#[derive(Debug, Clone)]
pub(crate) struct Located {
    pub permalink: Permalink,
    /// `page.url`: the URL, or `false` when nothing is written.
    pub url: Value,
    /// `page.outputPath`: the path as 11ty spells it, or `false`.
    pub output_path: Value,
    /// What `pagination.links` lists for the page: `"/"` and the output
    /// path as the permalink spells it, or `None` when nothing is written.
    pub link: Option<String>,
}

/// Renders a page's permalink against its data and resolves it. Built once
/// per page; pagination calls [`Locator::locate`] again for each page it
/// generates, with that page's data.
#[derive(Clone)]
pub(crate) struct Locator<'a> {
    pub langs: &'a Langs<'a>,
    /// What renders the permalink.
    pub data_engine: DataEngine,
    /// The page has computed data, which renders other strings too.
    pub computed: bool,
    /// The source file, relative to the input folder.
    pub rel: &'a Path,
    /// The source file as problems show it.
    pub shown: &'a Path,
    /// The output folder as 11ty spells it in `page.outputPath`.
    pub output_shown: &'a str,
    /// The data file or layout `permalink` comes from, when not the page.
    pub from: Option<String>,
    /// The line of `permalink:` in the page's front matter.
    pub line: Option<usize>,
    /// For a generated page after the first, when no `permalink` places
    /// it: its number, the folder 11ty adds to the default permalink.
    pub subdir: Option<usize>,
    /// The page's top-level keys Brrrty can't supply, and why: a
    /// `permalink` function that reads one is refused.
    pub traps: &'a [(String, String)],
}

impl Locator<'_> {
    /// Render and resolve the permalink in `data`, which is what 11ty's
    /// template data holds at that point: `permalink` is the template as
    /// written, and `page.url` and `page.outputPath` are not set yet.
    /// Variables read but undefined are added to `undefined`.
    pub fn locate(
        &self,
        data: &Data,
        undefined: &mut BTreeSet<String>,
    ) -> Result<Located, Problem> {
        self.locate_in(data, undefined, None)
    }

    /// [`Locator::locate`], with the page's data kept in the JavaScript
    /// runtime by its computed data: a `permalink` function is called with
    /// that object, as 11ty calls it with the page's live data.
    pub fn locate_in(
        &self,
        data: &Data,
        undefined: &mut BTreeSet<String>,
        page: Option<&mut crate::data::JsPage<'_>>,
    ) -> Result<Located, Problem> {
        self.locate_inner(data, undefined, page).map_err(|mut p| {
            p.path = Some(self.shown.to_path_buf());
            if p.line.is_none() && self.from.is_none() {
                p.line = self.line;
            }
            p
        })
    }

    fn locate_inner(
        &self,
        data: &Data,
        undefined: &mut BTreeSet<String>,
        page: Option<&mut crate::data::JsPage<'_>>,
    ) -> Result<Located, Problem> {
        if data
            .get("permalinkBypassOutputDir")
            .is_some_and(crate::js::js_truthy)
        {
            return Err(Problem::error(
                codes::PERMALINK_UNSUPPORTED,
                "`permalinkBypassOutputDir` writes outside the output folder",
            )
            .with_help("Brrrty only writes inside the output folder. Remove the key."));
        }
        // 11ty reads `permalink ?? eleventyComputed.permalink`.
        let value = match data.get("permalink") {
            None | Some(Value::Null | Value::Undefined) => match data.get("eleventyComputed") {
                Some(Value::Object(c)) => c.get("permalink"),
                _ => None,
            },
            v => v,
        };
        // `dynamicPermalink: false` writes the permalink as it is.
        let render = !matches!(data.get("dynamicPermalink"), Some(Value::Bool(false)));
        let compiles = |s: &str| -> Result<bool, Problem> {
            if !render {
                return Ok(false);
            }
            self.data_engine.compiles(s, self.computed)
        };
        let rendered = match value {
            Some(Value::String(s)) if compiles(s)? => {
                let out = self.render(s, data, undefined)?;
                if out.is_empty() {
                    return Err(empty_permalink("`permalink`", self.rel));
                }
                Some(Value::String(out.into()))
            }
            Some(Value::Object(o)) if !o.is_empty() => {
                let mut out = Data::with_capacity(o.len());
                for (key, v) in Value::js_entries(o) {
                    let Value::String(s) = v else {
                        return Err(map_unsupported(self.rel));
                    };
                    let text = if compiles(s)? {
                        self.render(s, data, undefined)?
                    } else {
                        s.to_string()
                    };
                    out.insert(key.clone(), Value::String(text.into()));
                }
                Some(Value::Object(Arc::new(out)))
            }
            Some(Value::Function(f)) => {
                if !render {
                    return Err(Problem::error(
                        codes::PERMALINK_UNSUPPORTED,
                        "`permalink` is a function and `dynamicPermalink` is false, so 11ty uses the function itself as the permalink",
                    )
                    .with_help("Remove `dynamicPermalink: false`, or make `permalink` a string."));
                }
                self.call(*f, data, page)?
            }
            other => other.cloned(),
        };
        let permalink = resolve_value(self.rel, rendered.as_ref(), data, self.subdir)?;
        let (url, output_path) = match &permalink {
            Permalink::Write { url, output_rel } => {
                let path: Vec<String> = output_rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                (
                    Value::String(url.clone().into()),
                    Value::String(format!("{}{}", self.output_shown, path.join("/")).into()),
                )
            }
            Permalink::Skip => (Value::Bool(false), Value::Bool(false)),
        };
        let link = link(rendered.as_ref(), &permalink);
        Ok(Located {
            permalink,
            url,
            output_path,
            link,
        })
    }

    /// Call a `permalink` function with the page's data, as 11ty's
    /// `renderPermalink` does: its result is the permalink, not rendered
    /// again.
    fn call(
        &self,
        f: brrrty_js::FnHandle,
        data: &Data,
        page: Option<&mut crate::data::JsPage<'_>>,
    ) -> Result<Option<Value>, Problem> {
        let result = match page {
            Some(p) => p.call(f, None)?,
            None => {
                let calls = crate::data::js_calls().ok_or_else(|| {
                    Problem::error(
                        codes::JS_DATA_FUNCTION,
                        "`permalink` is a function, and JavaScript data isn't running",
                    )
                })?;
                calls.page_once(
                    &Value::Object(Arc::new(data.clone())),
                    self.traps,
                    f,
                    self.shown,
                )?
            }
        };
        let returned = |what: &str| {
            Problem::error(
                codes::PERMALINK_INVALID,
                format!("the `permalink` function returned {what}"),
            )
            .with_help("Return a string, `false` to write nothing, or `undefined` for the default permalink.")
        };
        match result {
            // 11ty's default permalink when the function gives nothing.
            Value::Undefined => Ok(None),
            Value::String(s) if s.is_empty() => Err(empty_permalink(
                "the `permalink` function's result",
                self.rel,
            )),
            v @ (Value::String(_) | Value::Bool(_) | Value::Object(_)) => Ok(Some(v)),
            other => Err(returned(kind(&other))),
        }
    }

    fn render(
        &self,
        template: &str,
        data: &Data,
        undefined: &mut BTreeSet<String>,
    ) -> Result<String, Problem> {
        let fail = |e: TplError| {
            let mut p = self.langs.problem(&e, self.shown.to_path_buf(), 0, None);
            // The line is inside the permalink, not the file.
            p.line = None;
            p.column = None;
            p.message = match &self.from {
                Some(from) => format!("{} in `permalink` from {from}", p.message),
                None => format!("{} in `permalink`", p.message),
            };
            if p.code == codes::LIQUID_UNKNOWN_FILTER {
                let note = "Filters from your 11ty config aren't available yet: custom filters come with the Rhai config.";
                p.help = Some(match p.help.take() {
                    Some(hint) => format!("{hint} {note}"),
                    None => note.to_string(),
                });
            }
            p
        };
        let lang = self
            .data_engine
            .lang
            .expect("only a page with a template language compiles");
        let scope = Value::Object(Arc::new(data.clone()));
        self.langs
            .render_string(lang, "permalink", template, &scope, undefined)
            .map_err(fail)
    }
}

/// `"/"` plus 11ty's `toOutputPath()`: the permalink as written, with
/// `index.html` after a final `/` and Node's path normalization, so `/a/`
/// gives `//a/index.html` (probed). A default permalink is the output path.
fn link(rendered: Option<&Value>, permalink: &Permalink) -> Option<String> {
    let Permalink::Write { output_rel, .. } = permalink else {
        return None;
    };
    let build = match rendered {
        Some(Value::String(s)) if !s.is_empty() => Some(s.as_ref()),
        Some(Value::Object(o)) => match o.get("build") {
            Some(Value::String(s)) if !s.is_empty() => Some(s.as_ref()),
            _ => None,
        },
        _ => None,
    };
    let path = match build {
        Some(s) if s.ends_with('/') => node_normalize(&format!("{s}index.html")),
        Some(s) => node_normalize(s),
        None => output_rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    };
    Some(format!("/{path}"))
}

/// Node's `path.normalize` with eleventy-utils' trailing-slash trim: `.`
/// steps go, `..` steps remove the one before (or stay at the start of a
/// relative path), and runs of `/` are one.
fn node_normalize(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as Json, json};

    fn w(url: &str, out: &str) -> Permalink {
        Permalink::Write {
            url: url.into(),
            output_rel: out.into(),
        }
    }
    fn fm(v: Json) -> Data {
        let mut m = Data::new();
        m.insert("permalink".into(), Value::from_json(&v));
        m
    }
    fn r(v: Json) -> Result<Permalink, Problem> {
        resolve_permalink(Path::new("c.md"), &fm(v))
    }

    /// Probed: a file named like its folder is the folder's index, matched
    /// with case, and not at the top of the input folder.
    #[test]
    fn a_file_named_like_its_folder_is_its_index() {
        let none = Data::new();
        for (rel, url, out) in [
            ("posts/posts.md", "/posts/", "posts/index.html"),
            ("a/b/b.md", "/a/b/", "a/b/index.html"),
            ("a/a.liquid", "/a/", "a/index.html"),
            ("Up/up.md", "/Up/up/", "Up/up/index.html"),
            (
                "x/2020-01-01-x.md",
                "/x/2020-01-01-x/",
                "x/2020-01-01-x/index.html",
            ),
            ("src.md", "/src/", "src/index.html"),
        ] {
            assert_eq!(
                resolve_permalink(Path::new(rel), &none).unwrap(),
                w(url, out),
                "{rel}"
            );
        }
    }

    /// Probed: `pagination.links` spellings.
    #[test]
    fn links_follow_the_permalink_as_written() {
        let s = |x: &str| Value::String(x.into());
        let w = |out: &str| Permalink::Write {
            url: String::new(),
            output_rel: out.into(),
        };
        let l = |v: Option<Value>, out: &str| link(v.as_ref(), &w(out));
        assert_eq!(
            l(Some(s("/blog/")), "blog/index.html").unwrap(),
            "//blog/index.html"
        );
        assert_eq!(
            l(Some(s("rel/x0.html")), "rel/x0.html").unwrap(),
            "/rel/x0.html"
        );
        assert_eq!(
            l(Some(s("/up/../d/0/")), "d/0/index.html").unwrap(),
            "//d/0/index.html"
        );
        assert_eq!(
            l(Some(s("../0/q.html")), "0/q.html").unwrap(),
            "/../0/q.html"
        );
        assert_eq!(l(None, "list/1/index.html").unwrap(), "/list/1/index.html");
        assert_eq!(l(Some(s("")), "c/index.html").unwrap(), "/c/index.html");
        assert_eq!(link(Some(&Value::Bool(false)), &Permalink::Skip), None);
    }

    #[test]
    fn default_permalinks_take_the_page_folder() {
        assert_eq!(
            default_permalink(Path::new("list.liquid"), Some(2)),
            w("/list/2/", "list/2/index.html")
        );
        assert_eq!(
            default_permalink(Path::new("index.liquid"), Some(1)),
            w("/1/", "1/index.html")
        );
        assert_eq!(
            default_permalink(Path::new("posts/posts.md"), Some(1)),
            w("/posts/1/", "posts/1/index.html")
        );
    }

    #[test]
    fn defaults_match_11ty() {
        let none = Data::new();
        assert_eq!(
            resolve_permalink(Path::new("about.md"), &none).unwrap(),
            w("/about/", "about/index.html")
        );
        assert_eq!(
            resolve_permalink(Path::new("index.md"), &none).unwrap(),
            w("/", "index.html")
        );
        assert_eq!(
            resolve_permalink(Path::new("sub/deep/index.html"), &none).unwrap(),
            w("/sub/deep/", "sub/deep/index.html")
        );
        assert_eq!(
            resolve_permalink(Path::new("posts/2026/2026-11-08-hello-world.md"), &none).unwrap(),
            w(
                "/posts/2026/2026-11-08-hello-world/",
                "posts/2026/2026-11-08-hello-world/index.html"
            )
        );
    }

    #[test]
    fn explicit_permalinks() {
        assert_eq!(
            r("/custom/path.html".into()).unwrap(),
            w("/custom/path.html", "custom/path.html")
        );
        assert_eq!(
            r("custom/noslash.html".into()).unwrap(),
            w("/custom/noslash.html", "custom/noslash.html")
        );
        assert_eq!(r("blog/".into()).unwrap(), w("/blog/", "blog/index.html"));
        assert_eq!(r("/".into()).unwrap(), w("/", "index.html"));
        assert_eq!(
            r("/space dir/".into()).unwrap(),
            w("/space dir/", "space dir/index.html")
        );
        assert_eq!(r(false.into()).unwrap(), Permalink::Skip);
    }

    /// Probed: the URL drops a final `index.html` or `index`; the file stays.
    #[test]
    fn index_files_have_directory_urls() {
        assert_eq!(r("/a/index.html".into()).unwrap(), w("/a/", "a/index.html"));
        assert_eq!(r("/index.html".into()).unwrap(), w("/", "index.html"));
        assert_eq!(
            r("/a/index/".into()).unwrap(),
            w("/a/index/", "a/index/index.html")
        );
        assert_eq!(
            r_flag("/a/index", true.into()).unwrap(),
            w("/a/", "a/index")
        );
        assert_eq!(
            r("/a/myindex.html".into()).unwrap(),
            w("/a/myindex.html", "a/myindex.html")
        );
    }

    #[test]
    fn dot_dot_is_clamped_to_root() {
        assert_eq!(r("/x/../y/".into()).unwrap(), w("/y/", "y/index.html"));
        assert_eq!(
            r("/../etc/x.html".into()).unwrap(),
            w("/etc/x.html", "etc/x.html")
        );
        assert_eq!(
            r("/a/./b//c.html".into()).unwrap(),
            w("/a/b/c.html", "a/b/c.html")
        );
        assert_eq!(r("/../../".into()).unwrap(), w("/", "index.html"));
    }

    #[test]
    fn no_extension_is_an_error() {
        let e = r("/dir/noext".into()).unwrap_err();
        assert_eq!(e.code, codes::PERMALINK_NO_EXTENSION);
        assert!(e.message.contains("/dir/noext"));
        assert_eq!(
            e.help.as_deref(),
            Some(
                "Add a file extension (like /dir/noext.html), end it with / to write an index.html, or set `eleventyAllowMissingExtension: true` to write the file as named."
            )
        );
    }

    fn r_flag(permalink: &str, flag: Json) -> Result<Permalink, Problem> {
        let mut m = fm(permalink.into());
        m.insert(
            "eleventyAllowMissingExtension".into(),
            Value::from_json(&flag),
        );
        resolve_permalink(Path::new("c.md"), &m)
    }

    #[test]
    fn allow_missing_extension_flag_permits_extensionless() {
        assert_eq!(r_flag("/feed", true.into()).unwrap(), w("/feed", "feed"));
    }

    #[test]
    fn allow_missing_extension_flag_is_truthy_like_javascript() {
        for flag in [
            json!("yes"),
            json!(1),
            json!([1]),
            json!({"a": 1}),
            json!([]),
        ] {
            assert_eq!(r_flag("/feed", flag).unwrap(), w("/feed", "feed"));
        }
        for flag in [json!(false), json!(0), json!(""), json!(null)] {
            assert_eq!(
                r_flag("/feed", flag).unwrap_err().code,
                codes::PERMALINK_NO_EXTENSION
            );
        }
    }

    #[test]
    fn extensionless_without_flag_is_an_error() {
        assert_eq!(
            r("/CNAME".into()).unwrap_err().code,
            codes::PERMALINK_NO_EXTENSION
        );
        assert_eq!(
            r_flag("/CNAME", false.into()).unwrap_err().code,
            codes::PERMALINK_NO_EXTENSION
        );
    }

    #[test]
    fn well_known_extensionless_files_need_no_flag() {
        assert_eq!(
            r("/_redirects".into()).unwrap(),
            w("/_redirects", "_redirects")
        );
        assert_eq!(
            r("/.htaccess".into()).unwrap(),
            w("/.htaccess", ".htaccess")
        );
        assert_eq!(r("/_headers".into()).unwrap(), w("/_headers", "_headers"));
        assert_eq!(
            r("/sub/.htaccess".into()).unwrap(),
            w("/sub/.htaccess", "sub/.htaccess")
        );
    }

    #[test]
    fn invalid_types_are_errors() {
        let e = r(true.into()).unwrap_err();
        assert_eq!(e.code, codes::PERMALINK_INVALID);
        assert_eq!(e.help.as_deref(), Some("Did you mean `permalink: false`?"));
        assert_eq!(r(Json::from(5)).unwrap_err().code, codes::PERMALINK_INVALID);
    }

    #[test]
    fn null_and_empty_use_the_default() {
        assert_eq!(r(Json::Null).unwrap(), w("/c/", "c/index.html"));
        assert_eq!(r("".into()).unwrap(), w("/c/", "c/index.html"));
    }

    #[test]
    fn trailing_dot_dot_is_a_directory() {
        assert_eq!(r("/a/..".into()).unwrap(), w("/", "index.html"));
    }

    #[test]
    fn backslashes_and_drive_letters_are_rejected() {
        let help = "Permalinks use forward slashes and can't contain backslashes or drive letters.";
        for bad in [
            "/..\\..\\x.html",
            "/a\\b.html",
            "/C:/x.html",
            "c:/x.html",
            "/a/z:/x.html",
        ] {
            let e = r(bad.into()).unwrap_err();
            assert_eq!(e.code, codes::PERMALINK_INVALID, "{bad}");
            assert_eq!(e.help.as_deref(), Some(help), "{bad}");
        }
        assert!(r("/a:b/x.html".into()).is_ok());
    }
}
