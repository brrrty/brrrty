//! A JavaScript pattern (without `u` or `v`) as a tree of UTF-16 units, and
//! the checks that find where a backtracking engine's answer can differ
//! from JavaScript's.

use crate::classes;
use crate::lex::{How, LexError, Lexer, Open, Tok};
use crate::set::UnitSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// Matches the empty string.
    Empty,
    /// One UTF-16 unit.
    Unit(u16),
    /// One unit in `set` (or, if `negated`, not in it). Case folding with
    /// `i` applies to `set` before the negation.
    Set {
        set: UnitSet,
        negated: bool,
    },
    /// `^`.
    Start,
    /// `$`.
    End,
    /// `\b`, or `\B` if `negate`.
    Boundary {
        negate: bool,
    },
    /// A capturing group, numbered from 1.
    Group {
        index: u32,
        node: Box<Node>,
    },
    Look {
        behind: bool,
        negate: bool,
        node: Box<Node>,
    },
    /// A backreference to a group (named ones resolved to their number).
    Backref(u32),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
}

/// A parsed pattern.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub node: Node,
    /// The number of capturing groups.
    pub groups: u32,
}

/// The UTF-16 units of `c`.
fn units(c: char) -> Vec<u16> {
    let mut buf = [0u16; 2];
    c.encode_utf16(&mut buf).to_vec()
}

fn lex_error(e: LexError) -> String {
    match e {
        LexError::TrailingBackslash => "a backslash at the end of the pattern".into(),
        LexError::UnclosedClass => "a character class is never closed".into(),
        LexError::GroupName => "a group name is never closed".into(),
    }
}

struct Parser {
    lex: Lexer,
    groups: u32,
    names: Vec<(String, u32)>,
    next_group: u32,
    dot_all: bool,
}

/// Parse `pattern` as JavaScript does without the `u` and `v` flags.
/// `dot_all` is the `s` flag. Syntax JavaScript rejects is an error, and so
/// are the few forms this doesn't model: legacy octal escapes, `\k` without
/// named groups, a backreference past the last group, `(?` modifiers and a
/// quantified lookahead.
pub fn parse(pattern: &str, dot_all: bool) -> Result<Pattern, String> {
    // Count the groups first: a backreference may come before its group.
    let mut lex = Lexer::new(pattern);
    let mut groups = 0;
    let mut names = Vec::new();
    while let Some(t) = lex.next().map_err(lex_error)? {
        match t {
            Tok::Open(Open::Capture) => groups += 1,
            Tok::Open(Open::Named(name)) => {
                groups += 1;
                if names.iter().any(|(n, _)| *n == name) {
                    return Err(format!("the group name `{name}` is used twice"));
                }
                names.push((name, groups));
            }
            _ => {}
        }
    }
    let mut p = Parser {
        lex: Lexer::new(pattern),
        groups,
        names,
        next_group: 0,
        dot_all,
    };
    let node = p.disjunction()?;
    if p.lex.next().map_err(lex_error)?.is_some() {
        return Err("a `)` that closes no group".into());
    }
    Ok(Pattern { node, groups })
}

impl Parser {
    fn next(&mut self) -> Result<Option<Tok>, String> {
        self.lex.next().map_err(lex_error)
    }

    fn peek(&self) -> Result<Option<Tok>, String> {
        self.lex.peek().map_err(lex_error)
    }

