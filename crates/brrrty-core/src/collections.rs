//! Collections: `collections.all` and one collection per tag, as 11ty 3.1.8
//! builds them (`TemplateMap`, `TemplateCollection`, `TemplateDepGraph`).
//!
//! 11ty visits the templates and the collections in the order its
//! dependency graph gives: a template that paginates `collections.x` comes
//! after every template that publishes to `x`. A collection is a snapshot
//! of the pages added so far, sorted by date, then input path. Both the
//! order and the snapshots show in the output (the keys of `collections`,
//! and what a paginated template sees), so this follows the graph.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;

use crate::data::{clean_tags, js_object};
use crate::frontmatter::Data;
use crate::problem::Problem;

const PREFIX: &str = "__collection:";
const STAGES: [&str; 4] = ["[basic]", "[userconfig]", "[keys]", "all"];

/// `eleventyExcludeFromCollections`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Exclude {
    /// `true`: the page is in no collection, not even `all`.
    All,
    /// A list, or a string split at commas: the page is left out of those tag
    /// collections (`all` still has it).
    Tags(Vec<String>),
}

/// What decides which collections a template publishes to, read from its
/// data before it is paginated.
#[derive(Debug, Clone)]
pub(crate) struct Membership {
    pub tags: Vec<String>,
    pub exclude: Option<Exclude>,
    /// The collection `pagination.data` reads, if it reads one.
    pub consumes: Option<String>,
    /// `pagination.addAllPagesToCollections` is truthy.
    pub add_all: bool,
}

/// `tags` as 11ty's `getCleanedTagsImmutable` gives them: a string splits
/// at commas, a list stays, repeats go, entries become strings.
pub(crate) fn tags_of(data: &Data) -> Vec<String> {
    let mut only = Data::new();
    if let Some(t) = data.get("tags") {
        only.insert("tags".to_string(), t.clone());
    }
    if clean_tags(&mut only).is_err() {
        return Vec::new();
    }
    match only.get("tags") {
        Some(Value::Array(a)) => a.iter().map(|v| v.to_js_string()).collect(),
        _ => Vec::new(),
    }
}

/// `getNormalizedExcludedCollections`.
pub(crate) fn exclude_of(data: &Data) -> Option<Exclude> {
    match data.get("eleventyExcludeFromCollections") {
        Some(Value::Bool(true)) => Some(Exclude::All),
        Some(Value::Array(list)) => Some(Exclude::Tags(
            list.iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .collect(),
        )),
        Some(Value::String(s)) => Some(Exclude::Tags(s.split(',').map(str::to_string).collect())),
        _ => None,
    }
}

/// The tag collections a page is in: `getIncludedTagNames`.
pub(crate) fn included_tags(data: &Data) -> Vec<String> {
    let tags = tags_of(data);
    match exclude_of(data) {
        Some(Exclude::All) => Vec::new(),
        Some(Exclude::Tags(out)) => tags.into_iter().filter(|t| !out.contains(t)).collect(),
        None => tags,
    }
}

impl Membership {
    pub fn of(data: &Data) -> Membership {
        let pagination = match data.get("pagination") {
            Some(Value::Object(p)) => Some(p),
            _ => None,
        };
        let consumes = pagination
            .and_then(|p| match p.get("data") {
                Some(Value::String(s)) => tag_target(s),
                _ => None,
            })
            .filter(|t| !t.is_empty());
        Membership {
            tags: tags_of(data),
            exclude: exclude_of(data),
            consumes,
            add_all: pagination
                .and_then(|p| p.get("addAllPagesToCollections"))
                .is_some_and(Value::js_truthy),
        }
    }

    /// `TemplateData.getIncludedCollectionNames`, without empty names.
    fn publishes(&self) -> Vec<String> {
        if self.exclude == Some(Exclude::All) {
            return Vec::new();
        }
        let out: &[String] = match &self.exclude {
            Some(Exclude::Tags(t)) => t,
            _ => &[],
        };
        std::iter::once("all".to_string())
            .chain(self.tags.iter().cloned())
            .filter(|t| !out.contains(t) && !t.is_empty())
            .collect()
    }
}

