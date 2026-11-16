//! The feed templates of `virtualTemplate.js`, rendered without Nunjucks.
//!
//! [`source`] gives the template text 11ty renders (the page's
//! `page.rawInput`). [`render`] gives what Nunjucks 3.2.4 prints for it:
//! each `{%-` and `-%}` already applied to the text around it, every
//! `{{ }}` escaped (`& " ' < > \`), and each filter as the plugin and 11ty
//! define it. Anything 11ty would stop the build on is an `Err`.

use std::collections::BTreeSet;
use std::sync::Arc;

use brrrty_js::traps;
use brrrty_js::{Tz, Value};
use indexmap::IndexMap;

use super::{FeedKind, FeedSort, FeedTemplate, base};

const RSS: &str = r##"<?xml version="1.0" encoding="utf-8"?>
{STYLESHEET}<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xml:base="{{ metadata.base | addPathPrefixToFullUrl }}" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    {SCRIPT}
    <title>{{ metadata.title }}</title>
    <link>{{ metadata.base | addPathPrefixToFullUrl }}</link>
    <atom:link href="{{ permalink | htmlBaseUrl(metadata.base) }}" rel="self" type="application/rss+xml" />
    <description>{{ metadata.subtitle }}</description>
    <language>{{ metadata.language or page.lang }}</language>
    {%- if metadata.icon %}<image>{{ metadata.icon }}</image>{%- endif %}
    {%- for post in collections.{NAME} | eleventyFeedHasUrl{SORT} | eleventyFeedHead({LIMIT}) %}
    {%- set absolutePostUrl = post.url | htmlBaseUrl(metadata.base) %}
    <item>
      <title>{{ post.data.title }}</title>
      <link>{{ absolutePostUrl }}</link>
      {%- if (post.data.summary) -%}
      <description>{{ post.data.summary }}</description>
      <content:encoded>{{ post.content | renderTransforms(post.data.page, metadata.base) }}</content:encoded>
      {%- else -%}
      <description>{{ post.content | renderTransforms(post.data.page, metadata.base) }}</description>
      {%- endif -%}
      <pubDate>{{ post.date | dateToRfc822 }}</pubDate>
      <dc:creator>{{ metadata.author.name }}</dc:creator>
      <guid>{{ absolutePostUrl }}</guid>
    </item>
    {%- endfor %}
  </channel>
</rss>"##;
const ATOM: &str = r##"<?xml version="1.0" encoding="utf-8"?>
{STYLESHEET}<feed xmlns="http://www.w3.org/2005/Atom" xml:lang="{{ metadata.language or page.lang }}">
  {SCRIPT}
  <title>{{ metadata.title }}</title>
  <subtitle>{{ metadata.subtitle }}</subtitle>
  <link href="{{ permalink | htmlBaseUrl(metadata.base) }}" rel="self" />
  <link href="{{ metadata.base | addPathPrefixToFullUrl }}" />
  <updated>{{ collections['{NAME}'] | eleventyFeedHasUrl | getNewestCollectionItemDate | dateToRfc3339 }}</updated>
  <id>{{ metadata.base | addPathPrefixToFullUrl }}</id>
  {%- if metadata.icon %}
  <icon>{{ metadata.icon }}</icon>
  {%- endif %}
  {%- if metadata.logo %}
  <logo>{{ metadata.logo }}</logo>
  {%- endif %}
  <author>
    <name>{{ metadata.author.name }}</name>
    {%- if metadata.author.email %}
    <email>{{ metadata.author.email }}</email>
    {%- endif %}
  </author>
  {%- for post in collections['{NAME}'] | eleventyFeedHasUrl{SORT} | eleventyFeedHead({LIMIT}) %}
  {%- set absolutePostUrl %}{{ post.url | htmlBaseUrl(metadata.base) }}{% endset %}
  <entry>
    <title>{{ post.data.title }}</title>
    <link href="{{ absolutePostUrl }}" />
    <updated>{{ post.date | dateToRfc3339 }}</updated>
    <id>{{ absolutePostUrl }}</id>
    {%- if post.data.summary %}
    <summary>{{ post.data.summary }}</summary>
    {%- endif %}
    <content type="html">{{ post.content | renderTransforms(post.data.page, metadata.base) }}</content>
  </entry>
  {%- endfor %}
