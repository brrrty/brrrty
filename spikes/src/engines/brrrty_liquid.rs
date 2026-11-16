//! Adapter for `brrrty-liquid`, with Eleventy's own filters registered.

use crate::case::Case;
use crate::score::Engine;
use brrrty_js::Value;
use brrrty_liquid::{Engine as Liquid, EngineOptions, Loader, Tz};
use std::collections::BTreeMap;

pub struct BrrrtyLiquid {
    engine: Liquid,
}

impl BrrrtyLiquid {
    pub fn new() -> Self {
        let mut engine = Liquid::new(EngineOptions {
            strict_filters: true,
            timezone: Tz::Utc,
        });
        engine.register_eleventy_filters();
        Self { engine }
    }
}

impl Default for BrrrtyLiquid {
    fn default() -> Self {
        Self::new()
    }
}

struct Includes<'a>(&'a BTreeMap<String, String>);

impl Loader for Includes<'_> {
    fn load(&self, name: &str) -> Result<(String, String), brrrty_liquid::LoadError> {
        self.0
            .get(name)
            .map(|s| (name.to_string(), s.clone()))
            .ok_or_else(Default::default)
    }
}

/// The case's data plus what Eleventy adds to every template's data: the
/// cases are rendered as `liquid/<name>.liquid`, with output `.txt`.
fn with_eleventy_data(case: &Case) -> serde_json::Value {
    let mut data = case.data.clone();
    if case.group == "eleventy" {
        let page = serde_json::json!({
            "url": format!("/liquid/{}.txt", case.name),
            "fileSlug": case.name,
            "filePathStem": format!("/liquid/{}", case.name),
        });
        let provided = serde_json::json!({
            "page": page,
            "eleventy": {"generator": "Eleventy v3.1.8", "version": "3.1.8"},
            "collections": {"all": [{"page": page, "data": {}}]},
        });
        if let Some(provided) = provided.as_object() {
            for (k, v) in provided {
                data.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
    serde_json::Value::Object(data)
}

impl Engine for BrrrtyLiquid {
    fn name(&self) -> String {
        "brrrty-liquid".into()
    }

    fn render(&self, case: &Case) -> Result<String, String> {
        let data = Value::from_json(&with_eleventy_data(case));
        self.engine
            .parse(&case.name, &case.template)
            .and_then(|t| self.engine.render(&t, &data, &Includes(&case.includes)))
            .map_err(|e| e.to_string())
    }

    fn parse(&self, source: &str) -> Result<(), String> {
        self.engine
            .parse("template", source)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
