//! `eleventyComputed`: Liquid-string values rendered against the page's data
//! in 11ty's dependency order.
//!
//! Probed against 11ty 3.1.8 (`Template.addComputedData`, `ComputedData`):
//!
//! - Every leaf of the merged `eleventyComputed` object is one entry, named
//!   by its dotted path. Strings and arrays are template entries (an array is
//!   one entry, with every string inside rendered, nested objects and arrays
//!   too); an object recurses; anything else is set as it is.
//! - A template entry depends on another entry when one of its top-level
//!   `{{ }}` outputs starts with that entry's exact name: LiquidJS's
//!   `parseForSymbols` takes the text before the first `|`. Reads inside
//!   tags, in strings, through an index or of a parent object are not seen.
//! - The entries run in the order `dependency-graph` 1.0.0 gives, and a
//!   cycle aborts 11ty. Each result is stored with `lodash.set`.
//! - Entries read the data as it is when they run, so an entry the order
//!   puts later still holds its original value.
//! - `page.url` and `page.outputPath` are the first two entries, each a
//!   run of the permalink. They depend on a computed `permalink` (11ty finds
//!   that by running the permalink on marker data), so a plain permalink
//!   reading a computed key sees the key's original value, and a computed
//!   `permalink` reading `page.url` is a cycle.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_liquid::{Engine, Loader};
use indexmap::IndexMap;

use crate::frontmatter::Data;
use crate::permalink::{Locator, Permalink};
use crate::problem::{Problem, codes};
use crate::render::liquid_problem;

/// Top-level keys a computed value may not set: they change how the page is
/// built, or 11ty owns them. `date`, `tags` and `eleventyExcludeFromCollections`
/// are data only (probed: `page.date` keeps the original date).
const REFUSED_ROOTS: &[&str] = &[
    "layout",
    "templateEngineOverride",
    "eleventyComputed",
    "eleventyDataSchema",
    "pagination",
    "dynamicPermalink",
    "eleventyAllowMissingExtension",
    "permalinkBypassOutputDir",
    "page",
    "eleventy",
    "pkg",
];

/// The computed entries of a page, in the order to run them.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    /// `templateEngineOverride` of `html`, `""` or `false`: values are set
    /// as written (probed).
    raw: bool,
    steps: Vec<Step>,
}

#[derive(Debug, Clone)]
struct Step {
    path: Vec<String>,
    value: Value,
    /// An entry 11ty adds itself: it runs the permalink.
    page: Option<PageEntry>,
}

#[derive(Debug, Clone, Copy)]
enum PageEntry {
    Url,
    OutputPath,
}

/// A page's data with every computed value set, and where the permalink
/// put the page.
pub(crate) struct Applied {
    pub data: IndexMap<String, Value>,
    pub permalink: Permalink,
}

fn unsupported(message: String) -> Problem {
    Problem::error(codes::COMPUTED_UNSUPPORTED, message)
        .with_help("Computed data of this shape isn't supported yet.")
}

impl Plan {
    /// Whether an entry sets `root` at the top level of the data.
    pub fn sets_root(&self, root: &str) -> bool {
        self.steps
            .iter()
            .any(|s| s.page.is_none() && s.path.first().map(String::as_str) == Some(root))
    }

