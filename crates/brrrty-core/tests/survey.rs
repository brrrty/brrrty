//! Survey builds, which `brrrty doctor` runs: every refusal per template,
//! including the ones a plain build stops before reaching.

use brrrty_core::{BuildOptions, BuildReport, build, codes, survey};
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        ..BuildOptions::default()
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// A layout with a filter Brrrty doesn't have, used by a page that reads a
/// JavaScript data file and one that paginates it.
fn masked_site() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "src/_includes/base.liquid",
        "<title>{{ title | shout }}</title>\n{{ content }}\n",
    );
    write(
        root,
        "src/_data/stuff.js",
        "module.exports = { items: [1, 2] };\n",
    );
    write(
        root,
        "src/reads.liquid",
        "---\nlayout: base.liquid\ntitle: Reads\n---\n{{ stuff.items[0] }}\n",
    );
    write(
        root,
        "src/pages.liquid",
        "---\nlayout: base.liquid\npagination:\n  data: stuff.items\n  size: 1\n---\n{{ pagination.items[0] }}\n",
    );
    write(root, "src/plain.liquid", "plain\n");
    dir
}

fn codes_of(report: &BuildReport, template: &str) -> Vec<&'static str> {
    let survey = report.survey.as_ref().expect("a survey");
    let t = &survey.templates[Path::new(template)];
    t.problems.iter().map(|p| p.code).collect()
}

#[test]
fn a_plain_build_reports_only_the_layout() {
    let dir = masked_site();
    let report = build(&opts(dir.path()), &MarkdownIt::new());
    assert!(report.survey.is_none());
    let codes: Vec<&str> = report.problems.iter().map(|p| p.code).collect();
    assert_eq!(codes, [codes::LIQUID_UNKNOWN_FILTER]);
}

#[test]
fn a_survey_records_what_the_layout_hid() {
    let dir = masked_site();
    let report = survey(&opts(dir.path()), &MarkdownIt::new(), true);
    // What is reported is unchanged.
    let codes: Vec<&str> = report.problems.iter().map(|p| p.code).collect();
    assert_eq!(codes, [codes::LIQUID_UNKNOWN_FILTER]);
    let reads = codes_of(&report, "src/reads.liquid");
    assert_eq!(reads[0], codes::LIQUID_UNKNOWN_FILTER);
    assert!(reads.contains(&codes::DATA_JS_UNSUPPORTED), "{reads:?}");
    let pages = codes_of(&report, "src/pages.liquid");
    assert!(pages.contains(&codes::LIQUID_UNKNOWN_FILTER), "{pages:?}");
    assert!(pages.contains(&codes::DATA_JS_UNSUPPORTED), "{pages:?}");
    let s = report.survey.as_ref().unwrap();
    assert_eq!(
        s.templates[Path::new("src/reads.liquid")].layouts,
        [PathBuf::from("src/_includes/base.liquid")]
    );
    assert_eq!(s.templates[Path::new("src/plain.liquid")].written, 1);
    assert_eq!(s.templates[Path::new("src/reads.liquid")].refused, 1);
    assert_eq!(s.templates[Path::new("src/pages.liquid")].refused, 1);
    assert_eq!(
        s.js_keys.get("stuff"),
        Some(&PathBuf::from("src/_data/stuff.js"))
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let dir = masked_site();
    write(dir.path(), "static/a.txt", "a");
    let mut o = opts(dir.path());
    o.passthrough = vec![PathBuf::from("static")];
    let report = survey(&o, &MarkdownIt::new(), true);
    assert_eq!(report.written, 1);
    assert_eq!(report.copied, 1);
    assert!(!dir.path().join("_site").exists());
}

#[test]
fn a_dry_run_counts_pages_a_passthrough_file_replaces() {
    let dir = masked_site();
    write(dir.path(), "static/plain/index.html", "copied");
    let mut o = opts(dir.path());
    o.config.passthrough = vec![brrrty_core::PassthroughEntry {
        from: "static/plain".into(),
        to: Some("plain".into()),
    }];
    let dry = survey(&o, &MarkdownIt::new(), true);
    let real = build(&o, &MarkdownIt::new());
    assert_eq!(real.written, 0);
    assert_eq!(dry.written, real.written);
}

#[test]
fn a_survey_that_writes_matches_a_build() {
    let dir = masked_site();
    let mut o = opts(dir.path());
    o.output = dir.path().join("out");
    let report = survey(&o, &MarkdownIt::new(), false);
    assert_eq!(report.written, 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("out/plain/index.html")).unwrap(),
        "plain\n"
    );
}

fn found(report: &BuildReport, template: &str) -> Vec<(String, String, Option<usize>)> {
    let survey = report.survey.as_ref().expect("a survey");
    survey.templates[Path::new(template)]
        .problems
        .iter()
        .map(|p| {
            (
                p.message.clone(),
                p.path.as_ref().unwrap().display().to_string(),
                p.line,
            )
        })
        .collect()
}

