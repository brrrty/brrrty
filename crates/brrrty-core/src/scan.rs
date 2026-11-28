//! The static scan `brrrty doctor` runs on refused templates.
//!
//! Rendering stops at a template's first error, so a page refused for one
//! missing filter never shows the next one, or the JavaScript data it
//! reads. The scan parses each refused template, its layouts and every
//! include it names by a literal name, with Brrrty's own parsers, and lists
//! every filter the engine doesn't have and every read of a global data key
//! that a JavaScript data file sets. It doesn't render anything: a filter in
//! a branch the page never takes is listed too.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_js::Loader;

use crate::data::js_help;
use crate::frontmatter::split_front_matter;
use crate::options::TemplateEngines;
use crate::problem::{Problem, codes};
use crate::render::body_line;
use crate::survey::TemplateSurvey;
use crate::templates::{Lang, Langs, TplError};

/// A filter name, a variable read, or a partial named, with its 1-based
/// line and column in the parsed text.
#[derive(Debug, Default)]
struct Found {
    filters: Vec<(String, usize, usize)>,
    reads: Vec<(String, usize, usize)>,
    partials: Vec<String>,
    /// Names the template sets itself, which hide data of the same name.
    bound: BTreeSet<String>,
}

/// What scanning one file gave.
#[derive(Debug, Default)]
struct FileScan {
    problems: Vec<Problem>,
    /// The partials it names that were found, relative to the project root,
    /// with the file to read.
    partials: Vec<(PathBuf, PathBuf)>,
}

/// The language that reads a file with extension `ext` (lowercase) when
/// nothing overrides it.
pub(crate) fn lang_of_ext(ext: &str, engines: TemplateEngines) -> Option<Lang> {
    match ext {
        "liquid" => Some(Lang::Liquid),
        "njk" => Some(Lang::Nunjucks),
        "md" => engines.markdown,
        "html" => engines.html,
        _ => None,
    }
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub(crate) struct Scanner<'a> {
    langs: &'a Langs<'a>,
    engines: TemplateEngines,
    root: &'a Path,
    /// Global data key to the JavaScript file that sets it.
    js_keys: &'a BTreeMap<String, PathBuf>,
    cache: HashMap<(PathBuf, Lang, bool), Arc<FileScan>>,
}

impl<'a> Scanner<'a> {
    pub fn new(
        langs: &'a Langs<'a>,
        engines: TemplateEngines,
        root: &'a Path,
        js_keys: &'a BTreeMap<String, PathBuf>,
    ) -> Self {
        Scanner {
            langs,
            engines,
            root,
            js_keys,
            cache: HashMap::new(),
        }
    }

    /// Add what the scan finds to every refused template. A finding on a
    /// line where the template already has a problem of the same code is
    /// the same finding, and isn't added twice.
    pub fn add_findings(&mut self, templates: &mut BTreeMap<PathBuf, TemplateSurvey>) {
        let js_files: BTreeSet<PathBuf> = self.js_keys.values().cloned().collect();
        for (shown, t) in templates.iter_mut() {
            if t.refused == 0 {
                continue;
            }
            for p in self.template(shown, t) {
                // The build's report of the same thing: same cause, same place.
                let cause = crate::doctor::causes(&p, &js_files);
                let seen = t.problems.iter().any(|q| {
                    q.code == p.code
                        && q.path == p.path
                        && q.line == p.line
                        && crate::doctor::causes(q, &js_files) == cause
                });
                if !seen && !t.problems.contains(&p) {
                    t.problems.push(p);
                }
            }
        }
    }