    /// The plan for the merged `eleventyComputed` of a page, or `None` when
    /// there is nothing to compute (11ty ignores a value that isn't a plain
    /// object).
    pub fn new(data: &Data, raw: bool) -> Result<Option<Plan>, Problem> {
        let Some(Value::Object(computed)) = data.get("eleventyComputed") else {
            return Ok(None);
        };
        for (key, _) in Value::js_entries(computed) {
            if key == "permalink"
                && let Some(Value::Object(map)) = computed.get("permalink")
                && !map.is_empty()
            {
                return Err(unsupported(
                    "`eleventyComputed` key `permalink` as a map isn't supported yet".to_string(),
                ));
            }
            if REFUSED_ROOTS.contains(&key.as_str()) {
                return Err(unsupported(format!(
                    "`eleventyComputed` key `{key}` isn't supported yet"
                )));
            }
        }
        let mut leaves = Vec::new();
        collect(computed, &mut Vec::new(), &mut leaves)?;
        let computed_permalink = leaves.iter().any(|l| l.path == ["permalink"]);
        // 11ty adds `page.url` and `page.outputPath` before the keys.
        let own = |path: [&str; 2], entry| Leaf {
            path: path.map(String::from).to_vec(),
            value: Value::Null,
            page: Some(entry),
        };
        leaves.splice(
            0..0,
            [
                own(["page", "url"], PageEntry::Url),
                own(["page", "outputPath"], PageEntry::OutputPath),
            ],
        );

        // Dependency graph, built the way 11ty adds entries.
        let names: Vec<String> = leaves.iter().map(|l| l.path.join(".")).collect();
        let known: HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let mut graph = Graph::default();
        for (leaf, name) in leaves.iter().zip(&names) {
            graph.node(name);
            if leaf.page.is_some() {
                // A computed `permalink` is found by running the permalink
                // on marker data; a plain one isn't there to find.
                if computed_permalink {
                    let to = graph.node("permalink");
                    let from = graph.node(name);
                    graph.depend(from, to);
                }
                continue;
            }
            let symbols = match &leaf.value {
                Value::String(s) => symbols(s),
                Value::Array(items) => items
                    .iter()
                    .filter_map(|v| match v {
                        Value::String(s) => Some(symbols(s)),
                        _ => None,
                    })
                    .flatten()
                    .collect(),
                _ => Vec::new(),
            };
            let mut used = Vec::new();
            for symbol in symbols {
                if symbol.starts_with("collections.") {
                    return Err(Problem::error(
                        codes::DATA_UNSUPPORTED,
                        "this page uses `collections`, which Brrrty doesn't build yet",
                    )
                    .with_help("Collections are in progress."));
                }
                if symbol != *name && known.contains_key(symbol.as_str()) {
                    used.push(symbol);
                }
            }
            for symbol in used {
                let to = graph.node(&symbol);
                let from = graph.node(name);
                graph.depend(from, to);
            }
        }
        let order = graph.order().map_err(|cycle| {
            let path: Vec<&str> = cycle.iter().map(|&i| graph.names[i].as_str()).collect();
            Problem::error(
                codes::COMPUTED_CYCLE,
                format!("Dependency Cycle Found: {}", path.join(" -> ")),
            )
            .with_help("11ty stops the whole build here; make the computed values not depend on each other in a loop.")
        })?;
        let mut leaves: Vec<Option<Leaf>> = leaves.into_iter().map(Some).collect();
        let steps = order
            .into_iter()
            .filter_map(|i| known.get(graph.names[i].as_str()).copied())
            .filter_map(|i| leaves[i].take())
            .map(|l| Step {
                path: l.path,
                value: l.value,
                page: l.page,
            })
            .collect();
        Ok(Some(Plan { raw, steps }))
    }

