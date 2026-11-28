//! `@11ty/posthtml-urls` 1.0.3: call a function on every URL in the
//! attributes its filter lists, and put back what it returns.
//!
//! It reads `href`, `src` and the like, the URLs in `srcset`
//! (`parse-srcset` 1.0.2), the comma-separated URLs in `ping`
//! (`list-to-array` 1.1.0), and the URL in a `<meta http-equiv="refresh">`
//! (`http-equiv-refresh` 2.0.1). Tag and attribute names are compared as
//! written, so `<IMG SRC>` is left alone, and text (inside `<code>`,
//! `<pre>` or `<script>`) is never read.

use std::sync::LazyLock;

use super::posthtml::{Element, Node, js_space, walk_mut};

/// The attributes the plugin reads, by tag.
fn accepts(tag: &str, attr: &str, el: &Element) -> bool {
    match (tag, attr) {
        ("a" | "area", "href" | "ping")
        | ("audio" | "embed" | "iframe" | "script" | "track", "src")
        | ("base" | "link", "href")
        | ("blockquote" | "del" | "ins" | "q", "cite")
        | ("button", "formaction")
        | ("form", "action")
        | ("img" | "source", "src" | "srcset")
        | ("input", "formaction" | "src")
        | ("object", "data")
        | ("video", "poster" | "src") => true,
        ("meta", "content") => is_http_equiv_refresh(el),
        _ => false,
    }
}

fn is_http_equiv_refresh(el: &Element) -> bool {
    el.attrs.as_ref().is_some_and(|a| {
        a.iter()
            .any(|(k, v)| k == "http-equiv" && v.to_lowercase() == "refresh")
    })
}

/// Names JavaScript objects inherit, which the plugin's `attrName in
/// tagGroup` and `filter[node.tag]` find on every object: it would treat
/// them as URL attributes in ways that make no sense. They are refused.
pub(crate) const INHERITED: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

/// What the URL function gets: the URL, its attribute and tag.
pub type EachUrl<'a> = dyn FnMut(&str, &str, &str) -> Result<String, String> + 'a;

/// Run the plugin over `nodes`. An `Err` is the first error the function
/// gave, or an element Brrrty refuses (see [`INHERITED`]).
pub fn transform(nodes: &mut [Node], each: &mut EachUrl<'_>) -> Result<(), String> {
    let mut failed: Option<String> = None;
    walk_mut(nodes, &mut |el| {
        if failed.is_some() {
            return;
        }
        if let Err(e) = transform_element(el, each) {
            failed = Some(e);
        }
    });
    match failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn transform_element(el: &mut Element, each: &mut EachUrl<'_>) -> Result<(), String> {
    let Some(attrs) = &el.attrs else {
        return Ok(());
    };
    if INHERITED.contains(&el.tag.as_str()) {
        return Err(format!(
            "an HTML element named `{}`, which the URL plugin can't read the way 11ty does",
            el.tag
        ));
    }
    if let Some((name, _)) = attrs.iter().find(|(k, _)| INHERITED.contains(&k.as_str())) {
        return Err(format!(
            "an HTML attribute named `{name}`, which the URL plugin can't read the way 11ty does"
        ));
    }
    let tag = el.tag.clone();
    let names: Vec<String> = attrs
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| accepts(&tag, k, el))
        .collect();
    for name in names {
        let Some(attrs) = &mut el.attrs else { break };
        let Some(slot) = attrs.iter_mut().find(|(k, _)| *k == name) else {
            continue;
        };
        let value = slot.1.clone();
        let new = match name.as_str() {
            "content" => meta_refresh(&value, &name, &tag, each)?,
            "ping" => comma_separated(&value, &name, &tag, each)?,
            "srcset" => srcset(&value, &name, &tag, each)?,
            _ => Some(each(&value, &name, &tag)?),
        };
        if let Some(new) = new {
            slot.1 = new;
        }
    }
    Ok(())
}

