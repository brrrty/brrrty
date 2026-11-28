//! 11ty's bundle plugin (`eleventyConfig.addBundle`), the inline part: `{% css %}...{% endcss %}`
//! (any bundle's name) collects code for the page being rendered, and `{% getBundle "css" %}`
//! prints what the page collected.
//!
//! 11ty prints a placeholder where `getBundle` is called and swaps in the code once the page
//! is rendered, so code collected after the call (a footer include, say) is in. [`Bundles::resolve`]
//! does the same swap. Entries are kept once each, in the order added, and joined with a
//! newline. Not supported (named refusals): `addBundle` options, `getBundleFileUrl`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brrrty_js::{FilterFn, Value};
use brrrty_nunjucks::Engine;

use super::rss::posthtml::{self, Element, Node, js_space};

const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

type Entries = HashMap<(String, String), Vec<String>>;

/// The code the pages being rendered have collected, by page url, then bundle and bucket.
#[derive(Debug, Default)]
pub struct Bundles {
    names: Vec<String>,
    pages: Mutex<HashMap<String, Entries>>,
}

fn page_url(page: &Option<Value>) -> Result<String, String> {
    match page.as_ref().map(|p| p.get("url")) {
        Some(Value::String(s)) => Ok(s.to_string()),
        _ => Ok(String::new()),
    }
}

fn bucket(args: &[Value], kwargs_empty: bool, what: &str) -> Result<String, String> {
    if !kwargs_empty || args.len() > 1 {
        return Err(format!(
            "{}`{what}` takes only a bucket name here; Brrrty doesn't support more yet",
            brrrty_js::REFUSAL_PREFIX
        ));
    }
    match args.first() {
        None | Some(Value::Undefined) | Some(Value::Null) => Ok("default".to_string()),
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(_) => Err(format!("`{what}`'s bucket must be a string")),
    }
}

/// A bucket argument as 11ty reads it (`normalizeBuckets`): `split(",")`, nothing trimmed or dropped.
fn buckets(arg: &str) -> Vec<String> {
    arg.split(',').map(str::to_string).collect()
}

/// What 11ty's `pruneEmptyBundles` does to a page once any bundle is added: a posthtml pass
/// (the html transformer) on `.html` output only. It re-serializes the whole page (`<br/>` is
/// `<br>`, `disabled` is `disabled=""`), drops an empty `<style>`, a `<script>` with no text
/// and a blank `src`, and a `<link rel="stylesheet">` with a blank or missing `href`. An
/// element with `eleventy:keep` stays, minus that attribute. Matching is exact and case
/// sensitive, as posthtml-match-helper's is.
pub fn prune_empty(html: String, output_path: &str) -> Result<String, String> {
    if output_path.rsplit('.').next() != Some("html") {
        return Ok(html);
    }
    let mut tree = posthtml::parse(&html)?;
    prune_nodes(&mut tree);
    Ok(posthtml::render(&tree, posthtml::Closing::Default))
}

/// [`prune_empty`] on a page already parsed, so a later plugin of 11ty's one posthtml pass
/// can share the tree.
pub fn prune_tree(tree: &mut Vec<Node>) {
    prune_nodes(tree);
}

fn text_of(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|n| match n {
            Node::Text(t) => t.as_str().to_string(),
            Node::Element(e) => text_of(&e.content),
        })
        .collect()
}

fn attr<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
    e.attrs
        .as_ref()?
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn prune_nodes(nodes: &mut Vec<Node>) {
    nodes.retain_mut(|n| {
        let Node::Element(e) = n else { return true };
        let matched = match e.tag.as_str() {
            "style" | "script" => true,
            "link" => attr(e, "rel") == Some("stylesheet"),
            _ => false,
        };
        if matched {
            if attr(e, "eleventy:keep").is_some() {
                if let Some(a) = &mut e.attrs {
                    a.retain(|(k, _)| k != "eleventy:keep");
                }
            } else if e.tag == "link" {
                if attr(e, "href")
                    .unwrap_or("")
                    .trim_matches(js_space)
                    .is_empty()
                {
                    return false;
                }
            } else if text_of(&e.content).is_empty()
                && (e.tag == "style"
                    || attr(e, "src")
                        .unwrap_or("")
                        .trim_matches(js_space)
                        .is_empty())
            {
                return false;
            }
        }
        prune_nodes(&mut e.content);
        true
    });
}

