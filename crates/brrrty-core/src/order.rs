//! The order 11ty 3.1.8 adds templates to its template map.
//!
//! That one order seeds 11ty's dependency graph, so it decides the order of
//! `collections` keys, what a collection pagination sees, and the order
//! content renders in. It is not the file order: 11ty finds every template
//! with one glob, then loads all their data at once
//! (`TemplateWriter._addToTemplateMapFullBuild`), and `TemplateMap.add`
//! adds each one when its `getData()` settles. So the order is the order
//! those promises settle in, and that comes from the await structure of the
//! code:
//!
//! 1. `Template.#getData` first awaits the template's own and directory
//!    data files (`TemplateData.getTemplateDirectoryData`). JSON is read
//!    synchronously, so this is 3 microtasks plus 3 per data file. Then
//!    every template waits for global data, whose first load reads the disk
//!    (a glob of `_data`). That is a barrier: every template resumes from
//!    it, in the order it got there, which is fewer data files first, then
//!    the glob's order (shallowest first, then by path).
//! 2. After the barrier there is no I/O, only microtasks, so a simulated
//!    microtask queue gives the exact order. Four kinds of promise are
//!    shared between templates, each made by whoever asks first:
//!    - the template engine for each language (`TemplateEngineManager
//!      .getEngine`), which takes 10 microtasks for Liquid, 17 for HTML and
//!      25 for Markdown: asked for by every template first thing, and by
//!      each layout file for its own language;
//!    - per layout: its front matter, the layout map (its chain, read one
//!      layout after another), and its data (`TemplateLayout`).
//!
//!    A template with a layout waits for that layout's data; one whose
//!    layout chain is longer, or whose layouts nobody asked for yet, waits
//!    longer.
//! 3. Layouts are cached (`LayoutCache`) by the name as written, joined to
//!    the input folder, and each new layout is also registered under its
//!    path in `_includes`, replacing what was there. So `layout: post` and
//!    `layout: post.liquid` share a layout only sometimes; this is
//!    simulated as it happens.
//! 4. A page with `date: Created` and the like, or with no `date` and no
//!    `YYYY-MM-DD` in its input path, then reads the file's dates from the
//!    disk, so those pages come after all the others, in the order they
//!    asked. A date in the file or folder name needs no disk.
//!
//! Using a shared promise through one async wrapper resumes 2 microtasks
//! after it settles, or 3 after the call if it has already settled. The
//! other step counts were measured on 11ty itself: the simulation gives
//! every template's exact settle time on 79 random sites, and the order
//! 11ty builds with on 230 more.
//!
//! What can't be predicted is real I/O. The glob's walk order is the
//! warm-disk order (a first run on files just written can differ), the
//! undated pages finish in the order the disk answers, and a JavaScript
//! data file loads with `import()`. The first two are 11ty's own
//! randomness; for the last the caller refuses collections.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Microtasks the engine for a language takes to load: from the first
/// `TemplateEngineManager.getEngine(name)` call to its cached promise
/// settling (`import()` of the already-loaded engine module, then the
/// constructor). Measured with `tools/order-calibration` (`events.mjs -v`: "call engine" to
/// "settle engine").
fn engine_cost(language: &str) -> usize {
    match language {
        // `Liquid.js` engine.
        "liquid" => 10,
        // `Html.js` engine.
        "html" => 17,
        // `Nunjucks.js` engine.
        "njk" => 17,
        // `Markdown.js` engine (and anything else: JavaScript templates
        // refuse collections, so their place never counts).
        _ => 25,
    }
}