fn comma_separated(
    value: &str,
    attr: &str,
    tag: &str,
    each: &mut EachUrl<'_>,
) -> Result<Option<String>, String> {
    let urls: Vec<&str> = value
        .split(',')
        .map(|s| s.trim_matches(js_space))
        .filter(|s| !s.is_empty())
        .collect();
    if urls.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(urls.len());
    for u in urls {
        out.push(each(u, attr, tag)?);
    }
    Ok(Some(out.join(", ")))
}

/// `http-equiv-refresh`'s pattern, with JavaScript's `\s`, `.` and `\d`
/// spelled out.
static REFRESH: LazyLock<regex::Regex> = LazyLock::new(|| {
    let s = r"[\t\n\x0B\x0C\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
    let dot = r"[^\n\r\u{2028}\u{2029}]";
    let pattern = format!(
        r#"^{s}*([0-9]+)(?:{s}*;(?:{s}*[uU][rR][lL]{s}*=)?{s}*(?:["']{s}*({dot}*?){s}*['"]|({dot}*?)))?{s}*$"#
    );
    regex::Regex::new(&pattern).expect("valid pattern")
});

fn meta_refresh(
    value: &str,
    attr: &str,
    tag: &str,
    each: &mut EachUrl<'_>,
) -> Result<Option<String>, String> {
    let Some(caps) = REFRESH.captures(value) else {
        return Ok(None);
    };
    let digits = caps.get(1).map_or("", |m| m.as_str());
    let timeout = brrrty_js::js_number_to_string(digits.parse::<f64>().unwrap_or(f64::NAN));
    let url = [caps.get(2), caps.get(3)]
        .into_iter()
        .flatten()
        .map(|m| m.as_str())
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let new = each(url, attr, tag)?;
    Ok(Some(format!("{timeout}; url={new}")))
}

/// One candidate of a `srcset`, as `parse-srcset` gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub d: Option<f64>,
}

fn srcset(
    value: &str,
    attr: &str,
    tag: &str,
    each: &mut EachUrl<'_>,
) -> Result<Option<String>, String> {
    let candidates = parse_srcset(value);
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(candidates.len());
    for c in candidates {
        let url = each(&c.url, attr, tag)?;
        let n = brrrty_js::js_number_to_string;
        let w = c.w.map(|w| format!(" {}w", n(w))).unwrap_or_default();
        let h = c.h.map(|h| format!(" {}h", n(h))).unwrap_or_default();
        let d = c.d.map(|d| format!(" {}x", n(d))).unwrap_or_default();
        out.push(format!("{url}{w}{h}{d}"));
    }
    Ok(Some(out.join(", ")))
}

fn srcset_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r')
}

/// `/^-?(?:[0-9]+|[0-9]*\.[0-9]+)(?:[eE][+-]?[0-9]+)?$/`
fn is_float(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        None => digits(mantissa),
        Some((a, b)) => (a.is_empty() || digits(a)) && digits(b),
    };
    let exp_ok = match exp {
        None => true,
        Some(e) => digits(e.strip_prefix(['+', '-']).unwrap_or(e)),
    };
    mantissa_ok && exp_ok
}

