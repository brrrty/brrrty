//! `cargo xtask corpus-report`: Markdown tables from a `corpus` results file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::corpus::{SiteResult, Step};

/// Causes listed in the fine-grained blocker table.
const TOP_CAUSES: usize = 40;

fn median(v: &[u64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2] as f64
    } else {
        (s[n / 2 - 1] + s[n / 2]) as f64 / 2.0
    })
}

fn step(s: &Option<Step>) -> String {
    match s {
        None => "-".into(),
        Some(s) if s.ok => format!("ok {:.1}s", s.secs),
        Some(s) if s.timed_out => "timeout".into(),
        Some(_) => "fail".into(),
    }
}

fn ms(v: Option<f64>) -> String {
    v.map_or("-".into(), |m| format!("{:.0}", m))
}

/// The speedup of `b` over `a`, as medians.
fn speedup(a: &[u64], b: &[u64]) -> Option<f64> {
    let (a, b) = (median(a)?, median(b)?);
    (b > 0.0).then(|| a / b)
}

fn spread(xs: &[f64]) -> String {
    if xs.is_empty() {
        return "no sites".into();
    }
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    let geo = (s.iter().map(|x| x.ln()).sum::<f64>() / s.len() as f64).exp();
    let mid = if s.len() % 2 == 1 {
        s[s.len() / 2]
    } else {
        (s[s.len() / 2 - 1] + s[s.len() / 2]) / 2.0
    };
    format!(
        "{} sites: geometric mean {geo:.1}x, median {mid:.1}x, min {:.1}x, max {:.1}x",
        s.len(),
        s[0],
        s[s.len() - 1]
    )
}