    /// `base` with every computed value set, in order, and the permalink as
    /// the `page.outputPath` entry resolved it. Reads that nothing defines
    /// are added to `undefined`, like any other render.
    pub fn apply(
        &self,
        engine: &Engine,
        loader: &dyn Loader,
        shown: &Path,
        base: &IndexMap<String, Value>,
        undefined: &mut BTreeSet<String>,
        locator: &Locator<'_>,
    ) -> Result<Applied, Problem> {
        let mut data = base.clone();
        let mut permalink = None;
        for step in &self.steps {
            let key = step.path.join(".");
            let value = if let Some(entry) = step.page {
                // 11ty writes the file `page.outputPath` names, so that
                // run decides where the page goes.
                let found = locator.locate(&data, undefined)?;
                match entry {
                    PageEntry::Url => found.url,
                    PageEntry::OutputPath => {
                        permalink = Some(found.permalink);
                        found.output_path
                    }
                }
            } else {
                self.render(&step.value, &key, engine, loader, shown, &data, undefined)?
            };
            if matches!(step.value, Value::Array(_))
                && let Some(Value::Object(own)) = data.get_mut("eleventyComputed")
            {
                // 11ty renders an array in place, so the data's own
                // `eleventyComputed` shows the rendered strings (probed).
                let _ = set_path(Arc::make_mut(own), &step.path, value.clone());
            }
            set_path(&mut data, &step.path, value).map_err(|why| {
                unsupported(format!("computed key `{key}` can't be set: {why}")).with_path(shown)
            })?;
        }
        // The data is a JavaScript object: index-like keys come first. The
        // base is in that order already, so only the objects a computed
        // value was set in can be out of it; the rest stay shared.
        for step in &self.steps {
            reorder_along(&mut data, &step.path);
            if matches!(step.value, Value::Array(_))
                && let Some(Value::Object(own)) = data.get_mut("eleventyComputed")
            {
                reorder_along(Arc::make_mut(own), &step.path);
            }
        }
        Ok(Applied {
            data,
            permalink: permalink.expect("`page.outputPath` is always an entry"),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        value: &Value,
        key: &str,
        engine: &Engine,
        loader: &dyn Loader,
        shown: &Path,
        data: &IndexMap<String, Value>,
        undefined: &mut BTreeSet<String>,
    ) -> Result<Value, Problem> {
        match value {
            Value::String(s) => {
                // 11ty skips the engine when there is nothing to compile.
                if self.raw || !(s.contains("{{") || s.contains("{%")) {
                    return Ok(value.clone());
                }
                let name = format!("eleventyComputed.{key}");
                let fail = |e: brrrty_liquid::LiquidError| {
                    let mut p = liquid_problem(&e, shown.to_path_buf(), 0, None);
                    p.line = None;
                    p.column = None;
                    p.message = format!("{} in `{name}`", p.message);
                    p
                };
                let t = engine.parse(&name, s).map_err(fail)?;
                let scope = Value::Object(Arc::new(data.clone()));
                let r = engine.render_tracked(&t, &scope, loader).map_err(fail)?;
                undefined.extend(r.undefined);
                Ok(Value::String(r.output.into()))
            }
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items.iter() {
                    out.push(self.render(item, key, engine, loader, shown, data, undefined)?);
                }
                Ok(Value::Array(Arc::new(out)))
            }
            Value::Object(o) => {
                let mut out = IndexMap::with_capacity(o.len());
                for (k, v) in Value::js_entries(o) {
                    out.insert(
                        k.clone(),
                        self.render(v, key, engine, loader, shown, data, undefined)?,
                    );
                }
                Ok(Value::Object(Arc::new(out)))
            }
            other => Ok(other.clone()),
        }
    }
}

struct Leaf {
    path: Vec<String>,
    value: Value,
    page: Option<PageEntry>,
}

/// The entries under `object`, in JavaScript key order. An object with no
/// keys has no entries.
fn collect(object: &Data, path: &mut Vec<String>, out: &mut Vec<Leaf>) -> Result<(), Problem> {
    for (key, value) in Value::js_entries(object) {
        path.push(key.clone());
        if key.is_empty() || key.contains(['.', '[', ']']) {
            return Err(unsupported(format!(
                "`eleventyComputed` key `{}` isn't supported yet",
                path.join(".")
            )));
        }
        match value {
            Value::Object(o) => collect(o, path, out)?,
            other => out.push(Leaf {
                path: path.clone(),
                value: other.clone(),
                page: None,
            }),
        }
        path.pop();
    }
    Ok(())
}

/// `lodash.set` on plain data. A parent that is missing, null or a
/// primitive becomes a new object; an array or date parent, or an index-like
/// key under a new parent (lodash would make an array), is refused.
pub(crate) fn set_path(root: &mut Data, path: &[String], value: Value) -> Result<(), String> {
    let (first, rest) = path.split_first().expect("path has a key");
    let Some(next) = rest.first() else {
        root.insert(first.clone(), value);
        return Ok(());
    };
    match root.get_mut(first.as_str()) {
        Some(Value::Object(o)) => set_path(Arc::make_mut(o), rest, value),
        Some(Value::Array(_) | Value::Date(_)) => Err(format!(
            "`{first}` is a list or a date, not an object, so `{next}` can't be set on it"
        )),
        _ => {
            if brrrty_js::array_index(next).is_some() {
                return Err(format!(
                    "`{next}` under a new `{first}` would make a list, as JavaScript's lodash does"
                ));
            }
            let mut fresh = Data::new();
            set_path(&mut fresh, rest, value)?;
            root.insert(first.clone(), Value::Object(Arc::new(fresh)));
            Ok(())
        }
    }
}

/// Put `root` and each object on `path` below it in JavaScript key order.
fn reorder_along(root: &mut Data, path: &[String]) {
    if root.keys().any(|k| brrrty_js::array_index(k).is_some()) {
        *root = Value::js_entries(root)
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    }
    if let Some((first, rest)) = path.split_first()
        && !rest.is_empty()
        && let Some(Value::Object(o)) = root.get_mut(first.as_str())
    {
        reorder_along(Arc::make_mut(o), rest);
    }
}

/// The text before the first `|` of each top-level `{{ }}` in `s`, the
/// symbols LiquidJS's `parseForSymbols` returns. A tag or output that never
/// closes ends the scan: rendering reports the syntax error.
fn symbols(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut p = 0;
    let mut raw = false;
    while p < b.len() {
        if raw {
            // Everything up to `{% endraw %}` is text.
            match find_endraw(s, p) {
                Some(end) => {
                    raw = false;
                    p = end;
                    continue;
                }
                None => break,
            }
        }
        if s[p..].starts_with("{%") {
            let Some(end) = s[p..].find("%}").map(|i| p + i + 2) else {
                break;
            };
            let name = s[p + 2..end - 2]
                .trim_start_matches('-')
                .trim_start()
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .next()
                .unwrap_or("");
            raw = name == "raw";
            p = end;
        } else if s[p..].starts_with("{{") {
            let Some(end) = output_end(s, p) else { break };
            let mut content = &s[p + 2..end - 2];
            content = content.strip_prefix('-').unwrap_or(content);
            content = content.strip_suffix('-').unwrap_or(content);
            let symbol = content.trim().split('|').next().unwrap_or("").trim();
            out.push(symbol.to_string());
            p = end;
        } else {
            p += s[p..].find(['{']).map_or(s.len() - p, |i| i.max(1));
        }
    }
    out
}

/// Where the `{{` at `start` ends, past its `}}`; quoted strings may hold a `}}`.
fn output_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut p = start;
    while p < b.len() {
        if b[p] == b'"' || b[p] == b'\'' {
            let quote = b[p];
            p += 1;
            while p < b.len() && b[p] != quote {
                p += 1;
            }
            if p >= b.len() {
                return None;
            }
            p += 1;
            continue;
        }
        p += 1;
        if p >= 2 && &b[p - 2..p] == b"}}" {
            return Some(p);
        }
    }
    None
}

