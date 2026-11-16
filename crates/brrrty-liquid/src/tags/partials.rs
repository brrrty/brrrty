//! `include` and `render`.
//!
//! LiquidJS's behavior, as 11ty runs it:
//!
//! - **Names.** A quoted name is the file name. An unquoted name is a value
//!   (`{% include name %}` reads the variable `name`), and it must hold a
//!   string. `.liquid` is added when the name has no extension, by Node's
//!   `path.extname` rule. The engine does that and the [`Loader`] gets the
//!   final file name, so a loader only has to try its roots in order.
//! - **`include`** renders the partial in the caller's context. Its
//!   arguments are a new innermost scope, so they shadow variables and
//!   vanish afterwards, but `assign` in the partial reaches the caller.
//!   `with value` sets a variable named after the file as written.
//! - **`render`** renders the partial in a new context: no caller
//!   variables, no data, fresh `cycle` and `increment` state. Arguments,
//!   `with value as alias` and `for list as alias` are all it sees.
//!   `assign` in the partial stays inside it.
//!
//! Brrrty additions: unquoted names that are not strings say how to fix
//! them, a missing file lists where it looked, and partials nested more
//! than [`MAX_DEPTH`] levels deep are an error that names the cycle.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use brrrty_js::Value;

use crate::ast::{FilePath, Include, Render, Template};
use crate::context::{Context, LoopDrop, Val};
use crate::error::ErrorKind;
use crate::ops;
use crate::render::{MAX_ITEMS, R, RenderErr, Renderer, too_many};

/// The most partials that may be open at once.
const MAX_DEPTH: usize = 64;

/// The data a `render` partial sees: none.
fn empty_data() -> &'static Value {
    static EMPTY: OnceLock<Value> = OnceLock::new();
    EMPTY.get_or_init(|| Value::Object(Default::default()))
}

/// The file name, as written: a quoted name, a quoted name with Liquid in
/// it, or the string an unquoted name holds.
fn file_name(r: &mut Renderer<'_>, file: &FilePath, tag: &str) -> R<String> {
    let value = match file {
        FilePath::Name(name) => return non_empty(name.clone()),
        FilePath::Template(nodes) => {
            let mut name = String::new();
            r.render_nodes(nodes, &mut name)?;
            return non_empty(name);
        }
        FilePath::None => Val::undefined(),
        FilePath::Expr(e) => r.eval(e)?,
    };
    match &value {
        Val::V(Value::String(s)) if !s.is_empty() => Ok(s.to_string()),
        _ => {
            let err = illegal(&value.to_value().to_js_string());
            Err(match file {
                FilePath::Expr(_) => err.with_help(format!(
                    "{tag} names must be quoted: {{% {tag} \"name\" %}}"
                )),
                _ => err,
            })
        }
    }
}

fn non_empty(name: String) -> R<String> {
    if name.is_empty() {
        Err(illegal(""))
    } else {
        Ok(name)
    }
}

fn illegal(shown: &str) -> RenderErr {
    RenderErr::new(ErrorKind::Render, format!("illegal file path \"{shown}\""))
}

/// Adds `.liquid` to a name with no extension, as `path.extname` decides:
/// the last `.` in the last path part, unless it starts the part.
fn with_extension(name: &str) -> String {
    let trimmed = name.trim_end_matches('/');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let has_ext = base != ".." && base.rfind('.').is_some_and(|i| i > 0);
    if has_ext {
        name.to_string()
    } else {
        format!("{name}.liquid")
    }
}

/// Loads and parses a partial. Returns the path the loader found and the
/// parsed template, which is kept for the next use.
fn load(r: &mut Renderer<'_>, name: &str, tag: &str) -> R<(String, Rc<Template>)> {
    let file = with_extension(name);
    let Some((path, source)) = r.loader.load(&file) else {
        let mut help = Vec::new();
        let tried = r.loader.search_paths(&file);
        if !tried.is_empty() {
            help.push(format!("tried: {}", tried.join(", ")));
        }
        if file != name {
            help.push(format!(
                "\".liquid\" was added because \"{name}\" has no extension; write the whole file name to {tag} another kind of file"
            ));
        }
        let err = RenderErr::new(
            ErrorKind::Render,
            format!("{tag} file not found: \"{file}\""),
        );
        return Err(if help.is_empty() {
            err
        } else {
            err.with_help(help.join("; "))
        });
    };
    if let Some(t) = r.partials.get(&path) {
        return Ok((path, t.clone()));
    }
    let t = Rc::new(
        r.engine
            .parse(&path, &source)
            .map_err(|e| RenderErr::Located(Box::new(e)))?,
    );
    r.partials.insert(path.clone(), t.clone());
    Ok((path, t))
}

