//! The variables 11ty finds in a Nunjucks template for computed data,
//! against `probes/symbols.json` (made by `tools/gen-symbol-probes.cjs`
//! with 11ty 3.1.8's `parseForSymbols` and Nunjucks 3.2.4).

use serde::Deserialize;

#[derive(Deserialize)]
struct Probe {
    src: String,
    symbols: Option<Vec<String>>,
    error: Option<String>,
}

#[test]
fn symbols_match_11ty() {
    let probes: Vec<Probe> =
        serde_json::from_str(include_str!("probes/symbols.json")).expect("valid probes");
    let mut wrong = Vec::new();
    for p in &probes {
        let ours = brrrty_nunjucks::symbols(&p.src);
        match (&p.symbols, ours) {
            (Some(want), Ok(got)) if *want == got => {}
            (None, Err(_)) => {}
            // A template Brrrty refuses but Nunjucks parses is refused
            // in computed data too, never ordered wrong.
            (Some(_), Err(e)) if e.kind == brrrty_nunjucks::ErrorKind::Unsupported => {}
            (want, got) => wrong.push(format!(
                "{:?}: 11ty {:?} / {:?}, Brrrty {:?}",
                p.src, want, p.error, got
            )),
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {}:\n{}",
        wrong.len(),
        probes.len(),
        wrong.join("\n")
    );
}