/// `TemplateMap.getTagTarget`: the collection a `pagination.data` reads.
fn tag_target(s: &str) -> Option<String> {
    if s == "collections" {
        return Some("[keys]".to_string());
    }
    if let Some(rest) = s.strip_prefix("collections.") {
        return Some(rest.to_string());
    }
    for quote in ["collections['", "collections[\""] {
        if s.starts_with(quote) {
            // `str.slice(13, -2)`, in UTF-16 units.
            let units: Vec<u16> = s.encode_utf16().collect();
            let end = units.len().saturating_sub(2);
            let start = quote.len().min(end);
            return Some(String::from_utf16_lossy(&units[start..end]));
        }
    }
    None
}

/// The graph 11ty orders templates and collections with (`TemplateDepGraph`
/// over `dependency-graph` with `circular: true`).
struct Graph {
    names: Vec<String>,
    index: HashMap<String, usize>,
    out: Vec<Vec<usize>>,
    incoming: Vec<usize>,
}

impl Graph {
    fn new() -> Graph {
        let mut g = Graph {
            names: Vec::new(),
            index: HashMap::new(),
            out: Vec::new(),
            incoming: Vec::new(),
        };
        // all uses [keys] uses [userconfig] uses [basic]
        let mut previous: Option<String> = None;
        for stage in STAGES.iter().rev() {
            let key = format!("{PREFIX}{stage}");
            if let Some(p) = &previous {
                g.uses(p, &key);
            }
            previous = Some(key);
        }
        g
    }

    fn node(&mut self, name: &str) -> usize {
        if let Some(i) = self.index.get(name) {
            return *i;
        }
        let i = self.names.len();
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), i);
        self.out.push(Vec::new());
        self.incoming.push(0);
        i
    }

    fn uses(&mut self, from: &str, to: &str) {
        let a = self.node(from);
        let b = self.node(to);
        if !self.out[a].contains(&b) {
            self.out[a].push(b);
            self.incoming[b] += 1;
        }
    }

    /// `addTemplate`.
    fn template(&mut self, file: &str, m: &Membership) {
        let basic = format!("{PREFIX}[basic]");
        let consumes: Vec<String> = m.consumes.iter().cloned().collect();
        if consumes.is_empty() {
            self.uses(&basic, file);
        }
        let consumes_all = consumes.iter().any(|c| c == "all");
        let mut publishes = dedupe(m.publishes());
        if consumes_all {
            publishes.retain(|p| p != "all");
        }
        for tag in &publishes {
            let bracketed = tag.starts_with('[') && tag.ends_with(']');
            if !consumes_all && tag != "all" && !bracketed {
                self.uses(&basic, &format!("{PREFIX}{tag}"));
            }
            self.uses(&format!("{PREFIX}{tag}"), file);
        }
        for tag in &consumes {
            self.uses(file, &format!("{PREFIX}{tag}"));
            let stage = STAGES.iter().position(|s| s == tag);
            if let Some(i) = stage
                && i > 0
                && let Some(next) = STAGES.get(i + 1)
            {
                self.uses(&format!("{PREFIX}{next}"), file);
            }
        }
    }

    /// `overallOrder`, then 11ty's filter of the stage names.
    fn order(&self) -> Vec<String> {
        let mut result: Vec<usize> = Vec::new();
        let mut visited = vec![false; self.names.len()];
        let mut on_path = vec![false; self.names.len()];
        let roots = (0..self.names.len()).filter(|i| self.incoming[*i] == 0);
        for r in roots.collect::<Vec<_>>() {
            self.dfs(r, &mut visited, &mut on_path, &mut result);
        }
        for i in 0..self.names.len() {
            self.dfs(i, &mut visited, &mut on_path, &mut result);
        }
        let keys = format!("{PREFIX}[keys]");
        let all = format!("{PREFIX}all");
        let mut filtered: Vec<String> = result
            .into_iter()
            .map(|i| self.names[i].clone())
            .filter(|e| *e == keys || (!e.starts_with(&format!("{PREFIX}[")) && !e.ends_with(']')))
            .collect();
        if filtered.last() != Some(&all) {
            filtered.push(all);
        }
        filtered
    }

    /// The iterative DFS of `dependency-graph`: edges in order, a node
    /// already on the path is skipped, results in post-order.
    fn dfs(&self, start: usize, visited: &mut [bool], on_path: &mut [bool], out: &mut Vec<usize>) {
        if visited[start] {
            return;
        }
        // (node, next edge)
        let mut stack: Vec<(usize, usize)> = Vec::new();
        on_path[start] = true;
        stack.push((start, 0));
        while let Some(&(node, next)) = stack.last() {
            if let Some(&child) = self.out[node].get(next) {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                if visited[child] || on_path[child] {
                    continue;
                }
                on_path[child] = true;
                stack.push((child, 0));
            } else {
                stack.pop();
                on_path[node] = false;
                visited[node] = true;
                out.push(node);
            }
        }
    }
}

