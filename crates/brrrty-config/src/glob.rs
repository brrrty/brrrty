//! The globs of `getFilteredByGlob`, as 11ty 3.1.8 matches them.
//!
//! 11ty adds `./` to each glob (`TemplatePath.addLeadingDotSlash`), takes
//! `./` off the item's `inputPath`, and asks picomatch 4 with `dot: true`
//! and `nocase: true` (`Util/GlobMatcher.js`). So a glob is read from the
//! project root, not the input folder: `./src/posts/**/*.md` and
//! `src/posts/**/*.md` are the same, and `posts/**/*.md` matches nothing
//! when the input folder is `src`. An array matches an item that any of
//! its globs matches.
//!
//! Only part of picomatch's syntax is supported: literal ASCII text, `*`
//! (any run of characters within a folder name, dot files included), `?`
//! (one UTF-16 unit, as picomatch's regular expression has it) and `**` as
//! a whole folder name (any number of folders). Letters match in any case.
//! Everything else (braces, classes, extglobs, negation, escapes, absolute
//! or `..` paths, empty or `.` folder names, non-ASCII letters) is refused
//! with an error rather than guessed. Each rule was checked against
//! picomatch itself; see the tests.

/// One part of a folder name in a glob.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A literal ASCII byte, lowercased.
    Lit(u16),
    Star,
    Any,
}

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    /// `**`: any number of folders.
    Globstar,
    /// `**` at the end, right after a name with `*`: picomatch wants at
    /// least one more folder or file there (`./*/**` doesn't match `a`,
    /// `./a/**` matches `a`).
    GlobstarPlus,
    Name(Vec<Token>),
}

/// A compiled glob.
#[derive(Debug, Clone)]
pub struct Glob {
    segments: Vec<Segment>,
}

/// ASCII bytes that picomatch reads as themselves (probed one by one).
fn literal(c: char) -> bool {
    c.is_ascii_alphanumeric() || " -_.~$^+%=#,&';".contains(c)
}

impl Glob {
    /// Compile `glob` as 11ty would use it, or say why Brrrty can't.
    pub fn new(glob: &str) -> Result<Glob, String> {
        let refuse = |why: &str| {
            Err(format!(
                "the glob `{glob}` {why}; Brrrty supports `*`, `?`, `**` and literal ASCII text in globs, read from the project root"
            ))
        };
        // `addLeadingDotSlash`, then picomatch drops the `./`.
        let rest = match glob {
            "." | ".." => return refuse("names a folder, which matches no page"),
            g if g.starts_with('/') => return refuse("is absolute, which matches no page"),
            g if g.starts_with("../") => {
                return refuse("starts with `../`, which matches no page");
            }
            g => g.strip_prefix("./").unwrap_or(g),
        };
        if rest.is_empty() {
            return refuse("matches no page");
        }
        let mut segments = Vec::new();
        for part in rest.split('/') {
            if part.is_empty() {
                return refuse("has an empty folder name (`//`, or a `/` at the end)");
            }
            if part == "." || part == ".." {
                return refuse("has a `.` or `..` folder name");
            }
            if part == "**" {
                segments.push(Segment::Globstar);
                continue;
            }
            if part.contains("**") {
                return refuse("has `**` inside a name, not as a whole folder name");
            }
            let mut tokens = Vec::new();
            for c in part.chars() {
                match c {
                    '*' => tokens.push(Token::Star),
                    '?' => tokens.push(Token::Any),
                    c if literal(c) => tokens.push(Token::Lit(c.to_ascii_lowercase() as u16)),
                    c if !c.is_ascii() => {
                        return refuse(
                            "has a non-ASCII character, which picomatch matches its own way",
                        );
                    }
                    c => return refuse(&format!("uses `{c}`, which Brrrty doesn't support")),
                }
            }
            segments.push(Segment::Name(tokens));
        }
        let run = segments
            .iter()
            .rev()
            .take_while(|s| **s == Segment::Globstar)
            .count();
        let before = segments.len() - run;
        let starred = |s: &Segment| matches!(s, Segment::Name(t) if t.contains(&Token::Star));
        if run > 0 && before > 0 && starred(&segments[before - 1]) {
            segments.truncate(before);
            segments.push(Segment::GlobstarPlus);
        }
        Ok(Glob { segments })
    }