    /// Everything found in a template, its layouts and their partials.
    fn template(&mut self, shown: &Path, t: &TemplateSurvey) -> Vec<Problem> {
        let lang = if t.lang_known {
            t.lang
        } else {
            lang_of_ext(&ext_of(shown), self.engines)
        };
        let mut queue: Vec<(PathBuf, PathBuf, Lang, bool)> = Vec::new();
        if let Some(l) = lang {
            queue.push((shown.to_path_buf(), self.root.join(shown), l, true));
        }
        for layout in &t.layouts {
            if let Some(l) = lang_of_ext(&ext_of(layout), self.engines) {
                queue.push((layout.clone(), self.root.join(layout), l, true));
            }
        }
        let mut done: BTreeSet<(PathBuf, Lang)> = BTreeSet::new();
        let mut out = self.data_strings(shown, t);
        while let Some((shown, real, lang, front)) = queue.pop() {
            if !done.insert((shown.clone(), lang)) {
                continue;
            }
            let scan = self.file(&shown, &real, lang, front);
            out.extend(scan.problems.iter().cloned());
            for (s, r) in &scan.partials {
                queue.push((s.clone(), r.clone(), lang, false));
            }
        }
        out
    }

    /// What `permalink` and `eleventyComputed` use: 11ty renders them as templates in
    /// the page's data language. One that doesn't parse is reported by the build.
    fn data_strings(&self, shown: &Path, t: &TemplateSurvey) -> Vec<Problem> {
        let Some(lang) = t.data_lang else {
            return Vec::new();
        };
        let name = shown.display().to_string();
        let mut out = Vec::new();
        for (key, text, line) in &t.data_strings {
            if !crate::templates::has_tags(lang, text) {
                continue;
            }
            let mut found = Found::default();
            match lang {
                Lang::Liquid => match self.langs.liquid.parse_unchecked(&name, text) {
                    Ok(t) => liquid::nodes(&t.nodes, &mut found),
                    Err(_) => continue,
                },
                Lang::Nunjucks => match self.langs.nunjucks.parse(&name, text) {
                    Ok(t) => nunjucks::nodes(&t.nodes, &mut found),
                    Err(_) => continue,
                },
            }
            let place = |p: Problem| {
                let p = p.with_path(shown);
                match line {
                    Some(l) => p.at(*l, 1),
                    None => p,
                }
            };
            for (filter, _, _) in &found.filters {
                if let Some(p) = self.unknown_filter(lang, filter) {
                    out.push(place(Problem {
                        message: format!("{} in `{key}`", p.message),
                        ..p
                    }));
                }
            }
            for (var, _, _) in &found.reads {
                if let Some(file) = self.js_keys.get(var)
                    && !found.bound.contains(var)
                {
                    out.push(place(js_read(var, file, &format!(" in `{key}`"))));
                }
            }
        }
        out
    }

    /// The problem for a filter `lang` doesn't have, or `None` when it has it.
    fn unknown_filter(&self, lang: Lang, filter: &str) -> Option<Problem> {
        match lang {
            Lang::Liquid if !self.langs.liquid.has_filter(filter) => Some(Problem::error(
                codes::LIQUID_UNKNOWN_FILTER,
                format!("undefined filter: {filter}"),
            )),
            Lang::Nunjucks if !self.langs.nunjucks.has_filter(filter) => Some(Problem::error(
                codes::NUNJUCKS_RENDER,
                format!("Error: filter not found: {filter}"),
            )),
            _ => None,
        }
    }

    fn file(&mut self, shown: &Path, real: &Path, lang: Lang, front: bool) -> Arc<FileScan> {
        let key = (shown.to_path_buf(), lang, front);
        if let Some(s) = self.cache.get(&key) {
            return s.clone();
        }
        let scan = Arc::new(self.scan_file(shown, real, lang, front));
        self.cache.insert(key, scan.clone());
        scan
    }

