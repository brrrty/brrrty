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
                let mut pages = me.pages.lock().unwrap();
                let list = pages
                    .entry(url)
                    .or_default()
                    .entry((n.clone(), bucket))
                    .or_default();
                if !list.contains(&code) {
                    list.push(code);
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
                format!("{OPEN}{kind}:{bucket}{CLOSE}").into(),
            ))
        });
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
            if let Some((kind, bucket)) = after[..end].split_once(':')
                && let Some(list) = entries.get(&(kind.to_string(), bucket.to_string()))
            {
                out.push_str(&list.join("\n"));
            }
            rest = &after[end + CLOSE.len_utf8()..];
        }
        out.push_str(rest);
        out
    }
}
