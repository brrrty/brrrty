//! The full fuzz of the regular expression translation: every regular
//! expression in grammars/prism.json, run by Node (tools/regex-oracle.cjs)
//! and by its translation on the same code-like text, with `lastIndex` at
//! several positions. A translated pattern must give the same match, end
//! and groups, in UTF-16 units, on every case. Needs Node:
//!
//!   cargo test -p brrrty-prism --test fuzz -- --ignored --nocapture
//!
//! The text is made by a seeded generator from fragments of the code in
//! tests/fuzz/corpus.txt, mixed with random characters: punctuation,
//! whitespace, line terminators, letters outside ASCII that fold in odd ways
//! (the Kelvin sign, long s, final sigma, dotted I), emoji and lone
//! surrogates. `BRRRTY_FUZZ_STRINGS` sets the strings per pattern (default
//! 2000; each runs at 5 positions). `BRRRTY_FUZZ_SITES=<dir>` also takes
//! fragments from the fenced code blocks in `<dir>/*/src/**/*.md`; that run
//! never writes the probes. `BRRRTY_WRITE_PROBES=1` writes a sample of the
//! cases to tests/probes/regex.json (replayed by tests/probes.rs without
//! Node). A summary is written to target/prism-fuzz/report.txt.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use brrrty_jsregex::ast::Node;
use brrrty_jsregex::set::UnitSet;
use brrrty_prism::grammar::{RegexId, grammars};
use brrrty_prism::regex::JsText;
use serde_json::Value;

const MANIFEST: &str = env!("CARGO_MANIFEST_DIR");

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The sections of tests/fuzz/corpus.txt, by language.
fn corpus() -> HashMap<String, Vec<String>> {
    let text = std::fs::read_to_string(Path::new(MANIFEST).join("tests/fuzz/corpus.txt")).unwrap();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut lang = String::new();
    for line in text.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix("=== ") {
            lang = name.trim().to_string();
            out.entry(lang.clone()).or_default().push(String::new());
        } else if let Some(last) = out.get_mut(&lang).and_then(|v| v.last_mut()) {
            last.push_str(line);
        }
    }
    out
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "node_modules") {
                markdown_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// Fenced code blocks in `<dir>/*/src/**/*.md`, by normalized language.
fn site_corpus(dir: &Path) -> HashMap<String, Vec<String>> {
    let g = grammars();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut files = Vec::new();
    for site in std::fs::read_dir(dir).unwrap().flatten() {
        markdown_files(&site.path().join("src"), &mut files);
    }
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let mut block: Option<(String, String, String)> = None;
        for line in text.lines() {
            let trimmed = line.trim_start();
            match &mut block {
                None => {
                    for fence in ["```", "~~~"] {
                        if let Some(info) = trimmed.strip_prefix(fence) {
                            let name = info.split_whitespace().next().unwrap_or("").to_string();
                            if let Some(fl) = g.fences.iter().find(|x| x.name == name && x.known) {
                                block =
                                    Some((fence.to_string(), fl.normalized.clone(), String::new()));
                            }
                        }
                    }
                }
                Some((fence, lang, body)) => {
                    if trimmed.starts_with(fence.as_str()) {
                        out.entry(lang.clone())
                            .or_default()
                            .push(std::mem::take(body));
                        block = None;
                    } else {
                        body.push_str(line);
                        body.push('\n');
                    }
                }
            }
        }
    }
    out
}

const SPECIAL: &[&str] = &[
    "\r\n", "\r", "\n", "\n\n", "\u{2028}", "\u{2029}", "\t", "  ", "\\", "\"", "'", "`", "/*",
    "*/", "//", "<!--", "-->", "{{", "}}", "{%", "%}", "<<<", "<<", "$", "#", "@", "${", "#{", "=",
    "\\\n", "---", "```", ";", ":", "<", ">", "/", "(", ")", "[", "]", "{", "}",
];