    fn scan_file(&self, shown: &Path, real: &Path, lang: Lang, front: bool) -> FileScan {
        let mut out = FileScan::default();
        let Ok(text) = fs::read_to_string(real) else {
            return out;
        };
        let (body, offset) = if front {
            match split_front_matter(&text) {
                Ok((_, body)) => (body, body_line(&text, body)),
                Err(_) => return out,
            }
        } else {
            (text.as_str(), 0)
        };
        let name = shown.display().to_string();
        let found = match lang {
            Lang::Liquid => match self.langs.liquid.parse_unchecked(&name, body) {
                Ok(t) => {
                    let mut f = Found::default();
                    liquid::nodes(&t.nodes, &mut f);
                    f
                }
                Err(e) => {
                    out.problems.push(self.langs.problem(
                        &TplError::Liquid(e),
                        shown.to_path_buf(),
                        offset,
                        None,
                    ));
                    return out;
                }
            },
            Lang::Nunjucks => match self.langs.nunjucks.parse(&name, body) {
                Ok(t) => {
                    let mut f = Found::default();
                    nunjucks::nodes(&t.nodes, &mut f);
                    f
                }
                Err(e) => {
                    out.problems.push(self.langs.problem(
                        &TplError::Nunjucks(e),
                        shown.to_path_buf(),
                        offset,
                        None,
                    ));
                    return out;
                }
            },
        };
        for (filter, line, column) in &found.filters {
            if let Some(p) = self.unknown_filter(lang, filter) {
                out.problems
                    .push(p.with_path(shown).at(line + offset, *column));
            }
        }
        let mut reported = BTreeSet::new();
        for (var, line, column) in &found.reads {
            let Some(file) = self.js_keys.get(var) else {
                continue;
            };
            if found.bound.contains(var) || !reported.insert(var.clone()) {
                continue;
            }
            out.problems.push(
                js_read(var, file, "")
                    .with_path(shown)
                    .at(line + offset, *column),
            );
        }
        let loader = match lang {
            Lang::Liquid => self.langs.liquid_loader,
            Lang::Nunjucks => self.langs.nunjucks_loader,
        };
        for partial in &found.partials {
            let mut names = vec![partial.clone()];
            // LiquidJS adds `.liquid` to a name without an extension.
            if lang == Lang::Liquid && Path::new(partial).extension().is_none() {
                names.insert(0, format!("{partial}.liquid"));
            }
            for n in names {
                if let Ok((real, _)) = loader.load(&n) {
                    let real = PathBuf::from(real);
                    if let Some(s) = loader.shown(&real) {
                        out.partials.push((s, real));
                    }
                    break;
                }
            }
        }
        out
    }
}

/// The problem for a read of `var`, which the JavaScript data file `file` sets.
fn js_read(var: &str, file: &Path, suffix: &str) -> Problem {
    Problem::error(
        codes::DATA_JS_UNSUPPORTED,
        format!(
            "this template reads `{var}`, which {}, a JavaScript data file, sets{suffix}",
            file.display()
        ),
    )
    .with_help(js_help(file))
}

mod liquid {
    use super::Found;
    use brrrty_liquid::ast::{
        Expr, FilePath, FilterArg, FilteredValue, HashArg, Node, Segment, TagKind,
    };

    pub fn nodes(nodes: &[Node], f: &mut Found) {
        for n in nodes {
            match n {
                Node::Text(_) => {}
                Node::Output(o) => value(&o.value, f),
                Node::Tag(t) => tag(&t.kind, f),
            }
        }
    }