fn find_endraw(s: &str, from: usize) -> Option<usize> {
    let mut p = from;
    while let Some(i) = s[p..].find("{%") {
        let open = p + i;
        let close = s[open..].find("%}")? + open + 2;
        let inner = s[open + 2..close - 2].trim_matches('-').trim();
        if inner == "endraw" {
            return Some(close);
        }
        p = open + 2;
    }
    None
}

/// The dependency graph of `dependency-graph` 1.0.0: nodes in insertion
/// order, edges in insertion order.
#[derive(Default)]
struct Graph {
    names: Vec<String>,
    index: HashMap<String, usize>,
    out: Vec<Vec<usize>>,
    inc: Vec<Vec<usize>>,
}

impl Graph {
    fn node(&mut self, name: &str) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.names.len();
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), i);
        self.out.push(Vec::new());
        self.inc.push(Vec::new());
        i
    }

    fn depend(&mut self, from: usize, to: usize) {
        if !self.out[from].contains(&to) {
            self.out[from].push(to);
        }
        if !self.inc[to].contains(&from) {
            self.inc[to].push(from);
        }
    }

    /// `overallOrder()`: a cycle check from every node, then a depth-first
    /// walk from the nodes nothing depends on, dependencies first. A cycle
    /// comes back as the path 11ty names.
    fn order(&self) -> Result<Vec<usize>, Vec<usize>> {
        let n = self.names.len();
        let mut seen = vec![false; n];
        for start in 0..n {
            self.dfs(start, &mut seen, None)?;
        }
        let mut seen = vec![false; n];
        let mut result = Vec::with_capacity(n);
        for start in (0..n).filter(|&i| self.inc[i].is_empty()) {
            self.dfs(start, &mut seen, Some(&mut result))?;
        }
        Ok(result)
    }

    fn dfs(
        &self,
        start: usize,
        visited: &mut [bool],
        mut result: Option<&mut Vec<usize>>,
    ) -> Result<(), Vec<usize>> {
        if visited[start] {
            return Ok(());
        }
        let mut in_path = vec![false; self.names.len()];
        let mut path = Vec::new();
        // (node, edges pushed)
        let mut todo = vec![(start, false)];
        while let Some(&(node, processed)) = todo.last() {
            if !processed {
                if visited[node] {
                    todo.pop();
                    continue;
                }
                if in_path[node] {
                    path.push(node);
                    return Err(path);
                }
                in_path[node] = true;
                path.push(node);
                let top = todo.len() - 1;
                for &e in self.out[node].iter().rev() {
                    todo.push((e, false));
                }
                todo[top].1 = true;
            } else {
                todo.pop();
                path.pop();
                in_path[node] = false;
                visited[node] = true;
                if let Some(r) = result.as_deref_mut() {
                    r.push(node);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(s: &str) -> Vec<String> {
        symbols(s)
    }

    /// Probed with 11ty: only top-level outputs count, up to the first `|`.
    #[test]
    fn symbols_are_the_first_part_of_each_output() {
        assert_eq!(syms("[{{ c }}]"), ["c"]);
        assert_eq!(syms("{{ d | upcase }}"), ["d"]);
        assert_eq!(syms("{{-e-}}"), ["e"]);
        assert_eq!(syms("{{  c  }}{{ m.t }}"), ["c", "m.t"]);
        assert_eq!(syms("{{\n    d }}"), ["d"]);
        assert_eq!(syms("{{ 'a' | append: h }}"), ["'a'"]);
        assert_eq!(syms("{{ arr[0] }}"), ["arr[0]"]);
        assert_eq!(syms("{% if f %}{{ f | append: '' }}{% endif %}"), ["f"]);
        assert_eq!(syms("{% assign q = g %}"), Vec::<String>::new());
        assert_eq!(syms("{% raw %}{{ c }}{% endraw %}{{ x }}"), ["x"]);
        assert_eq!(syms("{{ '}}' }}"), ["'}}'"]);
        assert_eq!(syms("{{ unclosed"), Vec::<String>::new());
        assert_eq!(syms("plain { text"), Vec::<String>::new());
    }

    #[test]
    fn graph_orders_dependencies_first_and_names_the_cycle() {
        let mut g = Graph::default();
        let a = g.node("a");
        let b = g.node("b");
        let c = g.node("c");
        g.depend(a, c);
        // a needs c: c, then a, then the independent b.
        assert_eq!(g.order(), Ok(vec![c, a, b]));
        g.depend(c, a);
        assert!(g.order().is_err());
        let mut g = Graph::default();
        let a = g.node("a");
        let b = g.node("b");
        let c = g.node("c");
        g.depend(a, b);
        g.depend(b, c);
        g.depend(c, a);
        assert_eq!(g.order(), Err(vec![a, b, c, a]));
    }

    #[test]
    fn set_path_follows_lodash() {
        let mut d = Data::new();
        d.insert("s".into(), Value::String("str".into()));
        let path = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        set_path(&mut d, &path(&["s", "k"]), Value::Number(1.0)).unwrap();
        assert!(matches!(d.get("s"), Some(Value::Object(_))));
        assert!(set_path(&mut d, &path(&["fresh", "0"]), Value::Null).is_err());
        d.insert("arr".into(), Value::Array(Arc::new(vec![])));
        assert!(set_path(&mut d, &path(&["arr", "k"]), Value::Null).is_err());
        set_path(&mut d, &path(&["new", "a", "b"]), Value::Null).unwrap();
        assert!(matches!(d.get("new"), Some(Value::Object(_))));
    }
}
