//! Which Prism grammars other pages change.
//!
//! `Prism.languages` is one object for the whole build in 11ty. A page that
//! loads a language Brrrty doesn't have can change the grammars of the ones
//! it has (`regex` reloads `javascript`, `css-extras` changes `css`, ...),
//! and every page 11ty renders after it is highlighted with the changed
//! grammar. Brrrty can't know the order 11ty renders pages in, and doesn't
//! have the changed grammars, so a page that highlights with one of them is
//! refused once any page could load such a language.
//!
//! "Could" is read from the source, before anything renders, so that pages
//! Brrrty refuses or doesn't render (Nunjucks, includes, layouts, pages
//! that fail) count too: every Markdown fence's info string and every
//! `{% highlight %}` tag in every file under the input folder. A language
//! name that a template writes (`{{ lang }}`) could be any language.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use brrrty_prism::grammar::grammars;

use super::normalize;

/// A language some file could load that changes grammars Brrrty has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The file, as shown in problems.
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    /// The language as written.
    pub language: String,
    /// The `Prism.languages` keys it changes.
    pub keys: Vec<String>,
}

/// Larger files aren't templates anyone writes by hand; they are skipped.
const MAX_FILE: u64 = 16 << 20;

/// Every file under `input` (as `input_rel` shows it) that names a
/// language changing grammars Brrrty has, skipping `node_modules`, `.git`
/// and anything under `skip` (the output folder). `engines` says which
/// language renders Markdown and HTML.
pub fn scan(
    input: &Path,
    input_rel: &Path,
    skip: &Path,
    engines: crate::options::TemplateEngines,
) -> Vec<Found> {
    let mut files = Vec::new();
    walk(input, skip, &mut files);
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let Ok(bytes) = fs::read(&file) else { continue };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let shown = input_rel.join(file.strip_prefix(input).unwrap_or(&file));
        let nunjucks = may_be_nunjucks(&file, &text, engines);
        for (line, language) in languages(&text, nunjucks) {
            if let Some(keys) = changes(&language) {
                out.push(Found {
                    path: shown.clone(),
                    line,
                    language,
                    keys,
                });
            }
        }
    }
    out
}

/// Whether Nunjucks could render `file`: a `.njk` file, Markdown or HTML
/// that the config gives Nunjucks, or a file that names it anywhere (as
/// `templateEngineOverride` would).
fn may_be_nunjucks(file: &Path, text: &str, engines: crate::options::TemplateEngines) -> bool {
    use crate::options::TemplateLanguage;
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ext == "njk"
        || (ext == "md" && engines.markdown == Some(TemplateLanguage::Nunjucks))
        || (ext == "html" && engines.html == Some(TemplateLanguage::Nunjucks))
        || text.contains("njk")
}

fn walk(dir: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            let name = e.file_name();
            if name == "node_modules" || name == ".git" || is_same(&path, skip) {
                continue;
            }
            walk(&path, skip, out);
        } else if fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE) {
            out.push(path);
        }
    }
}

fn is_same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The keys loading `language` (as a fence or a tag names it) changes.
/// A name a template writes could be any language: it changes every key
/// any language changes.
pub(crate) fn changes(language: &str) -> Option<Vec<String>> {
    let g = grammars();
    if language.contains("{{") || language.contains("{%") || language.contains('&') {
        let mut all: Vec<String> = Vec::new();
        for (_, keys) in &g.mutators {
            for k in keys {
                if !all.contains(k) {
                    all.push(k.clone());
                }
            }
        }
        return Some(all);
    }
    // `js/1-2`: the language is before the first `/`; `diff-x` loads `x`.
    let name = language.split('/').next().unwrap_or("");
    let name = name.strip_prefix("diff-").unwrap_or(name);
    g.mutated_by(&normalize(name)).map(<[String]>::to_vec)
}

/// The languages `text` could load, with their lines: the first word of
/// every fence's info string (markdown-it's `info.split(/\s+/)[0]`, its
/// backslash escapes removed) and the first argument of every
/// `{% highlight %}` (quotes removed, for Nunjucks). Looser than Markdown
/// is: a fence anywhere in a line counts. In a file Nunjucks may render
/// (`nunjucks`), a tag whose arguments aren't one string could load any
/// language, and so could a tag whose arguments start on another line. A word with a backslash left in
/// it counts both whole and cut there, as a string in a data file would be
/// read.
fn languages(text: &str, nunjucks: bool) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let mut add = |word: &str| {
            let word = unescape(word);
            if let Some((head, _)) = word.split_once('\\') {
                out.push((i + 1, head.to_string()));
            }
            out.push((i + 1, word));
        };
        for fence in ["```", "~~~"] {
            let mut rest = line;
            while let Some(at) = rest.find(fence) {
                let after = rest[at..].trim_start_matches(fence.as_bytes()[0] as char);
                let info = after.trim_start_matches([' ', '\t']);
                let word: &str = info
                    .split(|c: char| c.is_whitespace() || (fence == "```" && c == '`'))
                    .next()
                    .unwrap_or("");
                if !word.is_empty() {
                    add(word);
                }
                rest = after;
            }
        }
        let mut rest = line;
        while let Some(at) = rest.find("{%") {
            let tag = rest[at + 2..].trim_start_matches('-').trim_start();
            if let Some(args) = tag.strip_prefix("highlight")
                && (args.is_empty()
                    || args.starts_with(char::is_whitespace)
                    || (nunjucks && args.starts_with(['"', '\''])))
            {
                let word = args
                    .trim_start()
                    .split(|c: char| c.is_whitespace() || c == '%')
                    .next()
                    .unwrap_or("");
                let word = word.trim_matches(['"', '\'']);
                if !word.is_empty() {
                    add(word);
                }
                // The arguments go on to another line, or Nunjucks
                // evaluates them as an expression other than one string:
                // the language could be any.
                if word.is_empty() || (nunjucks && !one_string(args)) {
                    let written = args.split("%}").next().unwrap_or("");
                    let written = written.trim().trim_end_matches('-').trim_end();
                    add(&format!("{{{{ {written} }}}}"));
                }
            }
            rest = &rest[at + 2..];
        }
    }
    out
}