</feed>"##;
const JSON: &str = r##"{
  "version": "https://jsonfeed.org/version/1.1",
  "title": "{{ metadata.title }}",
  "language": "{{ metadata.language or page.lang }}",
  "home_page_url": "{{ metadata.base | addPathPrefixToFullUrl }}",
  "feed_url": "{{ permalink | htmlBaseUrl(metadata.base) }}",
  "description": "{{ metadata.description or metadata.subtitle }}",
  "authors": [
    {
      "name": "{{ metadata.author.name }}"{% if metadata.author.email %},
      "url": "mailto:{{ metadata.author.email }}"
      {%- endif %}
    }
  ],
  "items": [
    {%- for post in collections['{NAME}'] | eleventyFeedHasUrl{SORT} | eleventyFeedHead({LIMIT}) %}
    {%- set absolutePostUrl %}{{ post.url | htmlBaseUrl(metadata.base) }}{% endset %}
    {
      "id": "{{ absolutePostUrl }}",
      "url": "{{ absolutePostUrl }}",
      "title": "{{ post.data.title }}",
      "content_html": {% if post.content %}{{ post.content | renderTransforms(post.data.page, metadata.base) | dump | safe }}{% else %}""{% endif %},
      "date_published": "{{ post.date | dateToRfc3339 }}"
    }
    {% if not loop.last %},{% endif %}
    {%- endfor %}
  ]
}"##;

/// The template text `getFeedContent` writes.
pub(crate) fn source(t: &FeedTemplate) -> String {
    let text = match t.kind {
        FeedKind::Rss => RSS,
        FeedKind::Atom => ATOM,
        FeedKind::Json => JSON,
    };
    let sort = match t.sort {
        FeedSort::Auto => " | reverse".to_string(),
        FeedSort::Ascending => " | eleventyFeedSortByDate(\"ascending\")".to_string(),
        FeedSort::Descending => " | eleventyFeedSortByDate(\"descending\")".to_string(),
    };
    let stylesheet = t
        .stylesheet
        .as_ref()
        .map(|s| format!("<?xml-stylesheet href=\"{s}\" type=\"text/xsl\"?>\n"))
        .unwrap_or_default();
    let script = t
        .script
        .as_ref()
        .map(|s| format!("<script src=\"{s}\" xmlns=\"http://www.w3.org/1999/xhtml\"></script>"))
        .unwrap_or_default();
    text.replace("{STYLESHEET}", &stylesheet)
        .replace("{SCRIPT}", &script)
        .replace("{NAME}", &t.collection)
        .replace("{SORT}", &sort)
        .replace("{LIMIT}", &t.limit.to_string())
}

/// A rendered feed, and the top-level data it read that nothing defined.
pub(crate) struct Rendered {
    pub text: String,
    pub undefined: BTreeSet<String>,
}

/// Nunjucks's `escape`.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' => out.push_str("&#92;"),
            c => out.push(c),
        }
    }
    out
}

/// What `{{ value }}` prints: nothing for `undefined` and `null`, else
/// `String(value)`, escaped.
fn out(v: &Value) -> String {
    match v {
        Value::Undefined | Value::Null => String::new(),
        v => escape(&v.to_js_string_in(Tz::Local)),
    }
}

/// `obj.key`, as Nunjucks's `memberLookup` reads it: nothing from
/// `undefined` or `null`.
fn member(obj: &Value, key: &str) -> Value {
    match obj {
        Value::Object(o) => traps::read(o, key, o.get(key).cloned()).unwrap_or(Value::Undefined),
        other => other.get(key),
    }
}

