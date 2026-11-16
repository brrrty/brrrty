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