/// `Template.#getData`: from resuming after the template's engine (in
/// `getTemplateRender`) to calling `layout.getData()`: the rest of reading
/// its front matter and the second `getTemplateRender`. Measured with
/// `tools/order-calibration`.
const TO_LAYOUT: usize = 13;
/// `Template.#getData` with no layout: from resuming after the engine to
/// `TemplateMap.add` pushing the entry (merge, `addPage`, `getMappedDate`,
/// `getTemplateMapEntries`). Measured with `tools/order-calibration`.
const PLAIN: usize = 19;
/// From resuming after `layout.getData()` to `TemplateMap.add` pushing the
/// entry. Measured with `tools/order-calibration`.
const AFTER_LAYOUT: usize = 6;
/// `TemplateContent.#getFrontMatterData` of a layout file: from resuming
/// after the layout's engine to its front matter promise settling (reading
/// and parsing the file). Measured with `tools/order-calibration`.
const FRONT_MATTER: usize = 10;

/// One template, as the order sees it.
#[derive(Clone)]
pub(crate) struct OrderTemplate {
    /// Relative to the input folder.
    pub rel: PathBuf,
    /// Its JSON template and directory data files.
    pub data_files: usize,
    /// The layout its data names, as written (front matter, then data
    /// files, then global data).
    pub layout: Option<String>,
    /// 11ty finds its date without reading the disk: a `date` in its data,
    /// or a `YYYY-MM-DD` in its input path.
    pub dated: bool,
    /// A virtual template (a feed): 11ty adds them to the glob's list of
    /// files, after every file.
    pub added_last: bool,
    /// A JavaScript template: 11ty waits on its `import()` before it can
    /// add it, so it goes with the pages that read the disk, and its place
    /// among them isn't known (see `build.rs`, `order_unknown`).
    pub js: bool,
}

/// A layout file, by a name as written.
#[derive(Clone)]
pub(crate) struct OrderLayout {
    /// Its path in `_includes`, which 11ty also caches it under.
    pub normalized: String,
    /// Its file extension: the engine its front matter needs.
    pub language: String,
    /// The `layout` its front matter names, as written.
    pub parent: Option<String>,
}

/// The glob's order: shallowest first, then by path.
fn walk_key(rel: &Path) -> (usize, &Path) {
    (rel.components().count(), rel)
}

