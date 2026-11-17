//! linkify-it 6.1.0, as markdown-it 15 sets it up (`new LinkifyIt()`: the default schemas
//! `http:`, `https:`, `ftp:`, `//` and `mailto:`, the default TLDs, `fuzzyEmail` on, `fuzzyIP`
//! off), with `fuzzyLink` a choice (`md.linkify.set({ fuzzyLink })`).
//!
//! The patterns are linkify-it's own (`linkify_re`, dumped from Node), run through
//! `brrrty_jsregex::backtrack`, which matches as JavaScript does (UTF-16 units) or refuses. `test`,
//! `match` and `matchAtStart` are ported line by line. Positions are UTF-16 units.
//!
//! Two of linkify-it's repetitions are bounded: `{0,1000}` in a bracketed pair of a path and
//! `{1,10000}` (`maxLength`) for a whole path. Neither can match a space or a control character
//! (uc.micro `Z` or `Cc`), so in a text with no run of 1,000 units without one they can't reach
//! their bound, and they run here unbounded (`*`, `+`), which the regex engine can compile. A
//! text with a longer run is refused instead of guessed at.

use std::sync::OnceLock;

use brrrty_jsregex::backtrack::{JsRegex, JsText, translate};

use crate::linkify_re as re;
use crate::uc_micro;

/// linkify-it's `maxLength` (characters a schema's validator reads past the schema).
const MAX_LENGTH: usize = 10_000;

/// The longest run of units with no space or control character linkify can read exactly.
const MAX_RUN: usize = 1_000;

/// Compiles one of linkify-it's patterns. `g` is how it is run (from a position) and `y` is
/// matched as `^(?:...)` on the text from that position (the patterns have no lookbehind).
fn compile((source, flags): (&str, &str)) -> JsRegex {
    let source = source.replace("{0,1000}", "*").replace("{1,10000}", "+");
    let sticky = flags.contains('y');
    let flags: String = flags.chars().filter(|&c| c != 'g' && c != 'y').collect();
    let source = if sticky {
        format!("^(?:{source})")
    } else {
        source
    };
    translate(&source, &flags)
        .unwrap_or_else(|e| panic!("linkify-it's pattern translates: {}", e.0))
}

/// One of linkify-it's patterns, compiled on first use (each costs tens of milliseconds, and
/// `fuzzy_link_search` isn't needed with `fuzzyLink` off).
struct Pattern {
    source: (&'static str, &'static str),
    re: OnceLock<JsRegex>,
}

impl Pattern {
    const fn new(source: (&'static str, &'static str)) -> Self {
        Pattern {
            source,
            re: OnceLock::new(),
        }
    }

    fn get(&self) -> &JsRegex {
        self.re.get_or_init(|| compile(self.source))
    }
}

static SCHEMA_SEARCH: Pattern = Pattern::new(re::SCHEMA_SEARCH);
static SCHEMA_AT_START: Pattern = Pattern::new(re::SCHEMA_AT_START);
static FUZZY_LINK_SEARCH: Pattern = Pattern::new(re::FUZZY_LINK_SEARCH);
static FUZZY_MAIL_HOST_SEARCH: Pattern = Pattern::new(re::FUZZY_MAIL_HOST_SEARCH);
static MAIL_NAME_VALIDATOR: Pattern = Pattern::new(re::MAIL_NAME_VALIDATOR);
static HTTP_VALIDATOR: Pattern = Pattern::new(re::HTTP_VALIDATOR);
static RELATIVE_PROTO_VALIDATOR: Pattern = Pattern::new(re::RELATIVE_PROTO_VALIDATOR);
static MAILTO_VALIDATOR: Pattern = Pattern::new(re::MAILTO_VALIDATOR);

fn in_table(table: &[(u16, u16)], u: u16) -> bool {
    let i = table.partition_point(|&(_, end)| end < u);
    table.get(i).is_some_and(|&(start, _)| start <= u)
}

/// uc.micro `Z` or `Cc`.
pub(crate) fn is_space_or_control(u: u16) -> bool {
    in_table(uc_micro::SPACE_OR_CONTROL, u)
}

/// A match: `exec`'s index, end and groups, in units.
struct Found {
    index: usize,
    end: usize,
    groups: Vec<Option<(usize, usize)>>,
}

fn exec(re: &JsRegex, text: &JsText, last: usize) -> Result<Option<Found>, String> {
    re.exec(text, last)
        .map(|m| {
            m.map(|m| Found {
                index: m.index,
                end: m.end,
                groups: m.groups,
            })
        })
        .map_err(|e| format!("linkify: {}", e.0))
}

