//! `parse_csv(text, options)`: csv-parse 7.0.3's `parse` from `csv-parse/sync`.
//!
//! A byte-level port of csv-parse's parser, for the whole input at once (the
//! sync API hands it one buffer and `end`), with the options a data file
//! typically passes. Every quirk is csv-parse's, pinned by
//! `tests/csv_probes.rs` against Node:
//!
//! - The record delimiter is found from the first line ending (`\r\n`, `\n` or
//!   `\r`); after that, only that one ends a record, so another stays in the
//!   field (unless `rtrim` trims it).
//! - Trimming is by byte sequence: with `ltrim`, a BOM or any JavaScript
//!   whitespace at the start of a field is dropped, so with `trim` and
//!   `skip_empty_lines` a line of spaces is an empty line. After a closing
//!   quote, `rtrim` takes only whitespace whose first byte starts one, so a
//!   no-break space there is an error, as in csv-parse.
//! - A closing quote may be followed by a NUL byte, which is kept.
//! - Errors are csv-parse's messages, word for word.
//!
//! Options csv-parse has and this port doesn't are refused by name, as is
//! any key csv-parse doesn't know (it would ignore a typo).

use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;

/// Most records one parse may give.
pub const MAX_RECORDS: usize = crate::api::MAX_ARRAY_SIZE;

/// The options this port supports, normalized as csv-parse normalizes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvOptions {
    pub columns: bool,
    pub delimiter: Vec<u8>,
    pub bom: bool,
    pub ltrim: bool,
    pub rtrim: bool,
    pub skip_empty_lines: bool,
    pub relax_column_count: bool,
    pub relax_column_count_less: bool,
    pub relax_column_count_more: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        CsvOptions {
            columns: false,
            delimiter: b",".to_vec(),
            bom: false,
            ltrim: false,
            rtrim: false,
            skip_empty_lines: false,
            relax_column_count: false,
            relax_column_count_less: false,
            relax_column_count_more: false,
        }
    }
}

/// Every option csv-parse 7.0.3 reads, in its underscored form.
const KNOWN: &[&str] = &[
    "bom",
    "cast",
    "cast_date",
    "columns",
    "group_columns_by_name",
    "comment",
    "comment_no_infix",
    "delimiter",
    "delimiter_auto",
    "encoding",
    "escape",
    "from",
    "from_line",
    "ignore_last_delimiters",
    "info",
    "ltrim",
    "max_record_size",
    "objname",
    "on_record",
    "on_skip",
    "quote",
    "raw",
    "record_delimiter",
    "relax_column_count",
    "relax_column_count_less",
    "relax_column_count_more",
    "relax_quotes",
    "rtrim",
    "skip_empty_lines",
    "skip_records_with_empty_values",
    "skip_records_with_error",
    "to",
    "to_line",
    "trim",
];