/// `a or b`.
fn or(a: Value, b: Value) -> Value {
    if a.js_truthy() { a } else { b }
}

fn date_ms(v: &Value, what: &str) -> Result<f64, String> {
    match v {
        Value::Date(d) => Ok(d.timestamp_millis() as f64),
        other => Err(format!(
            "{what} is {}, not a date",
            crate::data::json_text(other)
        )),
    }
}

/// `dateToRfc3339`: `toISOString()` without the milliseconds.
pub(crate) fn date_to_rfc3339(ms: f64) -> Result<String, String> {
    let Some(f) = brrrty_js::tz::fields(Tz::Utc, ms) else {
        return Err("RangeError: Invalid time value".into());
    };
    let year = if (0..=9999).contains(&f.year) {
        format!("{:04}", f.year)
    } else if f.year < 0 {
        format!("-{:06}", -f.year)
    } else {
        format!("+{:06}", f.year)
    };
    Ok(format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        f.month0 + 1,
        f.day,
        f.hour,
        f.minute,
        f.second
    ))
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `dateToRfc822`: `Intl.DateTimeFormat('en-US', ...)` in the local zone,
/// taken apart and put back together, like `Sat, 07 Nov 2026 18:00:00
/// -0600`. A zone offset with seconds keeps them (`-0550:36`). Years
/// before 1 or after 9999 are refused: `Intl` prints them in ways the
/// plugin doesn't take apart cleanly.
pub(crate) fn date_to_rfc822(ms: f64) -> Result<String, String> {
    let Some(f) = brrrty_js::tz::fields(Tz::Local, ms) else {
        return Err("RangeError: Invalid time value".into());
    };
    if !(1..=9999).contains(&f.year) {
        return Err(format!(
            "dateToRfc822 of a date in the year {}, which Brrrty doesn't print yet",
            f.year
        ));
    }
    let east = (-f.offset_west * 60.0).round() as i64;
    let sign = if east < 0 { '-' } else { '+' };
    let abs = east.abs();
    let (h, m, s) = (abs / 3600, abs / 60 % 60, abs % 60);
    let zone = if s == 0 {
        format!("{sign}{h:02}{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}:{s:02}")
    };
    Ok(format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} {zone}",
        DAYS[f.weekday as usize],
        f.day,
        MONTHS[f.month0 as usize],
        f.year,
        f.hour,
        f.minute,
        f.second
    ))
}

/// `new Date()`, frozen by `SOURCE_DATE_EPOCH`.
fn now_ms() -> f64 {
    match brrrty_js::clock::source_date_epoch() {
        Ok(Some(ms)) => ms as f64,
        _ => brrrty_js::clock::system_now_ms(),
    }
}

/// `getNewestCollectionItemDate(collection)`: the latest `date`, or now
/// for an empty collection.
fn newest_date(items: &[Value]) -> Result<f64, String> {
    if items.is_empty() {
        return Ok(now_ms());
    }
    let mut max = f64::NEG_INFINITY;
    for item in items {
        let ms = match member(item, "date") {
            Value::Date(d) => d.timestamp_millis() as f64,
            other => other.to_js_number(),
        };
        if ms.is_nan() {
            return Err("RangeError: Invalid time value".into());
        }
        max = max.max(ms);
    }
    Ok(max)
}

/// `eleventyFeedHasUrl`: the items with a truthy `url`.
fn has_url(collection: &Value, name: &str) -> Result<Vec<Value>, String> {
    match collection {
        Value::Array(a) => Ok(a
            .iter()
            .filter(|item| member(item, "url").js_truthy())
            .cloned()
            .collect()),
        Value::Undefined | Value::Null => Err(format!(
            "TypeError: Cannot read properties of {} (reading 'filter'): `collections.{name}` doesn't exist",
            collection.to_js_string()
        )),
        _ => Err(format!(
            "TypeError: array.filter is not a function: `collections.{name}` is not a list"
        )),
    }
}

