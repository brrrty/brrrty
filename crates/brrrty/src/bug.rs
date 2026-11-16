//! Bug reporting: issue links and the panic hook.

use std::io::Write as _;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Everything except the unreserved characters `A-Z a-z 0-9 - . _ ~`.
const ENCODE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

const ISSUES_NEW: &str = "https://github.com/brrrty/brrrty/issues/new";

/// A prefilled "new issue" link on the Brrrty repository.
pub fn issue_url(title: &str, body: &str) -> String {
    format!(
        "{ISSUES_NEW}?title={}&body={}",
        utf8_percent_encode(title, ENCODE),
        utf8_percent_encode(body, ENCODE)
    )
}

/// Environment details for issue bodies. Never includes file contents.
pub fn environment() -> String {
    format!(
        "Brrrty {}\nOS: {}\nArch: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// A dependency's source path from the crates.io registry, from its crate folder on, so a
/// Problem doesn't show the folder Brrrty was built in.
fn short_source_path(file: &str) -> &str {
    let unified = file.replace('\\', "/");
    match unified.find("/registry/src/") {
        Some(i) => {
            let rest = &file[i + "/registry/src/".len()..];
            rest.find(['/', '\\']).map_or(rest, |j| &rest[j + 1..])
        }
        None => file,
    }
}

/// Replace the default panic output with a friendly message, then exit 101.
///
/// Inside [`brrrty_core::contain::catch`] the hook only notes where the panic happened and
/// returns, so the panic unwinds to that `catch_unwind` and becomes a Problem.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if brrrty_core::contain::is_active() {
            if let Some(l) = info.location() {
                let file = short_source_path(l.file());
                brrrty_core::contain::note_location(format!("{file}:{}", l.line()));
            }
            return;
        }
        let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let mut keep = 500;
        let url = loop {
            let short = truncate_chars(&message, keep);
            let title = format!("Crash: {}", short.chars().take(60).collect::<String>());
            let body = format!("{}\n\nPanic: {short}\nAt: {location}", environment());
            let url = issue_url(&title, &body);
            if url.len() <= 6000 || keep <= 20 {
                break url;
            }
            keep /= 2;
        };
        let message = truncate_chars(&message, keep);
        // A closed stderr (`2>&1 | head -1`) must not panic inside the
        // hook, which would abort: each write's error is ignored.
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "Brrrty hit an internal bug. This is never your fault.");
        let _ = writeln!(err, "  {message}");
        let _ = writeln!(err, "  at {location}");
        if std::env::var_os("RUST_BACKTRACE").is_some() {
            let _ = writeln!(err, "{}", std::backtrace::Backtrace::force_capture());
        }
        let _ = writeln!(err, "Please report it: {url}");
        drop(err);
        std::process::exit(101);
    }));
}

#[cfg(test)]
mod tests {
    use super::short_source_path;

    #[test]
    fn registry_paths_start_at_the_crate() {
        assert_eq!(
            short_source_path(
                "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/markdown-it-0.6.1/src/lib.rs"
            ),
            "markdown-it-0.6.1/src/lib.rs"
        );
        assert_eq!(
            short_source_path("crates/brrrty-core/src/render.rs"),
            "crates/brrrty-core/src/render.rs"
        );
    }
}
