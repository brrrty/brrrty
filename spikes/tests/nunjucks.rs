use brrrty_spike::case::load_cases;
use brrrty_spike::engines::minijinja::MiniJinja;
use brrrty_spike::score::{Outcome, score, unreferenced};
use std::path::Path;

#[test]
fn minijinja_runs_all_cases() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/conformance/nunjucks");
    let cases = load_cases(&dir).unwrap();
    assert!(cases.len() >= 150, "need at least 150 cases");
    assert_eq!(unreferenced(&cases), 0, "run `reference nunjucks` first");
    let outcomes = score(&MiniJinja::new(), &cases);
    assert_eq!(outcomes.len(), cases.len());
    let panics: Vec<_> = outcomes
        .iter()
        .filter(|(_, _, o)| matches!(o, Outcome::Panic { .. }))
        .map(|(n, _, _)| n.as_str())
        .collect();
    assert!(panics.is_empty(), "panics: {panics:?}");
}

#[test]
fn brrrty_nunjucks_runs_all_cases() {
    use brrrty_spike::engines::brrrty_nunjucks::BrrrtyNunjucks;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/conformance/nunjucks");
    let cases = load_cases(&dir).unwrap();
    let outcomes = score(&BrrrtyNunjucks::new(), &cases);
    assert_eq!(outcomes.len(), cases.len());
    let not_passing: Vec<_> = outcomes
        .iter()
        .filter(|(_, _, o)| {
            matches!(
                o,
                Outcome::Panic { .. } | Outcome::Mismatch { .. } | Outcome::MissingError { .. }
            )
        })
        .map(|(n, _, _)| n.as_str())
        .collect();
    assert!(not_passing.is_empty(), "differs: {not_passing:?}");
}