/// `eleventyFeedSortByDate`: a stable sort by `date`.
fn sort_by_date(items: &mut [Value], ascending: bool) -> Result<(), String> {
    let mut keyed = Vec::with_capacity(items.len());
    for item in items.iter() {
        let ms = match member(item, "date") {
            Value::Date(d) => d.timestamp_millis() as f64,
            _ => return Err("eleventyFeedSortByDate needs every item to have a date".into()),
        };
        keyed.push((ms, item.clone()));
    }
    keyed.sort_by(|a, b| {
        let o = a.0.total_cmp(&b.0);
        if ascending { o } else { o.reverse() }
    });
    for (slot, (_, v)) in items.iter_mut().zip(keyed) {
        *slot = v;
    }
    Ok(())
}

/// `eleventyFeedHead(n)`.
fn head(items: Vec<Value>, n: i64) -> Vec<Value> {
    let len = items.len() as i64;
    match n {
        0 => items,
        n if n < 0 => {
            let start = (len + n).max(0) as usize;
            items[start..].to_vec()
        }
        n => items.into_iter().take(n as usize).collect(),
    }
}

/// The entries the `{% for %}` loops over.
fn entries(t: &FeedTemplate, collection: &Value) -> Result<Vec<Value>, String> {
    let mut items = has_url(collection, &t.collection)?;
    match t.sort {
        FeedSort::Auto => items.reverse(),
        FeedSort::Ascending => sort_by_date(&mut items, true)?,
        FeedSort::Descending => sort_by_date(&mut items, false)?,
    }
    Ok(head(items, t.limit))
}

/// Whether a read so far was a trap.
fn traps_hit() -> bool {
    let hits = traps::take();
    let any = !hits.is_empty();
    // Put them back for the caller.
    for h in hits {
        traps::record(h);
    }
    any
}

/// The page data the template reads, by name: a name the data doesn't
/// have is noted, so a page that needed it is refused for that reason.
struct Scope<'a> {
    data: &'a IndexMap<String, Value>,
    undefined: BTreeSet<String>,
}

impl Scope<'_> {
    fn get(&mut self, key: &str) -> Value {
        match self.data.get(key) {
            Some(v) => v.clone(),
            None => {
                self.undefined.insert(key.to_string());
                Value::Undefined
            }
        }
    }
}

fn string_of(v: &Value, what: &str) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.to_string()),
        other => Err(format!(
            "{what} is {}, and 11ty's URL filters need a string",
            crate::data::json_text(other)
        )),
    }
}

/// Render the feed page with `data`, its data with `collections`.
pub(crate) fn render(t: &FeedTemplate, data: &IndexMap<String, Value>) -> Result<Rendered, String> {
    traps::clear();
    let result = render_inner(t, data);
    let hits = traps::take();
    result.map(|mut r| {
        r.undefined.extend(hits);
        r
    })
}

