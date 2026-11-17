//! Edits `package.json` in place: new members go in with the file's own
//! indentation and line endings, and everything else stays byte for byte.

/// One member of an object: where its key and value are.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub key: String,
    /// The key's opening quote.
    pub key_start: usize,
    /// After the key's closing quote.
    pub key_end: usize,
    pub value_start: usize,
    pub value_end: usize,
}

/// An object: its braces and members.
#[derive(Debug, Clone)]
pub(crate) struct Object {
    pub open: usize,
    pub close: usize,
    pub members: Vec<Member>,
}

impl Object {
    /// The last member named `key`, which is the one `JSON.parse` keeps.
    pub(crate) fn get(&self, key: &str) -> Option<&Member> {
        self.members.iter().rev().find(|m| m.key == key)
    }
}

struct Scan<'a> {
    b: &'a [u8],
    src: &'a str,
    i: usize,
}

impl Scan<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Option<String> {
        let start = self.i;
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return serde_json::from_str(&self.src[start..self.i]).ok();
                }
                _ => self.i += 1,
            }
        }
        None
    }

    fn value(&mut self) -> Option<()> {
        match self.b.get(self.i)? {
            b'"' => self.string().map(|_| ()),
            b'{' => self.object().map(|_| ()),
            b'[' => {
                self.i += 1;
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Some(());
                }
                loop {
                    self.ws();
                    self.value()?;
                    self.ws();
                    match self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            _ => {
                while self.i < self.b.len()
                    && !matches!(
                        self.b[self.i],
                        b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'
                    )
                {
                    self.i += 1;
                }
                Some(())
            }
        }
    }

    fn object(&mut self) -> Option<Object> {
        let open = self.i;
        if self.b.get(self.i) != Some(&b'{') {
            return None;
        }
        self.i += 1;
        let mut members = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Some(Object {
                open,
                close: self.i - 1,
                members,
            });
        }
        loop {
            self.ws();
            let key_start = self.i;
            let key = self.string()?;
            let key_end = self.i;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return None;
            }
            self.i += 1;
            self.ws();
            let value_start = self.i;
            self.value()?;
            members.push(Member {
                key,
                key_start,
                key_end,
                value_start,
                value_end: self.i,
            });
            self.ws();
            match self.b.get(self.i)? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some(Object {
                        open,
                        close: self.i - 1,
                        members,
                    });
                }
                _ => return None,
            }
        }
    }
}

/// The object at `at` in `src` (valid JSON).
pub(crate) fn object_at(src: &str, at: usize) -> Option<Object> {
    let mut s = Scan {
        b: src.as_bytes(),
        src,
        i: at,
    };
    s.ws();
    s.object()
}

/// A replacement of `src[start..end]`.
#[derive(Debug)]
pub(crate) struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Apply `edits`, which don't overlap.
pub(crate) fn apply(src: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
    let mut out = src.to_string();
    for e in edits {
        out.replace_range(e.start..e.end, &e.text);
    }
    out
}

/// The file's line ending and one level of indentation.
#[derive(Debug, Clone)]
pub(crate) struct Style {
    pub newline: &'static str,
    pub unit: String,
    /// The whole file is on one line: new objects go on one line too.
    pub inline: bool,
    /// What goes between a key and its value, like `": "`.
    pub colon: String,
    /// What follows a comma between members on one line.
    pub after_comma: String,
}

impl Style {
    pub(crate) fn of(src: &str, top: &Object) -> Style {
        let newline = if src.contains("\r\n") { "\r\n" } else { "\n" };
        let unit = top
            .members
            .first()
            .map(|m| indent_before(src, m.key_start))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "  ".into());
        let inline = !src[top.open..=top.close].contains('\n');
        let colon = top
            .members
            .first()
            .map(|m| src[m.key_end..m.value_start].to_string())
            .unwrap_or_else(|| ": ".into());
        let after_comma = match top.members.get(..2) {
            Some([a, b]) => gap_after_comma(src, a.value_end, b.key_start),
            _ => {
                if inline {
                    String::new()
                } else {
                    " ".into()
                }
            }
        };
        Style {
            newline,
            unit,
            inline,
            colon,
            after_comma,
        }
    }
}

/// The whitespace after the comma between `end` and `next`.
fn gap_after_comma(src: &str, end: usize, next: usize) -> String {
    let gap = &src[end..next];
    gap.rsplit_once(',').map_or(gap, |(_, w)| w).to_string()
}

/// The spaces and tabs between the line start and `at`, when only those come before it.
fn indent_before(src: &str, at: usize) -> String {
    let line = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let lead = &src[line..at];
    if lead.chars().all(|c| c == ' ' || c == '\t') {
        lead.to_string()
    } else {
        String::new()
    }
}

/// Whether the object's members sit on lines of their own.
fn multiline(src: &str, o: &Object) -> bool {
    src[o.open..=o.close].contains('\n')
}

