//! markdown-it renderer rules in Rhai: `md.renderer.rules.link_open = |tokens, idx, options,
//! env, renderer| ...`, on 11ty's own instance (`config.amend_library("md", |md| ...)`) and on
//! instances a script makes (`markdown_it(options)`).
//!
//! Rules run for inline tokens; see `brrrty_markdown::RenderRules`. What a rule sees is
//! markdown-it's: the token list of one inline block, read-only as a list, whose tokens can be
//! read and changed; the options, read-only; `env`; and the renderer, for `render_token` and
//! `render_attrs`. What Brrrty can't give exactly is an error, never a guess.

use std::sync::{Arc, Mutex, MutexGuard};

use brrrty_markdown::{
    ConfiguredMarkdownIt, DefaultMarkdownIt, Options, RenderError, RenderOptions, RenderRules,
    Token,
};
use indexmap::IndexMap;
use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, INT, ImmutableString, NativeCallContext};

use crate::api::invalid;

type Res<T> = Result<T, Box<EvalAltResult>>;

fn err<T>(message: String) -> Res<T> {
    Err(invalid(message, rhai::Position::NONE))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which markdown-it a library is.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// 11ty's own (`amendLibrary("md", ...)`); it renders with the page's data as `env`.
    Eleventy,
    /// `markdownIt()`, the default preset.
    Default,
    /// `markdownIt({ html, linkify, typographer })`.
    Configured(Options),
}

#[derive(Debug)]
struct LibState {
    kind: Kind,
    rules: IndexMap<String, FnPtr>,
}

/// A markdown-it instance as a script sees it (`md`).
#[derive(Debug, Clone)]
pub struct MdLib(Arc<Mutex<LibState>>);

/// `md.renderer`.
#[derive(Debug, Clone)]
pub struct MdRenderer(Arc<Mutex<LibState>>);

/// `md.renderer.rules`.
#[derive(Debug, Clone)]
pub struct MdRules(Arc<Mutex<LibState>>);

impl MdLib {
    fn new(kind: Kind) -> MdLib {
        MdLib(Arc::new(Mutex::new(LibState {
            kind,
            rules: IndexMap::new(),
        })))
    }

    /// 11ty's own library, for `config.amend_library("md", ...)`.
    pub fn eleventy() -> MdLib {
        MdLib::new(Kind::Eleventy)
    }

    /// The rules set, by token type.
    pub fn rules(&self) -> IndexMap<String, FnPtr> {
        lock(&self.0).rules.clone()
    }
}