    fn tag(kind: &TagKind, f: &mut Found) {
        match kind {
            TagKind::If {
                branches,
                else_body,
            } => {
                for b in branches {
                    value(&b.condition, f);
                    nodes(&b.body, f);
                }
                if let Some(e) = else_body {
                    nodes(e, f);
                }
            }
            TagKind::Unless {
                branches,
                else_body,
            } => {
                for b in branches {
                    value(&b.condition, f);
                    nodes(&b.body, f);
                }
                nodes(else_body, f);
            }
            TagKind::Case {
                value: v,
                whens,
                else_body,
            } => {
                value(v, f);
                for w in whens {
                    for e in &w.values {
                        expr(e, f);
                    }
                    nodes(&w.body, f);
                }
                nodes(else_body, f);
            }
            TagKind::For(l) => {
                f.bound.insert(l.variable.clone());
                expr(&l.collection, f);
                args(&l.args, f);
                nodes(&l.body, f);
                nodes(&l.else_body, f);
            }
            TagKind::Tablerow(l) => {
                f.bound.insert(l.variable.clone());
                expr(&l.collection, f);
                args(&l.args, f);
                nodes(&l.body, f);
            }
            TagKind::Assign { name, value: v } => {
                f.bound.insert(name.clone());
                value(v, f);
            }
            TagKind::Capture { name, body } => {
                f.bound.insert(name.clone());
                nodes(body, f);
            }
            TagKind::Increment { name } | TagKind::Decrement { name } => {
                f.bound.insert(name.clone());
            }
            TagKind::Cycle { group, candidates } => {
                if let Some(g) = group {
                    expr(g, f);
                }
                for c in candidates {
                    value(c, f);
                }
            }
            TagKind::Include(i) => {
                file(&i.file, f);
                if let Some(w) = &i.with {
                    expr(w, f);
                }
                args(&i.args, f);
            }
            TagKind::Render(r) => {
                file(&r.file, f);
                for b in [&r.with, &r.for_binding].into_iter().flatten() {
                    expr(&b.value, f);
                }
                args(&r.args, f);
            }
            TagKind::Layout(l) => {
                file(&l.file, f);
                args(&l.args, f);
                nodes(&l.body, f);
            }
            TagKind::Block { body, .. } | TagKind::Liquid { body } => nodes(body, f),
            TagKind::Echo { value: v } => {
                if let Some(v) = v {
                    value(v, f);
                }
            }
            TagKind::RawShortcode { args: a, .. } => {
                for e in a {
                    expr(e, f);
                }
            }
            TagKind::Shortcode { args: a, body } => {
                for e in a {
                    expr(e, f);
                }
                if let Some(b) = body {
                    nodes(b, f);
                }
            }
            TagKind::Break
            | TagKind::Continue
            | TagKind::Raw { .. }
            | TagKind::Comment
            | TagKind::InlineComment
            | TagKind::RawBlock { .. } => {}
        }
    }

    fn file(path: &FilePath, f: &mut Found) {
        match path {
            FilePath::Name(n) => f.partials.push(n.clone()),
            FilePath::Template(n) => nodes(n, f),
            FilePath::Expr(e) => expr(e, f),
            FilePath::None => {}
        }
    }

    fn args(list: &[HashArg], f: &mut Found) {
        for a in list {
            if let Some(v) = &a.value {
                expr(v, f);
            }
        }
    }

    fn value(v: &FilteredValue, f: &mut Found) {
        expr(&v.expr, f);
        for filter in &v.filters {
            f.filters
                .push((filter.name.clone(), filter.span.line, filter.span.column));
            for a in &filter.args {
                match a {
                    FilterArg::Positional(e) => expr(e, f),
                    FilterArg::Keyword { value, .. } => {
                        if let Some(e) = value {
                            expr(e, f);
                        }
                    }
                }
            }
        }
    }

    fn expr(e: &Expr, f: &mut Found) {
        match e {
            Expr::Literal { .. } | Expr::Missing { .. } => {}
            Expr::Path { root, segments, .. } => {
                match root {
                    Some(r) => expr(r, f),
                    None => {
                        if let Some(Segment::Key { span, name }) = segments.first() {
                            f.reads.push((name.clone(), span.line, span.column));
                        }
                    }
                }
                for s in segments {
                    if let Segment::Index(i) = s {
                        expr(i, f);
                    }
                }
            }
            Expr::Range { start, end, .. } => {
                expr(start, f);
                expr(end, f);
            }
            Expr::Binary { lhs, rhs, .. } => {
                expr(lhs, f);
                expr(rhs, f);
            }
            Expr::Not { operand, .. } => expr(operand, f),
        }
    }
}

mod nunjucks {
    use super::Found;
    use brrrty_nunjucks::ast::{Args, Expr, Literal, Node, SetValue};

    pub fn nodes(nodes: &[Node], f: &mut Found) {
        for n in nodes {
            node(n, f);
        }
    }

    fn bind(e: &Expr, f: &mut Found) {
        match e {
            Expr::Symbol { name, .. } => {
                f.bound.insert(name.clone());
            }
            Expr::Array { items, .. } | Expr::Group { items, .. } => {
                for i in items {
                    bind(i, f);
                }
            }
            _ => {}
        }
    }