/// The text that adds `members` (key, raw JSON value) at the end of `o`,
/// which is nested `depth` levels deep, as an edit.
pub(crate) fn append(
    src: &str,
    o: &Object,
    depth: usize,
    style: &Style,
    members: &[(String, String)],
) -> Edit {
    let quoted = |k: &str| serde_json::to_string(k).unwrap_or_default();
    match o.members.last() {
        Some(last) => {
            let between = &src[last.key_end..last.value_start];
            let sep = if multiline(src, o) {
                let indent = indent_before(src, last.key_start);
                let indent = if indent.is_empty() {
                    style.unit.repeat(depth + 1)
                } else {
                    indent
                };
                format!("{}{indent}", style.newline)
            } else if o.members.len() >= 2 {
                let n = o.members.len();
                gap_after_comma(src, o.members[n - 2].value_end, o.members[n - 1].key_start)
            } else {
                style.after_comma.clone()
            };
            let mut text = String::new();
            for (k, v) in members {
                text.push_str(&format!(",{sep}{}{between}{v}", quoted(k)));
            }
            Edit {
                start: last.value_end,
                end: last.value_end,
                text,
            }
        }
        None if style.inline => {
            let body: Vec<String> = members
                .iter()
                .map(|(k, v)| format!("{}{}{v}", quoted(k), style.colon))
                .collect();
            Edit {
                start: o.open + 1,
                end: o.close,
                text: body.join(&format!(",{}", style.after_comma)),
            }
        }
        None => {
            let nl = style.newline;
            let inner = style.unit.repeat(depth + 1);
            let outer = style.unit.repeat(depth);
            let body: Vec<String> = members
                .iter()
                .map(|(k, v)| format!("{inner}{}: {v}", quoted(k)))
                .collect();
            Edit {
                start: o.open + 1,
                end: o.close,
                text: format!("{nl}{}{nl}{outer}", body.join(&format!(",{nl}"))),
            }
        }
    }
}

/// A new object of string members, nested `depth` levels deep, as JSON text.
pub(crate) fn new_object(depth: usize, style: &Style, members: &[(&str, &str)]) -> String {
    let nl = style.newline;
    let inner = style.unit.repeat(depth + 1);
    let outer = style.unit.repeat(depth);
    let pair = |k: &str, v: &str| {
        format!(
            "{}{}{}",
            serde_json::to_string(k).unwrap_or_default(),
            style.colon,
            serde_json::to_string(v).unwrap_or_default()
        )
    };
    if style.inline {
        let body: Vec<String> = members.iter().map(|(k, v)| pair(k, v)).collect();
        return format!("{{{}}}", body.join(&format!(",{}", style.after_comma)));
    }
    let body: Vec<String> = members
        .iter()
        .map(|(k, v)| format!("{inner}{}", pair(k, v)))
        .collect();
    format!("{{{nl}{}{nl}{outer}}}", body.join(&format!(",{nl}")))
}

/// An edit that puts `key: value` (raw JSON) in front of member `m`.
pub(crate) fn insert_before(
    src: &str,
    prev_end: usize,
    m: &Member,
    key: &str,
    value: &str,
) -> Edit {
    let gap = &src[prev_end..m.key_start];
    let ws = gap.rsplit_once(',').map_or(gap, |(_, w)| w);
    let between = &src[m.key_end..m.value_start];
    Edit {
        start: m.key_start,
        end: m.key_start,
        text: format!(
            "{}{between}{value},{ws}",
            serde_json::to_string(key).unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_with_the_file_style() {
        let src = "{\n\t\"name\": \"x\",\n\t\"scripts\": {\n\t\t\"test\": \"t\"\n\t}\n}\n";
        let top = object_at(src, 0).unwrap();
        let style = Style::of(src, &top);
        let scripts = object_at(src, top.get("scripts").unwrap().value_start).unwrap();
        let e = append(
            src,
            &scripts,
            1,
            &style,
            &[("build".into(), "\"b\"".into())],
        );
        assert_eq!(
            apply(src, vec![e]),
            "{\n\t\"name\": \"x\",\n\t\"scripts\": {\n\t\t\"test\": \"t\",\n\t\t\"build\": \"b\"\n\t}\n}\n"
        );
    }

    #[test]
    fn fills_an_empty_object_and_keeps_one_line_objects_on_one_line() {
        let src = "{\"a\": {}, \"b\": {\"x\":1}}";
        let top = object_at(src, 0).unwrap();
        let style = Style::of(src, &top);
        let a = object_at(src, top.get("a").unwrap().value_start).unwrap();
        let b = object_at(src, top.get("b").unwrap().value_start).unwrap();
        let edits = vec![
            append(src, &a, 1, &style, &[("k".into(), "\"v\"".into())]),
            append(src, &b, 1, &style, &[("y".into(), "2".into())]),
        ];
        let out = apply(src, edits);
        assert_eq!(out, "{\"a\": {\"k\": \"v\"}, \"b\": {\"x\":1, \"y\":2}}");
        serde_json::from_str::<serde_json::Value>(&out).unwrap();
    }

    #[test]
    fn one_line_files_stay_on_one_line() {
        let src = "{\"name\":\"x\",\"scripts\":{\"test\":\"t\"}}";
        let top = object_at(src, 0).unwrap();
        let style = Style::of(src, &top);
        let scripts = object_at(src, top.get("scripts").unwrap().value_start).unwrap();
        let edits = vec![
            append(
                src,
                &scripts,
                1,
                &style,
                &[("build".into(), "\"b\"".into())],
            ),
            append(
                src,
                &top,
                0,
                &style,
                &[("dev".into(), new_object(1, &style, &[("k", "v")]))],
            ),
        ];
        assert_eq!(
            apply(src, edits),
            "{\"name\":\"x\",\"scripts\":{\"test\":\"t\",\"build\":\"b\"},\"dev\":{\"k\":\"v\"}}"
        );
    }
}
