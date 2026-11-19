//! Every page's content without its layouts, the way 11ty 3.1.8 makes
//! `templateContent` (`TemplateMap.populateContentDataInMap`).
//!
//! 11ty renders the content of each page one at a time, in the order of its
//! dependency graph, each template's pages in order. Reading the content of
//! a page that isn't rendered yet throws; the template stops there (the
//! pages before stay rendered) and is tried again once, after every other
//! template, in the same order. If a page still reads missing content, 11ty
//! stops the whole build ("contains a circular reference (using
//! collections) to its own templateContent"). Layouts render after all of
//! this, so a layout can read any page's content, its own page's included.
//!
//! Brrrty renders in parallel instead. A page that reads content not
//! rendered yet is a draft: it waits for those pages and renders again when
//! they are done. Each page's content is rendered once and shared, along
//! with the list of pages it read. Then the two passes 11ty would make are
//! replayed from those lists, without rendering anything, to find the pages
//! 11ty would stop on. Those are refused; everything else gets exactly the
//! content 11ty gives it.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use brrrty_js::Value;
use brrrty_js::traps::{self, Content, ContentSource, Outcome};

use crate::plugins::syntaxhighlight::{Record, Recording, YAML_ORDER};
use crate::problem::{Problem, codes};
use crate::render::{Engines, HIGHLIGHT_HELP, Job, Output, Pipeline};

/// One page, as the content renderer sees it.
pub(crate) struct ContentPage<'a> {
    pub job: Job<'a>,
    /// The template's place in 11ty's order.
    pub template: usize,
    pub page_number: Option<usize>,
    /// The page's collection item ([`traps::object_id`]), if it has one.
    pub item: Option<usize>,
    /// 11ty renders it: false for a `permalink` map without `build`.
    pub renderable: bool,
}

impl ContentPage<'_> {
    /// The page as other pages' problems name it.
    fn label(&self) -> String {
        match self.page_number {
            Some(n) => format!("{} (pagination page {n})", self.job.shown.display()),
            None => self.job.shown.display().to_string(),
        }
    }
}

/// How rendering a page's content ended.
enum State {
    Done {
        content: Value,
        /// The pages whose content it read, in the order it read them.
        reads: Vec<usize>,
        /// What its syntax highlighting needs from the pages 11ty renders
        /// before it, and loads for the ones after.
        highlight: Record,
        /// What its render did that makes its content random.
        drawn: crate::random::Drawn,
    },
    /// The page's own problem.
    Failed(Problem),
    /// It read the content of a page that failed or that 11ty doesn't
    /// render.
    Needs(usize),
    /// It waits for content that never comes: these pages.
    Blocked(Vec<usize>),
    /// 11ty doesn't render it.
    Unrendered,
}

/// Content that is final.
enum Final {
    Done(Value),
    Refused,
}

/// The content every page has rendered so far, for pages rendering now.
struct Live {
    ids: Arc<HashMap<usize, usize>>,
    renderable: Vec<bool>,
    cells: Vec<OnceLock<Final>>,
}

impl ContentSource for Live {
    fn content(&self, item: usize) -> Option<Content> {
        let &i = self.ids.get(&item)?;
        if !self.renderable[i] {
            return Some(Content::Refused);
        }
        Some(match self.cells[i].get() {
            Some(Final::Done(v)) => Content::Ready(v.clone()),
            Some(Final::Refused) => Content::Refused,
            None => Content::Pending,
        })
    }
}

/// The content 11ty has once every page's content is rendered: what
/// layouts read.
struct Settled {
    ids: Arc<HashMap<usize, usize>>,
    content: Vec<Option<Value>>,
}

impl ContentSource for Settled {
    fn content(&self, item: usize) -> Option<Content> {
        let &i = self.ids.get(&item)?;
        Some(match &self.content[i] {
            Some(v) => Content::Ready(v.clone()),
            None => Content::Refused,
        })
    }
}

/// Why 11ty's two passes don't give a page its content.
enum Late {
    /// In the second pass, the page read this page's content before it
    /// existed.
    Read(usize),
    /// The page is never rendered: 11ty stopped before it.
    Unreached,
}

/// Every page's content, settled the way 11ty settles it.
pub(crate) struct Contents {
    ids: Arc<HashMap<usize, usize>>,
    labels: Vec<String>,
    shown: Vec<std::path::PathBuf>,
    page_numbers: Vec<Option<usize>>,
    renderable: Vec<bool>,
    states: Vec<State>,
    /// The page has its content once 11ty's two passes are done.
    settled: Vec<bool>,
    late: Vec<Option<Late>>,
}

