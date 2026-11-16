//! Prism's `tokenize` and `matchGrammar` (prism-core.js, Prism 1.30.0),
//! ported step for step.
//!
//! Text is UTF-16 units, as in JavaScript: every position, length and slice
//! here counts units, so a token can split a character outside the BMP
//! exactly where Prism splits it.
//!
//! `matchGrammar` keeps the text as a doubly linked list of strings and
//! tokens. Each pattern of each token of the grammar (in order) runs over
//! the strings left in the list: a match splits its string into the text
//! before, a token, and the text after. A `greedy` pattern instead runs on
//! the whole text from the current position, and a match that starts in a
//! string but runs over later tokens swallows them; the tokens it removed
//! are then matched again ("rematch") from the patterns before it. The
//! list here is an arena of nodes; a string node is a range of the text
//! (every string in Prism's list is the text at its position).

use crate::grammar::{GrammarId, Grammars, Pattern, grammars};
use crate::regex::{BacktrackBudget, JsRegex, JsText, Match};

/// UTF-16 units.
pub type Units = Vec<u16>;

/// One item of a token stream: a string or a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Str(Units),
    Token(Token),
}

/// A token's content: a string, or the token stream of its `inside`
/// grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Str(Units),
    Items(Vec<Item>),
}

/// Prism's `Token`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// `type`.
    pub kind: String,
    pub content: Content,
    /// `alias`, as classes in order (none for `undefined` or `null`).
    pub alias: Vec<String>,
    /// The length of the text the token was made from, in units.
    pub length: usize,
}

/// Why text can't be highlighted exactly as Prism does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

/// How deep `tokenize` and `highlight` may nest (an `inside` grammar, a
/// hook that highlights again). Real code nests a few levels; a grammar
/// that nests in itself on the same text would overflow Node's stack.
pub const MAX_DEPTH: usize = 100;

/// `Prism.tokenize(text, grammar)`.
pub fn tokenize(text: &[u16], grammar: GrammarId) -> Result<Vec<Item>, Refusal> {
    let mut budget = BacktrackBudget::for_code(text.len());
    Tokenizer {
        g: grammars(),
        budget: &mut budget,
    }
    .tokenize(text, grammar, 0)
}

pub(crate) struct Tokenizer<'a> {
    pub g: &'static Grammars,
    pub budget: &'a mut BacktrackBudget,
}

/// The rematch state of a nested `matchGrammar` call: stop at the pattern
/// that caused it, and at `reach`.
struct Rematch<'a> {
    cause: (&'a str, usize),
    reach: usize,
}

#[derive(Debug)]
enum Value {
    /// The list's head or tail.
    End,
    /// Text `start..end`.
    Str(usize, usize),
    Token(Token),
}

struct Node {
    value: Value,
    prev: usize,
    next: usize,
}

/// Prism's `LinkedList`.
struct List {
    nodes: Vec<Node>,
    /// `length`: the nodes between head and tail.
    length: usize,
}

const HEAD: usize = 0;
const TAIL: usize = 1;

impl List {
    fn new() -> Self {
        let nodes = vec![
            Node {
                value: Value::End,
                prev: HEAD,
                next: TAIL,
            },
            Node {
                value: Value::End,
                prev: HEAD,
                next: TAIL,
            },
        ];
        Self { nodes, length: 0 }
    }

    fn next(&self, n: usize) -> usize {
        self.nodes[n].next
    }

    fn prev(&self, n: usize) -> usize {
        self.nodes[n].prev
    }

    /// `currentNode.value.length`.
    fn len(&self, n: usize) -> usize {
        match &self.nodes[n].value {
            Value::End => 0,
            Value::Str(a, b) => b - a,
            Value::Token(t) => t.length,
        }
    }

    fn is_token(&self, n: usize) -> bool {
        matches!(self.nodes[n].value, Value::Token(_))
    }

