//! Node's default locale, which `toLocaleString()` and `localeCompare()`
//! use when they're given none.
//!
//! Node's ICU takes it from the environment: the first of `LC_ALL`,
//! `LC_MESSAGES` and `LANG` that is set (probed on Node 26.2.0:
//! `LC_MESSAGES=de_DE.UTF-8` over `LANG=en_US.UTF-8` gives `de-DE`, and
//! nothing set gives `en-US`). Brrrty's ports give the `en-US` results, so
//! they run only when that resolves to exactly `en-US`: `en_US` (any case,
//! also `en-US` and `en_US_POSIX`), `C`, `c` or `POSIX`, each with any
//! codeset and no `@modifier`, or nothing set. Anything else (`en_GB`,
//! `de_DE`, `en_US@euro`, `C.UTF-8@x`, `posix`, an empty value, which Node
//! reads as `und`) is refused, naming the variable.

use std::sync::OnceLock;

/// Whether `(name, value)`, the first of `LC_ALL`, `LC_MESSAGES` and
/// `LANG` that is set, makes Node's default locale exactly `en-US`.
pub fn default_locale_check(first_set: Option<(&str, &str)>) -> Result<(), String> {
    let Some((name, value)) = first_set else {
        return Ok(());
    };
    // Probed with `env -i LANG=<value> node`: these and only these give
    // `en-US`. Any `@modifier` gives a variant (`C.UTF-8@x` is
    // `en-US-posix-x-lvariant-x`, which neither groups digits nor sorts
    // like en-US), and lowercase `posix` is a locale of its own.
    let base = value.split_once('.').map_or(value, |(b, _)| b);
    let en_us = !value.contains('@')
        && (matches!(base, "C" | "c" | "POSIX")
            || ["en_US", "en-US", "en_US_POSIX"]
                .iter()
                .any(|b| base.eq_ignore_ascii_case(b)));
    if en_us {
        return Ok(());
    }
    let shown = if value.is_empty() { "empty" } else { value };
    Err(format!(
        "{name} is `{shown}`, so Node's default locale isn't en-US, and toLocaleString() or localeCompare() \
         with no locale would format or sort for it. Brrrty matches only en-US: pass \"en-US\", or build \
         with {name} set to en_US.UTF-8 (as for 11ty)"
    ))
}

/// [`default_locale_check`] for this process's environment, read once.
pub fn check_default_locale() -> Result<(), String> {
    static CHECK: OnceLock<Result<(), String>> = OnceLock::new();
    CHECK
        .get_or_init(|| {
            let vars: Vec<(&str, String)> = ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .filter_map(|n| std::env::var(n).ok().map(|v| (n, v)))
                .collect();
            default_locale_check(vars.first().map(|(n, v)| (*n, v.as_str())))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_locales_node_resolves_to_en_us_pass() {
        // Probed with `env -i <VAR>=<value> node`: these give en-US.
        for v in [
            "en_US.UTF-8",
            "en_US",
            "en_US.ISO8859-1",
            "C",
            "POSIX",
            "C.UTF-8",
        ] {
            assert_eq!(default_locale_check(Some(("LANG", v))), Ok(()), "{v}");
        }
        assert_eq!(default_locale_check(None), Ok(()));
        // en-GB, de-DE, en-US-x-lvariant-euro, en, und.
        for v in [
            "en_GB.UTF-8",
            "de_DE.UTF-8",
            "en_US@euro",
            "en",
            "",
            "sv_SE.UTF-8",
        ] {
            let why = default_locale_check(Some(("LC_MESSAGES", v))).unwrap_err();
            assert!(why.starts_with("LC_MESSAGES is `"), "{why}");
        }
    }
}