    fn disjunction(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.alternative()?];
        while self.peek()? == Some(Tok::Alt) {
            self.next()?;
            alts.push(self.alternative()?);
        }
        Ok(if alts.len() == 1 {
            alts.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alt(alts)
        })
    }

    fn alternative(&mut self) -> Result<Node, String> {
        let mut items: Vec<Node> = Vec::new();
        // Whether the last item can take a quantifier.
        let mut atom = false;
        loop {
            match self.peek()? {
                None | Some(Tok::Alt) | Some(Tok::Close) => break,
                _ => {}
            }
            let tok = self.next()?.unwrap_or(Tok::Alt);
            if let Tok::Quant { min, max, lazy, .. } = tok {
                if !atom {
                    return Err("a quantifier with nothing to repeat".into());
                }
                if max.is_some_and(|m| m < min) {
                    return Err("numbers out of order in a `{}` quantifier".into());
                }
                let node = items.pop().unwrap_or(Node::Empty);
                if matches!(node, Node::Look { .. }) {
                    return Err("a quantified lookahead isn't supported".into());
                }
                items.push(Node::Repeat {
                    node: Box::new(node),
                    min,
                    max,
                    greedy: !lazy,
                });
                atom = false;
                continue;
            }
            atom = true;
            match tok {
                Tok::Char { c, how } => {
                    if how == How::Plain && (c as u32) > 0xFFFF {
                        // Two units; a quantifier applies to the second.
                        items.extend(units(c).into_iter().map(Node::Unit));
                    } else {
                        items.push(Node::Unit(units(c)[0]));
                    }
                }
                Tok::Surrogate(u) => items.push(Node::Unit(u)),
                Tok::ClassEscape(l) => items.push(Node::Set {
                    set: classes::escape_class(l).unwrap_or_default(),
                    negated: false,
                }),
                Tok::Dot => items.push(Node::Set {
                    set: if self.dot_all {
                        UnitSet::empty()
                    } else {
                        classes::line_terminators()
                    },
                    negated: true,
                }),
                Tok::Start => {
                    items.push(Node::Start);
                    atom = false;
                }
                Tok::End => {
                    items.push(Node::End);
                    atom = false;
                }
                Tok::Boundary { negate } => {
                    items.push(Node::Boundary { negate });
                    atom = false;
                }
                Tok::Decimal(digits) => {
                    let n: u32 = digits.parse().unwrap_or(u32::MAX);
                    if n > self.groups {
                        return Err(format!(
                            "`\\{digits}` is a legacy octal escape (there is no group {n}), which isn't supported"
                        ));
                    }
                    items.push(Node::Backref(n));
                }
                Tok::Octal { .. } => {
                    return Err("legacy octal escapes aren't supported".into());
                }
                Tok::NamedRef(name) => {
                    let found = name
                        .as_ref()
                        .and_then(|n| self.names.iter().find(|(m, _)| m == n));
                    match found {
                        Some(&(_, index)) => items.push(Node::Backref(index)),
                        None => {
                            return Err(
                                "`\\k` that isn't a reference to a named group isn't supported"
                                    .into(),
                            );
                        }
                    }
                }
                Tok::Open(open) => {
                    let node = match open {
                        Open::Capture | Open::Named(_) => {
                            self.next_group += 1;
                            let index = self.next_group;
                            Node::Group {
                                index,
                                node: Box::new(self.group_body()?),
                            }
                        }
                        Open::NonCapture => self.group_body()?,
                        Open::Look { behind, negate } => {
                            // Only a lookahead can take a quantifier.
                            atom = !behind;
                            Node::Look {
                                behind,
                                negate,
                                node: Box::new(self.group_body()?),
                            }
                        }
                        Open::Other => {
                            return Err("a group starting `(?` other than `(?:`, `(?<name>` and lookaround isn't supported".into());
                        }
                    };
                    items.push(node);
                }
                Tok::ClassOpen { negated } => items.push(self.class(negated)?),
                Tok::Quant { .. } | Tok::Alt | Tok::Close | Tok::Dash | Tok::ClassClose => {
                    return Err("unexpected token".into());
                }
            }
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap_or(Node::Empty),
            _ => Node::Concat(items),
        })
    }

    fn group_body(&mut self) -> Result<Node, String> {
        let node = self.disjunction()?;
        match self.next()? {
            Some(Tok::Close) => Ok(node),
            _ => Err("a group is never closed".into()),
        }
    }

    fn class(&mut self, negated: bool) -> Result<Node, String> {
        enum Atom {
            Unit(u16),
            Set(UnitSet),
            Dash,
        }
        let mut atoms = Vec::new();
        loop {
            match self.next()? {
                Some(Tok::ClassClose) => break,
                Some(Tok::Dash) => atoms.push(Atom::Dash),
                Some(Tok::Char { c, how }) => {
                    if how == How::Plain && (c as u32) > 0xFFFF {
                        atoms.extend(units(c).into_iter().map(Atom::Unit));
                    } else {
                        atoms.push(Atom::Unit(units(c)[0]));
                    }
                }
                Some(Tok::Surrogate(u)) => atoms.push(Atom::Unit(u)),
                Some(Tok::ClassEscape(l)) => {
                    atoms.push(Atom::Set(classes::escape_class(l).unwrap_or_default()))
                }
                Some(Tok::Octal { .. }) => {
                    return Err("legacy octal escapes aren't supported".into());
                }
                _ => return Err("a character class is never closed".into()),
            }
        }
        let as_set = |a: &Atom| match a {
            Atom::Unit(u) => UnitSet::unit(*u),
            Atom::Dash => UnitSet::unit(u16::from(b'-')),
            Atom::Set(s) => s.clone(),
        };
        let mut set = UnitSet::empty();
        let mut i = 0;
        while i < atoms.len() {
            let range = i + 2 < atoms.len() && matches!(atoms[i + 1], Atom::Dash);
            if !range {
                set = set.union(&as_set(&atoms[i]));
                i += 1;
                continue;
            }
            let unit = |a: &Atom| match a {
                Atom::Unit(u) => Some(*u),
                Atom::Dash => Some(u16::from(b'-')),
                Atom::Set(_) => None,
            };
            match (unit(&atoms[i]), unit(&atoms[i + 2])) {
                (Some(a), Some(b)) => {
                    if a > b {
                        return Err("a range out of order in a character class".into());
                    }
                    set = set.union(&UnitSet::from_ranges([(a, b)]));
                }
                // A class escape at either end: the three are each in the
                // class (Annex B).
                _ => {
                    set = set
                        .union(&as_set(&atoms[i]))
                        .union(&UnitSet::unit(u16::from(b'-')))
                        .union(&as_set(&atoms[i + 2]));
                }
            }
            i += 3;
        }
        Ok(Node::Set { set, negated })
    }
}