    /// `addAfter`.
    fn add_after(&mut self, node: usize, value: Value) -> usize {
        let next = self.nodes[node].next;
        let id = self.nodes.len();
        self.nodes.push(Node {
            value,
            prev: node,
            next,
        });
        self.nodes[node].next = id;
        self.nodes[next].prev = id;
        self.length += 1;
        id
    }

    /// `removeRange`: up to `count` nodes after `node`.
    fn remove_range(&mut self, node: usize, count: usize) {
        let mut next = self.nodes[node].next;
        let mut i = 0;
        while i < count && next != TAIL {
            next = self.nodes[next].next;
            i += 1;
        }
        self.nodes[node].next = next;
        self.nodes[next].prev = node;
        self.length -= i;
    }

    /// `toArray`.
    fn into_items(mut self, text: &[u16]) -> Vec<Item> {
        let mut out = Vec::with_capacity(self.length);
        let mut n = self.nodes[HEAD].next;
        while n != TAIL {
            let next = self.nodes[n].next;
            match std::mem::replace(&mut self.nodes[n].value, Value::End) {
                Value::Str(a, b) => out.push(Item::Str(text[a..b].to_vec())),
                Value::Token(t) => out.push(Item::Token(t)),
                Value::End => {}
            }
            n = next;
        }
        out
    }
}

/// `matchPattern`'s result: where the match starts and ends once the
/// lookbehind group is left out.
struct Found {
    index: usize,
    end: usize,
}