pub fn render(results: &[SiteResult]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Per site\n");
    out.push_str("| # | site | license | 11ty | install | 11ty build | ref files | doctor | init TODOs | brrrty build | identical | equiv | different | missing | extra | 11ty ms (docker) | brrrty ms (docker) | brrrty ms (host) |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for (i, r) in results.iter().enumerate() {
        let doctor = r.doctor.as_ref().map_or_else(
            || {
                r.doctor_error
                    .as_ref()
                    .map_or("-".into(), |_| "error".into())
            },
            |d| {
                format!(
                    "{}/{}{}",
                    d.pages_building,
                    d.pages_total,
                    if d.builds { " builds" } else { "" }
                )
            },
        );
        let (ident, eq, diff, miss, extra) = r.compare.as_ref().map_or(
            ("-".into(), "-".into(), "-".into(), "-".into(), "-".into()),
            |c| {
                let mode = if c.mode == "full" { "" } else { " (p)" };
                (
                    format!("{}{mode}", c.identical),
                    c.equivalent.to_string(),
                    c.different.to_string(),
                    c.missing.to_string(),
                    c.extra.to_string(),
                )
            },
        );
        let t = r.timing.clone().unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {}{} | {} | {} | {} | {} | {ident} | {eq} | {diff} | {miss} | {extra} | {} | {} | {} |",
            i + 1,
            r.slug,
            r.license.as_deref().unwrap_or("-"),
            r.eleventy_version.as_deref().unwrap_or("-"),
            step(&r.install),
            step(&r.reference),
            r.timed_out
                .as_ref()
                .map_or(String::new(), |s| format!(" (timeout: {s})")),
            r.reference_files.map_or("-".into(), |n| n.to_string()),
            doctor,
            r.init_todos.len(),
            step(&r.build),
            ms(median(&t.eleventy_docker_ms)),
            ms(median(&t.brrrty_docker_ms)),
            ms(median(&t.brrrty_host_ms)),
        );
    }

    // Blockers.
    struct Agg {
        title: String,
        sites: BTreeSet<String>,
        pages: u64,
        inexact: bool,
        only: usize,
    }
    let mut blockers: BTreeMap<String, Agg> = BTreeMap::new();
    for r in results {
        let Some(d) = &r.doctor else { continue };
        let real: Vec<_> = d.groups.iter().filter(|g| g.blocker).collect();
        for g in &real {
            let a = blockers.entry(g.key.clone()).or_insert_with(|| Agg {
                title: g.title.clone(),
                sites: BTreeSet::new(),
                pages: 0,
                inexact: false,
                only: 0,
            });
            a.sites.insert(r.slug.clone());
            a.pages += g.pages;
            a.inexact |= !g.pages_exact;
            if real.len() == 1 {
                a.only += 1;
            }
        }
    }
    let mut ranked: Vec<_> = blockers.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.sites
            .len()
            .cmp(&a.1.sites.len())
            .then(b.1.pages.cmp(&a.1.pages))
    });
    let _ = writeln!(out, "\n## Blockers by cause (top {TOP_CAUSES})\n");
    out.push_str("| # | cause | doctor title | sites | pages blocked | sole blocker on |\n|---|---|---|---|---|---|\n");
    for (i, (key, a)) in ranked.iter().take(TOP_CAUSES).enumerate() {
        let _ = writeln!(
            out,
            "| {} | `{key}` | {} | {} | {}{} | {} |",
            i + 1,
            a.title.replace('|', "\\|"),
            a.sites.len(),
            if a.inexact { "at least " } else { "" },
            a.pages,
            a.only
        );
    }

    // Blockers by family, with the templates they block counted once per site.
    #[derive(Default)]
    struct Fam {
        sites: BTreeSet<String>,
        templates: usize,
        pages: u64,
    }
    let mut fams: BTreeMap<String, Fam> = BTreeMap::new();
    for r in results {
        let Some(d) = &r.doctor else { continue };
        let mut per: BTreeMap<&str, (BTreeSet<&str>, u64)> = BTreeMap::new();
        for g in d.groups.iter().filter(|g| g.blocker) {
            let e = per.entry(g.family.as_str()).or_default();
            e.0.extend(g.templates.iter().map(String::as_str));
            e.1 += g.pages;
        }
        for (fam, (templates, pages)) in per {
            let f = fams.entry(fam.to_string()).or_default();
            f.sites.insert(r.slug.clone());
            f.templates += templates.len();
            f.pages += pages;
        }
    }
    let mut fams: Vec<_> = fams.into_iter().collect();
    fams.sort_by(|a, b| {
        b.1.sites
            .len()
            .cmp(&a.1.sites.len())
            .then(b.1.templates.cmp(&a.1.templates))
    });
    let _ = writeln!(out, "\n## Blockers by kind\n");
    out.push_str("Templates are counted once per site per kind; pages add up the groups', so a page blocked twice counts twice.\n\n");
    out.push_str(
        "| # | kind | sites | templates blocked | pages (sum of groups) |\n|---|---|---|---|---|\n",
    );
    for (i, (fam, f)) in fams.iter().enumerate() {
        let _ = writeln!(
            out,
            "| {} | `{fam}` | {} | {} | {} |",
            i + 1,
            f.sites.len(),
            f.templates,
            f.pages
        );
    }

    // Init TODOs.
    let mut todos: BTreeMap<String, (BTreeSet<String>, usize)> = BTreeMap::new();
    let mut plugins: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in results {
        for t in &r.init_todos {
            let key = match &t.api {
                Some(api) => format!("{} (`{api}`)", t.why),
                None => t.why.clone(),
            };
            let e = todos.entry(key).or_default();
            e.0.insert(r.slug.clone());
            e.1 += 1;
            if let Some(p) = &t.plugin {
                plugins.entry(p.clone()).or_default().insert(r.slug.clone());
            }
        }
    }
    let mut todos: Vec<_> = todos.into_iter().collect();
    todos.sort_by(|a, b| b.1.0.len().cmp(&a.1.0.len()).then(b.1.1.cmp(&a.1.1)));
    let _ = writeln!(out, "\n## What `brrrty init` couldn't port\n");
    out.push_str("| # | TODO (config method) | sites | TODOs |\n|---|---|---|---|\n");
    for (i, (k, (sites, n))) in todos.iter().enumerate() {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {n} |",
            i + 1,
            k.replace('|', "\\|"),
            sites.len()
        );
    }
    let mut plugins: Vec<_> = plugins.into_iter().collect();
    plugins.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    if !plugins.is_empty() {
        let _ = writeln!(
            out,
            "\nPlugins in `addPlugin` TODOs (by the name the config imports them as):\n"
        );
        out.push_str("| plugin | sites |\n|---|---|\n");
        for (p, sites) in &plugins {
            let _ = writeln!(out, "| `{p}` | {} |", sites.len());
        }
    }

    // Mismatches.
    let _ = writeln!(out, "\n## Different files\n");
    let mut any = false;
    for r in results {
        let Some(c) = &r.compare else { continue };
        if c.different == 0 {
            continue;
        }
        any = true;
        let _ = writeln!(
            out,
            "- {} ({} build): {} different; first: {}",
            r.slug,
            c.mode,
            c.different,
            c.different_paths
                .iter()
                .take(8)
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !any {
        out.push_str("None.\n");
    }

    // Speed.
    let mut docker = Vec::new();
    let mut host = Vec::new();
    for r in results {
        let Some(t) = &r.timing else { continue };
        if let Some(s) = speedup(&t.eleventy_docker_ms, &t.brrrty_docker_ms) {
            docker.push(s);
        }
        if let Some(s) = speedup(&t.eleventy_docker_ms, &t.brrrty_host_ms) {
            host.push(s);
        }
    }
    let _ = writeln!(out, "\n## Speed (medians of each site's runs)\n");
    let _ = writeln!(
        out,
        "- 11ty in Docker vs Brrrty in Docker: {}",
        spread(&docker)
    );
    let _ = writeln!(
        out,
        "- 11ty in Docker vs Brrrty on the host (not like for like): {}",
        spread(&host)
    );
    out
}

pub fn run_report(results: &Path) -> Result<bool, String> {
    let src = fs::read_to_string(results)
        .map_err(|e| format!("cannot read {}: {e}", results.display()))?;
    let list: Vec<SiteResult> =
        serde_json::from_str(&src).map_err(|e| format!("bad results file: {e}"))?;
    print!("{}", render(&list));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medians_and_spread() {
        assert_eq!(median(&[3, 1, 2]), Some(2.0));
        assert_eq!(median(&[4, 1, 2, 3]), Some(2.5));
        assert_eq!(median(&[]), None);
        assert_eq!(speedup(&[100, 100, 100], &[10, 20, 10]), Some(10.0));
        assert!(spread(&[2.0, 8.0]).contains("geometric mean 4.0x"));
    }

    #[test]
    fn renders_an_empty_run() {
        let md = render(&[]);
        assert!(md.contains("## Blockers by cause"));
        assert!(md.contains("None."));
    }
}
