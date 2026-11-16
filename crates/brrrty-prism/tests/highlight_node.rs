//! Highlighting compared with Node running Prism 1.30.0 through the
//! plugin's own code (tools/gen-highlight-probes.cjs). Both tests are
//! ignored: they need Node and `BRRRTY_NODE_MODULES=<dir>`, a node_modules
//! with prismjs 1.30.0 and @11ty/eleventy-plugin-syntaxhighlight 5.0.2.
//!
//!   BRRRTY_HIGHLIGHT_SITES=<dir> cargo test -p brrrty-prism --release \
//!     --test highlight_node sites -- --ignored --nocapture
//!
//! highlights every fenced code block and `{% highlight %}` body in
//! `<dir>/*/src` (private code: the node output and the report stay in
//! target/prism-highlight/).
//!
//!   cargo test -p brrrty-prism --release --test highlight_node fuzz -- --ignored --nocapture
//!
//! highlights generated code: strings, comments, regular expression
//! literals, template strings and PHP/HTML mixes in JavaScript and PHP
//! (where greedy tokens rematch), and code in every language with emoji,
//! other astral characters, CJK and combining marks. `BRRRTY_FUZZ_CASES`
//! sets the cases per language (default 400).
//!
//! A refusal is never a difference; each is reported with its reason.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use brrrty_prism::grammar::grammars;
use brrrty_prism::{AllLoaded, highlight};
use serde_json::{Value, json};

const MANIFEST: &str = env!("CARGO_MANIFEST_DIR");

fn node_modules() -> String {
    std::env::var("BRRRTY_NODE_MODULES").expect(
        "set BRRRTY_NODE_MODULES to a node_modules with prismjs 1.30.0 and the plugin 5.0.2",
    )
}

fn out_dir() -> PathBuf {
    let d = Path::new(MANIFEST).join("../../target/prism-highlight");
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[derive(Default)]
struct Tally {
    cases: usize,
    same: usize,
    refused: usize,
    plain: usize,
}

#[derive(Default)]
struct Report {
    by_lang: BTreeMap<String, Tally>,
    diffs: Vec<String>,
    refusals: Vec<String>,
}

impl Report {
    /// Compare Brrrty with what node gave (`want`) for `code` in `lang`.
    fn compare(&mut self, lang: &str, code: &str, want: &Value, label: &str) {
        let t = self.by_lang.entry(lang.to_string()).or_default();
        t.cases += 1;
        if want.get("plain").is_some() {
            t.plain += 1;
            return;
        }
        if let Some(e) = want.get("error") {
            self.refusals
                .push(format!("{label} ({lang}): node threw {e}"));
            t.refused += 1;
            return;
        }
        let key = want["grammar"].as_str().unwrap();
        let Some(grammar) = grammars().language(key) else {
            self.diffs
                .push(format!("{label} ({lang}): no grammar {key}"));
            return;
        };
        let started = std::time::Instant::now();
        let got = highlight(code, grammar, lang, &mut AllLoaded::default());
        let took = started.elapsed();
        if took.as_secs() >= 2 {
            eprintln!("slow: {label} ({lang}) took {took:?}");
        }
        match got {
            Ok(html) if html == want["html"].as_str().unwrap() => t.same += 1,
            Ok(html) => self.diffs.push(format!(
                "{label} ({lang}):\n  code {code:?}\n  got  {html:?}\n  node {:?}",
                want["html"].as_str().unwrap()
            )),
            Err(e) => {
                t.refused += 1;
                self.refusals.push(format!("{label} ({lang}): {}", e.0));
            }
        }
    }

    fn write(&self, name: &str) -> String {
        let mut s = String::from("language  cases  identical  refused  plain\n");
        for (lang, t) in &self.by_lang {
            s.push_str(&format!(
                "{lang:<16} {:>6} {:>10} {:>8} {:>6}\n",
                t.cases, t.same, t.refused, t.plain
            ));
        }
        s.push_str(&format!("\n{} differences\n", self.diffs.len()));
        for d in &self.diffs {
            s.push_str(d);
            s.push('\n');
        }
        s.push_str(&format!("\n{} refusals\n", self.refusals.len()));
        for r in &self.refusals {
            s.push_str(r);
            s.push('\n');
        }
        std::fs::write(out_dir().join(name), &s).unwrap();
        s
    }
}

#[test]
#[ignore]
fn sites_match_node() {
    let sites = std::env::var("BRRRTY_HIGHLIGHT_SITES").expect("set BRRRTY_HIGHLIGHT_SITES");
    let file = out_dir().join("sites.jsonl");
    let status = Command::new("node")
        .arg(Path::new(MANIFEST).join("tools/gen-highlight-probes.cjs"))
        .args([&node_modules(), "sites", &sites])
        .arg(&file)
        .status()
        .expect("node runs");
    assert!(status.success());
    let mut report = Report::default();
    for line in std::fs::read_to_string(&file).unwrap().lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        let lang = v["lang"].as_str().unwrap();
        let code = v["code"].as_str().unwrap();
        report.compare(lang, code, &v, v["where"].as_str().unwrap());
    }
    let s = report.write("sites-report.txt");
    // The summary only: the differences hold site code.
    println!("{}", s.split("\n\n").next().unwrap());
    println!(
        "{} differences, {} refusals (details in target/prism-highlight/sites-report.txt)",
        report.diffs.len(),
        report.refusals.len()
    );
    assert!(report.diffs.is_empty());
}