/// csv-parse's `underscore`: `skipEmptyLines` is `skip_empty_lines`.
fn underscore(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A boolean option as csv-parse checks it: `undefined` and `null` are its
/// default. `Err` is csv-parse's message.
fn boolean(key: &str, v: &Value) -> Result<Option<bool>, String> {
    match v {
        Value::Undefined | Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        other => Err(format!(
            "Invalid Option: {key} must be a boolean, got {}",
            json(other)
        )),
    }
}

fn json(v: &Value) -> String {
    brrrty_liquid::json_stringify(v).unwrap_or_else(|| "undefined".into())
}

/// Read the options a script passed, as csv-parse's `normalize_options`
/// would, for the options this port has.
pub fn options(given: &[(String, Value)]) -> Result<CsvOptions, String> {
    let mut o = CsvOptions::default();
    let mut trim = None;
    let mut ltrim = None;
    let mut rtrim = None;
    let mut seen: Vec<(String, &str)> = Vec::new();
    for (raw_key, v) in given {
        let key = underscore(raw_key);
        // csv-parse takes the one written last; a Rhai map has no such order.
        if let Some((_, first)) = seen.iter().find(|(k, _)| *k == key) {
            return Err(format!(
                "`{first}` and `{raw_key}` are the same csv-parse option; give it once (csv-parse would take whichever was written last, and a Rhai map doesn't keep that order)"
            ));
        }
        seen.push((key.clone(), raw_key.as_str()));
        match key.as_str() {
            "columns" => match v {
                Value::Bool(b) => o.columns = *b,
                Value::Undefined | Value::Null => o.columns = false,
                Value::Array(_) => {
                    return Err(
                        "Brrrty doesn't support csv-parse's `columns` as a list yet; use `columns: true` with a header line"
                            .into(),
                    );
                }
                other => {
                    return Err(format!(
                        "Invalid option columns: expect an array, a function or true, got {}",
                        json(other)
                    ));
                }
            },
            "delimiter" => match v {
                Value::Undefined | Value::Null | Value::Bool(false) => o.delimiter = b",".to_vec(),
                Value::String(s) if !s.is_empty() => o.delimiter = s.as_bytes().to_vec(),
                Value::Array(_) => {
                    return Err(
                        "Brrrty doesn't support csv-parse's `delimiter` as a list yet; give one string"
                            .into(),
                    );
                }
                other => {
                    return Err(format!(
                        "Invalid option delimiter: delimiter must be a non empty string or buffer or array of string|buffer, got {}",
                        json(other)
                    ));
                }
            },
            "bom" => match v {
                Value::Undefined | Value::Null | Value::Bool(false) => o.bom = false,
                Value::Bool(true) => o.bom = true,
                other => {
                    return Err(format!(
                        "Invalid option bom: bom must be true, got {}",
                        json(other)
                    ));
                }
            },
            "trim" | "ltrim" | "rtrim" => {
                let b = match v {
                    Value::Undefined | Value::Null => None,
                    Value::Bool(b) => Some(*b),
                    other => {
                        return Err(format!(
                            "Invalid Option: {key} must be a boolean, got {}",
                            json(other)
                        ));
                    }
                };
                match key.as_str() {
                    "trim" => trim = b,
                    "ltrim" => ltrim = b,
                    _ => rtrim = b,
                }
            }
            "skip_empty_lines" => o.skip_empty_lines = boolean(&key, v)?.unwrap_or(false),
            "relax_column_count" => o.relax_column_count = boolean(&key, v)?.unwrap_or(false),
            "relax_column_count_less" => {
                o.relax_column_count_less = boolean(&key, v)?.unwrap_or(false)
            }
            "relax_column_count_more" => {
                o.relax_column_count_more = boolean(&key, v)?.unwrap_or(false)
            }
            k if KNOWN.contains(&k) => {
                return Err(format!(
                    "Brrrty doesn't support csv-parse's `{raw_key}` option yet; it supports columns, delimiter, bom, trim, ltrim, rtrim, skip_empty_lines and the relax_column_count options"
                ));
            }
            _ => {
                return Err(format!(
                    "`{raw_key}` isn't a csv-parse option. csv-parse would ignore it; Brrrty refuses it so a misspelled option can't go unnoticed"
                ));
            }
        }
    }
    // `trim: true` turns on both sides unless one is set to `false`.
    let trim = trim == Some(true);
    o.ltrim = if trim {
        ltrim != Some(false)
    } else {
        ltrim == Some(true)
    };
    o.rtrim = if trim {
        rtrim != Some(false)
    } else {
        rtrim == Some(true)
    };
    Ok(o)
}

/// What a parse gives: arrays of fields, or objects with `columns`.
pub type Records = Vec<Value>;

const CR: u8 = b'\r';
const NL: u8 = b'\n';
const QUOTE: u8 = b'"';
const BOM: &[u8] = b"\xef\xbb\xbf";

/// ECMAScript white space and line terminators, as UTF-8: csv-parse's
/// `timchars`, in its order.
const TRIM_CHARS: &[&str] = &[
    "\u{20}", "\u{9}", "\u{a}", "\u{d}", "\u{c}", "\u{b}", "\u{a0}", "\u{1680}", "\u{2000}",
    "\u{2001}", "\u{2002}", "\u{2003}", "\u{2004}", "\u{2005}", "\u{2006}", "\u{2007}", "\u{2008}",
    "\u{2009}", "\u{200a}", "\u{2028}", "\u{2029}", "\u{202f}", "\u{205f}", "\u{3000}", "\u{feff}",
];

struct Parser<'a> {
    o: &'a CsvOptions,
    buf: &'a [u8],
    record_delimiter: Option<&'static [u8]>,
    quoting: bool,
    was_quoting: bool,
    escaping: bool,
    was_row_delimiter: bool,
    field: Vec<u8>,
    record: Vec<String>,
    lines: usize,
    records: usize,
    /// The header's names once read, with `columns: true`.
    columns: Option<Vec<String>>,
    expected: Option<usize>,
    out: Records,
}