fn render_inner(t: &FeedTemplate, data: &IndexMap<String, Value>) -> Result<Rendered, String> {
    let mut scope = Scope {
        data,
        undefined: BTreeSet::new(),
    };
    let metadata = scope.get("metadata");
    let page = scope.get("page");
    let permalink = scope.get("permalink");
    let collections = scope.get("collections");
    let m = |k: &str| member(&metadata, k);
    let base_value = m("base");
    let base = match &base_value {
        Value::String(s) if base::is_valid_url(s) => s.to_string(),
        other => {
            return Err(format!(
                "the feed's `metadata.base` is {}; it must be a full URL, like \"https://example.com/\" (11ty stops the build on this feed)",
                crate::data::json_text(other)
            ));
        }
    };
    let page_url = match member(&page, "url") {
        Value::String(s) => Some(s.to_string()),
        _ => None,
    };
    // `metadata.base | addPathPrefixToFullUrl`
    let home = escape(&base::add_path_prefix_to_url(&base, None)?);
    // `permalink | htmlBaseUrl(metadata.base)`
    let self_url = escape(&base::transform_url(
        &string_of(&permalink, "`permalink`")?,
        &base,
        page_url.as_deref(),
    )?);
    let language = out(&or(m("language"), member(&page, "lang")));
    let author = m("author");
    let collection = member(&collections, &t.collection);
    // A collection Brrrty can't give (no `collections`, or a name the
    // config may add) is reported by the build, from what was read.
    if matches!(collection, Value::Undefined)
        && (matches!(collections, Value::Undefined) || traps_hit())
    {
        return Ok(Rendered {
            text: String::new(),
            undefined: scope.undefined,
        });
    }
    let posts = entries(t, &collection)?;
    let post_url = |post: &Value| -> Result<String, String> {
        base::transform_url(
            &string_of(&member(post, "url"), "a post's `url`")?,
            &base,
            page_url.as_deref(),
        )
    };
    let content = |post: &Value| -> Result<String, String> {
        let html = match member(post, "content") {
            Value::String(s) => s.to_string(),
            v if !v.js_truthy() => String::new(),
            v => v.to_js_string_in(Tz::Local),
        };
        let post_page = member(&member(post, "data"), "page");
        let post_page = if post_page.js_truthy() {
            post_page
        } else {
            page.clone()
        };
        let url = match member(&post_page, "url") {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        };
        super::render_transforms(
            &html,
            &member(&post_page, "outputPath"),
            url.as_deref(),
            &base,
        )
    };
    let stylesheet = t
        .stylesheet
        .as_ref()
        .map(|s| format!("<?xml-stylesheet href=\"{s}\" type=\"text/xsl\"?>\n"))
        .unwrap_or_default();
    let script = t
        .script
        .as_ref()
        .map(|s| format!("<script src=\"{s}\" xmlns=\"http://www.w3.org/1999/xhtml\"></script>"))
        .unwrap_or_default();
    let mut s = String::new();
    match t.kind {
        FeedKind::Atom => {
            let updated = date_to_rfc3339(newest_date(&has_url(&collection, &t.collection)?)?)?;
            s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
            s.push_str(&stylesheet);
            s.push_str(&format!(
                "<feed xmlns=\"http://www.w3.org/2005/Atom\" xml:lang=\"{language}\">\n  {script}\n  <title>{}</title>\n  <subtitle>{}</subtitle>\n  <link href=\"{self_url}\" rel=\"self\" />\n  <link href=\"{home}\" />\n  <updated>{}</updated>\n  <id>{home}</id>",
                out(&m("title")),
                out(&m("subtitle")),
                escape(&updated),
            ));
            if m("icon").js_truthy() {
                s.push_str(&format!("\n  <icon>{}</icon>", out(&m("icon"))));
            }
            if m("logo").js_truthy() {
                s.push_str(&format!("\n  <logo>{}</logo>", out(&m("logo"))));
            }
            s.push_str(&format!(
                "\n  <author>\n    <name>{}</name>",
                out(&member(&author, "name"))
            ));
            if member(&author, "email").js_truthy() {
                s.push_str(&format!(
                    "\n    <email>{}</email>",
                    out(&member(&author, "email"))
                ));
            }
            s.push_str("\n  </author>");
            for post in &posts {
                // `{% set %}...{% endset %}` keeps the escaped text, which
                // is escaped again where it is printed.
                let link = escape(&escape(&post_url(post)?));
                let data = member(post, "data");
                let date = date_to_rfc3339(date_ms(&member(post, "date"), "a post's `date`")?)?;
                s.push_str(&format!(
                    "\n  <entry>\n    <title>{}</title>\n    <link href=\"{link}\" />\n    <updated>{}</updated>\n    <id>{link}</id>",
                    out(&member(&data, "title")),
                    escape(&date)
                ));
                if member(&data, "summary").js_truthy() {
                    s.push_str(&format!(
                        "\n    <summary>{}</summary>",
                        out(&member(&data, "summary"))
                    ));
                }
                s.push_str(&format!(
                    "\n    <content type=\"html\">{}</content>\n  </entry>",
                    escape(&content(post)?)
                ));
            }
            s.push_str("\n</feed>");
        }
        FeedKind::Rss => {
            s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
            s.push_str(&stylesheet);
            s.push_str(&format!(
                "<rss version=\"2.0\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xml:base=\"{home}\" xmlns:atom=\"http://www.w3.org/2005/Atom\">\n  <channel>\n    {script}\n    <title>{}</title>\n    <link>{home}</link>\n    <atom:link href=\"{self_url}\" rel=\"self\" type=\"application/rss+xml\" />\n    <description>{}</description>\n    <language>{language}</language>",
                out(&m("title")),
                out(&m("subtitle")),
            ));
            if m("icon").js_truthy() {
                s.push_str(&format!("<image>{}</image>", out(&m("icon"))));
            }
            for post in &posts {
                let link = escape(&post_url(post)?);
                let data = member(post, "data");
                s.push_str(&format!(
                    "\n    <item>\n      <title>{}</title>\n      <link>{link}</link>",
                    out(&member(&data, "title"))
                ));
                if member(&data, "summary").js_truthy() {
                    s.push_str(&format!(
                        "<description>{}</description>\n      <content:encoded>{}</content:encoded>",
                        out(&member(&data, "summary")),
                        escape(&content(post)?)
                    ));
                } else {
                    s.push_str(&format!(
                        "<description>{}</description>",
                        escape(&content(post)?)
                    ));
                }
                let date = date_to_rfc822(date_ms(&member(post, "date"), "a post's `date`")?)?;
                s.push_str(&format!(
                    "<pubDate>{}</pubDate>\n      <dc:creator>{}</dc:creator>\n      <guid>{link}</guid>\n    </item>",
                    escape(&date),
                    out(&member(&author, "name"))
                ));
            }
            s.push_str("\n  </channel>\n</rss>");
        }
        FeedKind::Json => {
            s.push_str(&format!(
                "{{\n  \"version\": \"https://jsonfeed.org/version/1.1\",\n  \"title\": \"{}\",\n  \"language\": \"{language}\",\n  \"home_page_url\": \"{home}\",\n  \"feed_url\": \"{self_url}\",\n  \"description\": \"{}\",\n  \"authors\": [\n    {{\n      \"name\": \"{}\"",
                out(&m("title")),
                out(&or(m("description"), m("subtitle"))),
                out(&member(&author, "name")),
            ));
            if member(&author, "email").js_truthy() {
                s.push_str(&format!(
                    ",\n      \"url\": \"mailto:{}\"",
                    out(&member(&author, "email"))
                ));
            }
            s.push_str("\n    }\n  ],\n  \"items\": [");
            let last = posts.len().saturating_sub(1);
            for (i, post) in posts.iter().enumerate() {
                let link = escape(&escape(&post_url(post)?));
                let data = member(post, "data");
                let html = if member(post, "content").js_truthy() {
                    serde_json::to_string(&content(post)?).map_err(|e| e.to_string())?
                } else {
                    "\"\"".to_string()
                };
                let date = date_to_rfc3339(date_ms(&member(post, "date"), "a post's `date`")?)?;
                s.push_str(&format!(
                    "\n    {{\n      \"id\": \"{link}\",\n      \"url\": \"{link}\",\n      \"title\": \"{}\",\n      \"content_html\": {html},\n      \"date_published\": \"{}\"\n    }}\n    {}",
                    out(&member(&data, "title")),
                    escape(&date),
                    if i == last { "" } else { "," }
                ));
            }
            s.push_str("\n  ]\n}");
        }
    }
    Ok(Rendered {
        text: s,
        undefined: scope.undefined,
    })
}