#[test]
fn the_scan_lists_every_filter_and_data_read_a_render_stops_before() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/_data/stuff.js", "module.exports = {};\n");
    write(root, "src/_data/more.js", "module.exports = {};\n");
    write(
        root,
        "src/_includes/part.njk",
        "{{ x | four }}\n{{ more.y }}\n",
    );
    write(
        root,
        "src/page.njk",
        "---\ntitle: T\n---\n{{ title | one }}\n{% if false %}{{ title | two }}{% endif %}\n{{ stuff.a | upper | three }}\n{% include \"part.njk\" %}\n",
    );
    write(
        root,
        "src/shadow.njk",
        "{% set stuff = 1 %}{{ stuff | nope }}\n",
    );
    write(
        root,
        "src/broken.njk",
        "{% include \"bad.njk\" %}{{ 1 | nope }}\n",
    );
    write(root, "src/_includes/bad.njk", "{% if %}\n");
    let report = survey(&opts(root), &MarkdownIt::new(), true);
    let page = found(&report, "src/page.njk");
    let messages: Vec<&str> = page.iter().map(|(m, _, _)| m.as_str()).collect();
    for want in [
        "Error: filter not found: one",
        "Error: filter not found: two",
        "Error: filter not found: three",
        "Error: filter not found: four",
    ] {
        assert!(messages.contains(&want), "{want} in {page:#?}");
    }
    assert!(
        page.iter().any(|(m, p, l)| m.contains("`stuff`")
            && m.contains("src/_data/stuff.js")
            && p == "src/page.njk"
            && *l == Some(6)),
        "{page:#?}"
    );
    assert!(
        page.iter().any(|(m, p, l)| m.contains("`more`")
            && p == "src/_includes/part.njk"
            && *l == Some(2)),
        "{page:#?}"
    );
    // The build's own report of `one` and the scan's are one problem.
    assert_eq!(
        messages
            .iter()
            .filter(|m| **m == "Error: filter not found: one")
            .count(),
        1
    );
    let shadow = found(&report, "src/shadow.njk");
    assert!(
        !shadow.iter().any(|(m, _, _)| m.contains("`stuff`")),
        "{shadow:#?}"
    );
    let broken = found(&report, "src/broken.njk");
    assert!(
        broken.iter().any(|(_, p, _)| p == "src/_includes/bad.njk"),
        "{broken:#?}"
    );
    // Reported problems are unchanged by the scan.
    let plain = build(&opts(root), &MarkdownIt::new());
    assert_eq!(plain.problems, report.problems);
}

#[test]
fn the_scan_lists_every_unknown_liquid_filter_in_a_layout() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "src/_includes/base.liquid",
        "{{ title | first_one }}\n{% include 'meta' %}\n{{ content }}\n",
    );
    write(
        root,
        "src/_includes/meta.liquid",
        "{{ title | second_one | upcase }}\n",
    );
    write(
        root,
        "src/a.md",
        "---\nlayout: base.liquid\ntitle: A\n---\n# A\n",
    );
    let report = survey(&opts(root), &MarkdownIt::new(), true);
    let a = found(&report, "src/a.md");
    assert!(
        a.iter().any(|(m, p, _)| m == "undefined filter: first_one" && p == "src/_includes/base.liquid"),
        "{a:#?}"
    );
    assert!(
        a.iter().any(|(m, p, l)| m == "undefined filter: second_one"
            && p == "src/_includes/meta.liquid"
            && *l == Some(1)),
        "{a:#?}"
    );
}

#[test]
fn diagnose_groups_every_blocker_by_cause() {
    use brrrty_core::doctor::{Cause, diagnose};
    let dir = masked_site();
    let d = diagnose(survey(&opts(dir.path()), &MarkdownIt::new(), true));
    assert_eq!(d.building, 1);
    assert_eq!(d.pages, 3);
    assert_eq!(d.blockers(), 2);
    // `pages.liquid` paginates the JavaScript data, so its pages can't be counted.
    assert!(!d.pages_exact);
    assert_eq!(
        d.verdict(),
        "1 of at least 3 pages will build; 2 blockers, grouped below."
    );
    assert_eq!(
        d.groups[0].cause,
        Cause::JsData("src/_data/stuff.js".into())
    );
    assert!(d.groups[0].hides_pages);
    assert!(!d.groups[1].hides_pages);
    let filter = d
        .groups
        .iter()
        .find(|g| g.cause == Cause::Filter("shout".into()))
        .unwrap();
    assert_eq!(filter.pages, 2);
    assert_eq!(filter.locations.len(), 1);
    assert_eq!(filter.locations[0].line, Some(1));
    let js = d
        .groups
        .iter()
        .find(|g| g.cause == Cause::JsData("src/_data/stuff.js".into()))
        .unwrap();
    assert_eq!(js.pages, 2);
    assert_eq!(
        js.templates,
        [
            PathBuf::from("src/pages.liquid"),
            PathBuf::from("src/reads.liquid")
        ]
    );
    assert!(!d.builds());
}