/// Whether `node` can match without consuming anything, taking every
/// backreference as able to.
pub fn nullable(node: &Node) -> bool {
    nullable_with(node, &|_| true)
}

/// Whether `node` can match without consuming anything; a backreference
/// can when `group_nullable` says its group can.
fn nullable_with(node: &Node, group_nullable: &dyn Fn(u32) -> bool) -> bool {
    let n = |x: &Node| nullable_with(x, group_nullable);
    match node {
        Node::Empty | Node::Start | Node::End | Node::Boundary { .. } | Node::Look { .. } => true,
        Node::Backref(k) => group_nullable(*k),
        Node::Unit(_) | Node::Set { .. } => false,
        Node::Repeat { node, min, .. } => *min == 0 || n(node),
        Node::Group { node, .. } => n(node),
        Node::Concat(xs) => xs.iter().all(n),
        Node::Alt(xs) => xs.iter().any(n),
    }
}

/// For each group (by index), whether it can match nothing, taking every
/// backreference inside a group as able to.
fn groups_nullable(node: &Node, out: &mut Vec<(u32, bool)>) {
    match node {
        Node::Group { index, node } => {
            out.push((*index, nullable(node)));
            groups_nullable(node, out);
        }
        Node::Repeat { node, .. } | Node::Look { node, .. } => groups_nullable(node, out),
        Node::Concat(xs) | Node::Alt(xs) => xs.iter().for_each(|x| groups_nullable(x, out)),
        _ => {}
    }
}

/// Whether `node` has a repetition with optional iterations over something
/// that can match nothing. JavaScript fails an optional iteration that
/// matches nothing and tries the next choice, where a backtracking engine
/// may accept it: on `a`, `(?:b?|a)?` matches `a` in JavaScript. A
/// backreference matches nothing only if its group can.
pub fn repeats_nothing(node: &Node) -> bool {
    let mut groups = Vec::new();
    groups_nullable(node, &mut groups);
    let gn = move |k: u32| groups.iter().find(|(i, _)| *i == k).is_none_or(|(_, n)| *n);
    fn walk(node: &Node, gn: &dyn Fn(u32) -> bool) -> bool {
        match node {
            Node::Repeat { node, min, max, .. } => {
                (*max != Some(*min) && nullable_with(node, gn)) || walk(node, gn)
            }
            Node::Group { node, .. } | Node::Look { node, .. } => walk(node, gn),
            Node::Concat(xs) | Node::Alt(xs) => xs.iter().any(|x| walk(x, gn)),
            _ => false,
        }
    }
    walk(node, &gn)
}