/// A link found in a text: linkify-it's `Match`, after its schema's `normalize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    /// The schema, lowercased; empty for a fuzzy link.
    pub schema: String,
    /// Where it starts and ends, in UTF-16 units.
    pub index: usize,
    pub last_index: usize,
    /// The text as written.
    pub text: String,
    /// The URL: the text, with `http://` or `mailto:` in front when it had no schema.
    pub url: String,
}

/// A linkifier: linkify-it with `fuzzyLink` as given.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Linkify {
    pub fuzzy_link: bool,
}

/// Which candidate `match` took.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Schema,
    Email,
    Link,
}

#[derive(Clone, Copy)]
struct Candidate {
    schema: &'static str,
    index: usize,
    last_index: usize,
}

fn units_to_string(units: &[u16]) -> Result<String, String> {
    String::from_utf16(units)
        .map_err(|_| "linkify: a link would split a character outside the BMP in two".to_string())
}

/// The schema names linkify-it knows (`__schemas__`), and the one `name` stands for.
fn schema_name(name: &str) -> Option<&'static str> {
    ["http:", "https:", "ftp:", "//", "mailto:"]
        .into_iter()
        .find(|s| s.eq_ignore_ascii_case(name))
}

impl Linkify {
    /// Refuses a text linkify can't be sure to read as linkify-it does (see the module docs).
    fn check(units: &[u16]) -> Result<(), String> {
        let mut run = 0;
        for &u in units {
            if is_space_or_control(u) {
                run = 0;
            } else {
                run += 1;
                if run >= MAX_RUN {
                    return Err(format!(
                        "linkify: a run of {MAX_RUN} or more characters with no space is more than Brrrty's linkify reads exactly"
                    ));
                }
            }
        }
        Ok(())
    }

    /// `testSchemaAt(text, schema, pos)`: the length of the link after the schema, or 0.
    fn test_schema_at(text: &[u16], schema: &str, pos: usize) -> Result<usize, String> {
        let Some(schema) = schema_name(schema) else {
            return Ok(0);
        };
        let end = text.len().min(pos + MAX_LENGTH);
        let tail = JsText::from_units(&text[pos.min(end)..end]);
        let validator = match schema {
            "http:" | "https:" | "ftp:" => HTTP_VALIDATOR.get(),
            "//" => RELATIVE_PROTO_VALIDATOR.get(),
            _ => MAILTO_VALIDATOR.get(),
        };
        let Some(m) = exec(validator, &tail, 0)? else {
            return Ok(0);
        };
        if schema == "//"
            && pos >= 3
            && (text[pos - 3] == u16::from(b':') || text[pos - 3] == u16::from(b'/'))
        {
            return Ok(0);
        }
        Ok(m.end - m.index)
    }

