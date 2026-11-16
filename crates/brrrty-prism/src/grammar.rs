//! Prism's grammars, as tools/gen-grammars.cjs dumped them from Prism 1.30.0
//! loaded the way @11ty/eleventy-plugin-syntaxhighlight 5.0.2 loads it.
//!
//! A grammar is an ordered list of tokens, each with its patterns, and an
//! optional `rest` grammar that Prism's `tokenize` merges into it on first
//! use. `inside` and `rest` are [`GrammarId`]s, so grammars that Prism
//! shares (or that refer to themselves) stay shared. Each pattern's regular
//! expression is a [`RegexId`] into one table, translated on first use.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::regex::{JsRegex, Untranslatable, translate};

const JSON: &str = include_str!("../grammars/prism.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GrammarId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegexId(pub usize);

/// A fence language on the test sites, and what the plugin made of it.
#[derive(Clone, Debug)]
pub struct Fence {
    /// As written in the info string.
    pub name: String,
    /// After the plugin's alias normalization (`shell` is `bash`).
    pub normalized: String,
    /// Whether Prism has a grammar for it; if not, the plugin writes the
    /// code plain.
    pub known: bool,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub regex: RegexId,
    /// Group 1 is a lookbehind: it is left out of the token.
    pub lookbehind: bool,
    pub greedy: bool,
    /// Extra classes for the token, in order.
    pub alias: Vec<String>,
    /// The grammar the token's text is tokenized with.
    pub inside: Option<GrammarId>,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub name: String,
    pub patterns: Vec<Pattern>,
}

#[derive(Clone, Debug)]
pub struct Grammar {
    /// In Prism's order.
    pub tokens: Vec<Token>,
    pub rest: Option<GrammarId>,
}

/// A regular expression's JavaScript source and flags, and its
/// translation, made on first use.
#[derive(Debug)]
pub struct RegexEntry {
    pub source: String,
    pub flags: String,
    compiled: OnceLock<Result<Arc<JsRegex>, Untranslatable>>,
}

#[derive(Debug)]
pub struct Grammars {
    pub prism: String,
    pub plugin: String,
    pub fences: Vec<Fence>,
    /// Every key of `Prism.languages` that is a grammar (aliases included),
    /// in Prism's order.
    pub languages: Vec<(String, GrammarId)>,
    /// Prism's components and their aliases (components.json order), for
    /// the plugin's alias normalization.
    pub components: Vec<(String, Vec<String>)>,
    /// Languages whose grammar depends on load order, with the patterns
    /// that differ.
    order_dependent: Vec<(String, Vec<String>)>,
    /// The hooks each fence language adds when loaded (hook name, count).
    pub hooks_by_language: Vec<(String, Vec<(String, u64)>)>,
    pub grammars: Vec<Grammar>,
    pub regexes: Vec<RegexEntry>,
}

/// The grammars, read on first use.
pub fn grammars() -> &'static Grammars {
    static G: OnceLock<Grammars> = OnceLock::new();
    G.get_or_init(|| Grammars::parse(JSON).expect("grammars/prism.json is valid"))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|xs| {
            xs.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn text(v: &Value, key: &str) -> Result<String, String> {
    v[key]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("`{key}` is missing"))
}

fn id(v: &Value) -> Result<Option<usize>, String> {
    match v {
        Value::Null => Ok(None),
        v => v
            .as_u64()
            .map(|x| Some(x as usize))
            .ok_or_else(|| format!("bad id {v}")),
    }
}

impl Grammars {
    pub fn parse(json: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let obj = |k: &str| v[k].as_object().ok_or_else(|| format!("`{k}` is missing"));
        let fences = v["fences"]
            .as_array()
            .ok_or("`fences` is missing")?
            .iter()
            .map(|f| {
                Ok(Fence {
                    name: text(f, "name")?,
                    normalized: text(f, "normalized")?,
                    known: f["known"].as_bool().unwrap_or(false),
                })
            })
            .collect::<Result<_, String>>()?;
        let languages = obj("languages")?
            .iter()
            .map(|(k, x)| Ok((k.clone(), GrammarId(id(x)?.ok_or("null language")?))))
            .collect::<Result<_, String>>()?;
        let components = v["components"]
            .as_array()
            .ok_or("`components` is missing")?
            .iter()
            .map(|c| (c[0].as_str().unwrap_or("").to_string(), strings(&c[1])))
            .collect();
        let order_dependent = obj("orderDependent")?
            .iter()
            .map(|(k, x)| (k.clone(), strings(x)))
            .collect();
        let hooks_by_language = obj("hooksByLanguage")?
            .iter()
            .map(|(k, x)| {
                let hooks = x
                    .as_object()
                    .map(|h| {
                        h.iter()
                            .map(|(n, c)| (n.clone(), c.as_u64().unwrap_or(0)))
                            .collect()
                    })
                    .unwrap_or_default();
                (k.clone(), hooks)
            })
            .collect();
        let mut regexes: Vec<RegexEntry> = Vec::new();
        let mut regex_ids: HashMap<(String, String), RegexId> = HashMap::new();
        let raw = v["grammars"].as_array().ok_or("`grammars` is missing")?;
        let mut grammars = Vec::with_capacity(raw.len());
        for g in raw {
            let mut tokens = Vec::new();
            for t in g["tokens"].as_array().ok_or("`tokens` is missing")? {
                let name = t[0].as_str().ok_or("token name")?.to_string();
                let mut patterns = Vec::new();
                for p in t[1].as_array().ok_or("token patterns")? {
                    let key = (text(p, "source")?, text(p, "flags")?);
                    let regex = *regex_ids.entry(key.clone()).or_insert_with(|| {
                        regexes.push(RegexEntry {
                            source: key.0,
                            flags: key.1,
                            compiled: OnceLock::new(),
                        });
                        RegexId(regexes.len() - 1)
                    });
                    patterns.push(Pattern {
                        regex,
                        lookbehind: p["lookbehind"].as_bool().unwrap_or(false),
                        greedy: p["greedy"].as_bool().unwrap_or(false),
                        alias: strings(&p["alias"]),
                        inside: id(&p["inside"])?.map(GrammarId),
                    });
                }
                tokens.push(Token { name, patterns });
            }
            grammars.push(Grammar {
                tokens,
                rest: id(&g["rest"])?.map(GrammarId),
            });
        }
        let out = Self {
            prism: text(&v, "prism")?,
            plugin: text(&v, "plugin")?,
            fences,
            languages,
            components,
            order_dependent,
            hooks_by_language,
            grammars,
            regexes,
        };
        let n = out.grammars.len();
        let in_range = |g: &Option<GrammarId>| g.is_none_or(|g| g.0 < n);
        let ok = out.languages.iter().all(|(_, g)| g.0 < n)
            && out.grammars.iter().all(|g| {
                in_range(&g.rest)
                    && g.tokens
                        .iter()
                        .all(|t| t.patterns.iter().all(|p| in_range(&p.inside)))
            });
        if !ok {
            return Err("a grammar id is out of range".into());
        }
        Ok(out)
    }

    /// The grammar `Prism.languages[name]` holds.
    pub fn language(&self, name: &str) -> Option<GrammarId> {
        self.languages
            .iter()
            .find(|(k, _)| k == name)
            .map(|&(_, g)| g)
    }

    pub fn grammar(&self, id: GrammarId) -> &Grammar {
        &self.grammars[id.0]
    }

    pub fn regex_entry(&self, id: RegexId) -> &RegexEntry {
        &self.regexes[id.0]
    }

    /// The translated regular expression, or why it can't be.
    pub fn regex(&self, id: RegexId) -> Result<&JsRegex, &Untranslatable> {
        let e = &self.regexes[id.0];
        e.compiled
            .get_or_init(|| translate(&e.source, &e.flags).map(Arc::new))
            .as_ref()
            .map(|r| r.as_ref())
    }

    /// The patterns of `lang` whose grammar differs with the order
    /// languages are loaded in (empty for most).
    pub fn order_dependent(&self, lang: &str) -> Vec<String> {
        self.order_dependent
            .iter()
            .find(|(k, _)| k == lang)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// Every grammar `id` can reach through `inside` and `rest`, itself
    /// first.
    pub fn reachable(&self, id: GrammarId) -> Vec<GrammarId> {
        let mut seen = HashSet::from([id]);
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            let g = self.grammar(out[i]);
            let next = g.rest.into_iter().chain(
                g.tokens
                    .iter()
                    .flat_map(|t| t.patterns.iter().filter_map(|p| p.inside)),
            );
            for n in next {
                if seen.insert(n) {
                    out.push(n);
                }
            }
            i += 1;
        }
        out
    }

    /// Every regular expression the grammar `id` can use, in order.
    pub fn regexes_of(&self, id: GrammarId) -> Vec<RegexId> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for g in self.reachable(id) {
            for t in &self.grammar(g).tokens {
                for p in &t.patterns {
                    if seen.insert(p.regex) {
                        out.push(p.regex);
                    }
                }
            }
        }
        out
    }

    /// Whether every regular expression the grammar `id` can use is
    /// translated; if not, each one that isn't, with why.
    pub fn support(&self, id: GrammarId) -> Result<(), Vec<(RegexId, String)>> {
        let bad: Vec<(RegexId, String)> = self
            .regexes_of(id)
            .into_iter()
            .filter_map(|r| self.regex(r).err().map(|e| (r, e.0.clone())))
            .collect();
        if bad.is_empty() { Ok(()) } else { Err(bad) }
    }
}