/// Units that JavaScript and Rust treat differently, or that split.
const ODD: &[&[u16]] = &[
    &[0xD83D],
    &[0xDE00],
    &[0xD83D, 0xDE00],
    &[0xD83C, 0xDF89],
    &[0xE9],
    &[0xC9],
    &[0x17F],
    &[0x212A],
    &[0xDF],
    &[0x3A3],
    &[0x3C2],
    &[0x3C3],
    &[0x130],
    &[0x131],
    &[0xB5],
    &[0x1C5],
    &[0x4E2D],
    &[0x301],
    &[0xA0],
    &[0xFEFF],
    &[0x85],
    &[0x2014],
    &[0x201C],
    &[0x0],
    &[0xB],
    &[0xC],
];

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// A code-like string from fragments of `sources` and whole `tokens`.
fn gen_text(rng: &mut Rng, sources: &[&str], tokens: &[Vec<u16>]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    // Some strings are one token (for patterns that run on a token's text,
    // anchored at both ends), now and then with something after it.
    if !tokens.is_empty() && rng.below(4) == 0 {
        out.extend_from_slice(&tokens[rng.below(tokens.len())]);
        if rng.below(3) == 0 {
            out.extend(units(SPECIAL[rng.below(SPECIAL.len())]));
        }
        return out;
    }
    for _ in 0..1 + rng.below(6) {
        if !tokens.is_empty() && rng.below(3) == 0 {
            out.extend_from_slice(&tokens[rng.below(tokens.len())]);
            continue;
        }
        match rng.below(12) {
            0..=6 => {
                let src = sources[rng.below(sources.len())];
                let chars: Vec<char> = src.chars().collect();
                if chars.is_empty() {
                    continue;
                }
                let start = rng.below(chars.len());
                let len = 1 + rng.below(120);
                let frag: String = chars[start..(start + len).min(chars.len())]
                    .iter()
                    .collect();
                out.extend(units(&frag));
            }
            7 => {
                let src = sources[rng.below(sources.len())];
                let chars: Vec<char> = src.chars().collect();
                let start = rng.below(chars.len());
                let len = 1 + rng.below(40);
                for &c in &chars[start..(start + len).min(chars.len())] {
                    let c = if rng.below(2) == 0 {
                        c.to_ascii_uppercase()
                    } else {
                        c.to_ascii_lowercase()
                    };
                    out.extend(units(&c.to_string()));
                }
            }
            8 | 9 => {
                for _ in 0..1 + rng.below(3) {
                    out.extend(units(SPECIAL[rng.below(SPECIAL.len())]));
                }
            }
            10 => out.extend_from_slice(ODD[rng.below(ODD.len())]),
            _ => {
                for _ in 0..1 + rng.below(8) {
                    out.push(0x20 + rng.below(0x5F) as u16);
                }
            }
        }
    }
    out.truncate(400);
    out
}

/// `units` as a JSON string (lone surrogates as `\u` escapes).
fn json_units(units: &[u16]) -> String {
    let mut s = String::from("\"");
    for &u in units {
        match u {
            0x22 => s.push_str("\\\""),
            0x5C => s.push_str("\\\\"),
            0x20..=0x7E => s.push(u as u8 as char),
            _ => write!(s, "\\u{u:04x}").unwrap(),
        }
    }
    s.push('"');
    s
}

type Got = Option<Vec<i64>>;

struct Case {
    text: Vec<u16>,
    last: usize,
}

#[derive(Default)]
struct Stats {
    strings: usize,
    compared: usize,
    matched: usize,
    refused: usize,
    diffs: Vec<String>,
    probes: Vec<(Case, Got)>,
}

fn rust_exec(re: &brrrty_prism::regex::JsRegex, text: &JsText, last: usize) -> Result<Got, String> {
    re.exec(text, last).map_err(|e| e.0).map(|m| {
        m.map(|m| {
            let mut out = vec![m.index as i64, m.end as i64];
            for g in &m.groups {
                let (a, b) = g.map_or((-1, -1), |(a, b)| (a as i64, b as i64));
                out.extend([a, b]);
            }
            out
        })
    })
}