    /// `test(text)`.
    pub fn test(&self, text: &[u16]) -> Result<bool, String> {
        if text.is_empty() {
            return Ok(false);
        }
        Self::check(text)?;
        let js = JsText::from_units(text);
        let mut last = 0;
        while let Some(m) = exec(SCHEMA_SEARCH.get(), &js, last)? {
            last = m.end;
            let (a, b) = m.groups[1].expect("group 2 is always set");
            let schema = units_to_string(&text[a..b])?;
            if Self::test_schema_at(text, &schema, m.end)? > 0 {
                return Ok(true);
            }
        }
        if self.fuzzy_link && exec(FUZZY_LINK_SEARCH.get(), &js, 0)?.is_some() {
            return Ok(true);
        }
        if text.contains(&u16::from(b'@')) {
            let mut last = 0;
            while let Some(m) = exec(FUZZY_MAIL_HOST_SEARCH.get(), &js, last)? {
                last = m.end;
                let name = JsText::from_units(&text[m.index.saturating_sub(65)..m.index]);
                if exec(MAIL_NAME_VALIDATOR.get(), &name, 0)?.is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn to_match(
        text: &[u16],
        schema: &str,
        index: usize,
        last_index: usize,
    ) -> Result<Match, String> {
        let raw = units_to_string(&text[index..last_index])?;
        let schema = schema.to_ascii_lowercase();
        // linkify-it's `normalize`.
        let url = if schema.is_empty() {
            format!("http://{raw}")
        } else if schema == "mailto:"
            && !raw
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("mailto:"))
        {
            format!("mailto:{raw}")
        } else {
            raw.clone()
        };
        Ok(Match {
            schema,
            index,
            last_index,
            text: raw,
            url,
        })
    }

    /// `match(text)`: every link, in order. Empty where linkify-it returns `null`.
    pub fn find_all(&self, text: &[u16]) -> Result<Vec<Match>, String> {
        let mut result = Vec::new();
        if text.is_empty() {
            return Ok(result);
        }
        Self::check(text)?;
        let js = JsText::from_units(text);
        let mut schema_last = 0;
        let mut link_last = 0;
        let mut mail_last = 0;
        let mut link_candidate: Option<Candidate> = None;
        let mut email_candidate: Option<Candidate> = None;
        let mut schema_prefix: Option<(String, usize, usize)> = None;
        let (mut schema_done, mut link_done, mut email_done) = (false, !self.fuzzy_link, false);
        let mut pos: usize = 0;
        let before = |a: &Candidate, b: &Candidate| {
            a.index < b.index || (a.index == b.index && a.last_index > b.last_index)
        };
        loop {
            let scan_from = pos.saturating_sub(1);
            if !email_done && email_candidate.is_none_or(|c| c.index < pos) {
                mail_last = mail_last.max(scan_from);
                loop {
                    let Some(m) = exec(FUZZY_MAIL_HOST_SEARCH.get(), &js, mail_last)? else {
                        email_done = true;
                        email_candidate = None;
                        break;
                    };
                    mail_last = m.end;
                    let start = m.index.saturating_sub(65);
                    let name = JsText::from_units(&text[start..m.index]);
                    let Some(n) = exec(MAIL_NAME_VALIDATOR.get(), &name, 0)? else {
                        continue;
                    };
                    let (a, b) = n.groups[0].expect("group 1 is always set");
                    let c = Candidate {
                        schema: "mailto:",
                        index: m.index - (b - a),
                        last_index: m.end,
                    };
                    email_candidate = Some(c);
                    if c.index >= pos {
                        break;
                    }
                    mail_last = mail_last.max(scan_from);
                }
            }
            if !link_done && link_candidate.is_none_or(|c| c.index < pos) {
                link_last = link_last.max(scan_from);
                loop {
                    let Some(m) = exec(FUZZY_LINK_SEARCH.get(), &js, link_last)? else {
                        link_done = true;
                        link_candidate = None;
                        break;
                    };
                    link_last = m.end;
                    let (a, b) = m.groups[0].expect("group 1 is always set");
                    let c = Candidate {
                        schema: "",
                        index: m.index + (b - a),
                        last_index: m.end,
                    };
                    link_candidate = Some(c);
                    if c.index >= pos {
                        break;
                    }
                    link_last = link_last.max(scan_from);
                }
            }
            let mut fuzzy = email_candidate;
            if let Some(l) = link_candidate
                && fuzzy.is_none_or(|f| before(&l, &f))
            {
                fuzzy = Some(l);
            }
            let mut schema_candidate: Option<(String, usize, usize)> = None;
            if !schema_done {
                loop {
                    if schema_prefix.is_none() {
                        schema_last = schema_last.max(scan_from);
                        let Some(m) = exec(SCHEMA_SEARCH.get(), &js, schema_last)? else {
                            schema_done = true;
                            break;
                        };
                        schema_last = m.end;
                        let (a1, b1) = m.groups[0].expect("group 1 is always set");
                        let (a2, b2) = m.groups[1].expect("group 2 is always set");
                        schema_prefix =
                            Some((units_to_string(&text[a2..b2])?, m.index + (b1 - a1), m.end));
                    }
                    let (schema, index, last_index) = schema_prefix.clone().expect("set above");
                    if index < pos {
                        schema_prefix = None;
                        continue;
                    }
                    if fuzzy.is_some_and(|f| index > f.index) {
                        break;
                    }
                    schema_prefix = None;
                    let len = Self::test_schema_at(text, &schema, last_index)?;
                    if len > 0 {
                        schema_candidate = Some((schema, index, last_index + len));
                        break;
                    }
                }
            }
            let mut candidate: Option<(Source, String, usize, usize)> =
                schema_candidate.map(|(s, i, l)| (Source::Schema, s, i, l));
            for (source, c) in [
                (Source::Email, email_candidate),
                (Source::Link, link_candidate),
            ] {
                let Some(c) = c else { continue };
                let wins = match &candidate {
                    None => true,
                    Some((_, _, index, last_index)) => {
                        c.index < *index || (c.index == *index && c.last_index > *last_index)
                    }
                };
                if wins {
                    candidate = Some((source, c.schema.to_string(), c.index, c.last_index));
                }
            }
            let Some((source, schema, index, last_index)) = candidate else {
                break;
            };
            match source {
                Source::Email => email_candidate = None,
                Source::Link => link_candidate = None,
                Source::Schema => {}
            }
            result.push(Self::to_match(text, &schema, index, last_index)?);
            pos = last_index;
        }
        Ok(result)
    }

    /// `matchAtStart(text)`: a link with a schema at the very start of the text.
    pub fn match_at_start(&self, text: &[u16]) -> Result<Option<Match>, String> {
        if text.is_empty() {
            return Ok(None);
        }
        Self::check(text)?;
        let js = JsText::from_units(text);
        let Some(m) = exec(SCHEMA_AT_START.get(), &js, 0)? else {
            return Ok(None);
        };
        let (a2, b2) = m.groups[1].expect("group 2 is always set");
        let schema = units_to_string(&text[a2..b2])?;
        let len = Self::test_schema_at(text, &schema, m.end)?;
        if len == 0 {
            return Ok(None);
        }
        let (a1, b1) = m.groups[0].expect("group 1 is always set");
        Self::to_match(text, &schema, m.index + (b1 - a1), m.end + len).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn every_pattern_translates() {
        for p in [
            &SCHEMA_SEARCH,
            &SCHEMA_AT_START,
            &FUZZY_LINK_SEARCH,
            &FUZZY_MAIL_HOST_SEARCH,
            &MAIL_NAME_VALIDATOR,
            &HTTP_VALIDATOR,
            &RELATIVE_PROTO_VALIDATOR,
            &MAILTO_VALIDATOR,
        ] {
            let _ = p.get();
        }
    }

    #[test]
    fn finds_links_as_linkify_it_does() {
        let fuzzy = Linkify { fuzzy_link: true };
        let strict = Linkify { fuzzy_link: false };
        let text = units("see http://example.com/a?b=c, foo@bar.com or github.com/x! Ok");
        assert!(strict.test(&text).unwrap());
        let found: Vec<(String, String)> = fuzzy
            .find_all(&text)
            .unwrap()
            .into_iter()
            .map(|m| (m.text, m.url))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "http://example.com/a?b=c".into(),
                    "http://example.com/a?b=c".into()
                ),
                ("foo@bar.com".into(), "mailto:foo@bar.com".into()),
                ("github.com/x".into(), "http://github.com/x".into()),
            ]
        );
        assert_eq!(strict.find_all(&text).unwrap().len(), 2);
        assert!(!strict.test(&units("github.com")).unwrap());
        assert!(fuzzy.test(&units("github.com")).unwrap());
        let at = fuzzy
            .match_at_start(&units("https://x.org/p) y"))
            .unwrap()
            .unwrap();
        assert_eq!(
            (at.index, at.last_index, at.url.as_str()),
            (0, 15, "https://x.org/p")
        );
        let spaced = fuzzy
            .match_at_start(&units(" https://x.org"))
            .unwrap()
            .unwrap();
        assert_eq!((spaced.index, spaced.last_index), (1, 14));
        assert!(
            fuzzy
                .match_at_start(&units("xhttps://x.org"))
                .unwrap()
                .is_none()
        );
    }