    fn partial(e: &Expr, f: &mut Found) {
        match e {
            Expr::Literal {
                value: Literal::String(s),
                ..
            } => f.partials.push(s.clone()),
            other => expr(other, f),
        }
    }

    fn node(n: &Node, f: &mut Found) {
        match n {
            Node::Text(_) => {}
            Node::Output(o) => expr(&o.expr, f),
            Node::If(i) => {
                for b in &i.branches {
                    expr(&b.cond, f);
                    nodes(&b.body, f);
                }
                if let Some(e) = &i.else_body {
                    nodes(e, f);
                }
            }
            Node::For(l) => {
                for t in &l.targets {
                    bind(t, f);
                }
                expr(&l.iter, f);
                nodes(&l.body, f);
                if let Some(e) = &l.else_body {
                    nodes(e, f);
                }
            }
            Node::Set(s) => {
                for t in &s.targets {
                    bind(t, f);
                }
                match &s.value {
                    SetValue::Expr(e) => expr(e, f),
                    SetValue::Capture(c) => nodes(&c.body, f),
                }
            }
            Node::Block(b) => nodes(&b.body, f),
            Node::Extends(e) => partial(&e.template, f),
            Node::Include(i) => partial(&i.template, f),
            Node::Macro(m) => {
                bind(&m.name, f);
                // A macro's arguments are its own; its body sees nothing else.
                let mut inner = Found::default();
                for a in &m.args.positional {
                    bind(a, &mut inner);
                }
                for p in &m.args.keyword {
                    bind(&p.key, &mut inner);
                    expr(&p.value, f);
                }
                nodes(&m.body, &mut inner);
                f.filters.extend(inner.filters);
                f.partials.extend(inner.partials);
                let bound = inner.bound;
                f.reads.extend(
                    inner
                        .reads
                        .into_iter()
                        .filter(|(n, _, _)| !bound.contains(n)),
                );
            }
            Node::Import(i) => {
                partial(&i.template, f);
                bind(&i.target, f);
            }
            Node::FromImport(i) => {
                partial(&i.template, f);
                for n in &i.names {
                    bind(n.alias.as_ref().unwrap_or(&n.name), f);
                }
            }
            Node::Switch(s) => {
                expr(&s.expr, f);
                for c in &s.cases {
                    expr(&c.cond, f);
                    nodes(&c.body, f);
                }
                if let Some(d) = &s.default {
                    nodes(d, f);
                }
            }
        }
    }

    fn args(a: &Args, f: &mut Found) {
        for e in &a.positional {
            expr(e, f);
        }
        for p in &a.keyword {
            expr(&p.value, f);
        }
    }

    fn expr(e: &Expr, f: &mut Found) {
        match e {
            Expr::Literal { .. } | Expr::Missing { .. } => {}
            Expr::Symbol { span, name } => f.reads.push((name.clone(), span.line, span.column)),
            Expr::Group { items, .. } | Expr::Array { items, .. } => {
                for i in items {
                    expr(i, f);
                }
            }
            Expr::Dict { pairs, .. } => {
                for p in pairs {
                    expr(&p.value, f);
                }
            }
            Expr::Lookup { target, key, .. } => {
                expr(target, f);
                expr(key, f);
            }
            Expr::Call {
                callee, args: a, ..
            } => {
                expr(callee, f);
                args(a, f);
            }
            Expr::Filter {
                span,
                name,
                args: a,
            } => {
                f.filters.push((name.clone(), span.line, span.column));
                args(a, f);
            }
            Expr::InlineIf {
                cond, body, else_, ..
            } => {
                expr(cond, f);
                expr(body, f);
                if let Some(e) = else_ {
                    expr(e, f);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                expr(lhs, f);
                expr(rhs, f);
            }
            Expr::Unary { operand, .. } => expr(operand, f),
            Expr::Compare {
                expr: first, ops, ..
            } => {
                expr(first, f);
                for o in ops {
                    expr(&o.expr, f);
                }
            }
            Expr::Caller { args: a, body, .. } => {
                args(a, f);
                nodes(body, f);
            }
            Expr::Capture(c) => nodes(&c.body, f),
        }
    }
}