struct Oracle {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Oracle {
    fn new() -> Self {
        let mut child = Command::new("node")
            .arg(Path::new(MANIFEST).join("tools/regex-oracle.cjs"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("node runs");
        let out = BufReader::new(child.stdout.take().unwrap());
        Self { child, out }
    }

    fn run(&mut self, source: &str, flags: &str, cases: &[Case]) -> Vec<Got> {
        let mut req = String::new();
        write!(
            req,
            "{{\"source\":{},\"flags\":{},\"cases\":[",
            serde_json::to_string(source).unwrap(),
            serde_json::to_string(flags).unwrap()
        )
        .unwrap();
        for (i, c) in cases.iter().enumerate() {
            if i > 0 {
                req.push(',');
            }
            write!(req, "[{},{}]", json_units(&c.text), c.last).unwrap();
        }
        req.push_str("]}\n");
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(req.as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_array()
                    .map(|xs| xs.iter().map(|x| x.as_i64().unwrap()).collect())
            })
            .collect()
    }
}

/// The corpus sections each regular expression's languages give.
fn sources_by_regex(corpus: &HashMap<String, Vec<String>>) -> HashMap<RegexId, Vec<String>> {
    let g = grammars();
    let mut out: HashMap<RegexId, Vec<String>> = HashMap::new();
    for f in g.fences.iter().filter(|f| f.known) {
        let Some(root) = g.language(&f.normalized) else {
            continue;
        };
        for r in g.regexes_of(root) {
            let langs = out.entry(r).or_default();
            if !langs.contains(&f.normalized) {
                langs.push(f.normalized.clone());
            }
        }
    }
    for langs in out.values_mut() {
        langs.retain(|l| corpus.contains_key(l) || l == "text");
    }
    out
}

/// Usually `u`; now and then a unit that Unicode case folding (but not
/// always JavaScript) takes for it, or an odd unit.
fn near(rng: &mut Rng, u: u16) -> u16 {
    use regex_syntax::hir::{ClassUnicode, ClassUnicodeRange};
    match rng.below(16) {
        0 | 1 => {
            let Some(c) = char::from_u32(u32::from(u)) else {
                return u;
            };
            let mut class = ClassUnicode::new([ClassUnicodeRange::new(c, c)]);
            class.case_fold_simple();
            let all: Vec<u16> = class
                .iter()
                .flat_map(|r| r.start() as u32..=r.end() as u32)
                .filter(|&x| x <= 0xFFFF)
                .map(|x| x as u16)
                .collect();
            all[rng.below(all.len())]
        }
        2 => ODD[rng.below(ODD.len())][0],
        _ => u,
    }
}

/// Append a string `node` likely matches (lookarounds aside) to `out`.
fn sample(
    node: &Node,
    rng: &mut Rng,
    icase: bool,
    groups: &mut HashMap<u32, Vec<u16>>,
    out: &mut Vec<u16>,
) {
    match node {
        Node::Unit(u) => {
            let mut u = *u;
            if icase && u < 0x80 && rng.below(2) == 0 {
                u = (u as u8 as char).to_ascii_uppercase() as u16;
            }
            out.push(near(rng, u));
        }
        Node::Set { set, negated } => {
            let set = if *negated {
                set.complement()
            } else {
                set.clone()
            };
            if set.is_empty() {
                return;
            }
            // Mostly printable ASCII, when the class has some.
            let ascii = set.intersect(&UnitSet::from_ranges([(0x9, 0xA), (0x20, 0x7E)]));
            let pick = if !ascii.is_empty() && rng.below(5) > 0 {
                ascii
            } else {
                set
            };
            let mut n = rng.below(pick.len());
            for &(a, b) in pick.ranges() {
                let size = usize::from(b - a) + 1;
                if n < size {
                    out.push(near(rng, a + n as u16));
                    return;
                }
                n -= size;
            }
        }
        Node::Group { index, node } => {
            let start = out.len();
            sample(node, rng, icase, groups, out);
            groups.insert(*index, out[start..].to_vec());
        }
        Node::Look {
            behind: false,
            negate: false,
            node,
        } if rng.below(2) == 0 => {
            // What follows a lookahead often starts with it.
            let mut ahead = Vec::new();
            sample(node, rng, icase, groups, &mut ahead);
            out.extend(ahead);
        }
        Node::Backref(k) => out.extend(groups.get(k).cloned().unwrap_or_default()),
        Node::Repeat { node, min, max, .. } => {
            let hi = max.unwrap_or(min + 3).min(min + 3);
            let n = *min as usize + rng.below((hi - min) as usize + 1);
            for _ in 0..n.min(12) {
                sample(node, rng, icase, groups, out);
            }
        }
        Node::Concat(xs) => xs.iter().for_each(|x| sample(x, rng, icase, groups, out)),
        Node::Alt(xs) => sample(&xs[rng.below(xs.len())], rng, icase, groups, out),
        _ => {}
    }
}

/// What the regular expressions `ids` match in `sources`, and in those
/// matches again: the text of tokens and of tokens inside them.
fn token_pool(sources: &[&str], ids: &[RegexId]) -> Vec<Vec<u16>> {
    let g = grammars();
    let mut pool: Vec<Vec<u16>> = Vec::new();
    let mut texts: Vec<Vec<u16>> = sources.iter().map(|s| units(s)).collect();
    for _level in 0..2 {
        let mut found = Vec::new();
        for text in &texts {
            let t = JsText::from_units(text);
            for &id in ids {
                let Ok(re) = g.regex(id) else { continue };
                let mut pos = 0;
                let mut n = 0;
                while n < 40 {
                    let Ok(Some(m)) = re.exec(&t, pos) else { break };
                    if m.end > m.index {
                        found.push(text[m.index..m.end].to_vec());
                        n += 1;
                        pos = m.end;
                    } else {
                        pos = m.index + 1;
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        pool.extend(found.iter().cloned());
        texts = found;
    }
    pool.sort();
    pool.dedup();
    pool
}

fn fuzz_one(
    oracle: &mut Oracle,
    id: RegexId,
    sources: &[&str],
    tokens: &[Vec<u16>],
    strings: usize,
    stats: &mut Stats,
) {
    let g = grammars();
    let e = g.regex_entry(id);
    let Ok(re) = g.regex(id) else {
        return;
    };
    let seed = 0x9E37_79B9_7F4A_7C15 ^ ((id.0 as u64 + 1) * 0x1000_0001);
    let p = Pat {
        source: &e.source,
        flags: &e.flags,
        re,
        spans_only: false,
    };
    fuzz_pattern(oracle, &p, seed, sources, tokens, strings, stats);
}

/// A pattern to fuzz: its JavaScript source and flags, its translation, and
/// whether only the match's span is compared (a hook pattern translated by
/// `translate_spans`, which leaves the groups out).
struct Pat<'a> {
    source: &'a str,
    flags: &'a str,
    re: &'a brrrty_prism::regex::JsRegex,
    spans_only: bool,
}

fn fuzz_pattern(
    oracle: &mut Oracle,
    p: &Pat<'_>,
    seed: u64,
    sources: &[&str],
    tokens: &[Vec<u16>],
    strings: usize,
    stats: &mut Stats,
) {
    struct E<'a> {
        source: &'a str,
        flags: &'a str,
    }
    let e = E {
        source: p.source,
        flags: p.flags,
    };
    let re = p.re;
    let mut rng = Rng(seed);
    let ast = brrrty_jsregex::ast::parse(e.source, e.flags.contains('s')).unwrap();
    let icase = e.flags.contains('i');
    let mut cases = Vec::new();
    for _ in 0..strings {
        let mut text = gen_text(&mut rng, sources, tokens);
        // A third of the strings hold text made from the pattern itself,
        // alone or among the rest.
        if rng.below(3) == 0 {
            let mut made = Vec::new();
            sample(&ast.node, &mut rng, icase, &mut HashMap::new(), &mut made);
            text = match rng.below(3) {
                0 => made,
                1 => [made, text].concat(),
                _ => {
                    let at = rng.below(text.len() + 1);
                    [&text[..at], &made[..], &text[at..]].concat()
                }
            };
            text.truncate(600);
        }
        let n = text.len();
        for last in [0, n, rng.below(n + 1), rng.below(n + 1), rng.below(n + 1)] {
            cases.push(Case {
                text: text.clone(),
                last,
            });
        }
    }
    stats.strings += strings;
    // Node in chunks, so a line stays a manageable size.
    let mut node = Vec::with_capacity(cases.len());
    for chunk in cases.chunks(2000) {
        node.extend(oracle.run(e.source, e.flags, chunk));
    }
    let mut picked = (0, 0, 0);
    for (case, want) in cases.into_iter().zip(node) {
        let text = JsText::from_units(&case.text);
        let got = match rust_exec(re, &text, case.last) {
            Ok(got) if p.spans_only => got.map(|g| g[..2].to_vec()),
            Ok(got) => got,
            Err(_) => {
                stats.refused += 1;
                continue;
            }
        };
        stats.compared += 1;
        if want.is_some() {
            stats.matched += 1;
        }
        let want = if p.spans_only {
            want.map(|w| w[..2].to_vec())
        } else {
            want
        };
        if got != want {
            if stats.diffs.len() < 5 {
                stats.diffs.push(format!(
                    "/{}/{} at {} on {}: got {got:?}, node gave {want:?} (as {})",
                    e.source,
                    e.flags,
                    case.last,
                    json_units(&case.text),
                    re.as_str()
                ));
            }
            continue;
        }
        // A sample for the probes: matches, misses, and text outside the BMP.
        let astral = case.text.iter().any(|u| (0xD800..0xE000).contains(u));
        let take = if astral && picked.2 < 3 {
            picked.2 += 1;
            true
        } else if want.is_some() && picked.0 < 12 {
            picked.0 += 1;
            true
        } else if want.is_none() && picked.1 < 5 {
            picked.1 += 1;
            true
        } else {
            false
        };
        if take {
            stats.probes.push((case, want));
        }
    }
}

fn probe_json(id: RegexId, probes: &[(Case, Got)]) -> String {
    let g = grammars();
    let e = g.regex_entry(id);
    let mut s = String::new();
    writeln!(
        s,
        "  {{\"source\": {}, \"flags\": {}, \"cases\": [",
        serde_json::to_string(&e.source).unwrap(),
        serde_json::to_string(&e.flags).unwrap()
    )
    .unwrap();
    for (i, (case, want)) in probes.iter().enumerate() {
        let text = match String::from_utf16(&case.text) {
            Ok(t) => serde_json::to_string(&t).unwrap(),
            Err(_) => serde_json::to_string(&case.text).unwrap(),
        };
        let m = serde_json::to_string(want).unwrap();
        write!(s, "   {{\"t\": {text}, \"i\": {}, \"m\": {m}}}", case.last).unwrap();
        s.push_str(if i + 1 < probes.len() { ",\n" } else { "\n" });
    }
    s.push_str("  ]}");
    s
}

#[test]
#[ignore = "needs Node; run with --ignored"]
fn every_translated_regex_matches_node() {
    let g = grammars();
    let mut corpus = corpus();
    let sites = std::env::var_os("BRRRTY_FUZZ_SITES").map(PathBuf::from);
    if let Some(dir) = &sites {
        let blocks = site_corpus(dir);
        eprintln!(
            "{} code blocks from {}",
            blocks.values().map(Vec::len).sum::<usize>(),
            dir.display()
        );
        for (lang, blocks) in blocks {
            corpus.entry(lang).or_default().extend(blocks);
        }
    }
    let strings: usize = std::env::var("BRRRTY_FUZZ_STRINGS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let by_regex = sources_by_regex(&corpus);
    let all: Vec<&str> = corpus.values().flatten().map(String::as_str).collect();
    // Each language's tokens, from its own patterns.
    let pools: HashMap<String, Vec<Vec<u16>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = corpus
            .keys()
            .filter_map(|lang| {
                let root = g.language(lang)?;
                let srcs: Vec<&str> = corpus[lang].iter().map(String::as_str).collect();
                Some((
                    lang.clone(),
                    scope.spawn(move || token_pool(&srcs, &g.regexes_of(root))),
                ))
            })
            .collect();
        handles
            .into_iter()
            .map(|(l, h)| (l, h.join().unwrap()))
            .collect()
    });
    let ids: Vec<RegexId> = (0..g.regexes.len()).map(RegexId).collect();
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(10);
    let results: Vec<(RegexId, Stats)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|w| {
                let ids = &ids;
                let by_regex = &by_regex;
                let corpus = &corpus;
                let all = &all;
                let pools = &pools;
                scope.spawn(move || {
                    let mut oracle = Oracle::new();
                    let mut out = Vec::new();
                    for &id in ids.iter().skip(w).step_by(workers) {
                        let mut sources: Vec<&str> = by_regex
                            .get(&id)
                            .map(|langs| {
                                langs
                                    .iter()
                                    .flat_map(|l| corpus.get(l).into_iter().flatten())
                                    .map(String::as_str)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut tokens: Vec<Vec<u16>> = by_regex
                            .get(&id)
                            .map(|langs| {
                                langs
                                    .iter()
                                    .flat_map(|l| pools.get(l).into_iter().flatten())
                                    .cloned()
                                    .collect()
                            })
                            .unwrap_or_default();
                        if sources.is_empty() {
                            sources = all.clone();
                            tokens = pools.values().flatten().cloned().collect();
                        }
                        let mut stats = Stats::default();
                        fuzz_one(&mut oracle, id, &sources, &tokens, strings, &mut stats);
                        out.push((id, stats));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let results: BTreeMap<usize, Stats> = results.into_iter().map(|(id, s)| (id.0, s)).collect();

    let mut report = String::new();
    let (mut diffs, mut compared, mut refused, mut untranslated) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for (&i, s) in &results {
        let e = g.regex_entry(RegexId(i));
        match g.regex(RegexId(i)) {
            Err(why) => {
                untranslated += 1;
                writeln!(
                    report,
                    "UNTRANSLATABLE /{}/{}: {}",
                    e.source, e.flags, why.0
                )
                .unwrap();
            }
            Ok(_) => {
                writeln!(
                    report,
                    "{} /{}/{}: {} strings, {} cases, {} matched, {} refused, {} differ",
                    if s.diffs.is_empty() { "ok" } else { "DIFF" },
                    e.source,
                    e.flags,
                    s.strings,
                    s.compared,
                    s.matched,
                    s.refused,
                    s.diffs.len()
                )
                .unwrap();
                assert!(s.strings >= strings);
            }
        }
        diffs += s.diffs.len();
        compared += s.compared;
        refused += s.refused;
        failures.extend(s.diffs.iter().cloned());
    }
    let dir = Path::new(MANIFEST).join("../../target/prism-fuzz");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.txt"), &report).unwrap();
    eprintln!(
        "{} regexes, {untranslated} untranslatable, {compared} cases compared, {refused} refused, {diffs} differ",
        results.len()
    );
    assert!(
        failures.is_empty(),
        "{} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    if std::env::var_os("BRRRTY_WRITE_PROBES").is_some() {
        assert!(
            sites.is_none(),
            "probes are written from the committed corpus only"
        );
        let mut s = String::new();
        writeln!(
            s,
            "{{\n \"generator\": \"tests/fuzz.rs\",\n \"strings\": {strings},\n \"regexes\": ["
        )
        .unwrap();
        let blocks: Vec<String> = results
            .iter()
            .filter(|(_, st)| !st.probes.is_empty())
            .map(|(&i, st)| probe_json(RegexId(i), &st.probes))
            .collect();
        s.push_str(&blocks.join(",\n"));
        s.push_str("\n ]\n}\n");
        std::fs::write(Path::new(MANIFEST).join("tests/probes/regex.json"), s).unwrap();
    }
}

/// The hooks' own patterns (php and liquid placeholders, diff-highlight's
/// lines and tags, markdown's languages and entities), on the same kind of
/// text plus HTML that Prism wrote (tests/probes/highlight), which is what
/// diff-highlight and markdown's `textContent` read. Report in
/// target/prism-fuzz/hooks.txt.
#[test]
#[ignore = "needs Node; run with --ignored"]
fn hook_regexes_match_node() {
    use brrrty_prism::highlight::{compile_hook_regex, hook_regex, markup_tag_source};
    let corpus = corpus();
    let mut texts: Vec<String> = corpus.values().flatten().cloned().collect();
    let dir = Path::new(MANIFEST).join("tests/probes/highlight");
    for f in std::fs::read_dir(dir).unwrap() {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(f.unwrap().path()).unwrap()).unwrap();
        for c in v["cases"].as_array().unwrap() {
            texts.push(c["code"].as_str().unwrap().to_string());
            texts.push(c["html"].as_str().unwrap().to_string());
        }
    }
    let sources: Vec<&str> = texts.iter().map(String::as_str).collect();
    let strings: usize = std::env::var("BRRRTY_FUZZ_STRINGS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let mut oracle = Oracle::new();
    let mut report = String::new();
    let (mut compared, mut refused, mut diffs) = (0, 0, 0);
    for (i, r) in hook_regex::ALL.iter().enumerate() {
        let source = if r.0 == hook_regex::TAG.0 {
            markup_tag_source()
        } else {
            r.1
        };
        let re = compile_hook_regex(*r).unwrap_or_else(|e| panic!("/{source}/{}: {}", r.2, e.0));
        let p = Pat {
            source,
            flags: r.2,
            re,
            spans_only: r.3,
        };
        // Token pools are the grammars'; the hooks get plain fragments.
        let mut stats = Stats::default();
        fuzz_pattern(
            &mut oracle,
            &p,
            0xC0FF_EE00 + i as u64,
            &sources,
            &[],
            strings,
            &mut stats,
        );
        compared += stats.compared;
        refused += stats.refused;
        diffs += stats.diffs.len();
        writeln!(
            report,
            "{} /{source}/{}: {} cases, {} matched, {} refused, {} differ",
            if stats.diffs.is_empty() { "ok" } else { "DIFF" },
            r.2,
            stats.compared + stats.refused,
            stats.matched,
            stats.refused,
            stats.diffs.len()
        )
        .unwrap();
        for d in &stats.diffs {
            writeln!(report, "  {d}").unwrap();
        }
    }
    let out = Path::new(MANIFEST).join("../../target/prism-fuzz");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("hooks.txt"), &report).unwrap();
    println!("{report}");
    println!(
        "{} hook patterns, {compared} cases compared, {refused} refused, {diffs} differ",
        hook_regex::ALL.len()
    );
    assert_eq!(diffs, 0);
}