/// The units a group can match (taking a backreference inside it as any
/// unit), and `None` for a group that isn't in `node`.
pub fn group_alphabet(node: &Node, index: u32, icase: bool) -> Option<UnitSet> {
    fn alphabet(node: &Node, icase: bool) -> UnitSet {
        match node {
            Node::Unit(u) => {
                let s = UnitSet::unit(*u);
                if icase { classes::case_closure(&s) } else { s }
            }
            Node::Set { set, negated } => {
                let s = if icase {
                    classes::case_closure(set)
                } else {
                    set.clone()
                };
                if *negated { s.complement() } else { s }
            }
            Node::Backref(_) => UnitSet::full(),
            Node::Group { node, .. } | Node::Repeat { node, .. } => alphabet(node, icase),
            // A lookaround consumes nothing.
            Node::Look { .. } => UnitSet::empty(),
            Node::Concat(xs) | Node::Alt(xs) => xs
                .iter()
                .fold(UnitSet::empty(), |acc, x| acc.union(&alphabet(x, icase))),
            _ => UnitSet::empty(),
        }
    }
    match node {
        Node::Group { index: i, node } if *i == index => Some(alphabet(node, icase)),
        Node::Group { node, .. } | Node::Repeat { node, .. } | Node::Look { node, .. } => {
            group_alphabet(node, index, icase)
        }
        Node::Concat(xs) | Node::Alt(xs) => xs.iter().find_map(|x| group_alphabet(x, index, icase)),
        _ => None,
    }
}

/// Every capturing group index inside `node`.
pub fn capture_indexes(node: &Node, out: &mut Vec<u32>) {
    match node {
        Node::Group { index, node } => {
            out.push(*index);
            capture_indexes(node, out);
        }
        Node::Repeat { node, .. } | Node::Look { node, .. } => capture_indexes(node, out),
        Node::Concat(xs) | Node::Alt(xs) => xs.iter().for_each(|x| capture_indexes(x, out)),
        _ => {}
    }
}

/// The capturing groups every match of `node` sets (a positive lookahead
/// keeps its groups; a negative one never sets any).
pub fn always_set(node: &Node) -> Vec<u32> {
    match node {
        Node::Repeat { node, min, .. } if *min >= 1 => always_set(node),
        Node::Group { index, node } => {
            let mut out = always_set(node);
            out.push(*index);
            out
        }
        Node::Look {
            negate: false,
            node,
            ..
        } => always_set(node),
        Node::Concat(xs) => xs.iter().flat_map(always_set).collect(),
        Node::Alt(xs) => {
            let mut sets = xs.iter().map(always_set);
            let first = sets.next().unwrap_or_default();
            sets.fold(first, |acc, s| {
                acc.into_iter().filter(|i| s.contains(i)).collect()
            })
        }
        _ => Vec::new(),
    }
}

/// Whether every group that captures inside a repetition of more than one
/// pass is set by each pass. JavaScript clears such a group at the start of
/// every pass, and a backtracking engine may keep the value of the last pass
/// that set it.
pub fn captures_reset_alike(node: &Node) -> bool {
    match node {
        Node::Repeat { node, max, .. } => {
            let mut inside = Vec::new();
            capture_indexes(node, &mut inside);
            let set = always_set(node);
            (*max == Some(1) || inside.iter().all(|i| set.contains(i)))
                && captures_reset_alike(node)
        }
        Node::Group { node, .. } | Node::Look { node, .. } => captures_reset_alike(node),
        Node::Concat(xs) | Node::Alt(xs) => xs.iter().all(captures_reset_alike),
        _ => true,
    }
}