fn is_non_negative_integer(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `parseFloat` of a string `is_float` accepted.
fn parse_float(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(f64::NAN)
}

/// `parse-srcset` 1.0.2, which follows the HTML standard's algorithm.
/// A candidate with a bad descriptor is left out.
pub fn parse_srcset(input: &str) -> Vec<Candidate> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut pos = 0;
    let mut candidates = Vec::new();
    loop {
        while pos < len && (chars[pos] == ',' || srcset_space(chars[pos])) {
            pos += 1;
        }
        if pos >= len {
            return candidates;
        }
        let start = pos;
        while pos < len && !srcset_space(chars[pos]) {
            pos += 1;
        }
        let mut url: String = chars[start..pos].iter().collect();
        let mut descriptors: Vec<String> = Vec::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',').to_string();
        } else {
            // Tokenize descriptors.
            while pos < len && srcset_space(chars[pos]) {
                pos += 1;
            }
            let mut current = String::new();
            #[derive(PartialEq)]
            enum St {
                In,
                Parens,
                After,
            }
            let mut state = St::In;
            loop {
                let c = chars.get(pos).copied();
                match state {
                    St::In => match c {
                        Some(c) if srcset_space(c) => {
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                                state = St::After;
                            }
                        }
                        Some(',') => {
                            pos += 1;
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                            }
                            break;
                        }
                        Some('(') => {
                            current.push('(');
                            state = St::Parens;
                        }
                        None => {
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                            }
                            break;
                        }
                        Some(c) => current.push(c),
                    },
                    St::Parens => match c {
                        Some(')') => {
                            current.push(')');
                            state = St::In;
                        }
                        None => {
                            descriptors.push(std::mem::take(&mut current));
                            break;
                        }
                        Some(c) => current.push(c),
                    },
                    St::After => match c {
                        Some(c) if srcset_space(c) => {}
                        None => break,
                        Some(_) => {
                            state = St::In;
                            pos -= 1;
                        }
                    },
                }
                pos += 1;
            }
        }
        // Parse descriptors.
        let (mut w, mut d, mut h): (Option<f64>, Option<f64>, Option<f64>) = (None, None, None);
        let mut error = false;
        let truthy = |v: Option<f64>| v.is_some_and(|n| n != 0.0 && !n.is_nan());
        for desc in &descriptors {
            let last = desc.chars().last();
            let value: String = {
                let mut it = desc.chars();
                it.next_back();
                it.collect()
            };
            if is_non_negative_integer(&value) && last == Some('w') {
                if truthy(w) || truthy(d) {
                    error = true;
                }
                let n = parse_float(&value);
                if n == 0.0 {
                    error = true;
                } else {
                    w = Some(n);
                }
            } else if is_float(&value) && last == Some('x') {
                if truthy(w) || truthy(d) || truthy(h) {
                    error = true;
                }
                let n = parse_float(&value);
                if n < 0.0 {
                    error = true;
                } else {
                    d = Some(n);
                }
            } else if is_non_negative_integer(&value) && last == Some('h') {
                if truthy(h) || truthy(d) {
                    error = true;
                }
                let n = parse_float(&value);
                if n == 0.0 {
                    error = true;
                } else {
                    h = Some(n);
                }
            } else {
                error = true;
            }
        }
        if !error {
            candidates.push(Candidate {
                url,
                w: w.filter(|n| *n != 0.0),
                h: h.filter(|n| *n != 0.0),
                d: d.filter(|n| *n != 0.0 && !n.is_nan()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(url: &str, w: Option<f64>, h: Option<f64>, d: Option<f64>) -> Candidate {
        Candidate {
            url: url.into(),
            w,
            h,
            d,
        }
    }

    #[test]
    fn srcset_like_parse_srcset() {
        assert_eq!(
            parse_srcset("a.png 1x, b.png 2x"),
            vec![
                c("a.png", None, None, Some(1.0)),
                c("b.png", None, None, Some(2.0))
            ]
        );
        assert_eq!(
            parse_srcset("a.png 100w 50h,b.png"),
            vec![
                c("a.png", Some(100.0), Some(50.0), None),
                c("b.png", None, None, None)
            ]
        );
        assert_eq!(parse_srcset("a.png 1x 2x"), vec![]);
        assert_eq!(
            parse_srcset("a.png,, b.png"),
            vec![c("a.png", None, None, None), c("b.png", None, None, None)]
        );
        assert_eq!(parse_srcset("  "), vec![]);
    }

    #[test]
    fn floats() {
        for ok in ["1", "1.5", ".5", "-1", "1e3", "1.5E-2"] {
            assert!(is_float(ok), "{ok}");
        }
        for bad in ["", "1.", "+1", "e3", "1e", "1.2.3", "--1"] {
            assert!(!is_float(bad), "{bad}");
        }
    }
}