/// What a page waits for, and the pages that wait for it.
struct Waits {
    /// For each page, the pages waiting for its content.
    waiters: HashMap<usize, Vec<usize>>,
    /// For each page, how many pages it still waits for.
    count: Vec<usize>,
}

struct Run<'a, 'p> {
    pipeline: &'a Pipeline<'a>,
    pages: &'a [ContentPage<'p>],
    check: &'a (dyn Fn(usize, &BTreeSet<String>) -> Option<Problem> + Sync),
    live: Arc<Live>,
    waits: Mutex<Waits>,
    states: Vec<Mutex<Option<State>>>,
    /// The pages each waiting page waits for, for a page left blocked.
    pending: Vec<Mutex<Vec<usize>>>,
}

impl Run<'_, '_> {
    fn render<'s>(&'s self, i: usize, scope: &rayon::Scope<'s>) {
        let unit =
            brrrty_js::functions::UnitGuard::enter(format!("5 content {}", self.pages[i].label()));
        let draws = crate::random::PageGuard::enter(
            self.pages[i].job.shown,
            format!("content {}", self.pages[i].label()),
        );
        let guard = traps::install(self.live.clone());
        let recording = Recording::start();
        let own = brrrty_js::functions::OwnPageGuard::enter(self.pages[i].job.data);
        let result = self.pipeline.content(&self.pages[i].job);
        drop(own);
        let highlight = recording.finish();
        let reads = traps::take_item_reads();
        drop(guard);
        let mut drawn = draws.finish();
        // A feed reads the collections; so may a JavaScript template, in Node.
        let job = &self.pages[i].job;
        drawn.touched |= job.feed.is_some() || matches!(job.engines, Engines::Js(_));
        drop(unit);

        let mut ready = Vec::new();
        let mut pending = Vec::new();
        let mut refused = None;
        for (id, outcome) in reads {
            let Some(&j) = self.live.ids.get(&id) else {
                continue;
            };
            match outcome {
                Outcome::Ready => {
                    if !ready.contains(&j) {
                        ready.push(j);
                    }
                }
                Outcome::Pending => {
                    if !pending.contains(&j) {
                        pending.push(j);
                    }
                }
                Outcome::Refused => {
                    refused.get_or_insert(j);
                }
            }
        }

        if !pending.is_empty() {
            // A draft: wait for the content it read, then render again.
            let mut waits = lock(&self.waits);
            let mut count = 0;
            for &j in &pending {
                if self.live.cells[j].get().is_none() {
                    waits.waiters.entry(j).or_default().push(i);
                    count += 1;
                }
            }
            waits.count[i] = count;
            *lock(&self.pending[i]) = pending;
            drop(waits);
            if count == 0 {
                scope.spawn(move |s| self.render(i, s));
            }
            return;
        }

        let state = match (refused, result) {
            (Some(j), _) => State::Needs(j),
            (None, Err(p)) => State::Failed(p),
            (
                None,
                Ok(Output {
                    html,
                    undefined,
                    js,
                }),
            ) => match (self.check)(i, &undefined) {
                Some(p) => State::Failed(p),
                None => State::Done {
                    // What a JavaScript template gave, as it gave it.
                    content: js.unwrap_or_else(|| Value::String(html.into())),
                    reads: ready,
                    highlight,
                    drawn,
                },
            },
        };
        let fin = match &state {
            State::Done { content, .. } => Final::Done(content.clone()),
            _ => Final::Refused,
        };
        *lock(&self.states[i]) = Some(state);
        let _ = self.live.cells[i].set(fin);

