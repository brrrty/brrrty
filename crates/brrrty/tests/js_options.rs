//! `config.set_js_data` and `--js-data`: which wins, and where it came from.

use std::fs;

use brrrty::options::{Flags, build_options};
use brrrty_core::{JsData, JsDataSource};

fn site(config: Option<&str>) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    if let Some(c) = config {
        fs::write(t.path().join("brrrty.config.rhai"), c).unwrap();
    }
    t
}

fn mode(config: Option<&str>, flag: Option<JsData>) -> (JsData, JsDataSource) {
    let t = site(config);
    let flags = Flags {
        js_data: flag,
        ..Flags::default()
    };
    let o = build_options(t.path(), &flags).expect("options");
    (o.js.mode, o.js.source)
}

#[test]
fn no_call_and_no_flag_is_off() {
    assert_eq!(mode(None, None), (JsData::Off, JsDataSource::Default));
    assert_eq!(
        mode(Some("let x = 1;"), None),
        (JsData::Off, JsDataSource::Default)
    );
}

#[test]
fn the_config_sets_it_at_its_line() {
    let (m, s) = mode(Some("\nconfig.set_js_data(\"node\");"), None);
    assert_eq!(m, JsData::Node);
    assert_eq!(
        s,
        JsDataSource::Config {
            file: "brrrty.config.rhai".into(),
            line: Some(2)
        }
    );
}

#[test]
fn the_flag_beats_the_config_both_ways() {
    assert_eq!(
        mode(Some("config.set_js_data(\"node\");"), Some(JsData::Off)),
        (JsData::Off, JsDataSource::Flag)
    );
    assert_eq!(
        mode(Some("config.set_js_data(\"off\");"), Some(JsData::Node)),
        (JsData::Node, JsDataSource::Flag)
    );
}

#[test]
fn node_is_looked_for_only_when_on() {
    let t = site(None);
    let off = build_options(t.path(), &Flags::default()).unwrap();
    assert!(off.js.node.is_none() && off.js.start.is_none());
    let on = build_options(
        t.path(),
        &Flags {
            js_data: Some(JsData::Node),
            ..Flags::default()
        },
    )
    .unwrap();
    assert!(on.js.node.is_some());
    assert_eq!(on.js.start.is_some(), matches!(on.js.node, Some(Ok(_))));
}
