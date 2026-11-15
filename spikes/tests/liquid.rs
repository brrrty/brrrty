use brrrty_spike::case::load_cases;
use brrrty_spike::engines::liquid_crate::LiquidCrate;
use brrrty_spike::score::{Outcome, score, unreferenced};
use std::path::Path;

#[test]
fn liquid_crate_runs_all_cases() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/conformance/liquid");
    let cases = load_cases(&dir).unwrap();
    assert!(cases.len() >= 150, "need at least 150 cases");
    assert_eq!(unreferenced(&cases), 0, "run `reference liquid` first");
    let outcomes = score(&LiquidCrate::new(), &cases);
    assert_eq!(outcomes.len(), cases.len());
    let panics: Vec<_> = outcomes
        .iter()
        .filter(|(_, _, o)| matches!(o, Outcome::Panic { .. }))
        .map(|(n, _, _)| n.as_str())
        .collect();
    assert!(panics.is_empty(), "panics: {panics:?}");
}