thread_local! {
    /// How many scripts are running their top level on this thread (the config, a data file):
    /// the only time renderer rules can be set.
    static LOADING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// How many renderer rules are running on this thread.
    static IN_RULE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Adds one to a counter until dropped, panics included.
struct Count(&'static std::thread::LocalKey<std::cell::Cell<u32>>);

impl Count {
    fn new(key: &'static std::thread::LocalKey<std::cell::Cell<u32>>) -> Count {
        key.with(|c| c.set(c.get() + 1));
        Count(key)
    }
}

impl Drop for Count {
    fn drop(&mut self) {
        self.0.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// Runs a script's top level (the config, or a data file), while renderer rules can be set.
pub fn loading<T>(f: impl FnOnce() -> T) -> T {
    let _loading = Count::new(&LOADING);
    f()
}

/// Libraries are frozen outside a script's top level, and inside a rule: a filter, a
/// collection or a rule that changed one would make pages depend on the order they render in,
/// which 11ty's single thread never does.
fn check_unfrozen(kind: &str) -> Res<()> {
    let loading = LOADING.with(|c| c.get()) > 0;
    let in_rule = IN_RULE.with(|c| c.get()) > 0;
    if loading && !in_rule {
        return Ok(());
    }
    err(format!(
        "md.renderer.rules.{kind}: renderer rules can't be changed {}; set them while the config or the data file loads",
        if in_rule {
            "from inside a renderer rule"
        } else {
            "while pages render (in a filter, a collection or a rule)"
        }
    ))
}

fn block_rule_error<T>(kind: &str) -> Res<T> {
    err(format!(
        "md.renderer.rules.{kind}: Brrrty runs renderer rules for inline tokens only (text, link_open, link_close, image, code_inline, softbreak, hardbreak, html_inline, em_open, em_close, strong_open, strong_close, s_open, s_close); `{kind}` is a block token, which it can't run a rule for yet"
    ))
}

/// The name of the native function a default rule's pointer calls.
const DEFAULT_RULE_FN: &str = "markdown_it_default_rule";

/// `md.renderer.rules[kind]`: the rule set, markdown-it's own default rule, or `()`.
fn get_rule(state: &LibState, kind: &str) -> Res<Dynamic> {
    if brrrty_markdown::is_block_type(kind) {
        return block_rule_error(kind);
    }
    if let Some(f) = state.rules.get(kind) {
        return Ok(Dynamic::from(f.clone()));
    }
    if brrrty_markdown::has_default_rule(kind) {
        let mut f = FnPtr::new(DEFAULT_RULE_FN)?;
        f.add_curry(Dynamic::from(ImmutableString::from(kind)));
        return Ok(Dynamic::from(f));
    }
    Ok(Dynamic::UNIT)
}

fn set_rule(state: &mut LibState, kind: &str, rule: Dynamic) -> Res<()> {
    if brrrty_markdown::is_block_type(kind) {
        return block_rule_error(kind);
    }
    check_unfrozen(kind)?;
    if rule.is_unit() {
        state.rules.shift_remove(kind);
        return Ok(());
    }
    let Some(f) = rule.clone().try_cast::<FnPtr>() else {
        return err(format!(
            "md.renderer.rules.{kind} must be a function, not {}",
            rule.type_name()
        ));
    };
    // A default rule's own pointer, set back, is the default again.
    if f.fn_name() == DEFAULT_RULE_FN
        && f.curry()
            .first()
            .and_then(|c| c.clone().into_immutable_string().ok())
            .as_deref()
            == Some(kind)
    {
        state.rules.shift_remove(kind);
    } else {
        state.rules.insert(kind.to_string(), f);
    }
    Ok(())
}

/// A token of the list a rule is given, shared, so a change is seen by the renderer.
#[derive(Debug, Clone)]
pub struct TokenRef(Arc<Mutex<Token>>);

/// The token list a rule is given (`tokens`).
#[derive(Debug, Clone)]
pub struct Tokens(Arc<Vec<TokenRef>>);

/// `options`, read-only.
#[derive(Debug, Clone)]
pub struct MdOptions(RenderOptions);

/// `env`. In 11ty it is the page's data, which rules can't read yet; a script's own instance
/// renders with `{}`.
#[derive(Debug, Clone)]
pub struct MdEnv {
    eleventy: bool,
}

/// The renderer a rule is given (`self` in JavaScript, a reserved word in Rhai).
#[derive(Debug, Clone)]
pub struct RuleRenderer;

fn str_arg(v: &Dynamic, what: &str) -> Res<String> {
    match v.clone().into_immutable_string() {
        Ok(s) => Ok(s.to_string()),
        Err(t) => err(format!(
            "{what} must be a string, not {t} (JavaScript would turn it into one; Brrrty asks for a string)"
        )),
    }
}

fn attrs_value(t: &Token) -> Dynamic {
    match &t.attrs {
        None => Dynamic::UNIT,
        Some(a) => Dynamic::from_array(
            a.iter()
                .map(|(n, v)| Dynamic::from_array(vec![n.clone().into(), v.clone().into()]))
                .collect(),
        ),
    }
}

fn attr_pair(v: &Dynamic) -> Res<(String, String)> {
    let Some(pair) = v.clone().try_cast::<Array>() else {
        return err(format!(
            "an attribute must be a [name, value] array, not {}",
            v.type_name()
        ));
    };
    if pair.len() != 2 {
        return err(format!(
            "an attribute must be a [name, value] array, not one of {} items",
            pair.len()
        ));
    }
    Ok((
        str_arg(&pair[0], "an attribute's name")?,
        str_arg(&pair[1], "an attribute's value")?,
    ))
}

fn set_attrs(t: &mut Token, v: Dynamic) -> Res<()> {
    if v.is_unit() {
        t.attrs = None;
        return Ok(());
    }
    let Some(list) = v.clone().try_cast::<Array>() else {
        return err(format!(
            "token.attrs must be an array or (), not {}",
            v.type_name()
        ));
    };
    t.attrs = Some(list.iter().map(attr_pair).collect::<Res<_>>()?);
    Ok(())
}

fn index(tokens: &Tokens, idx: INT) -> Option<TokenRef> {
    usize::try_from(idx)
        .ok()
        .and_then(|i| tokens.0.get(i).cloned())
}

/// `tokens[idx]`, which must be there.
fn token_at(tokens: &Tokens, idx: INT, what: &str) -> Res<TokenRef> {
    index(tokens, idx).map_or_else(
        || {
            err(format!(
                "{what}: there is no token {idx} (the list has {})",
                tokens.0.len()
            ))
        },
        Ok,
    )
}

fn snapshot(tokens: &Tokens) -> Vec<Token> {
    tokens.0.iter().map(|t| lock(&t.0).clone()).collect()
}

/// `renderToken(tokens, idx, options)`.
fn render_token(tokens: &Tokens, idx: INT, options: &RenderOptions) -> Res<String> {
    token_at(tokens, idx, "render_token")?;
    let i = usize::try_from(idx).unwrap_or(0);
    Ok(brrrty_markdown::render_token(
        &snapshot(tokens),
        i,
        options.xhtml_out,
        options.v15,
    ))
}

/// markdown-it's default rule for `tokens[idx]` (`renderToken` for a type without one).
fn render_default(tokens: &Tokens, idx: usize, options: &RenderOptions) -> Result<String, String> {
    let mut all = snapshot(tokens);
    let out = brrrty_markdown::render_default(&mut all, idx, options)?;
    // The image rule sets the token's `alt`.
    *lock(&tokens.0[idx].0) = all.swap_remove(idx);
    Ok(out)
}

fn options_of(v: &Dynamic) -> Res<RenderOptions> {
    match v.clone().try_cast::<MdOptions>() {
        Some(o) => Ok(o.0),
        None => err(format!(
            "pass a rule's own `options` on to the renderer, not {}",
            v.type_name()
        )),
    }
}

/// Runs a rule's function: within a script's call, or later with the config's engine.
pub type CallRule<'a> = dyn Fn(&FnPtr, Vec<Dynamic>) -> Res<Dynamic> + 'a;

/// `renderInline(tokens, options, env)` with `rules`.
fn render_inline(
    rules: &IndexMap<String, FnPtr>,
    tokens: Vec<Token>,
    options: &RenderOptions,
    eleventy: bool,
    call: &CallRule<'_>,
    describe: &dyn Fn(&str, EvalAltResult) -> String,
) -> Result<String, String> {
    let tokens = Tokens(Arc::new(
        tokens
            .into_iter()
            .map(|t| TokenRef(Arc::new(Mutex::new(t))))
            .collect(),
    ));
    let _in_rule = Count::new(&IN_RULE);
    let mut out = String::new();
    for i in 0..tokens.0.len() {
        let kind = lock(&tokens.0[i].0).kind.clone();
        let Some(rule) = rules.get(&kind) else {
            out.push_str(&render_default(&tokens, i, options)?);
            continue;
        };
        let args = vec![
            Dynamic::from(tokens.clone()),
            Dynamic::from(INT::try_from(i).unwrap_or(INT::MAX)),
            Dynamic::from(MdOptions(*options)),
            Dynamic::from(MdEnv { eleventy }),
            Dynamic::from(RuleRenderer),
        ];
        let result = call(rule, args).map_err(|e| describe(&kind, *e))?;
        match result.into_immutable_string() {
            Ok(s) => out.push_str(&s),
            Err(t) => {
                return Err(format!(
                    "renderer rule `{kind}` returned {t}, not a string (markdown-it would write it as text)"
                ));
            }
        }
    }
    Ok(out)
}

/// Rules run within a script's own call (`md.render(s)` in a data file or a filter).
struct InScript<'a> {
    rules: IndexMap<String, FnPtr>,
    ctx: &'a NativeCallContext<'a>,
}

impl RenderRules for InScript<'_> {
    fn render_inline(&self, tokens: Vec<Token>, options: &RenderOptions) -> Result<String, String> {
        let call = |f: &FnPtr, args: Vec<Dynamic>| f.call_within_context::<Dynamic>(self.ctx, args);
        let describe = |kind: &str, e: EvalAltResult| {
            let parts = crate::error_parts(e);
            format!("renderer rule `{kind}` failed: {}", parts.message)
        };
        render_inline(&self.rules, tokens, options, false, &call, &describe)
    }
}

/// 11ty's library's rules, run while pages render with the config's engine.
pub struct ConfigRules {
    pub runtime: Arc<crate::filters::Runtime>,
    pub rules: IndexMap<String, FnPtr>,
}

impl RenderRules for ConfigRules {
    fn render_inline(&self, tokens: Vec<Token>, options: &RenderOptions) -> Result<String, String> {
        crate::api::start_clock();
        let rt = &self.runtime;
        let call = |f: &FnPtr, args: Vec<Dynamic>| {
            stacker::maybe_grow(
                crate::filters::STACK_RED_ZONE,
                crate::filters::CALL_STACK,
                || f.call::<Dynamic>(&rt.engine, &rt.ast, args),
            )
        };
        let describe = |kind: &str, e: EvalAltResult| {
            let parts = crate::error_parts(e);
            let mut text = format!("renderer rule `{kind}` failed: {}", parts.message);
            if let (Some(line), col) = (parts.position.line(), parts.position.position()) {
                text.push_str(&format!(
                    " (at {}:{line}:{})",
                    crate::CONFIG_FILE,
                    col.unwrap_or(1)
                ));
            }
            text
        };
        let run = || render_inline(&self.rules, tokens, options, true, &call, &describe);
        match brrrty_core::contain::catch(run) {
            Ok(r) => r,
            Err(why) => Err(format!("a renderer rule failed: {why}")),
        }
    }
}

fn instance(
    kind: Kind,
) -> Option<Result<&'static ConfiguredMarkdownIt, &'static DefaultMarkdownIt>> {
    use std::sync::OnceLock;
    static DEFAULT: OnceLock<DefaultMarkdownIt> = OnceLock::new();
    static MDS: [OnceLock<ConfiguredMarkdownIt>; 16] = [const { OnceLock::new() }; 16];
    match kind {
        Kind::Eleventy => None,
        Kind::Default => Some(Err(DEFAULT.get_or_init(DefaultMarkdownIt::new))),
        Kind::Configured(o) => {
            let i = usize::from(o.html)
                | usize::from(o.linkify) << 1
                | usize::from(o.typographer) << 2
                | usize::from(o.fuzzy_link) << 3;
            Some(Ok(MDS[i].get_or_init(|| ConfiguredMarkdownIt::new(o))))
        }
    }
}

fn md_render(ctx: NativeCallContext, md: &mut MdLib, s: Dynamic) -> Res<String> {
    let (kind, rules) = {
        let st = lock(&md.0);
        (st.kind, st.rules.clone())
    };
    let Some(inst) = instance(kind) else {
        return err(
            "md.render: 11ty's own library renders the site's Markdown pages; Brrrty can't call it from a script"
                .into(),
        );
    };
    // markdown-it's `render` throws on anything but a string (unlike the filters' `s || ''`).
    let Some(s) = s.clone().into_immutable_string().ok() else {
        return err(format!(
            "md.render: Input data should be a String, not {} (markdown-it throws)",
            s.type_name()
        ));
    };
    let rules = InScript { rules, ctx: &ctx };
    let result = match inst {
        Ok(c) => c.render_with_rules(&s, &rules),
        Err(d) => d.render_with_rules(&s, &rules),
    };
    result.map_err(|e| match e {
        RenderError::Refused(why) => invalid(format!("md.render: {why}"), rhai::Position::NONE),
        RenderError::Rule(why) => invalid(format!("md.render: {why}"), rhai::Position::NONE),
    })
}

/// Registers `markdown_it`, `md.renderer.rules`, tokens and the rest.
pub fn register(e: &mut Engine) {
    e.register_type_with_name::<MdLib>("MarkdownIt")
        .register_type_with_name::<MdRenderer>("Renderer")
        .register_type_with_name::<MdRules>("RendererRules")
        .register_type_with_name::<TokenRef>("Token")
        .register_type_with_name::<Tokens>("Tokens")
        .register_type_with_name::<MdOptions>("MarkdownItOptions")
        .register_type_with_name::<MdEnv>("MarkdownItEnv")
        .register_type_with_name::<RuleRenderer>("RuleRenderer");

    e.register_fn("markdown_it", || MdLib::new(Kind::Default));
    e.register_fn("markdown_it", |options: Dynamic| -> Res<MdLib> {
        Ok(MdLib::new(Kind::Configured(
            crate::builtins::markdown_options(&options)?,
        )))
    });
    e.register_fn("render", md_render);
    e.register_get("renderer", |md: &mut MdLib| MdRenderer(md.0.clone()));
    e.register_get("rules", |r: &mut MdRenderer| MdRules(r.0.clone()));
    e.register_indexer_get(|r: &mut MdRules, kind: ImmutableString| -> Res<Dynamic> {
        get_rule(&lock(&r.0), &kind)
    });
    e.register_indexer_set(
        |r: &mut MdRules, kind: ImmutableString, rule: Dynamic| -> Res<()> {
            set_rule(&mut lock(&r.0), &kind, rule)
        },
    );

    // `tokens`: read-only as a list.
    e.register_indexer_get(|t: &mut Tokens, idx: INT| -> Dynamic {
        index(t, idx).map_or(Dynamic::UNIT, Dynamic::from)
    });
    e.register_get("length", |t: &mut Tokens| {
        INT::try_from(t.0.len()).unwrap_or(INT::MAX)
    });
    e.register_get("len", |t: &mut Tokens| {
        INT::try_from(t.0.len()).unwrap_or(INT::MAX)
    });
    e.register_fn("len", |t: &mut Tokens| {
        INT::try_from(t.0.len()).unwrap_or(INT::MAX)
    });

    // A token's fields.
    macro_rules! string_field {
        ($name:literal, $field:ident) => {
            e.register_get($name, |t: &mut TokenRef| -> ImmutableString {
                lock(&t.0).$field.clone().into()
            });
            e.register_set($name, |t: &mut TokenRef, v: Dynamic| -> Res<()> {
                lock(&t.0).$field = str_arg(&v, concat!("token.", $name))?;
                Ok(())
            });
        };
    }
    e.register_get("type", |t: &mut TokenRef| -> ImmutableString {
        lock(&t.0).kind.clone().into()
    });
    e.register_set("type", |t: &mut TokenRef, v: Dynamic| -> Res<()> {
        let kind = str_arg(&v, "token.type")?;
        // markdown-it renders a block type with rules Brrrty doesn't run for inline tokens
        // (`fence`, `code_block`, `html_block`) or with block newlines; the rest are checked
        // against markdown-it.
        if brrrty_markdown::is_block_type(&kind) {
            return err(format!(
                "token.type = \"{kind}\": Brrrty renders inline tokens only, and `{kind}` is a block token type"
            ));
        }
        lock(&t.0).kind = kind;
        Ok(())
    });
    string_field!("tag", tag);
    string_field!("content", content);
    string_field!("markup", markup);
    string_field!("info", info);
    macro_rules! int_field {
        ($name:literal, $field:ident) => {
            e.register_get($name, |t: &mut TokenRef| -> INT { lock(&t.0).$field });
            e.register_set($name, |t: &mut TokenRef, v: INT| lock(&t.0).$field = v);
        };
    }
    int_field!("nesting", nesting);
    int_field!("level", level);
    macro_rules! bool_field {
        ($name:literal, $field:ident) => {
            e.register_get($name, |t: &mut TokenRef| -> bool { lock(&t.0).$field });
            e.register_set($name, |t: &mut TokenRef, v: bool| lock(&t.0).$field = v);
        };
    }
    bool_field!("block", block);
    bool_field!("hidden", hidden);
    e.register_get("attrs", |t: &mut TokenRef| attrs_value(&lock(&t.0)));
    e.register_set("attrs", |t: &mut TokenRef, v: Dynamic| -> Res<()> {
        set_attrs(&mut lock(&t.0), v)
    });
    e.register_get("children", |t: &mut TokenRef| -> Res<Dynamic> {
        if lock(&t.0).kind == "image" {
            err("token.children: Brrrty doesn't give renderer rules an image's children yet".into())
        } else {
            Ok(Dynamic::UNIT)
        }
    });
    e.register_get("map", |_: &mut TokenRef| Dynamic::UNIT);
    e.register_get("meta", |_: &mut TokenRef| Dynamic::UNIT);
    for name in ["children", "map", "meta"] {
        e.register_set(name, move |_: &mut TokenRef, _: Dynamic| -> Res<()> {
            err(format!(
                "token.{name} can't be set by a renderer rule in Brrrty yet"
            ))
        });
    }
    e.register_fn(
        "attr_get",
        |t: &mut TokenRef, name: Dynamic| -> Res<Dynamic> {
            let name = str_arg(&name, "attr_get's name")?;
            Ok(lock(&t.0)
                .attr_get(&name)
                .map_or(Dynamic::UNIT, |v| v.to_string().into()))
        },
    );
    e.register_fn(
        "attr_index",
        |t: &mut TokenRef, name: Dynamic| -> Res<INT> {
            let name = str_arg(&name, "attr_index's name")?;
            Ok(lock(&t.0)
                .attr_index(&name)
                .map_or(-1, |i| INT::try_from(i).unwrap_or(INT::MAX)))
        },
    );
    e.register_fn(
        "attr_set",
        |t: &mut TokenRef, name: Dynamic, value: Dynamic| -> Res<()> {
            let name = str_arg(&name, "attr_set's name")?;
            let value = str_arg(&value, "attr_set's value")?;
            lock(&t.0).attr_set(name, value);
            Ok(())
        },
    );
    e.register_fn(
        "attr_join",
        |t: &mut TokenRef, name: Dynamic, value: Dynamic| -> Res<()> {
            let name = str_arg(&name, "attr_join's name")?;
            let value = str_arg(&value, "attr_join's value")?;
            lock(&t.0).attr_join(name, value);
            Ok(())
        },
    );
    e.register_fn("attr_push", |t: &mut TokenRef, pair: Dynamic| -> Res<()> {
        let (name, value) = attr_pair(&pair)?;
        lock(&t.0).attr_push(name, value);
        Ok(())
    });

    // `options`, read-only.
    e.register_indexer_get(|o: &mut MdOptions, key: ImmutableString| -> Res<Dynamic> {
        let o = o.0;
        Ok(match key.as_str() {
            "html" => o.html.into(),
            "xhtmlOut" => o.xhtml_out.into(),
            "breaks" => o.breaks.into(),
            "linkify" => o.linkify.into(),
            "typographer" => o.typographer.into(),
            "langPrefix" => brrrty_markdown::LANG_PREFIX.into(),
            "quotes" => brrrty_markdown::QUOTES.into(),
            _ => {
                return err(format!(
                    "options.{key}: renderer rules in Brrrty can read html, xhtmlOut, breaks, linkify, typographer, langPrefix and quotes"
                ));
            }
        })
    });
    e.register_indexer_set(
        |_: &mut MdOptions, key: ImmutableString, _: Dynamic| -> Res<()> {
            err(format!(
                "options.{key}: markdown-it's options are shared by every rule and render, so Brrrty doesn't let a rule change them"
            ))
        },
    );

    // `env`.
    e.register_indexer_get(|env: &mut MdEnv, key: ImmutableString| -> Res<Dynamic> {
        if env.eleventy {
            err(format!(
                "env.{key}: 11ty renders Markdown with the page's data as `env`, which Brrrty doesn't give renderer rules yet"
            ))
        } else {
            Ok(Dynamic::UNIT)
        }
    });
    e.register_indexer_set(
        |_: &mut MdEnv, key: ImmutableString, _: Dynamic| -> Res<()> {
            err(format!(
                "env.{key}: `env` is shared by every rule of a render, so Brrrty doesn't let a rule change it yet"
            ))
        },
    );

    // The renderer.
    e.register_fn(
        "render_token",
        |_: &mut RuleRenderer, tokens: Tokens, idx: INT, options: Dynamic| -> Res<String> {
            render_token(&tokens, idx, &options_of(&options)?)
        },
    );
    e.register_fn("render_attrs", |_: &mut RuleRenderer, token: TokenRef| {
        brrrty_markdown::render_attrs(&lock(&token.0))
    });
    e.register_fn("escape_html", |s: Dynamic| -> Res<String> {
        Ok(brrrty_markdown::escape_html(&str_arg(
            &s,
            "escape_html's argument",
        )?))
    });
    e.register_fn(
        DEFAULT_RULE_FN,
        |kind: ImmutableString,
         tokens: Tokens,
         idx: INT,
         options: Dynamic,
         _env: Dynamic,
         _renderer: Dynamic|
         -> Res<String> {
            let options = options_of(&options)?;
            let at = token_at(&tokens, idx, &format!("md.renderer.rules.{kind}"))?;
            let i = usize::try_from(idx).unwrap_or(0);
            // markdown-it's rule for `kind`, whatever the token's type is now.
            let saved = std::mem::replace(&mut lock(&at.0).kind, kind.to_string());
            let out = render_default(&tokens, i, &options);
            lock(&at.0).kind = saved;
            out.or_else(|why| err(format!("md.renderer.rules.{kind}: {why}")))
        },
    );
}
