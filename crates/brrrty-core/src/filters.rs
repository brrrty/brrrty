//! Which filters each template engine gets.
//!
//! 11ty registers a filter added with `addFilter` in every engine, and the
//! RSS plugin's date and URL filters in Nunjucks only. [`register_nunjucks_filters`]
//! gives a Nunjucks engine the same set the Liquid engine gets in
//! `build`, plus the Nunjucks-only ones.

use std::sync::Arc;

use brrrty_js::FilterFn;
use brrrty_nunjucks::Engine;

use crate::options::{ConfigFilter, FilterEngine, JsConfigFilter, JsConfigShortcode};
use crate::plugins::rss;

/// A filter written for the Liquid engine, as a shared one. It takes its
/// arguments as 11ty passes them to a JavaScript filter, which is how
/// Nunjucks passes them too: in order, with the keyword arguments as a last
/// object.
///
/// A `SafeString` (from `safe`, `escape` or a macro) is an object in
/// JavaScript, which a filter written in Rhai or Rust can't tell from a
/// string, so a call that passes one is refused.
fn shared(name: &str, f: brrrty_liquid::ArgsFilterFn) -> FilterFn {
    let name = name.to_string();
    Arc::new(move |input, args, env| {
        if env.any_safe() {
            return Err(format!(
                "{}can't give a SafeString (from `safe`, `escape` or a macro) to the filter `{name}`: JavaScript would pass an object, not a string",
                brrrty_js::REFUSAL_PREFIX
            ));
        }
        f(input, args, &brrrty_liquid::FilterCtx::new(env.timezone))
    })
}

/// Registers in `engine` what 11ty registers in Nunjucks: its own filters,
/// the config's filters, and, when the RSS plugin is on (`rss`), its
/// filters for every engine and those for Nunjucks alone. A later filter
/// replaces an earlier one of the same name, so a plugin's win over the
/// config's, as in 11ty.
pub fn register_nunjucks_filters(
    engine: &mut Engine,
    js: &[JsConfigFilter],
    config: &[ConfigFilter],
    rss: bool,
) {
    engine.register_eleventy_filters();
    for f in js {
        if f.engine != FilterEngine::Liquid {
            engine.register_filter(&f.name, shared(&f.name, f.run.clone()));
        }
    }
    for f in config {
        engine.register_filter(&f.name, shared(&f.name, f.run.clone()));
    }
    if rss {
        for (name, f) in rss::universal_filters() {
            engine.register_filter(name, shared(name, f));
        }
        for (name, f) in rss::nunjucks_filters() {
            engine.register_filter(name, f);
        }
    }
}

/// Registers the JS config's shortcodes as tags in Nunjucks. A later one replaces an earlier
/// one of the same name.
pub fn register_nunjucks_shortcodes(engine: &mut Engine, shortcodes: &[JsConfigShortcode]) {
    for s in shortcodes {
        if s.engine != FilterEngine::Liquid {
            engine.register_shortcode(&s.name, s.paired, shared(&s.name, s.run.clone()));
        }
    }
}
