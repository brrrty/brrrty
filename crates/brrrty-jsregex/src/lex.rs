//! A lexer for JavaScript regular expression patterns, as Node reads them
//! without the `u` or `v` flag (with the web-compatibility grammar of
//! ECMA-262 Annex B.1.2).
//!
//! Each token says how it was written, so a caller can accept only the
//! forms it supports and name the one it refuses.

/// How a [`Tok::Char`] was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// The character itself.
    Plain,
    /// A backslash and this character: a control escape (`\n`, `\t`, ...),
    /// `\0`, `\b` in a class, or an identity escape such as `\.` or `\a`.
    Escape(char),
    /// `\xHH`.
    Hex,
    /// `\uHHHH`.
    Unicode,
    /// `\cX`, a control character.
    Control,
    /// A backslash before a `c` that doesn't start a control escape: the
    /// backslash is itself, and the `c` is read next.
    BackslashC,
}

/// What a `(` opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// `(`.
    Capture,
    /// `(?:`.
    NonCapture,
    /// `(?<name>`.
    Named(String),
    /// `(?=`, `(?!`, `(?<=` or `(?<!`.
    Look { behind: bool, negate: bool },
    /// `(?` followed by anything else, such as `(?i:`.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    /// One character: a code point when written plainly, else the value
    /// of the escape (a UTF-16 unit for `\uHHHH`).
    Char {
        c: char,
        how: How,
    },
    /// A `\uHHHH` escape of a surrogate (U+D800 to U+DFFF), which isn't a
    /// `char`.
    Surrogate(u16),
    /// `\d`, `\D`, `\s`, `\S`, `\w` or `\W`, by its letter.
    ClassEscape(char),
    /// `\b` (`negate` false) or `\B` outside a class.
    Boundary {
        negate: bool,
    },
    /// `\` and a decimal number not starting with 0, outside a class: a
    /// backreference, or (with fewer groups) a legacy octal escape.
    Decimal(String),
    /// A legacy octal escape: `\0` followed by a digit outside a class, or
    /// `\` and a digit in one. `letter` is the first digit.
    Octal {
        letter: char,
    },
    /// `\k`, with the name of `\k<name>`.
    NamedRef(Option<String>),
    /// `.`.
    Dot,
    /// `^`.
    Start,
    /// `$`.
    End,
    /// `|`.
    Alt,
    Open(Open),
    /// `)`.
    Close,
    /// `*`, `+`, `?` or a braced count, and whether `?` makes it lazy.
    /// `brace` is the text between the braces, for a braced count.
    Quant {
        min: u32,
        max: Option<u32>,
        lazy: bool,
        brace: Option<String>,
        op: char,
    },
    /// `[` or `[^`.
    ClassOpen {
        negated: bool,
    },
    /// An unescaped `-` in a class.
    Dash,
    /// The `]` that closes a class.
    ClassClose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LexError {
    /// A `\` with nothing after it.
    TrailingBackslash,
    /// A `[` with no `]`.
    UnclosedClass,
    /// `(?<` with no `>`.
    GroupName,
}

pub struct Lexer {
    chars: Vec<char>,
    i: usize,
    in_class: bool,
    /// Just after `[` or `[^`.
    class_start: bool,
}

