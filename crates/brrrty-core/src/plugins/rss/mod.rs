//! `@11ty/eleventy-plugin-rss` 3.1.0: `feedPlugin`, which adds an Atom,
//! RSS or JSON feed of a collection as a virtual template, and the filters
//! the plugin registers.
//!
//! 11ty renders the feed from a Nunjucks template the plugin writes
//! (`virtualTemplate.js`). [`feed`] renders those three templates
//! directly, without the Nunjucks engine: the same text, the same whitespace
//! control, Nunjucks's escaping, and the same filters, down to the double
//! escaping of an Atom entry's link. The feed is a page of the site like any
//! other: it has its place in `collections.all`, its data cascade, and its
//! `eleventyImport`.
//!
//! The post HTML in a feed goes through 11ty's `renderTransforms` filter,
//! which runs the HTML `<base>` plugin's URL transform over it: every URL in
//! the attributes `@11ty/posthtml-urls` reads becomes a full URL on the
//! feed's `metadata.base` (see [`posthtml`], [`urls`] and [`base`]).

pub mod base;
pub(crate) mod feed;
pub(crate) mod nunjucks;
pub mod posthtml;
pub mod urls;

use std::sync::Arc;

use brrrty_js::Value;

use crate::frontmatter::Data;

/// `type`: which template the feed uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Atom,
    Rss,
    Json,
}

impl FeedKind {
    pub fn name(self) -> &'static str {
        match self {
            FeedKind::Atom => "atom",
            FeedKind::Rss => "rss",
            FeedKind::Json => "json",
        }
    }

    pub fn from_name(name: &str) -> Option<FeedKind> {
        match name {
            "atom" => Some(FeedKind::Atom),
            "rss" => Some(FeedKind::Rss),
            "json" => Some(FeedKind::Json),
            _ => None,
        }
    }
}

/// `collection.sort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedSort {
    /// Reverse the collection (it is oldest first).
    Auto,
    Ascending,
    Descending,
}

/// What the template text needs that isn't data: the parts the plugin
/// writes into it with `${...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedTemplate {
    pub kind: FeedKind,
    pub collection: String,
    /// `collection.limit`; 0 is no limit.
    pub limit: i64,
    pub sort: FeedSort,
    pub stylesheet: Option<String>,
    pub script: Option<String>,
}

/// One `feedPlugin` call, as `getFeedTemplate` makes it: a virtual
/// template's path, its data and its template.
#[derive(Debug, Clone)]
pub struct FeedOptions {
    /// The virtual template's path in the input folder, like
    /// `eleventy-plugin-feed-my-blog-atom.njk`.
    pub input_path: String,
    /// Its data: `templateData`, then `permalink`,
    /// `eleventyExcludeFromCollections`, `eleventyImport`, `layout` and
    /// `metadata`.
    pub data: Data,
    pub template: Arc<FeedTemplate>,
}

impl FeedOptions {
    /// The template text 11ty would render, which becomes the page's
    /// `page.rawInput`.
    pub fn content(&self) -> String {
        feed::source(&self.template)
    }
}

/// The filters the plugin, and the HTML `<base>` plugin it adds, register
/// for every template language. The rest (`dateToRfc3339`,
/// `dateToRfc822`, `getNewestCollectionItemDate`, `absoluteUrl`,
/// `htmlToAbsoluteUrls`) are Nunjucks filters, which a Liquid template
/// can't use in 11ty either.
pub fn universal_filters() -> Vec<(&'static str, brrrty_liquid::ArgsFilterFn)> {
    feed::universal_filters()
}

/// The filters the plugin adds to Nunjucks only (see [`universal_filters`]).
pub fn nunjucks_filters() -> Vec<(&'static str, brrrty_js::FilterFn)> {
    nunjucks::filters()
}

/// The plugin's `absoluteUrl(url, base)`: `new URL(url, base)`, or `url`
/// when that throws.
pub fn absolute_url(url: &str, base: &str) -> String {
    url::Url::parse(base)
        .and_then(|b| base::join(&b, url))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| url.to_string())
}

/// `new URL(url, base)`, or `None` when that throws.
pub(crate) fn absolute_url_checked(url: &str, base: &str) -> Option<String> {
    url::Url::parse(base)
        .and_then(|b| base::join(&b, url))
        .ok()
        .map(|u| u.to_string())
}

/// `new URL(url, base)` with no `base` (`undefined`).
pub(crate) fn absolute_url_no_base(url: &str) -> Option<String> {
    url::Url::parse(url).ok().map(|u| u.to_string())
}

/// The plugin's `htmlToAbsoluteUrls(html, base)` (a Nunjucks filter):
/// every URL `@11ty/posthtml-urls` reads made absolute with
/// [`absolute_url`], printed with `closingSingleTag: "slash"`.
pub fn html_to_absolute_urls(html: &str, base: &str) -> Result<String, String> {
    if base.is_empty() {
        return Err("eleventy-plugin-rss: htmlToAbsoluteUrls(absolutePostUrl) was missing the full URL base `absolutePostUrl` argument.".into());
    }
    let mut tree = posthtml::parse(html)?;
    urls::transform(&mut tree, &mut |u, _, _| {
        Ok(absolute_url(u.trim_matches(posthtml::js_space), base))
    })?;
    Ok(posthtml::render(&tree, posthtml::Closing::Slash))
}

/// `renderTransforms(html, page, base)` as 11ty 3.1.8 runs it for a feed,
/// with no transforms of the site's own: the HTML `<base>` plugin's URL
/// transform, for a page whose output is `.html`. `page_url` is the page's
/// `url`.
pub fn render_transforms(
    html: &str,
    output_path: &Value,
    page_url: Option<&str>,
    base: &str,
) -> Result<String, String> {
    let out = match output_path {
        Value::String(s) => s.to_string(),
        v if !v.js_truthy() => String::new(),
        v => v.to_js_string(),
    };
    if out.rsplit('.').next() != Some("html") {
        return Ok(html.to_string());
    }
    let mut tree = posthtml::parse(html)?;
    urls::transform(&mut tree, &mut |u, _, _| {
        base::transform_url(u.trim_matches(posthtml::js_space), base, page_url)
    })?;
    Ok(posthtml::render(&tree, posthtml::Closing::Default))
}