    /// Whether the glob matches `input_path` (`./src/page.md`).
    pub fn matches(&self, input_path: &str) -> bool {
        let path = input_path.strip_prefix("./").unwrap_or(input_path);
        let parts: Vec<Vec<u16>> = path
            .split('/')
            .map(|p| p.encode_utf16().collect())
            .collect();
        match_segments(&self.segments, &parts)
    }
}

fn match_segments(glob: &[Segment], path: &[Vec<u16>]) -> bool {
    match glob.split_first() {
        None => path.is_empty(),
        Some((Segment::Globstar, rest)) => {
            (0..=path.len()).any(|skip| match_segments(rest, &path[skip..]))
        }
        Some((Segment::GlobstarPlus, _)) => !path.is_empty(),
        Some((Segment::Name(tokens), rest)) => match path.split_first() {
            Some((name, more)) => match_name(tokens, name) && match_segments(rest, more),
            None => false,
        },
    }
}

/// A folder or file name against its tokens, by UTF-16 unit: the usual
/// wildcard walk that goes back to the last `*` on a mismatch.
fn match_name(tokens: &[Token], name: &[u16]) -> bool {
    let unit = |t: &Token, u: u16| match t {
        Token::Any => true,
        Token::Lit(c) => u < 128 && (u as u8).to_ascii_lowercase() as u16 == *c,
        Token::Star => false,
    };
    let (mut t, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if t < tokens.len() && unit(&tokens[t], name[n]) {
            t += 1;
            n += 1;
        } else if t < tokens.len() && tokens[t] == Token::Star {
            star = Some((t, n));
            t += 1;
        } else if let Some((st, sn)) = star {
            t = st + 1;
            n = sn + 1;
            star = Some((st, sn + 1));
        } else {
            return false;
        }
    }
    tokens[t..].iter().all(|x| *x == Token::Star)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(path: &str, glob: &str) -> bool {
        Glob::new(glob).unwrap().matches(path)
    }

    /// Each row is what 11ty 3.1.8's `isGlobMatch` (picomatch 4.0.7) gave.
    #[test]
    fn matches_like_picomatch() {
        let rows: &[(&str, &str, bool)] = &[
            (
                "./src/posts/2020/2020-01-01-a.md",
                "./src/posts/**/*.md",
                true,
            ),
            ("./src/posts/a.md", "./src/posts/**/*.md", true),
            ("./src/posts/2020/x/y.md", "./src/posts/**/*.md", true),
            ("./src/POSTS/b.MD", "./src/posts/**/*.md", true),
            ("./src/posts/.hidden.md", "./src/posts/**/*.md", true),
            ("./src/posts/.d/x.md", "./src/posts/**/*.md", true),
            ("./src/posts/2020/a.liquid", "./src/posts/**/*.md", false),
            ("./src/postsx/a.md", "./src/posts/**/*.md", false),
            ("./src/posts/2020/a.md.bak", "./src/posts/**/*.md", false),
            ("./src/posts/a b.md", "./src/posts/**/*.md", true),
            ("./src/posts/é.md", "./src/posts/**/*.md", true),
            ("./src/posts/a.md", "src/posts/**/*.md", true),
            ("./src/posts/a.md", "posts/**/*.md", false),
            ("./src/posts/2020/a.md", "./src/posts/*.md", false),
            ("./src/posts/.hidden.md", "./src/posts/*.md", true),
            ("./src/index.md", "./src/**", true),
            ("./src/posts/2020/a.liquid", "./src/posts/**", true),
            ("./index.md", "**/*.md", true),
            ("./src/index.md", "**/*.md", true),
            ("./src/index.md", "*.md", false),
            ("./index.md", "*.md", true),
            ("./src/posts/a.md", "./src/posts/?.md", true),
            ("./src/posts/é.md", "./src/posts/?.md", true),
            ("./src/posts/a b.md", "./src/posts/?.md", false),
            ("./src/posts/a.md", "./src/**/a.md", true),
            ("./src/postsx/a.md", "./src/**/a.md", true),
            ("./src/posts/a.md", "./SRC/Posts/**/*.MD", true),
            ("./src/posts/2020/x/y.md", "./src/posts/2020/**/*.md", true),
            ("./src/posts/a.md", "./src/posts/a.md", true),
            ("./src/posts/.md", "./src/posts/*.md", true),
            ("./src/posts/😀.md", "./src/posts/?.md", false),
            ("./src/posts/😀.md", "./src/posts/??.md", true),
            ("./src/posts/😀.md", "./src/posts/*.md", true),
            ("./src/ſ.md", "./src/s.md", false),
            ("./src/posts/a.md", "**", true),
            ("./src/posts/a.md", "./**", true),
            ("./src/posts/a.md", "./src/*/a.md", true),
            ("./src/posts/a.md", "./src/*/*/a.md", false),
            ("./src/posts/a.md", "./src/**/**/a.md", true),
            ("./src/a/b/c.md", "./src/**/b/**/c.md", true),
            ("./src/a/b/c.md", "./src/**/b/**/*", true),
            ("./src/x.md", "./src/*", true),
            ("./src/x/y.md", "./src/*", false),
            ("./src/x.md", "./src/*.*", true),
            ("./src/x", "./src/*.*", false),
            ("./src/a.b.md", "./src/*.md", true),
            ("./src/a-b.md", "./src/a*b.md", true),
            ("./src/ab.md", "./src/a*b.md", true),
            ("./src/a/b.md", "./src/a*b.md", false),
            ("./src/a$b.md", "./src/a$b.md", true),
            ("./src/a^b.md", "./src/a^b.md", true),
            ("./src/a+b.md", "./src/a+b.md", true),
            ("./src/a b.md", "./src/a b.md", true),
            ("./src/a%b.md", "./src/a%b.md", true),
            ("./src/a=b.md", "./src/a=b.md", true),
            ("./src/a#b.md", "./src/a#b.md", true),
            ("./src/a~b.md", "./src/a~b.md", true),
            ("./src/a,b.md", "./src/a,b.md", true),
            ("./src/a&b.md", "./src/a&b.md", true),
            ("./src/a'b.md", "./src/a'b.md", true),
            ("./src/a;b.md", "./src/a;b.md", true),
            ("./src/a.md", "./src/A.MD", true),
            ("./src/a_b.md", "./src/A_B.md", true),
            ("./src/x/.git/a.md", "./src/**/*.md", true),
            ("./a.md", "./*", true),
            ("./src", "./src/**", true),
            ("./a", "./*/**", false),
            ("./a", "./a*/**", false),
            ("./a", "./?/**", true),
            ("./a/b", "./*/**", true),
            ("./a/b", "./*/*/**", false),
            ("./src/a", "./src/a/**", true),
            ("./src/a", "./src/*/**", false),
            ("./src/ab", "./src/a?/**", true),
            ("./a", "./**/**", true),
            ("./a", "./**/a/**", true),
            ("./a/b", "./*/**/b", true),
            ("./a/x/b", "./*/**/b", true),
            ("./a/c/b.md", "./*/**/*.md", true),
            ("./src/posts/a.md", "./src/*/**/*.md", true),
            ("./a", "./**/*/**", false),
            ("./a/b", "./**/*/**", true),
            ("./a", "./*/**/**", false),
            ("./src/a", "./src/*/**/**", false),
            ("./a/b", "./*?/**", true),
            ("./x/a", "./**/a*/**", false),
            ("./x/a/y", "./**/a*/**", true),
        ];
        for (path, glob, want) in rows {
            assert_eq!(m(path, glob), *want, "{path} {glob}");
        }
    }

    #[test]
    fn unsupported_syntax_is_refused() {
        for g in [
            "./src/posts/{a,b}.md",
            "./src/posts/[ab].md",
            "./src/posts/!(a).md",
            "!src/a.md",
            "./src/a(b).md",
            "./src/a|b.md",
            "./src/a\\*.md",
            "/src/posts/**/*.md",
            "../src/posts/**/*.md",
            "./src//posts/**/*.md",
            "./src/./posts/**/*.md",
            "./src/posts/**.md",
            "./src/posts/***/a.md",
            "./src/posts/",
            "./src/posts/**/",
            ".",
            "./",
            "./src/É.md",
            "./src/a@b.md",
        ] {
            assert!(Glob::new(g).is_err(), "{g}");
        }
    }
}
