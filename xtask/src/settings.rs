//! Per-site settings read from `<settings-dir>/<SITE>.toml`.

use serde::Deserialize;

/// Site settings. `[[allow]]` and any other keys are ignored here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SiteSettings {
    #[serde(default = "default_input")]
    pub input: String,
    #[serde(default)]
    pub passthrough: Vec<String>,
}

fn default_input() -> String {
    ".".to_string()
}

impl SiteSettings {
    pub fn from_toml(src: &str) -> Result<SiteSettings, String> {
        toml::from_str(src).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_site_settings_and_ignores_allow_entries() {
        let s = SiteSettings::from_toml(
            "input = \"src\"\npassthrough = [\"src/images/\"]\n[[allow]]\nid=\"x\"\n",
        )
        .unwrap();
        assert_eq!(s.input, "src");
        assert_eq!(s.passthrough, vec!["src/images/"]);
    }

    #[test]
    fn defaults_when_keys_missing() {
        let s = SiteSettings::from_toml("").unwrap();
        assert_eq!(s.input, ".");
        assert!(s.passthrough.is_empty());
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(SiteSettings::from_toml("input = [").is_err());
    }
}