        let woken: Vec<usize> = {
            let mut waits = lock(&self.waits);
            let list = waits.waiters.remove(&i).unwrap_or_default();
            list.into_iter()
                .filter(|&j| {
                    waits.count[j] -= 1;
                    waits.count[j] == 0
                })
                .collect()
        };
        for j in woken {
            scope.spawn(move |s| self.render(j, s));
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Render the content of every page 11ty renders, in parallel. `check`
/// turns what a page read that nothing defined into its problem, if any.
pub(crate) fn render_all(
    pipeline: &Pipeline<'_>,
    pages: &[ContentPage<'_>],
    check: &(dyn Fn(usize, &BTreeSet<String>) -> Option<Problem> + Sync),
) -> Contents {
    let n = pages.len();
    let ids: HashMap<usize, usize> = pages
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.item.map(|id| (id, i)))
        .collect();
    let ids = Arc::new(ids);
    let live = Arc::new(Live {
        ids: ids.clone(),
        renderable: pages.iter().map(|p| p.renderable).collect(),
        cells: (0..n).map(|_| OnceLock::new()).collect(),
    });
    let run = Run {
        pipeline,
        pages,
        check,
        live,
        waits: Mutex::new(Waits {
            waiters: HashMap::new(),
            count: vec![0; n],
        }),
        states: (0..n).map(|_| Mutex::new(None)).collect(),
        pending: (0..n).map(|_| Mutex::new(Vec::new())).collect(),
    };
    rayon::scope(|s| {
        for (i, p) in pages.iter().enumerate() {
            if p.renderable {
                let run = &run;
                s.spawn(move |s| run.render(i, s));
            }
        }
    });

    let Run {
        states, pending, ..
    } = run;
    let states: Vec<State> = states
        .into_iter()
        .zip(pending)
        .zip(pages)
        .map(|((state, pending), page)| {
            let state = state.into_inner().unwrap_or_else(|e| e.into_inner());
            let pending = pending.into_inner().unwrap_or_else(|e| e.into_inner());
            match state {
                Some(s) => s,
                None if !page.renderable => State::Unrendered,
                None => State::Blocked(pending),
            }
        })
        .collect();
    let mut contents = Contents {
        ids,
        labels: pages.iter().map(ContentPage::label).collect(),
        shown: pages.iter().map(|p| p.job.shown.to_path_buf()).collect(),
        page_numbers: pages.iter().map(|p| p.page_number).collect(),
        renderable: pages.iter().map(|p| p.renderable).collect(),
        states,
        settled: vec![false; n],
        late: (0..n).map(|_| None).collect(),
    };
    // A page whose highlighting needs YAML loaded by an earlier page is
    // refused when no page 11ty renders before it loads YAML, and so is
    // every page that read its content; then the passes are replayed again.
    loop {
        let order = contents.replay(pages);
        let unmet = contents.yaml_unmet(&order);
        if unmet.is_empty() {
            break;
        }
        for (i, p) in unmet {
            contents.states[i] = State::Failed(p);
        }
        contents.refuse_readers();
        contents.settled = vec![false; n];
        contents.late = (0..n).map(|_| None).collect();
    }
    contents
}

impl Contents {
    /// What page `i`'s content render did that makes it random, and the
    /// pages whose content it read; `None` when it has no content.
    pub fn random(&self, i: usize) -> Option<(&crate::random::Drawn, &[usize])> {
        match &self.states[i] {
            State::Done { drawn, reads, .. } => Some((drawn, reads)),
            _ => None,
        }
    }

    /// The pages that need YAML loaded by a page 11ty renders before them
    /// (in `order`) when none does, with their problems.
    fn yaml_unmet(&self, order: &[usize]) -> Vec<(usize, Problem)> {
        let mut loaded = false;
        let mut unmet = Vec::new();
        for &i in order {
            let State::Done { highlight, .. } = &self.states[i] else {
                continue;
            };
            if let Some((at, language)) = &highlight.needs_yaml
                && !loaded
            {
                let p = Problem::error(
                    codes::SYNTAX_HIGHLIGHT_UNSUPPORTED,
                    format!(
                        "the fenced code block in `{language}` can't be highlighted the way 11ty highlights it: {YAML_ORDER}, and no page 11ty renders before this one does"
                    ),
                )
                .with_help(HIGHLIGHT_HELP);
                let p = match at {
                    Some((file, line)) => p.with_path(file).at(*line, 1),
                    None => p.with_path(&self.shown[i]),
                };
                unmet.push((i, p));
            }
            loaded |= highlight.loaded_yaml;
        }
        unmet
    }

    /// A page that read the content of a page that has none reads nothing
    /// that 11ty gives it either.
    fn refuse_readers(&mut self) {
        loop {
            let mut changed = false;
            for i in 0..self.states.len() {
                let State::Done { reads, .. } = &self.states[i] else {
                    continue;
                };
                if let Some(&r) = reads
                    .iter()
                    .find(|&&r| !matches!(self.states[r], State::Done { .. }))
                {
                    self.states[i] = State::Needs(r);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// 11ty's two passes, replayed from what each page read. Gives the
    /// pages in the order 11ty first renders their content.
    fn replay(&mut self, pages: &[ContentPage<'_>]) -> Vec<usize> {
        // Templates in 11ty's order, each with its pages in order.
        let mut order: Vec<usize> = (0..pages.len()).filter(|&i| pages[i].renderable).collect();
        order.sort_by_key(|&i| pages[i].template);
        let mut templates: Vec<Vec<usize>> = Vec::new();
        for i in order {
            match templates.last_mut() {
                Some(t) if pages[t[0]].template == pages[i].template => t.push(i),
                _ => templates.push(vec![i]),
            }
        }
        let missing = |settled: &[bool], i: usize, states: &[State]| match &states[i] {
            State::Done { reads, .. } => reads.iter().copied().find(|&r| !settled[r]),
            _ => None,
        };
        let done = |i: usize, states: &[State]| matches!(states[i], State::Done { .. });

        // First pass: a template stops at its first page that can't render.
        let mut rendered = Vec::new();
        let mut again = Vec::new();
        for (t, template) in templates.iter().enumerate() {
            for &i in template {
                if done(i, &self.states) && missing(&self.settled, i, &self.states).is_none() {
                    self.settled[i] = true;
                    rendered.push(i);
                } else {
                    again.push(t);
                    break;
                }
            }
        }
        // Second pass: where 11ty would stop the build, the page is refused
        // and the rest goes on.
        for t in again {
            for &i in &templates[t] {
                if !done(i, &self.states) {
                    continue;
                }
                match missing(&self.settled, i, &self.states) {
                    None if self.settled[i] => {}
                    None => {
                        self.settled[i] = true;
                        rendered.push(i);
                    }
                    Some(r) => self.late[i] = Some(Late::Read(r)),
                }
            }
        }
        for i in 0..pages.len() {
            if done(i, &self.states) && !self.settled[i] && self.late[i].is_none() {
                self.late[i] = Some(Late::Unreached);
            }
        }
        rendered
    }

    /// The content layouts read: every page that has its content.
    pub fn settled_source(&self) -> Arc<dyn ContentSource> {
        Arc::new(Settled {
            ids: self.ids.clone(),
            content: self
                .states
                .iter()
                .zip(&self.settled)
                .map(|(s, &ok)| match s {
                    State::Done { content, .. } if ok => Some(content.clone()),
                    _ => None,
                })
                .collect(),
        })
    }

    /// The page an item is, by its [`traps::object_id`].
    pub fn page_of(&self, item: usize) -> Option<usize> {
        self.ids.get(&item).copied()
    }

    /// Whether 11ty renders page `i` at all.
    pub fn renders(&self, i: usize) -> bool {
        self.renderable[i]
    }

    /// Page `i`'s content, or why it has none, and the page whose problem
    /// explains it, if another. Text, except what a JavaScript template
    /// gave that isn't (`undefined`, a number).
    pub fn content(&self, i: usize) -> Result<&Value, (Problem, Option<usize>)> {
        match &self.states[i] {
            State::Done { content, .. } if self.settled[i] => Ok(content),
            State::Done { .. } => Err(self.late_problem(i)),
            State::Failed(p) => Err((p.clone(), None)),
            State::Needs(j) => Err((self.needs_problem(i, *j), Some(*j))),
            State::Blocked(waits) => Err(self.blocked_problem(i, waits)),
            State::Unrendered => {
                let p = Problem::error(
                    codes::CONTENT_UNAVAILABLE,
                    self.on_page(
                        i,
                        "11ty doesn't render this page: its `permalink` map has no `build` key"
                            .to_string(),
                    ),
                );
                Err((self.at(i, p), None))
            }
        }
    }

    fn on_page(&self, i: usize, message: String) -> String {
        match self.page_numbers[i] {
            Some(n) => format!("{message} (pagination page {n})"),
            None => message,
        }
    }

    fn at(&self, i: usize, p: Problem) -> Problem {
        p.with_path(&self.shown[i])
    }

    /// Page `i` reads the content of page `j`, which has none.
    pub fn needs_problem(&self, i: usize, j: usize) -> Problem {
        let label = &self.labels[j];
        let p = if self.renderable[j] {
            Problem::error(
                codes::CONTENT_UNAVAILABLE,
                self.on_page(
                    i,
                    format!(
                        "this page reads `templateContent` of {label}, which could not be rendered"
                    ),
                ),
            )
            .with_help("Fix the problem reported for that page.")
        } else {
            Problem::error(
                codes::CONTENT_UNAVAILABLE,
                self.on_page(
                    i,
                    format!(
                        "this page reads `templateContent` of {label}, which 11ty doesn't render: its `permalink` map has no `build` key"
                    ),
                ),
            )
            .with_help(
                "11ty stops the whole build here (\"Tried to use templateContent on unrendered template\").",
            )
        };
        self.at(i, p)
    }

    fn late_problem(&self, i: usize) -> (Problem, Option<usize>) {
        match self.late[i] {
            Some(Late::Read(r)) if matches!(self.states[r], State::Done { .. }) => {
                let label = &self.labels[r];
                let p = Problem::error(
                    codes::CONTENT_UNAVAILABLE,
                    self.on_page(
                        i,
                        format!(
                            "this page reads `templateContent` of {label} before 11ty has rendered it"
                        ),
                    ),
                )
                .with_help(format!(
                    "11ty renders a page that reads content not rendered yet once more, after every other page. {label} is still not rendered then, so 11ty stops the whole build here. A layout can read it instead: layouts render after every page's content."
                ));
                (self.at(i, p), None)
            }
            Some(Late::Read(r)) => (self.needs_problem(i, r), Some(r)),
            _ => {
                // Not reached by the replay: an earlier failure in its own
                // template, which is reported for that page.
                let p = Problem::error(
                    codes::CONTENT_UNAVAILABLE,
                    self.on_page(
                        i,
                        "11ty never renders this page's content: it stops the build on another page first".to_string(),
                    ),
                )
                .following("Fix the problems reported above.");
                (self.at(i, p), None)
            }
        }
    }

    fn blocked_problem(&self, i: usize, waits: &[usize]) -> (Problem, Option<usize>) {
        if waits.contains(&i) {
            let p = Problem::error(
                codes::CONTENT_CYCLE,
                self.on_page(
                    i,
                    "this page reads its own `templateContent` through a collection".to_string(),
                ),
            )
            .with_help(
                "Skip the page itself when you loop, for example with `{% if p.url != page.url %}`.",
            );
            return (self.at(i, p), None);
        }
        if let Some(path) = self.cycle_from(i) {
            // Every page of one circle names it the same way, from its first
            // page, so the problem reads the same for each.
            let start = path
                .iter()
                .enumerate()
                .min_by_key(|(_, j)| **j)
                .map_or(0, |(k, _)| k);
            let mut ring: Vec<&str> = path[start..]
                .iter()
                .chain(&path[..start])
                .map(|&j| self.labels[j].as_str())
                .collect();
            ring.push(ring[0]);
            let p = Problem::error(
                codes::CONTENT_CYCLE,
                format!("`templateContent` reads go in a circle: {}", shorten(&ring)),
            )
            .with_help(
                "Change one of these pages so it doesn't read the next one's `templateContent`.",
            );
            return (self.at(i, p), None);
        }
        let j = waits.first().copied().unwrap_or(i);
        (self.needs_problem(i, j), Some(j))
    }

    /// The shortest circle of waiting pages from `i` back to `i`, as the
    /// pages along it, starting with `i`.
    fn cycle_from(&self, i: usize) -> Option<Vec<usize>> {
        let waits = |j: usize| match &self.states[j] {
            State::Blocked(w) => w.as_slice(),
            _ => &[],
        };
        let mut back: HashMap<usize, usize> = HashMap::new();
        let mut queue = VecDeque::from([i]);
        while let Some(j) = queue.pop_front() {
            for &k in waits(j) {
                if k == i {
                    let mut path = vec![j];
                    let mut at = j;
                    while let Some(&prev) = back.get(&at) {
                        path.push(prev);
                        at = prev;
                    }
                    path.reverse();
                    return Some(path);
                }
                if let std::collections::hash_map::Entry::Vacant(e) = back.entry(k) {
                    e.insert(j);
                    queue.push_back(k);
                }
            }
        }
        None
    }
}

/// A long circle of names as its first five, how many are left out, and
/// the last.
fn shorten(names: &[&str]) -> String {
    if names.len() <= 7 {
        return names.join(" -> ");
    }
    let left_out = names.len() - 6;
    format!(
        "{} -> ... {left_out} more -> {}",
        names[..5].join(" -> "),
        names[names.len() - 1]
    )
}

/// 11ty renders a page unless its `permalink` (or computed `permalink`) is
/// a map without a `build` key (`TemplateBehavior.setRenderViaDataCascade`).
pub(crate) fn renderable(data: &indexmap::IndexMap<String, Value>) -> bool {
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    if let Some(Value::Object(p)) = data.get("permalink") {
        keys.extend(p.keys().map(String::as_str));
    }
    if let Some(Value::Object(c)) = data.get("eleventyComputed")
        && let Some(Value::Object(p)) = c.get("permalink")
    {
        keys.extend(p.keys().map(String::as_str));
    }
    keys.is_empty() || keys.contains("build")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_circles_are_shortened() {
        let names: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        let mut ring: Vec<&str> = names.iter().map(String::as_str).collect();
        ring.push("p0");
        assert_eq!(
            shorten(&ring),
            "p0 -> p1 -> p2 -> p3 -> p4 -> ... 5 more -> p0"
        );
        assert_eq!(shorten(&["a", "b", "a"]), "a -> b -> a");
    }
}