/// Whether Nunjucks tag arguments are one string literal without escapes,
/// up to the tag's end.
fn one_string(args: &str) -> bool {
    let args = args.trim_start();
    let Some(q) = args.chars().next().filter(|c| *c == '"' || *c == '\'') else {
        return false;
    };
    let Some(end) = args[1..].find(q) else {
        return false;
    };
    let body = &args[1..1 + end];
    let after = args[2 + end..].trim_start();
    !body.contains('\\') && (after.starts_with("%}") || after.starts_with("-%}"))
}

/// markdown-it's backslash escapes: `\` before ASCII punctuation goes.
fn unescape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
            && n.is_ascii_punctuation()
        {
            out.push(n);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// What other pages change, by `Prism.languages` key: the first file and
/// line (in path order) that names a language changing it, and that
/// language.
pub type Mutated = BTreeMap<String, (PathBuf, usize, String)>;

/// `found`, by the keys each changes.
pub fn by_key(found: &[Found]) -> Mutated {
    let mut out = Mutated::new();
    for f in found {
        for k in &f.keys {
            out.entry(k.clone())
                .or_insert_with(|| (f.path.clone(), f.line, f.language.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_and_tags_name_their_languages() {
        let text = "x\n```regex\n~~~ diff-jsdoc/1 more\n  > ```js/1-2\n{%- highlight \"css-extras\" -%}\n{% highlight php-extras 1 %}\n`not` ``a``\n";
        let got: Vec<(usize, String)> = languages(text, false);
        assert_eq!(
            got,
            vec![
                (2, "regex".to_string()),
                (3, "diff-jsdoc/1".to_string()),
                (4, "js/1-2".to_string()),
                (5, "css-extras".to_string()),
                (6, "php-extras".to_string()),
            ]
        );
    }

    #[test]
    fn a_language_from_a_string_counts_cut_at_a_backslash() {
        assert_eq!(
            languages(r#"{"md": "```regex\nx\n```"}"#, false),
            vec![
                (1, "regex".to_string()),
                (1, r"regex\nx\n".to_string()),
                (1, "\"}".to_string())
            ]
        );
        assert_eq!(
            languages(r"```reg\-ex", false),
            vec![(1, "reg-ex".to_string())]
        );
    }

    #[test]
    fn nunjucks_arguments_other_than_one_string_could_be_any_language() {
        let text = "{% highlight \"regex 1\" %}\n{% highlight lang %}\n{%- highlight \"js\" ~ x -%}\n{% highlight'css' -%}\n{% highlight\n";
        let got = languages(text, true);
        assert_eq!(
            got,
            vec![
                (1, "regex".to_string()),
                (2, "lang".to_string()),
                (2, "{{ lang }}".to_string()),
                (3, "js".to_string()),
                (3, "{{ \"js\" ~ x }}".to_string()),
                (4, "css".to_string()),
                (5, "{{  }}".to_string()),
            ]
        );
        for (_, l) in &got[2..] {
            if l.starts_with("{{") {
                assert!(changes(l).is_some_and(|k| k.iter().any(|k| k == "javascript")));
            }
        }
        // Liquid reads the first word as written; only a tag cut at the
        // line could be any language.
        assert_eq!(
            languages("{% highlight lang %}\n{% highlight\n", false),
            vec![(1, "lang".to_string()), (2, "{{  }}".to_string())]
        );
    }

    #[test]
    fn languages_that_change_grammars_brrrty_has() {
        let keys = changes("regex").unwrap();
        assert!(keys.iter().any(|k| k == "javascript"));
        assert!(!keys.iter().any(|k| k == "bash"));
        assert!(changes("diff-regex/1").is_some());
        assert_eq!(changes("css-extras"), changes("css-extras/2"));
        assert!(changes("js").is_none());
        assert!(changes("java").is_some());
        assert!(changes("kotlin").is_none());
        assert!(changes("REGEX").is_none());
        let any = changes("{{ lang }}").unwrap();
        assert!(any.iter().any(|k| k == "bash"));
    }
}