/// Check that each backreference refers to a group that is set wherever the
/// reference is reached, and that none is in a lookbehind. JavaScript
/// matches a reference to an unset group as the empty string; a
/// backtracking engine may fail it.
pub fn backrefs_resolved(node: &Node) -> Result<(), String> {
    fn walk(node: &Node, set: &[u32], behind: bool) -> Result<Vec<u32>, String> {
        Ok(match node {
            Node::Backref(k) => {
                if behind {
                    return Err("a backreference in a lookbehind isn't supported".into());
                }
                if !set.contains(k) {
                    return Err(format!(
                        "a backreference to group {k} where it may be unset isn't supported"
                    ));
                }
                set.to_vec()
            }
            Node::Group { index, node } => {
                let mut out = walk(node, set, behind)?;
                out.push(*index);
                out
            }
            Node::Look {
                behind: b,
                negate,
                node,
            } => {
                let inner = walk(node, set, behind || *b)?;
                if *negate { set.to_vec() } else { inner }
            }
            Node::Repeat { node, min, .. } => {
                let inner = walk(node, set, behind)?;
                if *min >= 1 { inner } else { set.to_vec() }
            }
            Node::Concat(xs) => {
                let mut cur = set.to_vec();
                for x in xs {
                    cur = walk(x, &cur, behind)?;
                }
                cur
            }
            Node::Alt(xs) => {
                let mut sets = Vec::new();
                for x in xs {
                    sets.push(walk(x, set, behind)?);
                }
                let first = sets.first().cloned().unwrap_or_default();
                first
                    .into_iter()
                    .filter(|i| sets.iter().all(|s| s.contains(i)))
                    .collect()
            }
            _ => set.to_vec(),
        })
    }
    walk(node, &[], false).map(|_| ())
}

/// Whether a capturing group is inside a lookbehind (JavaScript matches a
/// lookbehind from right to left, so its groups can hold other text).
pub fn captures_in_lookbehind(node: &Node) -> bool {
    match node {
        Node::Look {
            behind: true, node, ..
        } => {
            let mut v = Vec::new();
            capture_indexes(node, &mut v);
            !v.is_empty() || captures_in_lookbehind(node)
        }
        Node::Group { node, .. } | Node::Look { node, .. } | Node::Repeat { node, .. } => {
            captures_in_lookbehind(node)
        }
        Node::Concat(xs) | Node::Alt(xs) => xs.iter().any(captures_in_lookbehind),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Node {
        parse(s, false).unwrap().node
    }

    #[test]
    fn classes_follow_annex_b() {
        let Node::Set { set, negated } = p(r"[\w-a]") else {
            panic!()
        };
        assert!(!negated && set.contains(u16::from(b'-')) && set.contains(u16::from(b'Z')));
        let Node::Set { set, .. } = p("[a-]") else {
            panic!()
        };
        assert_eq!(set.ranges(), &[(0x2D, 0x2D), (0x61, 0x61)]);
        assert!(parse("[z-a]", false).is_err());
        let Node::Set { set, negated } = p("[^]") else {
            panic!()
        };
        assert!(set.is_empty() && negated);
    }

    #[test]
    fn errors_and_backreferences() {
        assert!(parse("a**", false).is_err());
        assert!(parse("(?=a)*", false).is_err());
        assert!(parse(r"\2(a)", false).is_err());
        assert_eq!(parse(r"(a)\1", false).unwrap().groups, 1);
        assert!(backrefs_resolved(&p(r"(a)\1")).is_ok());
        assert!(backrefs_resolved(&p(r"(a)?\1")).is_err());
        assert!(backrefs_resolved(&p(r"(?:(a)|b)\1")).is_err());
        assert!(backrefs_resolved(&p(r#"(["'])(?:\\.|(?!\1)[^\\])*\1"#)).is_ok());
        assert!(backrefs_resolved(&p(r"\1(a)")).is_err());
    }

    #[test]
    fn analyses() {
        assert!(repeats_nothing(&p("(?:a|)*")));
        assert!(repeats_nothing(&p("(a*)?")));
        assert!(!repeats_nothing(&p("(?:a*b)*")));
        // A backreference to a group that always consumes isn't empty.
        assert!(!repeats_nothing(&p(r"(a)(?:b|\1\1)*")));
        assert!(repeats_nothing(&p(r"(a?)(?:b|\1)*")));
        assert_eq!(
            group_alphabet(&p("x([a-c]k)"), 1, true).unwrap().ranges(),
            &[(0x41, 0x43), (0x4B, 0x4B), (0x61, 0x63), (0x6B, 0x6B)]
        );
        assert!(!captures_reset_alike(&p("(?:(a)|b)+")));
        assert!(captures_reset_alike(&p(r"(?:([a-z])\d)+")));
        assert!(captures_in_lookbehind(&p("(?<=(a))b")));
        // A literal outside the BMP is two units.
        assert_eq!(
            p("😀+"),
            Node::Concat(vec![
                Node::Unit(0xD83D),
                Node::Repeat {
                    node: Box::new(Node::Unit(0xDE00)),
                    min: 1,
                    max: None,
                    greedy: true
                }
            ])
        );
    }
}
