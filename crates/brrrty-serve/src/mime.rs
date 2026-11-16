//! Content types, the way the `mime` package the dev server uses answers.

use crate::mime_table::TYPES;

/// The `Content-Type` value for a file name, or `None` when the extension is
/// unknown or missing (the dev server sends no `Content-Type` then).
///
/// Only `text/html` gets a charset, as in the dev server.
pub(crate) fn content_type(file_name: &str) -> Option<String> {
    let (_, ext) = file_name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    let idx = TYPES
        .binary_search_by(|(e, _)| (*e).cmp(ext.as_str()))
        .ok()?;
    let ty = TYPES[idx].1;
    Some(if ty == "text/html" {
        "text/html; charset=utf-8".to_string()
    } else {
        ty.to_string()
    })
}

/// Whether a content type is HTML, which is where a hook may rewrite the body.
pub(crate) fn is_html(content_type: &str) -> bool {
    content_type.starts_with("text/html")
}
