//! Hostile code: each piece below repeated (unterminated strings,
//! heredocs, comments, tags, nested templates) and highlighted in every
//! fence language, timed. Each must finish (a refusal is fine) without
//! hanging. Ignored (it takes a while):
//!
//!   cargo test -p brrrty-prism --release --test backtrack -- --ignored --nocapture
//!
//! `BRRRTY_HOSTILE_UNITS` sets the size (default 5000 units).

use std::time::{Duration, Instant};

use brrrty_prism::grammar::grammars;
use brrrty_prism::{AllLoaded, highlight};

const PIECES: &[&str] = &[
    "\"",
    "'",
    "`",
    "`${",
    "${",
    "/*",
    "<!--",
    "<<<EOT\na\n",
    "<<<\"E\"\na\n",
    "<<EOF\na\n",
    "<a b=1 ",
    "<a b=\"",
    "{%",
    "{{",
    "[",
    "(",
    "{",
    "\\",
    "#",
    "//",
    "*",
    "_",
    "**a",
    "```\n",
    "a\n",
    "\n",
    " ",
    "a ",
    "a\t",
    "-",
    "--",
    ">",
    "=",
    "@media (",
    "f\"{",
    "r#\"",
    "$(",
    "$((",
    "%{",
    "<?php ",
    "<?php /*",
    "?",
    "a.",
    "0x",
    "1e",
    "'''",
    "\"\"\"",
    "#{",
    "<script>",
    "<style>",
    "&a",
    "| a",
    "- a: ",
    "a: |\n",
    "[a](",
    "![",
    "<<-EOS\n",
    "q{",
    "s/a/",
];

#[test]
#[ignore]
fn hostile_code_never_hangs() {
    let units: usize = std::env::var("BRRRTY_HOSTILE_UNITS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000);
    let g = grammars();
    let mut langs: Vec<(&str, String)> = g
        .fences
        .iter()
        .filter(|f| f.known && f.normalized != "text")
        .map(|f| (f.name.as_str(), f.normalized.clone()))
        .collect();
    langs.push(("diff", "diff".into()));
    langs.push(("diff-javascript", "diff".into()));
    let mut worst: Vec<(Duration, String)> = Vec::new();
    let mut refused = 0;
    let mut total = Duration::ZERO;
    let mut runs = 0;
    for (lang, key) in &langs {
        let grammar = g.language(key).unwrap();
        for piece in PIECES {
            let n = units / piece.encode_utf16().count().max(1);
            let code = piece.repeat(n);
            let t = Instant::now();
            let r = highlight(&code, grammar, lang, &mut AllLoaded::default());
            let took = t.elapsed();
            total += took;
            runs += 1;
            let what = match &r {
                Ok(_) => "ok".to_string(),
                Err(e) => {
                    refused += 1;
                    format!("refused: {}", e.0)
                }
            };
            worst.push((took, format!("{lang} {piece:?} x{n}: {what}")));
        }
    }
    worst.sort_by_key(|a| std::cmp::Reverse(a.0));
    println!("{runs} runs of about {units} units, {refused} refused, {total:?} in all; slowest:");
    for (d, s) in worst.iter().take(15) {
        println!("  {d:?} {s}");
    }
    let refusals: Vec<_> = worst
        .iter()
        .filter(|(_, s)| s.contains("refused"))
        .collect();
    println!("refusals:");
    for (d, s) in &refusals {
        println!("  {d:?} {s}");
    }
    assert!(
        worst[0].0 < Duration::from_secs(60),
        "a highlight took {:?}",
        worst[0].0
    );
}

/// Highlighting time grows linearly with the number of lines and tokens:
/// each input below, four times longer, takes less than six times as long
/// (best of three runs each). Release builds only (debug timings are
/// meaningless):
///
///   cargo test -p brrrty-prism --release --test backtrack scaling
#[test]
fn time_grows_linearly_with_lines_and_tokens() {
    if cfg!(debug_assertions) {
        eprintln!("skipped: timing needs a release build");
        return;
    }
    let cases: &[(&str, &str, &str, usize)] = &[
        ("diff", "diff", "-a\n+b\n c\n", 2000),
        ("diff-javascript", "diff", "-a = 1;\n+b = 2;\n", 1500),
        (
            "javascript",
            "javascript",
            "const a = f(\"s\", 1) / 2; // c\n",
            1500,
        ),
        ("javascript", "javascript", ";", 40_000),
        ("json", "json", "1,", 40_000),
        ("json", "json", "{\"a\": [1, true, null]},\n", 2000),
        (
            "php",
            "php",
            "<?php echo $a['b']; ?>\n<p class=\"x\">y</p>\n",
            1500,
        ),
        (
            "liquid",
            "liquid",
            "{{ a | b }} {% if c %}<b>d</b>{% endif %}\n",
            1500,
        ),
        (
            "markdown",
            "markdown",
            "# T\n\n- a *b* `c`\n\n```js\nx(1);\n```\n\n",
            800,
        ),
        (
            "html",
            "markup",
            "<div class=\"a\" id='b'>t &amp; u</div>\n",
            2000,
        ),
        ("jsx", "jsx", "<A b={c}>d {e} f</A>;\n", 2000),
        (
            "bash",
            "bash",
            "echo \"$a\" | grep -v x > /dev/null\n",
            2000,
        ),
        ("css", "css", ".a > b:hover { color: #fff; }\n", 2000),
        ("yaml", "yaml", "a:\n  - b: \"c\"\n", 2000),
        ("python", "python", "def f(a, b='c'):\n    return a\n", 2000),
        ("sql", "sql", "SELECT a FROM b WHERE c = 'd';\n", 2000),
        ("jsx", "jsx", "<div>a {b} c d </div>\n", 2000),
        ("markdown", "markdown", "_", 8000),
        ("nginx", "nginx", "a ", 8000),
    ];
    let g = grammars();
    let time = |lang: &str, key: &str, code: &str| {
        let grammar = g.language(key).unwrap();
        (0..3)
            .map(|_| {
                let t = Instant::now();
                highlight(code, grammar, lang, &mut AllLoaded::default()).unwrap();
                t.elapsed()
            })
            .min()
            .unwrap()
    };
    let mut slow = Vec::new();
    for &(lang, key, piece, n) in cases {
        let small = time(lang, key, &piece.repeat(n));
        let large = time(lang, key, &piece.repeat(4 * n));
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-4);
        println!(
            "{lang} {piece:?}: x{n} {small:?}, x{} {large:?}, {ratio:.1}x",
            4 * n
        );
        // Below a few milliseconds, noise dominates.
        if ratio >= 6.0 && large > Duration::from_millis(20) {
            slow.push(format!("{lang} {piece:?}: {ratio:.1}x"));
        }
    }
    assert!(slow.is_empty(), "not linear: {slow:?}");
}
