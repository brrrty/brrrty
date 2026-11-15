//! Bug reporting: issue links and the panic hook.

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

/// Replace the default panic output with a friendly message, then exit 101.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
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
        let title: String = format!("Crash: {}", message.chars().take(60).collect::<String>());
        let body = format!("{}\n\nPanic: {message}\nAt: {location}", environment());
        eprintln!("Brrrty hit an internal bug. This is never your fault.");
        eprintln!("  {message}");
        eprintln!("  at {location}");
        if std::env::var_os("RUST_BACKTRACE").is_some() {
            eprintln!("{}", std::backtrace::Backtrace::force_capture());
        }
        eprintln!("Please report it: {}", issue_url(&title, &body));
        std::process::exit(101);
    }));
}