struct Oracle {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Oracle {
    fn new() -> Self {
        let mut child = Command::new("node")
            .arg(Path::new(MANIFEST).join("tools/gen-highlight-probes.cjs"))
            .args([&node_modules(), "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("node runs");
        let out = BufReader::new(child.stdout.take().unwrap());
        Self { child, out }
    }

    fn run(&mut self, lang: &str, code: &str) -> Value {
        let stdin = self.child.stdin.as_mut().unwrap();
        // Node's readline also ends a line at U+2028 and U+2029.
        let req = json!({"lang": lang, "code": code})
            .to_string()
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

/// Pieces of JavaScript where greedy strings, comments, regular expression
/// literals and template strings overlap.
const JS_PIECES: &[&str] = &[
    "\"a\\\"b\"",
    "'it\\'s'",
    "\"un",
    "'un",
    "`t ${a + `in ${b}`} c`",
    "`un",
    "${",
    "}",
    "// line \"q\n",
    "/* block 'q */",
    "/* un",
    "*/",
    "/re[/]g\\/x/gi",
    "/[\"']/",
    "a / b / c",
    "x = /=/.test(s)",
    "/* /re/ */",
    "// /re/\n",
    "'/* not */'",
    "\"// not\"",
    "`// ${'`'}`",
    "<div class=\"a\">{x}</div>",
    "</div>",
    "<br/>",
    "{/* c */}",
    "#!/usr/bin/env node\n",
    "function f(a, b = '/') {",
    "return a ? /x/ : b;",
    "class A extends B {}",
    "async () => {}",
    "\n",
    "\r\n",
    " ",
    "\t",
    ";",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "'",
    "\"",
    "`",
    "/",
    "*",
    "\\",
    "0x1F",
    "1e3",
    "const",
    "let",
    "=>",
    "?.",
    "??",
    "@decorator",
    "import x from 'y';",
];

/// Pieces of PHP, alone and mixed with HTML.
const PHP_PIECES: &[&str] = &[
    "<?php ",
    "<?= ",
    "?>",
    "<? ",
    "\"a $x {$y['k']} \\\" b\"",
    "'it\\'s ?>'",
    "\"un",
    "'un",
    "// c ?> x\n",
    "# c\n",
    "#[Attr]",
    "/* c ?> */",
    "/* un",
    "<<<EOT\nheredoc $x\nEOT;\n",
    "<<<'NOW'\nnowdoc ?>\nNOW;\n",
    "<<<\"QQ\"\nq\nQQ;\n",
    "$a = /* c */ 1;",
    "echo \"<b>\";",
    "<div class=\"<?= $c ?>\">",
    "</div>",
    "<!-- <?php x ?> -->",
    "<script>var s = '?>';</script>",
    "<style>a{color:red}</style>",
    "___PHP0___",
    "function f(?int $a): ?string {",
    "}",
    "\n",
    "\r\n",
    " ",
    "'",
    "\"",
    "?",
    ">",
    "<",
    "/",
    "*",
    "#",
    "\\",
    "{",
    "}",
    "$this->x",
    "namespace A\\B;",
    "use C\\D;",
    "fn($x) => $x * 2",
    "match($x) { 1 => 'a' }",
];

/// Text that splits differently in UTF-16 and UTF-8.
const WIDE: &[&str] = &[
    "😀",
    "👩\u{200D}💻",
    "𝒳",
    "𐍈",
    "🎉🎉",
    "中文",
    "日本語のテキスト",
    "한국어",
    "é",
    "e\u{301}",
    "a\u{308}\u{332}",
    "\u{A0}",
    "\u{2028}",
    "\u{FEFF}",
    "ſ",
    "\u{212A}",
    "İ",
    "ß",
    "Σς",
];

fn make_code(rng: &mut Rng, pieces: &[&str], wide: bool) -> String {
    let mut s = String::new();
    for _ in 0..1 + rng.below(30) {
        if wide && rng.below(4) == 0 {
            s.push_str(rng.pick(WIDE));
        } else {
            s.push_str(rng.pick(pieces));
        }
    }
    s
}

/// The corpus sections and fixture code, by language, for the wide-text
/// fuzz.
fn sources() -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let dir = Path::new(MANIFEST).join("tests/probes/highlight");
    for e in std::fs::read_dir(dir).unwrap() {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
        let lang = v["language"].as_str().unwrap().to_string();
        for c in v["cases"].as_array().unwrap() {
            out.entry(lang.clone())
                .or_default()
                .push(c["code"].as_str().unwrap().to_string());
        }
    }
    out
}

/// A slice of `src` (at character boundaries) with wide text put in.
fn sprinkle(rng: &mut Rng, src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let start = rng.below(chars.len());
    let len = 1 + rng.below(400);
    let mut out = String::new();
    for &c in &chars[start..(start + len).min(chars.len())] {
        if rng.below(12) == 0 {
            out.push_str(rng.pick(WIDE));
        }
        out.push(c);
    }
    out
}

#[test]
#[ignore]
fn fuzz_matches_node() {
    let n: usize = std::env::var("BRRRTY_FUZZ_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let mut oracle = Oracle::new();
    let mut report = Report::default();
    let mut rng = Rng(0x005E_ED7A_B1E5_u64);
    let mut run = |lang: &str, code: String, label: &str, report: &mut Report| {
        let want = oracle.run(lang, &code);
        report.compare(lang, &code, &want, label);
    };
    // Review Focus 2: greedy rematching in JavaScript and PHP.
    for lang in ["javascript", "typescript", "jsx"] {
        for _ in 0..n {
            let code = make_code(&mut rng, JS_PIECES, false);
            run(lang, code, "js mix", &mut report);
        }
    }
    for lang in ["php", "html"] {
        for _ in 0..n {
            let code = make_code(&mut rng, PHP_PIECES, false);
            run(lang, code, "php mix", &mut report);
        }
    }
    for _ in 0..n {
        let inner = make_code(&mut rng, JS_PIECES, true);
        let code = format!("# T\n\n```js\n{inner}\n```\n\ntext `{inner}`\n");
        run("markdown", code, "markdown js", &mut report);
    }
    // The php heredoc guard: labels and bodies with letters that fold
    // differently, quoted and bare, with the letter before, in and after.
    let pairs = [
        ("k", "\u{212a}"),
        ("s", "ſ"),
        ("i", "İ"),
        ("σ", "ς"),
        ("é", "É"),
        ("ß", "SS"),
        ("a", "A"),
    ];
    for (x, y) in pairs {
        for (open, close) in [
            (format!("<<<\"{x}\""), format!("{y};")),
            (format!("<<<\"L{x}\""), format!("L{x};")),
            ("<<<\"EOT\"".to_string(), "EOT;".to_string()),
            (format!("<<<E{x}"), format!("E{y};")),
            ("<<<EOT".to_string(), "EOT;".to_string()),
            ("<<<'NOW'".to_string(), "NOW;".to_string()),
        ] {
            for body in [x, y, "plain"] {
                for before in ["", "// café\n", "<p>é</p>\n"] {
                    let heredoc = format!("$a = {open}\n{body} $v\n{close}\necho {y};\n");
                    run(
                        "php",
                        format!("{before}<?php\n{heredoc}"),
                        "heredoc",
                        &mut report,
                    );
                    run("php", format!("{before}{heredoc}"), "heredoc", &mut report);
                }
            }
        }
    }
    // Review Focus 3: astral, CJK and combining marks, in every language.
    let sources = sources();
    for (lang, codes) in &sources {
        for _ in 0..n {
            let src = &codes[rng.below(codes.len())];
            if src.is_empty() {
                continue;
            }
            let code = sprinkle(&mut rng, src);
            run(lang, code, "wide", &mut report);
        }
        for _ in 0..n / 4 {
            let pieces: &[&str] = if lang.contains("php") {
                PHP_PIECES
            } else {
                JS_PIECES
            };
            let code = make_code(&mut rng, pieces, true);
            run(lang, code, "wide mix", &mut report);
        }
    }
    let s = report.write("fuzz-report.txt");
    println!("{}", s.split("\n\n").next().unwrap());
    println!(
        "{} differences, {} refusals (details in target/prism-highlight/fuzz-report.txt)",
        report.diffs.len(),
        report.refusals.len()
    );
    for d in report.diffs.iter().take(5) {
        println!("{d}");
    }
    assert!(report.diffs.is_empty());
}

/// Each hook pattern brrrty-prism embeds is written, as a JavaScript
/// literal, in the Prism file it comes from (HTML_LINE as the template it
/// is built from).
#[test]
#[ignore]
fn hook_patterns_are_written_in_prism() {
    use brrrty_prism::highlight::{hook_regex, markup_tag_source};
    let nm = PathBuf::from(node_modules());
    let prism = nm.join("prismjs");
    let read = |p: &str| std::fs::read_to_string(prism.join(p)).unwrap();
    let diff = read("plugins/diff-highlight/prism-diff-highlight.js");
    let files = [
        (hook_regex::LANGUAGE, &diff),
        (hook_regex::HTML_TAG, &diff),
        (hook_regex::LINE_PREFIX, &diff),
        (hook_regex::LAST_LINE, &diff),
        (hook_regex::PHP_OPEN, &read("components/prism-php.js")),
        (hook_regex::PHP, &read("components/prism-php.js")),
        (hook_regex::LIQUID, &read("components/prism-liquid.js")),
        (hook_regex::LIQUID_TAG, &read("components/prism-liquid.js")),
        (hook_regex::SHARP, &read("components/prism-markdown.js")),
        (hook_regex::PLUS_PLUS, &read("components/prism-markdown.js")),
        (
            hook_regex::FIRST_WORD,
            &read("components/prism-markdown.js"),
        ),
        (
            hook_regex::LANGUAGE_CLASS,
            &read("components/prism-markdown.js"),
        ),
        (hook_regex::ENTITY, &read("components/prism-markdown.js")),
    ];
    for (r, text) in files {
        let literal = format!("/{}/{}", r.1, r.2);
        assert!(text.contains(&literal), "{literal} isn't in Prism");
    }
    let template = r"/(?:__|[^\r\n<])*(?:\r\n?|\n|(?:__|[^\r\n<])(?![^\r\n]))/.source";
    assert!(diff.contains(template));
    assert!(diff.contains("'gi'"));
    let html_tag = hook_regex::HTML_TAG.1;
    assert_eq!(
        hook_regex::HTML_LINE.1,
        r"(?:__|[^\r\n<])*(?:\r\n?|\n|(?:__|[^\r\n<])(?![^\r\n]))".replace("__", html_tag)
    );
    let markdown = read("components/prism-markdown.js");
    assert!(markdown.contains("RegExp(Prism.languages.markup.tag.pattern.source, 'gi')"));
    assert!(!markup_tag_source().is_empty());
    // diff's prefixes, as prism-diff.js lists them.
    let d = read("components/prism-diff.js");
    for (kind, prefix, _) in brrrty_prism::highlight::PREFIXES {
        assert!(d.contains(&format!("'{kind}': '{prefix}'")), "{kind}");
    }
}