    /// Replays a corpus from `tools/gen-linkify-fuzz.cjs` and lists every difference.
    #[test]
    #[ignore = "needs LINKIFY_FUZZ=<corpus.json>"]
    fn linkify_corpus() {
        use serde_json::{Value, json};
        let path = std::env::var("LINKIFY_FUZZ").expect("LINKIFY_FUZZ");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let as_json = |m: &Match| json!([m.schema, m.index, m.last_index, m.text, m.url]);
        let mut bad = 0;
        for c in v.as_array().unwrap() {
            let s = c["s"].as_str().unwrap();
            let u = units(s);
            for (i, l) in [false, true].into_iter().enumerate() {
                let l = Linkify { fuzzy_link: l };
                let test = l.test(&u).map(Value::from);
                let found = l
                    .find_all(&u)
                    .map(|ms| Value::from(ms.iter().map(as_json).collect::<Vec<_>>()));
                for (kind, got, want) in [
                    ("test", test, &c["test"][i]),
                    ("match", found, &c["match"][i]),
                ] {
                    if got.as_ref() != Ok(want) {
                        bad += 1;
                        println!(
                            "{kind} fuzzy={} {s:?}\n  want {want}\n  got  {got:?}",
                            i == 1
                        );
                    }
                }
            }
            let at = Linkify { fuzzy_link: true }
                .match_at_start(&u)
                .map(|m| m.as_ref().map_or(Value::Null, as_json));
            if at.as_ref() != Ok(&c["at_start"]) {
                bad += 1;
                println!("at_start {s:?}\n  want {}\n  got  {at:?}", c["at_start"]);
            }
        }
        println!("{bad} differences");
        assert_eq!(bad, 0);
    }

    #[test]
    fn refuses_a_long_run_without_spaces() {
        let long = "a".repeat(MAX_RUN);
        assert!(Linkify { fuzzy_link: true }.test(&units(&long)).is_err());
        let short = "a".repeat(MAX_RUN - 1);
        assert!(!Linkify { fuzzy_link: true }.test(&units(&short)).unwrap());
    }
}
