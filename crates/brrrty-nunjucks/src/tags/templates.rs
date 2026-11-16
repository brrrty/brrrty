//! The tags that load other templates: `include`, `import`, `from`,
//! `extends`, and blocks and `super()`.
//!
//! As Nunjucks's `Environment.getTemplate` does it:
//!
//! - A name must be a string. One starting `./` or `../` is resolved
//!   against the path of the template that names it; any other is given to
//!   the [`Loader`](brrrty_js::Loader) as written. No extension is added.
//! - A missing template is `Error: template not found: <name>`, unless an
//!   include says `ignore missing`.
//! - An include renders with a copy of the context's variables, in a frame
//!   pushed on the current one that writes stay in. An import runs the
//!   template for its exports: `with context`, on a copy of the variables
//!   and in a frame whose writes can reach the importer's frames; without,
//!   on nothing.
//! - `extends` adds the parent's blocks after the template's own. The
//!   template keeps running (its output is dropped), then the parent's
//!   root runs with the same context and frame.

use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::LoadError;

use crate::compile::{E, Program};
use crate::context::{BlockRef, Context, Frame, Scope, TplEnv};
use crate::error::ErrorKind;
use crate::jsops::{self, Hint};
use crate::render::{R, RenderErr, Renderer, error};
use crate::value::{Obj, Val};

impl Renderer<'_> {
    /// `env.getTemplate(name, eagerCompile, parentName, ignoreMissing)`.
    /// `None` when the template is missing and that is allowed.
    pub(crate) fn get_template(
        &mut self,
        name: &Val,
        parent: Option<&str>,
        ignore_missing: bool,
    ) -> R<Option<Arc<Program>>> {
        let Some(name) = name.as_str() else {
            let shown =
                jsops::prim_to_string(&jsops::to_primitive(name, Hint::Default, self.ops())?);
            return Err(error(format!("template names must be a string: {shown}")));
        };
        let resolved = match parent {
            Some(p) if name.starts_with("./") || name.starts_with("../") => resolve(p, name),
            _ => name.to_string(),
        };
        if let Some(p) = self.cache.get(&resolved) {
            return Ok(Some(p.clone()));
        }
        let (path, source) = match self.loader.load(&resolved) {
            Ok(found) => found,
            Err(LoadError { io: None, tried }) => {
                if ignore_missing {
                    return Ok(None);
                }
                let err = error(format!("template not found: {resolved}"));
                return Err(if tried.is_empty() {
                    err
                } else {
                    err.with_help(format!("tried: {}", tried.join(", ")))
                });
            }
            Err(LoadError { io: Some(io), .. }) => {
                return Err(RenderErr::new(
                    ErrorKind::Render,
                    format!("cannot read template \"{resolved}\": {io}"),
                ));
            }
        };
        let program = self
            .engine
            .compile_program(&resolved, Some(&path), &source)
            .map_err(|e| RenderErr::Located(Box::new(e)))?;
        self.cache.insert(resolved, program.clone());
        Ok(Some(program))
    }

    pub(crate) fn include(
        &mut self,
        template: &E,
        ignore_missing: bool,
        scope: &Rc<Scope>,
        out: &mut String,
    ) -> R<()> {
        let name = self.eval(template, scope)?;
        let tpl = &scope.tpl;
        let Some(program) =
            self.get_template(&name, tpl.program.path.as_deref(), ignore_missing)?
        else {
            return Ok(());
        };
        let vars = tpl.context.vars.borrow().clone();
        let s = self.render_program(&program, vars, Some(tpl.frame()))?;
        self.write(out, &s)
    }

    pub(crate) fn extends(&mut self, template: &E, scope: &Rc<Scope>) -> R<()> {
        let name = self.eval(template, scope)?;
        let tpl = &scope.tpl;
        let Some(parent) = self.get_template(&name, tpl.program.path.as_deref(), false)? else {
            return Ok(());
        };
        for (index, b) in parent.blocks.iter().enumerate() {
            tpl.context.add_block(
                &b.name,
                BlockRef {
                    program: parent.clone(),
                    index,
                },
            );
        }
        *tpl.parent_template.borrow_mut() = Some(parent);
        Ok(())
    }

    /// Loads a template and runs it for its exports.
    pub(crate) fn import(
        &mut self,
        template: &E,
        with_context: bool,
        scope: &Rc<Scope>,
    ) -> R<Rc<Obj>> {
        let name = self.eval(template, scope)?;
        let tpl = &scope.tpl;
        let Some(program) = self.get_template(&name, tpl.program.path.as_deref(), false)? else {
            return Ok(Rc::new(Obj::default()));
        };
        if with_context {
            let vars = tpl.context.vars.borrow().clone();
            self.exported(&program, Some(vars), Some(tpl.frame()))
        } else {
            self.exported(&program, None, None)
        }
    }

    /// Runs a block function: `b_name(env, context, frame, runtime, cb)`.
    pub(crate) fn invoke_block(
        &mut self,
        block: &BlockRef,
        context: &Rc<Context>,
        frame: Rc<Frame>,
    ) -> R<String> {
        let def = &block.program.blocks[block.index];
        let tpl = Rc::new(TplEnv {
            program: block.program.clone(),
            context: context.clone(),
            frame: std::cell::RefCell::new(frame.push(true)),
            parent_template: std::cell::RefCell::new(None),
        });
        let scope = Scope::new(def.body.slots, None, tpl.clone());
        self.enter_template(&block.program.name)?;
        let r = (|| {
            if let Some(slot) = def.super_slot {
                let list = context
                    .blocks
                    .borrow()
                    .get(&def.name)
                    .cloned()
                    .unwrap_or_default();
                let next = list
                    .iter()
                    .position(|b| b.is(block))
                    .and_then(|i| list.get(i + 1).cloned());
                let Some(next) = next else {
                    return Err(error(format!(
                        "no super block available for \"{}\"",
                        def.name
                    )));
                };
                let s = self.invoke_block(&next, context, tpl.frame())?;
                scope.put(0, slot, Val::safe(s));
            }
            self.capture(&def.body.stmts, &scope)
        })();
        self.templates -= 1;
        r
    }
}

/// `path.resolve(path.dirname(from), to)`, lexically. An absolute `from`
/// gives an absolute path; a relative one stays relative.
fn resolve(from: &str, to: &str) -> String {
    let absolute = from.starts_with('/');
    let mut parts: Vec<&str> = from
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    parts.pop();
    for p in to.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn resolve_is_path_resolve() {
        assert_eq!(resolve("/a/b/c.njk", "./d.njk"), "/a/b/d.njk");
        assert_eq!(resolve("/a/b/c.njk", "../d.njk"), "/a/d.njk");
        assert_eq!(resolve("/a/b/c.njk", "../../../../d.njk"), "/d.njk");
        assert_eq!(resolve("./cases/t.njk", "./x.njk"), "cases/x.njk");
    }
}