fn language(rel: &Path) -> String {
    rel.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The indexes of `templates` in the order 11ty adds them to its map.
/// `layouts` resolves every layout name the templates and layouts use;
/// `input_dir` is the input folder as 11ty spells it (`.`, `src`).
pub(crate) fn entry_order(
    templates: &[OrderTemplate],
    layouts: &HashMap<String, OrderLayout>,
    input_dir: &str,
) -> Vec<usize> {
    let mut barrier: Vec<usize> = (0..templates.len()).collect();
    barrier.sort_by(|&a, &b| {
        let (ta, tb) = (&templates[a], &templates[b]);
        let ka = (ta.data_files, ta.added_last, walk_key(&ta.rel));
        let kb = (tb.data_files, tb.added_last, walk_key(&tb.rel));
        ka.cmp(&kb)
    });
    let mut sim = Sim::new(templates, layouts, input_dir);
    for &t in &barrier {
        let p = sim.new_proc(Kind::Template(t), None);
        sim.advance(p);
    }
    sim.run();
    let (dated, undated): (Vec<usize>, Vec<usize>) =
        sim.done.iter().partition(|&&t| templates[t].dated);
    dated.into_iter().chain(undated).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Shared {
    Engine(usize),
    FrontMatter(usize),
    Map(usize),
    Data(usize),
}

#[derive(Clone, Copy)]
enum Kind {
    Template(usize),
    Engine(usize),
    FrontMatter(usize),
    /// The layout being read.
    Map(usize),
    Data(usize),
}

struct Proc {
    kind: Kind,
    step: usize,
    /// The promise it settles, if shared.
    promise: Option<usize>,
}

struct Promise {
    done: bool,
    subs: Vec<usize>,
}

enum Job {
    /// Resume the process `n` microtasks from when this job was queued.
    Delay(usize, usize),
    /// The thenable job of an async wrapper (`async f() { return cached }`)
    /// around a shared promise: one microtask after the call. If the promise
    /// has settled, its caller resumes 2 microtasks later (call + 3);
    /// otherwise it subscribes. Checked with `tools/order-calibration`.
    Thenable(usize, usize),
    /// A subscriber's reaction to a settled shared promise: the wrapper
    /// settles, and its caller resumes one microtask later (settle + 2).
    Reaction(usize),
}

/// What a process does next.
enum Next {
    Hop(usize),
    Use(Shared),
    Done,
}

struct Sim<'a> {
    templates: &'a [OrderTemplate],
    layouts: &'a HashMap<String, OrderLayout>,
    input_dir: &'a str,
    queue: VecDeque<Job>,
    procs: Vec<Proc>,
    promises: Vec<Promise>,
    shared: HashMap<Shared, usize>,
    languages: Vec<String>,
    /// `LayoutCache`: key to layout instance.
    cache: HashMap<String, usize>,
    /// Layout instances: their language and the parent name.
    instances: Vec<(usize, Option<String>)>,
    /// Templates, as their data settles.
    done: Vec<usize>,
}

impl<'a> Sim<'a> {
    fn new(
        templates: &'a [OrderTemplate],
        layouts: &'a HashMap<String, OrderLayout>,
        input_dir: &'a str,
    ) -> Sim<'a> {
        Sim {
            templates,
            layouts,
            input_dir,
            queue: VecDeque::new(),
            procs: Vec::new(),
            promises: Vec::new(),
            shared: HashMap::new(),
            languages: Vec::new(),
            cache: HashMap::new(),
            instances: Vec::new(),
            done: Vec::new(),
        }
    }

    fn language_id(&mut self, name: &str) -> usize {
        match self.languages.iter().position(|l| l == name) {
            Some(i) => i,
            None => {
                self.languages.push(name.to_string());
                self.languages.len() - 1
            }
        }
    }

    /// `TemplateLayout.getTemplate`: the cached layout for `name`, or a new
    /// one registered under its full key and its path in `_includes`.
    fn layout(&mut self, name: &str) -> Option<usize> {
        let full = match self.input_dir {
            "" | "." | "./" => name.to_string(),
            dir => format!("{}/{name}", dir.trim_end_matches('/')),
        };
        if let Some(&i) = self.cache.get(&full) {
            return Some(i);
        }
        let info = self.layouts.get(name)?.clone();
        let lang = self.language_id(&info.language);
        let i = self.instances.len();
        self.instances.push((lang, info.parent));
        self.cache.insert(full, i);
        self.cache.insert(info.normalized, i);
        Some(i)
    }

    fn new_proc(&mut self, kind: Kind, promise: Option<usize>) -> usize {
        self.procs.push(Proc {
            kind,
            step: 0,
            promise,
        });
        self.procs.len() - 1
    }

    /// The shared promise `s`, made (and its process started) on first use.
    fn shared(&mut self, s: Shared) -> usize {
        if let Some(&p) = self.shared.get(&s) {
            return p;
        }
        self.promises.push(Promise {
            done: false,
            subs: Vec::new(),
        });
        let promise = self.promises.len() - 1;
        self.shared.insert(s, promise);
        let kind = match s {
            Shared::Engine(l) => Kind::Engine(l),
            Shared::FrontMatter(i) => Kind::FrontMatter(i),
            Shared::Map(i) => Kind::Map(i),
            Shared::Data(i) => Kind::Data(i),
        };
        let proc = self.new_proc(kind, Some(promise));
        self.advance(proc);
        promise
    }

    /// The next instruction of process `p`, moving it on a step.
    fn step(&mut self, p: usize) -> Next {
        let step = self.procs[p].step;
        self.procs[p].step += 1;
        match self.procs[p].kind {
            Kind::Template(t) => {
                let layout = self.templates[t].layout.clone();
                match (step, layout) {
                    (0, _) => {
                        let lang = language(&self.templates[t].rel);
                        let l = self.language_id(&lang);
                        Next::Use(Shared::Engine(l))
                    }
                    (1, None) => Next::Hop(PLAIN),
                    (1, Some(_)) => Next::Hop(TO_LAYOUT),
                    (2, Some(name)) => match self.layout(&name) {
                        Some(i) => Next::Use(Shared::Data(i)),
                        None => Next::Hop(AFTER_LAYOUT),
                    },
                    (3, Some(_)) => Next::Hop(AFTER_LAYOUT),
                    _ => {
                        self.done.push(t);
                        Next::Done
                    }
                }
            }
            Kind::Engine(l) => match step {
                0 => Next::Hop(engine_cost(&self.languages[l])),
                _ => Next::Done,
            },
            Kind::FrontMatter(i) => match step {
                0 => Next::Use(Shared::Engine(self.instances[i].0)),
                1 => Next::Hop(FRONT_MATTER),
                _ => Next::Done,
            },
            Kind::Map(i) => {
                // Read the layout's front matter, move on a microtask, then
                // (`#getTemplateLayoutMap`, on resuming) look up the parent.
                if step == 0 {
                    Next::Use(Shared::FrontMatter(i))
                } else if step % 2 == 1 {
                    // `#getTemplateLayoutMap` resumes one microtask after
                    // `getTemplateLayoutMapEntry` settles.
                    Next::Hop(1)
                } else {
                    match self.instances[i].1.clone().and_then(|n| self.layout(&n)) {
                        Some(parent) => {
                            self.procs[p].kind = Kind::Map(parent);
                            Next::Use(Shared::FrontMatter(parent))
                        }
                        None => Next::Done,
                    }
                }
            }
            Kind::Data(i) => match step {
                0 => Next::Use(Shared::Map(i)),
                _ => Next::Done,
            },
        }
    }

    /// Run process `p` until it waits (`await` runs the async function at
    /// once until its first await).
    fn advance(&mut self, p: usize) {
        match self.step(p) {
            Next::Hop(n) => self.queue.push_back(Job::Delay(n, p)),
            Next::Use(s) => {
                let promise = self.shared(s);
                self.queue.push_back(Job::Thenable(promise, p));
            }
            Next::Done => self.settle(p),
        }
    }

    fn settle(&mut self, p: usize) {
        if let Some(promise) = self.procs[p].promise {
            self.promises[promise].done = true;
            for s in std::mem::take(&mut self.promises[promise].subs) {
                self.queue.push_back(Job::Reaction(s));
            }
        }
    }

    fn run(&mut self) {
        while let Some(job) = self.queue.pop_front() {
            match job {
                Job::Delay(1, p) => self.advance(p),
                Job::Delay(n, p) => self.queue.push_back(Job::Delay(n - 1, p)),
                Job::Thenable(promise, p) => {
                    if self.promises[promise].done {
                        self.queue.push_back(Job::Delay(2, p));
                    } else {
                        self.promises[promise].subs.push(p);
                    }
                }
                Job::Reaction(p) => self.queue.push_back(Job::Delay(1, p)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(rel: &str, data_files: usize, layout: Option<&str>) -> OrderTemplate {
        OrderTemplate {
            rel: PathBuf::from(rel),
            data_files,
            layout: layout.map(str::to_string),
            dated: true,
            added_last: false,
            js: false,
        }
    }

    fn layouts(list: &[(&str, &str, Option<&str>)]) -> HashMap<String, OrderLayout> {
        let mut m = HashMap::new();
        for (file, lang, parent) in list {
            let info = OrderLayout {
                normalized: file.to_string(),
                language: lang.to_string(),
                parent: parent.map(str::to_string),
            };
            m.insert(file.to_string(), info.clone());
            let stem = file.split('.').next().unwrap_or(file);
            m.entry(stem.to_string()).or_insert(info);
        }
        m
    }

    fn order(templates: &[OrderTemplate], l: &HashMap<String, OrderLayout>) -> Vec<String> {
        entry_order(templates, l, ".")
            .into_iter()
            .map(|i| templates[i].rel.display().to_string())
            .collect()
    }

    /// Measured (each template's settle time on 11ty's microtask queue):
    /// no layout, Liquid 31, HTML 38, Markdown 46; data files hold a
    /// template back within its time; a layout 51, a chain of two 55.
    #[test]
    fn data_files_and_languages() {
        let l = layouts(&[
            ("l.liquid", "liquid", None),
            ("chain.liquid", "liquid", Some("l.liquid")),
        ]);
        let ts = [
            t("a.md", 0, None),
            t("a1.liquid", 0, None),
            t("a2.liquid", 1, None),
            t("b.liquid", 0, None),
            t("c.html", 0, None),
            t("c1.liquid", 0, Some("chain.liquid")),
            t("l1.liquid", 0, Some("l.liquid")),
            t("l2.md", 0, Some("l.liquid")),
            t("d/x.liquid", 1, None),
            t("d/y.md", 1, None),
            t("e/z.liquid", 0, None),
        ];
        assert_eq!(
            order(&ts, &l),
            [
                "a1.liquid",
                "b.liquid",
                "e/z.liquid",
                "a2.liquid",
                "d/x.liquid",
                "c.html",
                "a.md",
                "d/y.md",
                "l1.liquid",
                "l2.md",
                "c1.liquid"
            ]
        );
    }

    /// Measured: a chain whose parent nobody asked for yet settles after a
    /// plain layout; `layout: post` makes a new layout that takes over
    /// `post.liquid` for every template that asks after it.
    #[test]
    fn layout_cache_and_chains() {
        let l = layouts(&[("post.liquid", "liquid", None)]);
        let ts = [
            t("a9.md", 0, Some("post")),
            t("b8.liquid", 0, None),
            t("c2.md", 0, Some("post.liquid")),
            t("keys.liquid", 0, None),
            t("docs/Post3.liquid", 2, Some("post.liquid")),
            t("docs/c5.md", 1, Some("post.liquid")),
            t("a/b/b6.md", 1, Some("post.liquid")),
            t("posts/2020/a0.md", 2, None),
        ];
        assert_eq!(
            order(&ts, &l),
            [
                "b8.liquid",
                "keys.liquid",
                "posts/2020/a0.md",
                "docs/Post3.liquid",
                "a9.md",
                "c2.md",
                "docs/c5.md",
                "a/b/b6.md"
            ]
        );
    }

    /// Measured: a Markdown layout costs the Markdown engine's load unless a
    /// Markdown template asked for it first.
    #[test]
    fn a_layout_shares_the_engine_of_its_language() {
        let l = layouts(&[("l.md", "md", None)]);
        let alone = [t("a.liquid", 0, Some("l.md")), t("z.liquid", 0, None)];
        assert_eq!(order(&alone, &l), ["z.liquid", "a.liquid"]);
        let shared = [
            t("a.liquid", 0, Some("l.md")),
            t("m.md", 0, None),
            t("n.html", 0, None),
        ];
        // n settles at 38, m at 46, a at 51.
        assert_eq!(order(&shared, &l), ["n.html", "m.md", "a.liquid"]);
    }

    /// Measured: the Nunjucks engine loads as fast as HTML's, for a
    /// template (`z.njk` at 38, with `h.html`) and for a layout (`b` at 51).
    #[test]
    fn nunjucks_takes_its_place() {
        let l = layouts(&[("n.njk", "njk", None)]);
        let ts = [
            t("a.md", 0, None),
            t("b.liquid", 0, Some("n.njk")),
            t("h.html", 0, None),
            t("y.liquid", 0, None),
            t("z.njk", 0, None),
        ];
        assert_eq!(
            order(&ts, &l),
            ["y.liquid", "h.html", "z.njk", "a.md", "b.liquid"]
        );
    }

    /// Pages that read their date from the disk come last.
    #[test]
    fn undated_pages_come_last() {
        let l = layouts(&[]);
        let mut ts = [t("a.liquid", 0, None), t("b.md", 0, None)];
        ts[0].dated = false;
        assert_eq!(order(&ts, &l), ["b.md", "a.liquid"]);
    }
}