impl Tokenizer<'_> {
    pub fn tokenize(
        &mut self,
        text: &[u16],
        grammar: GrammarId,
        depth: usize,
    ) -> Result<Vec<Item>, Refusal> {
        if depth > MAX_DEPTH {
            return Err(Refusal(format!(
                "the code nests tokens more than {MAX_DEPTH} levels deep"
            )));
        }
        let js = JsText::from_units(text);
        let mut list = List::new();
        list.add_after(HEAD, Value::Str(0, text.len()));
        self.match_grammar(&js, text, &mut list, grammar, HEAD, 0, None, depth)?;
        Ok(list.into_items(text))
    }

    /// `matchPattern`: `exec` at `pos`, with the lookbehind group (group 1,
    /// when it matched something) left out of the match.
    fn match_pattern(
        &mut self,
        re: &JsRegex,
        pos: usize,
        text: &JsText,
        lookbehind: bool,
    ) -> Result<Option<Found>, Refusal> {
        let m: Option<Match> = re
            .exec_budgeted(text, pos, self.budget)
            .map_err(|e| Refusal(format!("{} (Prism's pattern {})", e.0, re.js_source())))?;
        Ok(m.map(|m| {
            let mut index = m.index;
            if lookbehind
                && let Some(Some((a, b))) = m.groups.first()
                && b > a
            {
                index += b - a;
            }
            Found { index, end: m.end }
        }))
    }

    fn regex(&self, p: &Pattern) -> Result<&'static JsRegex, Refusal> {
        self.g.regex(p.regex).map_err(|e| {
            let entry = self.g.regex_entry(p.regex);
            Refusal(format!(
                "Prism's pattern /{}/{} can't be matched exactly: {}",
                entry.source, entry.flags, e.0
            ))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn match_grammar(
        &mut self,
        js: &JsText,
        text: &[u16],
        list: &mut List,
        grammar: GrammarId,
        start_node: usize,
        start_pos: usize,
        mut rematch: Option<&mut Rematch<'_>>,
        depth: usize,
    ) -> Result<(), Refusal> {
        let tokens = self.g.tokens(grammar);
        for token in tokens {
            for (j, pattern) in token.patterns.iter().enumerate() {
                if let Some(r) = &rematch
                    && r.cause == (token.name.as_str(), j)
                {
                    return Ok(());
                }
                let lookbehind = pattern.lookbehind;
                let greedy = pattern.greedy;
                let re = self.regex(pattern)?;

                let mut current = list.next(start_node);
                let mut pos = start_pos;
                while current != TAIL {
                    if let Some(r) = &rematch
                        && pos >= r.reach
                    {
                        break;
                    }
                    if list.length > text.len() {
                        // "Something went terribly wrong, ABORT, ABORT!"
                        return Ok(());
                    }
                    if list.is_token(current) {
                        pos += list.len(current);
                        current = list.next(current);
                        continue;
                    }

                    let mut remove_count = 1;
                    // The string the match is in (text `str_start..str_end`)
                    // and the match within it.
                    let (str_start, str_end, from, match_len);
                    if greedy {
                        let Some(m) = self.match_pattern(re, pos, js, lookbehind)? else {
                            break;
                        };
                        if m.index >= text.len() {
                            break;
                        }
                        let (abs_from, to) = (m.index, m.end);
                        let mut p = pos;
                        // Find the node that holds the match.
                        p += list.len(current);
                        while abs_from >= p {
                            current = list.next(current);
                            if current == TAIL {
                                return Err(Refusal(
                                    "the token list lost track of the text".into(),
                                ));
                            }
                            p += list.len(current);
                        }
                        p -= list.len(current);
                        pos = p;
                        // A match that starts inside a token is invalid.
                        if list.is_token(current) {
                            pos += list.len(current);
                            current = list.next(current);
                            continue;
                        }
                        // Find the last node the match covers.
                        let mut k = current;
                        while k != TAIL && (p < to || !list.is_token(k)) {
                            remove_count += 1;
                            p += list.len(k);
                            k = list.next(k);
                        }
                        remove_count -= 1;
                        str_start = pos;
                        str_end = p;
                        from = abs_from - pos;
                        match_len = to - abs_from;
                    } else {
                        let Value::Str(a, b) = list.nodes[current].value else {
                            unreachable!("a string node")
                        };
                        let s = js.slice(a, b);
                        let Some(m) = self.match_pattern(re, 0, &s, lookbehind)? else {
                            pos += list.len(current);
                            current = list.next(current);
                            continue;
                        };
                        str_start = a;
                        str_end = b;
                        from = m.index;
                        match_len = m.end - m.index;
                    }

                    let str_len = str_end - str_start;
                    let match_start = str_start + from;
                    let match_end = match_start + match_len;
                    let reach = pos + str_len;
                    if let Some(r) = rematch.as_deref_mut()
                        && reach > r.reach
                    {
                        r.reach = reach;
                    }

                    let mut remove_from = list.prev(current);
                    if from > 0 {
                        remove_from =
                            list.add_after(remove_from, Value::Str(str_start, match_start));
                        pos += from;
                    }
                    list.remove_range(remove_from, remove_count);

                    let matched = &text[match_start..match_end];
                    let content = match pattern.inside {
                        Some(inside) => {
                            Content::Items(self.tokenize(matched, inside, depth + 1)?)
                        }
                        None => Content::Str(matched.to_vec()),
                    };
                    let wrapped = Token {
                        kind: token.name.clone(),
                        content,
                        alias: pattern.alias.clone(),
                        length: match_len,
                    };
                    current = list.add_after(remove_from, Value::Token(wrapped));
                    if match_end < str_end {
                        list.add_after(current, Value::Str(match_end, str_end));
                    }

                    if remove_count > 1 {
                        // At least one token was removed: match again.
                        let mut nested = Rematch {
                            cause: (token.name.as_str(), j),
                            reach,
                        };
                        let prev = list.prev(current);
                        self.match_grammar(
                            js,
                            text,
                            list,
                            grammar,
                            prev,
                            pos,
                            Some(&mut nested),
                            depth,
                        )?;
                        if let Some(r) = rematch.as_deref_mut()
                            && nested.reach > r.reach
                        {
                            r.reach = nested.reach;
                        }
                    }

                    pos += list.len(current);
                    current = list.next(current);
                }
            }
        }
        Ok(())
    }
}