fn hex_value(chars: &[char], at: usize, n: usize) -> Option<u32> {
    if chars.len() < at + n || !chars[at..at + n].iter().all(char::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(&chars[at..at + n].iter().collect::<String>(), 16).ok()
}

/// A braced count at `at` (just past `{`): `n`, `n,` or `n,m`, and the
/// length of the text through `}`.
fn brace(chars: &[char], at: usize) -> Option<(u32, Option<u32>, String, usize)> {
    let close = chars[at..].iter().position(|&c| c == '}')?;
    let body: String = chars[at..at + close].iter().collect();
    let mut parts = body.splitn(2, ',');
    let lo = parts.next().unwrap_or("");
    let hi = parts.next();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(lo) || hi.is_some_and(|h| !h.is_empty() && !digits(h)) {
        return None;
    }
    let num = |s: &str| s.parse::<u32>().unwrap_or(u32::MAX);
    let max = match hi {
        None => Some(num(lo)),
        Some("") => None,
        Some(h) => Some(num(h)),
    };
    Some((num(lo), max, body, close + 1))
}

impl Lexer {
    pub fn new(pattern: &str) -> Self {
        Self {
            chars: pattern.chars().collect(),
            i: 0,
            in_class: false,
            class_start: false,
        }
    }

    /// The pattern's characters.
    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    /// The index of the next character to read.
    pub fn pos(&self) -> usize {
        self.i
    }

    pub fn in_class(&self) -> bool {
        self.in_class
    }

    /// Whether the next token is the first in a class.
    pub fn at_class_start(&self) -> bool {
        self.class_start
    }

    /// The next token, without reading it.
    pub fn peek(&self) -> Result<Option<Tok>, LexError> {
        let mut copy = Self {
            chars: Vec::new(),
            i: self.i,
            in_class: self.in_class,
            class_start: self.class_start,
        };
        copy.next_in(&self.chars)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Tok>, LexError> {
        let chars = std::mem::take(&mut self.chars);
        let out = self.next_in(&chars);
        self.chars = chars;
        out
    }

    fn next_in(&mut self, chars: &[char]) -> Result<Option<Tok>, LexError> {
        let Some(&c) = chars.get(self.i) else {
            return if self.in_class {
                Err(LexError::UnclosedClass)
            } else {
                Ok(None)
            };
        };
        self.class_start = false;
        self.i += 1;
        if self.in_class {
            return Ok(Some(match c {
                ']' => {
                    self.in_class = false;
                    Tok::ClassClose
                }
                '-' => Tok::Dash,
                '\\' => self.escape(chars, true)?,
                c => Tok::Char { c, how: How::Plain },
            }));
        }
        Ok(Some(match c {
            '\\' => self.escape(chars, false)?,
            '[' => {
                self.in_class = true;
                self.class_start = true;
                let negated = chars.get(self.i) == Some(&'^');
                if negated {
                    self.i += 1;
                }
                Tok::ClassOpen { negated }
            }
            '.' => Tok::Dot,
            '^' => Tok::Start,
            '$' => Tok::End,
            '|' => Tok::Alt,
            ')' => Tok::Close,
            '(' => {
                if chars.get(self.i) != Some(&'?') {
                    Tok::Open(Open::Capture)
                } else {
                    let a = chars.get(self.i + 1).copied();
                    let b = chars.get(self.i + 2).copied();
                    let (open, len) = match (a, b) {
                        (Some(':'), _) => (Open::NonCapture, 2),
                        (Some('='), _) => (
                            Open::Look {
                                behind: false,
                                negate: false,
                            },
                            2,
                        ),
                        (Some('!'), _) => (
                            Open::Look {
                                behind: false,
                                negate: true,
                            },
                            2,
                        ),
                        (Some('<'), Some('=')) => (
                            Open::Look {
                                behind: true,
                                negate: false,
                            },
                            3,
                        ),
                        (Some('<'), Some('!')) => (
                            Open::Look {
                                behind: true,
                                negate: true,
                            },
                            3,
                        ),
                        (Some('<'), _) => {
                            let start = self.i + 2;
                            let Some(end) = chars[start..].iter().position(|&c| c == '>') else {
                                return Err(LexError::GroupName);
                            };
                            let name: String = chars[start..start + end].iter().collect();
                            (Open::Named(name), 2 + end + 1)
                        }
                        _ => (Open::Other, 1),
                    };
                    self.i += len;
                    Tok::Open(open)
                }
            }
            '*' | '+' | '?' => {
                let (min, max) = match c {
                    '*' => (0, None),
                    '+' => (1, None),
                    _ => (0, Some(1)),
                };
                Tok::Quant {
                    min,
                    max,
                    lazy: self.lazy(chars),
                    brace: None,
                    op: c,
                }
            }
            '{' => match brace(chars, self.i) {
                Some((min, max, body, len)) => {
                    self.i += len;
                    Tok::Quant {
                        min,
                        max,
                        lazy: self.lazy(chars),
                        brace: Some(body),
                        op: '{',
                    }
                }
                None => Tok::Char { c, how: How::Plain },
            },
            c => Tok::Char { c, how: How::Plain },
        }))
    }

    fn lazy(&mut self, chars: &[char]) -> bool {
        let lazy = chars.get(self.i) == Some(&'?');
        if lazy {
            self.i += 1;
        }
        lazy
    }

    /// The escape after a `\` (already read).
    fn escape(&mut self, chars: &[char], class: bool) -> Result<Tok, LexError> {
        let Some(&n) = chars.get(self.i) else {
            return Err(LexError::TrailingBackslash);
        };
        self.i += 1;
        let digit_next = chars.get(self.i).is_some_and(char::is_ascii_digit);
        let ch = |c: char, how: How| Tok::Char { c, how };
        Ok(match n {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => Tok::ClassEscape(n),
            'b' if class => ch('\u{8}', How::Escape('b')),
            'b' | 'B' if !class => Tok::Boundary { negate: n == 'B' },
            '0' if !digit_next => ch('\0', How::Escape('0')),
            '0' => Tok::Octal { letter: '0' },
            '1'..='9' if class => Tok::Octal { letter: n },
            '1'..='9' => {
                let mut digits = String::from(n);
                while let Some(&d) = chars.get(self.i).filter(|d| d.is_ascii_digit()) {
                    digits.push(d);
                    self.i += 1;
                }
                Tok::Decimal(digits)
            }
            'k' if !class => {
                if chars.get(self.i) == Some(&'<') {
                    let start = self.i + 1;
                    match chars[start..].iter().position(|&c| c == '>') {
                        Some(end) => {
                            self.i = start + end + 1;
                            Tok::NamedRef(Some(chars[start..start + end].iter().collect()))
                        }
                        None => Tok::NamedRef(None),
                    }
                } else {
                    Tok::NamedRef(None)
                }
            }
            'f' => ch('\u{C}', How::Escape(n)),
            'n' => ch('\n', How::Escape(n)),
            'r' => ch('\r', How::Escape(n)),
            't' => ch('\t', How::Escape(n)),
            'v' => ch('\u{B}', How::Escape(n)),
            'x' => match hex_value(chars, self.i, 2) {
                Some(v) => {
                    self.i += 2;
                    ch(char::from_u32(v).unwrap_or('\0'), How::Hex)
                }
                None => ch('x', How::Escape('x')),
            },
            'u' => match hex_value(chars, self.i, 4) {
                Some(v) => {
                    self.i += 4;
                    match char::from_u32(v) {
                        Some(c) => ch(c, How::Unicode),
                        None => Tok::Surrogate(v as u16),
                    }
                }
                None => ch('u', How::Escape('u')),
            },
            'c' => {
                let letter = chars.get(self.i).copied().filter(|l| {
                    l.is_ascii_alphabetic() || (class && (l.is_ascii_digit() || *l == '_'))
                });
                match letter {
                    Some(l) => {
                        self.i += 1;
                        ch(char::from_u32(l as u32 % 32).unwrap_or('\0'), How::Control)
                    }
                    None => {
                        // The backslash is itself; `c` is read next.
                        self.i -= 1;
                        ch('\\', How::BackslashC)
                    }
                }
            }
            n => ch(n, How::Escape(n)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(p: &str) -> Vec<Tok> {
        let mut l = Lexer::new(p);
        let mut out = Vec::new();
        while let Some(t) = l.next().unwrap() {
            out.push(t);
        }
        out
    }

    #[test]
    fn annex_b_forms() {
        use Tok::*;
        assert_eq!(
            all(r"a{,2}\c-[\c1\b]"),
            vec![
                Char {
                    c: 'a',
                    how: How::Plain
                },
                Char {
                    c: '{',
                    how: How::Plain
                },
                Char {
                    c: ',',
                    how: How::Plain
                },
                Char {
                    c: '2',
                    how: How::Plain
                },
                Char {
                    c: '}',
                    how: How::Plain
                },
                Char {
                    c: '\\',
                    how: How::BackslashC
                },
                Char {
                    c: 'c',
                    how: How::Plain
                },
                Char {
                    c: '-',
                    how: How::Plain
                },
                ClassOpen { negated: false },
                Char {
                    c: '\u{11}',
                    how: How::Control
                },
                Char {
                    c: '\u{8}',
                    how: How::Escape('b')
                },
                ClassClose,
            ]
        );
        assert_eq!(
            all(r"x{2,}?\12\k<n>\ud83d"),
            vec![
                Char {
                    c: 'x',
                    how: How::Plain
                },
                Quant {
                    min: 2,
                    max: None,
                    lazy: true,
                    brace: Some("2,".into()),
                    op: '{'
                },
                Decimal("12".into()),
                NamedRef(Some("n".into())),
                Surrogate(0xD83D),
            ]
        );
        assert_eq!(
            Lexer::new("[a").next().unwrap(),
            Some(ClassOpen { negated: false })
        );
        let mut l = Lexer::new("[a");
        l.next().unwrap();
        l.next().unwrap();
        assert_eq!(l.next(), Err(LexError::UnclosedClass));
    }
}