/// Renders a loaded partial into `out` in the current context.
fn run(
    r: &mut Renderer<'_>,
    name: &str,
    path: String,
    t: &Template,
    tag: &str,
    out: &mut String,
) -> R<()> {
    if r.stack.len() >= MAX_DEPTH {
        return Err(too_deep(r, name, &path, tag));
    }
    r.stack.push((path, name.to_string()));
    let saved = std::mem::replace(&mut r.template, Rc::from(t.name.as_str()));
    let result = r.render_nodes(&t.nodes, out);
    r.template = saved;
    r.stack.pop();
    result
}

/// The error for a partial opened at the depth limit: a cycle if the
/// partial is already open, else just too deep.
fn too_deep(r: &Renderer<'_>, name: &str, path: &str, tag: &str) -> RenderErr {
    match r.stack.iter().rposition(|(p, _)| p == path) {
        Some(at) => {
            let mut chain: Vec<&str> = r.stack[at..].iter().map(|(_, n)| n.as_str()).collect();
            chain.push(name);
            RenderErr::new(
                ErrorKind::Render,
                format!("include cycle: {}", chain.join(" -> ")),
            )
            .with_help(format!(
                "a partial that {tag}s itself, directly or through others, needs a condition that stops it before {MAX_DEPTH} levels"
            ))
        }
        None => RenderErr::new(
            ErrorKind::Render,
            format!("partials are nested more than {MAX_DEPTH} levels deep"),
        )
        .with_help(format!("the innermost is \"{name}\"")),
    }
}

pub(super) fn include(r: &mut Renderer<'_>, inc: &Include, out: &mut String) -> R<()> {
    let name = file_name(r, &inc.file, "include")?;
    let mut scope: HashMap<String, Val> = r.eval_hash(&inc.args)?.into_iter().collect();
    if let Some(with) = &inc.with {
        let v = r.eval(with)?;
        scope.insert(name.clone(), v);
    }
    let (path, t) = load(r, &name, "include")?;
    r.ctx.scopes.push(scope);
    let result = run(r, &name, path, &t, "include", out);
    r.ctx.scopes.pop();
    result
}

pub(super) fn render(r: &mut Renderer<'_>, rn: &Render, out: &mut String) -> R<()> {
    let name = file_name(r, &rn.file, "render")?;
    let mut scope: HashMap<String, Val> = r.eval_hash(&rn.args)?.into_iter().collect();
    if let Some(with) = &rn.with {
        let v = r.eval(&with.value)?;
        let key = match with.alias.as_deref() {
            Some(a) if !a.is_empty() => a.to_string(),
            _ => name.clone(),
        };
        scope.insert(key, v);
    }
    // The loop's items and name are worked out in the caller's context.
    let looped = match &rn.for_binding {
        Some(b) => {
            let v = r.eval(&b.value)?;
            let len = ops::enumerable_len(&v);
            if len > MAX_ITEMS {
                return Err(too_many(format!(
                    "the render loop has {len} items, more than the limit of {MAX_ITEMS}"
                )));
            }
            let alias = b.alias.clone();
            let shown = alias.clone().unwrap_or_else(|| "undefined".to_string());
            Some((
                ops::to_enumerable(&v),
                alias.unwrap_or_else(|| "undefined".to_string()),
                format!("{shown}-{}", b.text),
            ))
        }
        None => None,
    };

    let mut child = Context::new(empty_data());
    child.scopes[0] = scope;
    child.written = r.ctx.written;
    let parent = std::mem::replace(&mut r.ctx, child);
    let result = render_in_child(r, &name, looped, out);
    let child = std::mem::replace(&mut r.ctx, parent);
    r.ctx.written = child.written;
    result
}

fn render_in_child(
    r: &mut Renderer<'_>,
    name: &str,
    looped: Option<(Vec<Value>, String, String)>,
    out: &mut String,
) -> R<()> {
    match looped {
        Some((items, variable, loop_name)) => {
            let drop = Rc::new(LoopDrop::new(items.len(), loop_name, None));
            r.ctx.scopes[0].insert("forloop".to_string(), Val::Loop(drop.clone()));
            for item in items {
                r.ctx.scopes[0].insert(variable.clone(), Val::V(item));
                let (path, t) = load(r, name, "render")?;
                run(r, name, path, &t, "render", out)?;
                drop.next();
            }
            Ok(())
        }
        None => {
            let (path, t) = load(r, name, "render")?;
            run(r, name, path, &t, "render", out)
        }
    }
}