fn dedupe(v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

/// One page as a collection holds it.
#[derive(Clone)]
pub(crate) struct Page {
    /// The collection item: what a template reads.
    pub item: Value,
    date_ms: i64,
    input_path: String,
    /// The tag collections the page is in.
    tags: Vec<String>,
}

impl Page {
    /// The item for a page with `data` (which has its `page` filled in).
    /// `number` is the page's number when pagination made it.
    pub fn new(data: &Data, number: Option<usize>) -> Page {
        let page = match data.get("page") {
            Some(Value::Object(p)) => p.clone(),
            _ => Arc::new(IndexMap::new()),
        };
        let field = |k: &str| page.get(k).cloned().unwrap_or(Value::Undefined);
        let date = field("date");
        let date_ms = match &date {
            Value::Date(d) => d.timestamp_millis(),
            _ => 0,
        };
        let input_path = match field("inputPath") {
            Value::String(s) => s.to_string(),
            _ => String::new(),
        };
        // The keys, in the order 11ty's `getTemplates` writes them. The
        // markers are what Brrrty doesn't build: the template object, the
        // rendered content, and `collections` inside the item's own data.
        let mut item = IndexMap::new();
        let mut item_data = data.clone();
        item_data.insert("collections".to_string(), Value::Undefined);
        item.insert("template".to_string(), Value::Undefined);
        item.insert("rawInput".to_string(), field("rawInput"));
        match number {
            Some(n) => {
                item.insert("pageNumber".to_string(), Value::Number(n as f64));
                item.insert("groupNumber".to_string(), Value::Number(0.0));
                item.insert("data".to_string(), Value::Object(Arc::new(item_data)));
            }
            None => {
                item.insert("groupNumber".to_string(), Value::Number(0.0));
                item.insert("data".to_string(), Value::Object(Arc::new(item_data)));
                item.insert("page".to_string(), Value::Object(page.clone()));
            }
        }
        item.insert("inputPath".to_string(), field("inputPath"));
        item.insert("fileSlug".to_string(), field("fileSlug"));
        item.insert("filePathStem".to_string(), field("filePathStem"));
        if number.is_some() {
            item.insert("page".to_string(), Value::Object(page.clone()));
        }
        item.insert("date".to_string(), date);
        item.insert("outputPath".to_string(), field("outputPath"));
        item.insert("url".to_string(), field("url"));
        item.insert("templateContent".to_string(), Value::Undefined);
        item.insert("content".to_string(), Value::Undefined);
        Page {
            item: Value::Object(Arc::new(item)),
            date_ms,
            input_path,
            tags: included_tags(data),
        }
    }
}

/// JavaScript's `<` on strings compares UTF-16 code units.
fn compare_js(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `Sortable.sortFunctionDateInputPath`.
fn by_date_input_path(a: &Page, b: &Page) -> Ordering {
    a.date_ms
        .cmp(&b.date_ms)
        .then_with(|| compare_js(&a.input_path, &b.input_path))
}

/// A template, as the collections see it.
pub(crate) struct Entry {
    /// `./src/page.md`, as 11ty names it.
    pub input_path: String,
    pub membership: Membership,
    /// The pages, or `None` when they come from [`Collections::build`]'s
    /// callback because the template paginates a collection.
    pub pages: Option<Vec<Page>>,
}

/// What building the collections gave.
pub(crate) struct Built {
    /// `collections`, in the order 11ty fills it in.
    pub collections: Arc<IndexMap<String, Value>>,
    /// The entries `generate` made no pages for because an earlier one
    /// failed, and the one that did.
    pub skipped: Vec<usize>,
    pub failed: Vec<(usize, Problem)>,
}

/// Build the collections. `entries` are in 11ty's template order. `generate`
/// makes the pages of an entry that paginates a collection, given the
/// collections so far (as an object); it runs in 11ty's order, so a
/// template sees what 11ty shows it.
pub(crate) fn build(
    mut entries: Vec<Entry>,
    mut generate: impl FnMut(usize, &Arc<IndexMap<String, Value>>) -> Result<Vec<Page>, Problem>,
) -> Built {
    let mut graph = Graph::new();
    for e in &entries {
        graph.template(&e.input_path, &e.membership);
    }
    let by_path: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.input_path.clone(), i))
        .collect();

    let mut added: Vec<Page> = Vec::new();
    let mut sorted: Vec<usize> = Vec::new();
    let mut dirty = false;
    let mut data: IndexMap<String, Value> = IndexMap::new();
    let mut failed: Vec<(usize, Problem)> = Vec::new();
    let mut skipped: Vec<usize> = Vec::new();

    let snapshot = |added: &Vec<Page>, sorted: &mut Vec<usize>, dirty: &mut bool, tag: &str| {
        if *dirty {
            let mut order: Vec<usize> = (0..added.len()).collect();
            order.sort_by(|a, b| by_date_input_path(&added[*a], &added[*b]));
            *sorted = order;
            *dirty = false;
        }
        let all = tag.is_empty() || tag == "all";
        Value::Array(Arc::new(
            sorted
                .iter()
                .map(|i| &added[*i])
                .filter(|p| all || p.tags.iter().any(|t| t == tag))
                .map(|p| p.item.clone())
                .collect(),
        ))
    };

    let keys = format!("{PREFIX}[keys]");
    for node in graph.order() {
        if node == keys {
            let v = snapshot(&added, &mut sorted, &mut dirty, "all");
            data.insert("all".to_string(), v);
        } else if let Some(tag) = node.strip_prefix(PREFIX) {
            let v = snapshot(&added, &mut sorted, &mut dirty, tag);
            data.insert(tag.to_string(), v);
        } else if let Some(&i) = by_path.get(&node) {
            let pages = match entries[i].pages.take() {
                Some(p) => p,
                None if !failed.is_empty() => {
                    skipped.push(i);
                    continue;
                }
                None => {
                    let so_far = Arc::new(js_object(&data));
                    match generate(i, &so_far) {
                        Ok(p) => p,
                        Err(problem) => {
                            failed.push((i, problem));
                            continue;
                        }
                    }
                }
            };
            let add_all = entries[i].membership.add_all;
            let excluded = entries[i].membership.exclude == Some(Exclude::All);
            for (counter, page) in pages.iter().enumerate() {
                if (counter == 0 || add_all) && !excluded {
                    added.push(page.clone());
                    dirty = true;
                }
            }
        }
    }
    Built {
        collections: Arc::new(js_object(&data)),
        skipped,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(tags: &[&str], consumes: Option<&str>) -> Membership {
        Membership {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            exclude: None,
            consumes: consumes.map(str::to_string),
            add_all: false,
        }
    }

    /// The collections, in the order 11ty fills `collections` (probed).
    fn collection_order(templates: &[(&str, Membership)]) -> Vec<String> {
        let mut g = Graph::new();
        for (path, membership) in templates {
            g.template(path, membership);
        }
        g.order()
            .into_iter()
            .filter_map(|e| e.strip_prefix(PREFIX).map(str::to_string))
            .map(|n| {
                if n == "[keys]" {
                    "(all set)".to_string()
                } else {
                    n
                }
            })
            .collect()
    }

    /// Probed: tags come in the order templates publish them, and `all` is
    /// set once the tags are, then again last.
    #[test]
    fn tags_come_in_template_order() {
        let order = collection_order(&[
            ("./a.md", m(&["t_a"], None)),
            ("./b.md", m(&["t_b"], None)),
            ("./c.md", m(&["t_c"], None)),
            ("./aa/d.md", m(&["t_aa_d"], None)),
            ("./sub/a.md", m(&["t_sub_a"], None)),
        ]);
        assert_eq!(
            order,
            ["t_a", "t_b", "t_c", "t_aa_d", "t_sub_a", "(all set)", "all"]
        );
    }

    /// Probed: a template that paginates a collection comes after the
    /// templates that publish to it, and the collection it names exists
    /// even if nothing publishes to it. `all` is first set where `[keys]`
    /// falls, and the last `all` is the one a template that reads it comes
    /// after (the key keeps its first place).
    #[test]
    fn consumers_come_after_publishers() {
        let order = collection_order(&[
            ("./a.md", m(&["post"], None)),
            ("./b.md", m(&["post"], None)),
            ("./br.md", m(&["other"], Some("post"))),
            ("./every.md", m(&[], Some("all"))),
            ("./list.md", m(&["listing"], Some("post"))),
            ("./meta.md", m(&[], Some("listing"))),
            ("./nothing.md", m(&[], Some("nothing"))),
        ]);
        assert_eq!(
            order,
            [
                "post",
                "other",
                "listing",
                "(all set)",
                "nothing",
                "all",
                "all"
            ]
        );
    }

    #[test]
    fn tag_targets_follow_11ty() {
        assert_eq!(tag_target("collections").as_deref(), Some("[keys]"));
        assert_eq!(tag_target("collections.posts").as_deref(), Some("posts"));
        assert_eq!(tag_target("collections.a.b").as_deref(), Some("a.b"));
        assert_eq!(tag_target("collections[\"post\"]").as_deref(), Some("post"));
        assert_eq!(tag_target("collections['a b']").as_deref(), Some("a b"));
        assert_eq!(tag_target("posts"), None);
        assert_eq!(tag_target("collectionsx"), None);
    }

    #[test]
    fn input_paths_sort_by_utf16_units() {
        // U+FF5E is one unit above the surrogates of U+1F600, which JavaScript
        // sorts first; UTF-8 bytes sort the other way.
        assert_eq!(compare_js("\u{ff5e}", "\u{1f600}"), Ordering::Greater);
        assert!("\u{ff5e}" < "\u{1f600}");
        assert_eq!(compare_js("./a", "./b"), Ordering::Less);
    }

    fn data(pairs: &[(&str, Value)]) -> Data {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn exclusion_follows_11ty() {
        let s = |t: &str| Value::String(t.into());
        let tags = || ("tags", Value::Array(Arc::new(vec![s("t1"), s("t2")])));
        let key = "eleventyExcludeFromCollections";
        let included = |v: Value| included_tags(&data(&[tags(), (key, v)]));
        assert_eq!(included(Value::Bool(true)), Vec::<String>::new());
        assert_eq!(included(s("t1")), ["t2"]);
        assert_eq!(included(s("t1,t2")), Vec::<String>::new());
        assert_eq!(included(Value::Array(Arc::new(vec![s("t2")]))), ["t1"]);
        // `all` and non-string entries exclude no tag.
        assert_eq!(
            included(Value::Array(Arc::new(vec![s("all")]))),
            ["t1", "t2"]
        );
        assert_eq!(included(Value::Number(1.0)), ["t1", "t2"]);
        assert_eq!(included(s("true")), ["t1", "t2"]);
        assert_eq!(included(Value::Bool(false)), ["t1", "t2"]);
    }
}