#[test]
fn diagnose_a_site_that_builds() {
    use brrrty_core::doctor::diagnose;
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.md", "# A\n");
    write(dir.path(), "src/b.md", "# B\n");
    let d = diagnose(survey(&opts(dir.path()), &MarkdownIt::new(), true));
    assert!(d.builds());
    assert_eq!(d.verdict(), "All 2 pages will build.");
    assert!(d.groups.is_empty());
}

#[test]
fn a_javascript_template_counts_as_at_least_one_page() {
    use brrrty_core::doctor::{Cause, diagnose};
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.md", "# A\n");
    write(dir.path(), "src/gen.11ty.js", "export default () => 'x';\n");
    let d = diagnose(survey(&opts(dir.path()), &MarkdownIt::new(), true));
    assert_eq!(
        d.verdict(),
        "1 of at least 2 pages will build; 1 blocker, grouped below."
    );
    assert_eq!(d.groups[0].cause, Cause::TemplateLanguage("11ty.js".into()));
    assert_eq!(d.groups[0].templates, [PathBuf::from("src/gen.11ty.js")]);
    assert!(!d.groups[0].pages_exact);
}

#[test]
fn a_group_with_an_uncounted_template_is_never_exact() {
    use brrrty_core::doctor::{Cause, diagnose};
    let dir = masked_site();
    let d = diagnose(survey(&opts(dir.path()), &MarkdownIt::new(), true));
    let shout = d
        .groups
        .iter()
        .find(|g| g.cause == Cause::Filter("shout".into()))
        .unwrap();
    // `pages.liquid` uses the layout too, and its pagination never ran.
    assert!(!shout.pages_exact);
    assert!(!shout.hides_pages);
    let js = d
        .groups
        .iter()
        .find(|g| g.cause == Cause::JsData("src/_data/stuff.js".into()))
        .unwrap();
    assert!(js.hides_pages);
    assert!(!js.pages_exact);
}

#[test]
fn a_page_that_needs_refused_content_is_not_a_blocker_of_its_own() {
    use brrrty_core::doctor::{Cause, diagnose};
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/posts/a.njk",
        "---\ntags: post\n---\n{{ 1 | nope }}\n",
    );
    write(
        dir.path(),
        "src/index.njk",
        "{% for p in collections.post %}{{ p.templateContent | safe }}{% endfor %}\n",
    );
    let d = diagnose(survey(&opts(dir.path()), &MarkdownIt::new(), true));
    assert_eq!(d.blockers(), 1, "{:#?}", d.groups);
    let g = &d.groups[0];
    assert_eq!(g.cause, Cause::Filter("nope".into()));
    assert_eq!(g.templates, [PathBuf::from("src/posts/a.njk")]);
    assert_eq!(g.also_templates, [PathBuf::from("src/index.njk")]);
    assert_eq!(g.also_pages, 1);
    assert_eq!(
        d.verdict(),
        "0 of 2 pages will build; 1 blocker, grouped below."
    );
}

#[test]
fn the_scan_reads_permalink_and_computed_data() {
    use brrrty_core::doctor::{Cause, diagnose};
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/_data/more.js", "module.exports = {};\n");
    write(
        dir.path(),
        "src/page.njk",
        "---\nx: hi\npermalink: \"/{{ x | slugish }}/\"\neleventyComputed:\n  title: \"{{ x | seoTitle }}\"\n  meta:\n    description: \"{{ more.d | seoDesc }}\"\n---\n{{ 1 | nope }}\n",
    );
    let d = diagnose(survey(&opts(dir.path()), &MarkdownIt::new(), true));
    for name in ["slugish", "seoTitle", "seoDesc", "nope"] {
        assert!(
            d.groups
                .iter()
                .any(|g| g.cause == Cause::Filter(name.into())),
            "{name} in {:#?}",
            d.groups
        );
    }
    let desc = d
        .groups
        .iter()
        .find(|g| g.cause == Cause::Filter("seoDesc".into()))
        .unwrap();
    assert_eq!(desc.locations[0].line, Some(4));
    assert!(
        desc.locations[0]
            .message
            .ends_with("in `eleventyComputed.meta.description`"),
        "{:?}",
        desc.locations
    );
    assert!(
        d.groups
            .iter()
            .any(|g| g.cause == Cause::JsData("src/_data/more.js".into())),
        "{:#?}",
        d.groups
    );
}