impl Bundles {
    pub fn new(names: Vec<String>) -> Arc<Bundles> {
        Arc::new(Bundles {
            names,
            pages: Mutex::new(HashMap::new()),
        })
    }

    /// Adds the `{% name %}` paired tag of each bundle, and `getBundle`, to Nunjucks.
    pub fn register(self: &Arc<Self>, engine: &mut Engine) {
        for name in &self.names {
            let (me, n) = (self.clone(), name.clone());
            let f: FilterFn = Arc::new(move |input, args, env| {
                let bucket = bucket(args, env.kwargs.is_empty(), &format!("{{% {n} %}}"))?;
                let url = page_url(&env.page)?;
                let code = match input {
                    Value::String(s) => s.to_string(),
                    other => other.to_js_string(),
                };
                // 11ty trims on add, drops what's left empty, and de-duplicates on the trim.
                let code = code.trim().to_string();
                if !code.is_empty() {
                    let mut pages = me.pages.lock().unwrap();
                    let page = pages.entry(url).or_default();
                    for b in buckets(&bucket) {
                        let list = page.entry((n.clone(), b)).or_default();
                        if !list.contains(&code) {
                            list.push(code.clone());
                        }
                    }
                }
                Ok(Value::String("".into()))
            });
            engine.register_shortcode(name, true, f);
        }
        let me = self.clone();
        let f: FilterFn = Arc::new(move |input, args, env| {
            let Value::String(kind) = input else {
                return Err("`getBundle` needs the name of a bundle".to_string());
            };
            if !me.names.iter().any(|n| **n == **kind) {
                return Err(format!(
                    "{}`getBundle` asks for the bundle `{kind}`, which the config doesn't add (`addBundle`)",
                    brrrty_js::REFUSAL_PREFIX
                ));
            }
            let bucket = bucket(args, env.kwargs.is_empty(), "getBundle")?;
            Ok(Value::String(
                format!("{OPEN}{kind}:{}{CLOSE}", buckets(&bucket).join(",")).into(),
            ))
        });
        engine.register_shortcode("getBundle", false, f);
    }

    /// Registers the same tags for Liquid as named refusals.
    pub fn register_liquid_refusals(&self, engine: &mut brrrty_liquid::Engine) {
        let msg = format!(
            "{}bundles ({{% css %}}, getBundle) are Nunjucks-only for now",
            brrrty_js::REFUSAL_PREFIX
        );
        for name in &self.names {
            let m = msg.clone();
            let f: brrrty_liquid::ShortcodeFn = Arc::new(move |_, _| Err(m.clone()));
            engine.register_shortcode(name, true, f);
        }
        let f: brrrty_liquid::ShortcodeFn = Arc::new(move |_, _| Err(msg.clone()));
        engine.register_shortcode("getBundle", false, f);
    }

    /// `html` with each `getBundle` placeholder replaced by what the page at `url` collected,
    /// which the page then forgets.
    pub fn resolve(&self, url: &str, html: String) -> String {
        let entries = self.pages.lock().unwrap().remove(url).unwrap_or_default();
        if !html.contains(OPEN) {
            return html;
        }
        let mut out = String::with_capacity(html.len());
        let mut rest = html.as_str();
        while let Some(start) = rest.find(OPEN) {
            out.push_str(&rest[..start]);
            let after = &rest[start + OPEN.len_utf8()..];
            let Some(end) = after.find(CLOSE) else {
                rest = &rest[start..];
                break;
            };
            if let Some((kind, bs)) = after[..end].split_once(':') {
                let mut seen: Vec<&String> = Vec::new();
                for b in bs.split(',') {
                    if let Some(list) = entries.get(&(kind.to_string(), b.to_string())) {
                        for c in list {
                            if !seen.contains(&c) {
                                seen.push(c);
                            }
                        }
                    }
                }
                out.push_str(
                    &seen
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            rest = &after[end + CLOSE.len_utf8()..];
        }
        out.push_str(rest);
        out
    }
}