impl Parser<'_> {
    /// How many bytes of trimmable character start at `pos`, or 0.
    fn trimmable(&self, pos: usize) -> usize {
        let rest = &self.buf[pos.min(self.buf.len())..];
        TRIM_CHARS
            .iter()
            .find(|t| rest.starts_with(t.as_bytes()))
            .map_or(0, |t| t.len())
    }

    fn delimiter_at(&self, pos: usize) -> usize {
        let d = &self.o.delimiter;
        if self.buf[pos.min(self.buf.len())..].starts_with(d) {
            d.len()
        } else {
            0
        }
    }

    fn record_delimiter_at(&self, pos: usize) -> usize {
        match self.record_delimiter {
            Some(rd) if self.buf[pos.min(self.buf.len())..].starts_with(rd) => rd.len(),
            _ => 0,
        }
    }

    /// csv-parse's `__autoDiscoverRecordDelimiter`: the first line ending
    /// seen becomes the record delimiter.
    fn discover(&mut self, pos: usize) -> usize {
        let rest = &self.buf[pos.min(self.buf.len())..];
        for rd in [&b"\r\n"[..], b"\n", b"\r"] {
            if rest.starts_with(rd) {
                self.record_delimiter = Some(rd);
                return rd.len();
            }
        }
        0
    }

    /// `info.column` in an error: the column's name once the header is
    /// read (`null` past its end), else the field's position.
    fn column(&self) -> String {
        match &self.columns {
            Some(cols) => match cols.get(self.record.len()) {
                Some(name) => json(&Value::String(name.as_str().into())),
                None => "null".into(),
            },
            None => self.record.len().to_string(),
        }
    }

    fn on_field(&mut self) {
        let mut field = String::from_utf8_lossy(&self.field).into_owned();
        if self.o.rtrim && !self.was_quoting {
            let kept = field.trim_end_matches(brrrty_js::is_js_whitespace).len();
            field.truncate(kept);
        }
        self.record.push(field);
        self.field.clear();
        self.was_quoting = false;
    }

    fn on_record(&mut self) -> Result<(), String> {
        let record = std::mem::take(&mut self.record);
        if self.o.columns && self.columns.is_none() {
            self.expected = Some(record.len());
            self.columns = Some(record);
            return Ok(());
        }
        if !self.o.columns && self.records == 0 {
            self.expected = Some(record.len());
        }
        let expected = self.expected.unwrap_or(0);
        let len = record.len();
        if len != expected {
            let relaxed = self.o.relax_column_count
                || (self.o.relax_column_count_less && len < expected)
                || (self.o.relax_column_count_more && len > expected);
            if !relaxed {
                return Err(match &self.columns {
                    None => format!(
                        "Invalid Record Length: expect {expected}, got {len} on line {}",
                        self.lines
                    ),
                    Some(cols) => format!(
                        "Invalid Record Length: columns length is {}, got {len} on line {}",
                        cols.len(),
                        self.lines
                    ),
                });
            }
        }
        self.records += 1;
        if self.out.len() >= MAX_RECORDS {
            return Err(format!(
                "Brrrty stops at {MAX_RECORDS} records; this CSV has more"
            ));
        }
        let value = match &self.columns {
            Some(cols) => {
                let mut obj: IndexMap<String, Value> = IndexMap::new();
                for (name, field) in cols.iter().zip(record) {
                    // `Object.defineProperty`: a repeated name keeps its
                    // first place and takes the last value.
                    obj.insert(name.clone(), Value::String(field.into()));
                }
                Value::Object(Arc::new(obj))
            }
            None => Value::Array(Arc::new(
                record
                    .into_iter()
                    .map(|f| Value::String(f.into()))
                    .collect(),
            )),
        };
        self.out.push(value);
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        let len = self.buf.len();
        let mut pos = 0;
        while pos < len {
            if self.was_row_delimiter {
                self.lines += 1;
                self.was_row_delimiter = false;
            }
            if !self.quoting && self.record_delimiter.is_none() {
                self.discover(pos);
            }
            let chr = self.buf[pos];
            if (chr == CR || chr == NL) && !self.was_row_delimiter {
                self.was_row_delimiter = true;
            }
            if self.escaping {
                self.escaping = false;
            } else {
                // The escape is the quote: `""` inside quotes is one quote.
                if self.quoting && chr == QUOTE && pos + 1 < len && self.buf[pos + 1] == QUOTE {
                    self.escaping = true;
                    pos += 1;
                    continue;
                }
                if chr == QUOTE {
                    if self.quoting {
                        let next = self.buf.get(pos + 1).copied();
                        let next_trimmable = self.o.rtrim && self.trimmable(pos + 1) > 0;
                        let next_delimiter = self.delimiter_at(pos + 1) > 0;
                        let next_record_delimiter = if self.record_delimiter.is_none() {
                            self.discover(pos + 1) > 0
                        } else {
                            self.record_delimiter_at(pos + 1) > 0
                        };
                        // `!nextChr`: the end, or a NUL byte.
                        if matches!(next, None | Some(0))
                            || next_delimiter
                            || next_record_delimiter
                            || next_trimmable
                        {
                            self.quoting = false;
                            self.was_quoting = true;
                            pos += 1;
                            continue;
                        }
                        let got = char::from(next.unwrap_or(0));
                        return Err(format!(
                            "Invalid Closing Quote: got \"{got}\" at line {} instead of delimiter, record delimiter, trimable character (if activated) or comment",
                            self.lines
                        ));
                    } else if !self.field.is_empty() {
                        let value = String::from_utf8_lossy(&self.field).into_owned();
                        let bom = if self.field == BOM { " (utf8 bom)" } else { "" };
                        return Err(format!(
                            "Invalid Opening Quote: a quote is found on field {} at line {}, value is {}{bom}",
                            self.column(),
                            self.lines,
                            json(&Value::String(value.into()))
                        ));
                    } else {
                        self.quoting = true;
                        pos += 1;
                        continue;
                    }
                }
                if !self.quoting {
                    let rdl = self.record_delimiter_at(pos);
                    if rdl != 0 {
                        if self.o.skip_empty_lines
                            && !self.was_quoting
                            && self.record.is_empty()
                            && self.field.is_empty()
                        {
                            pos += rdl;
                            continue;
                        }
                        self.on_field();
                        self.on_record()?;
                        pos += rdl;
                        continue;
                    }
                    let dl = self.delimiter_at(pos);
                    if dl != 0 {
                        self.on_field();
                        pos += dl;
                        continue;
                    }
                }
            }
            let trimmable = self.trimmable(pos);
            let lappend = !self.o.ltrim || self.quoting || !self.field.is_empty() || trimmable == 0;
            let rappend = !self.o.rtrim || !self.was_quoting;
            if lappend && rappend {
                self.field.push(chr);
            } else if self.o.rtrim && trimmable == 0 {
                return Err(format!(
                    "Invalid Closing Quote: found non trimable byte after quote at line {}",
                    self.lines
                ));
            } else if !lappend {
                pos += trimmable;
                continue;
            }
            pos += 1;
        }
        if self.quoting {
            return Err(format!(
                "Quote Not Closed: the parsing is finished with an opening quote at line {}",
                self.lines
            ));
        }
        if self.was_quoting || !self.record.is_empty() || !self.field.is_empty() {
            self.on_field();
            self.on_record()?;
        }
        Ok(())
    }
}

/// `parse(text, options)`. `Err` is csv-parse's message.
pub fn parse(text: &str, o: &CsvOptions) -> Result<Records, String> {
    let mut buf = text.as_bytes();
    if o.bom && buf.starts_with(BOM) {
        buf = &buf[BOM.len()..];
    }
    let mut p = Parser {
        o,
        buf,
        record_delimiter: None,
        quoting: false,
        was_quoting: false,
        escaping: false,
        was_row_delimiter: false,
        field: Vec::new(),
        record: Vec::new(),
        lines: 1,
        records: 0,
        columns: None,
        expected: None,
        out: Vec::new(),
    };
    p.run()?;
    Ok(p.out)
}