// ---- The filters for every template language ----

type Res = Result<Value, String>;

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

/// `htmlBaseUrl(url, baseOverride, pageUrlOverride)` with the default
/// base `/` (11ty's `pathPrefix`).
fn html_base_url(input: &Value, args: &[Value]) -> Res {
    let base = arg(args, 0);
    let base = if base.js_truthy() {
        base.to_js_string()
    } else {
        "/".to_string()
    };
    if base == "/" {
        return Ok(input.clone());
    }
    let Value::String(url) = input else {
        return Err("htmlBaseUrl: Brrrty only takes a string URL here".into());
    };
    if !base::is_valid_url(&base) {
        return Err(
            "htmlBaseUrl with a base that isn't a full URL isn't supported by Brrrty yet".into(),
        );
    }
    let page_url = match arg(args, 1) {
        Value::String(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    };
    if page_url.is_none() && !base::is_valid_url(url) && !url.starts_with('/') {
        return Err(
            "htmlBaseUrl of a relative URL needs the page's URL, which Brrrty can't give a filter yet; pass `page.url` as the third argument".into(),
        );
    }
    Ok(Value::String(
        base::transform_url(url, &base, page_url.as_deref())?.into(),
    ))
}

fn add_path_prefix_to_full_url(input: &Value, _args: &[Value]) -> Res {
    let url = input.to_js_string();
    Ok(Value::String(
        base::add_path_prefix_to_url(&url, None)
            .map_err(|e| format!("TypeError: {e}"))?
            .into(),
    ))
}

fn feed_has_url(input: &Value, _args: &[Value]) -> Res {
    Ok(Value::Array(Arc::new(has_url(input, "?").map_err(
        |_| "TypeError: array.filter is not a function".to_string(),
    )?)))
}

fn feed_sort_by_date(input: &Value, args: &[Value]) -> Res {
    let Value::Array(a) = input else {
        return Err("TypeError: array is not iterable".into());
    };
    let ascending = matches!(arg(args, 0), Value::String(s) if &*s == "ascending");
    let mut items = (**a).clone();
    sort_by_date(&mut items, ascending)?;
    Ok(Value::Array(Arc::new(items)))
}

fn feed_head(input: &Value, args: &[Value]) -> Res {
    let n = arg(args, 0);
    if !n.js_truthy() {
        return Ok(input.clone());
    }
    let Value::Array(a) = input else {
        return Err("eleventyFeedHead: Brrrty only takes a list here".into());
    };
    let Value::Number(n) = n else {
        return Err("eleventyFeedHead: Brrrty only takes a number here".into());
    };
    if n.fract() != 0.0 {
        return Err("eleventyFeedHead: Brrrty only takes a whole number here".into());
    }
    Ok(Value::Array(Arc::new(head((**a).clone(), n as i64))))
}

fn wrap(f: fn(&Value, &[Value]) -> Res) -> brrrty_liquid::ArgsFilterFn {
    Arc::new(move |input, args, _ctx| f(input, args))
}

pub(crate) fn universal_filters() -> Vec<(&'static str, brrrty_liquid::ArgsFilterFn)> {
    vec![
        ("addPathPrefixToFullUrl", wrap(add_path_prefix_to_full_url)),
        ("htmlBaseUrl", wrap(html_base_url)),
        ("eleventyFeedHasUrl", wrap(feed_has_url)),
        ("eleventyFeedSortByDate", wrap(feed_sort_by_date)),
        ("eleventyFeedHead", wrap(feed_head)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probed: the plugin's own `dateRfc3339.js` in Node.
    #[test]
    fn rfc3339_like_the_plugin() {
        assert_eq!(date_to_rfc3339(0.0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(
            date_to_rfc3339(1_767_268_800_123.0).unwrap(),
            "2026-01-01T12:00:00Z"
        );
        assert_eq!(
            date_to_rfc3339(253_402_300_800_000.0).unwrap(),
            "+010000-01-01T00:00:00Z"
        );
        assert!(date_to_rfc3339(f64::NAN).is_err());
    }

    #[test]
    fn escapes_like_nunjucks() {
        assert_eq!(
            escape(r#"a&b"c'd<e>f\g"#),
            "a&amp;b&quot;c&#39;d&lt;e&gt;f&#92;g"
        );
    }
}
