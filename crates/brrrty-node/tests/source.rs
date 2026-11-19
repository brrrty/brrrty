//! The worker's source is handed to Node without its comment lines (see
//! `worker_source` in the client). The stripping goes by lines: a line
//! whose first characters are `//`. That is only right while every such
//! line is a comment, which this test asks a real parser (Node's own
//! acorn): each one must be a line comment of the worker, or, inside the
//! one template literal that holds a program (the watcher thread's), a
//! line comment of that program.

use std::process::Command;

const CHECK: &str = r#"
const fs = require("node:fs");
const source = fs.readFileSync(0, "utf8");
const m = { exports: {} };
new Function("exports", "module", process.binding("natives")["internal/deps/acorn/acorn/dist/acorn"])(m.exports, m);
const acorn = m.exports;
function lineComments(text, options) {
  const starts = new Set();
  const templates = [];
  const tree = acorn.parse(text, {
    ecmaVersion: "latest",
    ...options,
    onComment(block, _t, start) { if (!block) starts.add(start); },
  });
  const stack = [tree];
  while (stack.length) {
    const node = stack.pop();
    if (node.type === "TemplateLiteral") templates.push([node.start + 1, node.end - 1]);
    for (const k of Object.keys(node)) {
      const child = node[k];
      if (Array.isArray(child)) { for (const c of child) if (c && typeof c.type === "string") stack.push(c); }
      else if (child && typeof child.type === "string") stack.push(child);
    }
  }
  return { starts, templates };
}
function check(text, options, depth) {
  const { starts, templates } = lineComments(text, options);
  const bad = [];
  let at = 0;
  let n = 0;
  for (const line of text.split("\n")) {
    n++;
    const first = at + (line.length - line.trimStart().length);
    if (line.trimStart().startsWith("//") && !starts.has(first)) {
      const inside = templates.find(([a, b]) => first >= a && first < b);
      if (!inside || depth > 0) bad.push(`line ${n}: ${line.trim().slice(0, 60)}`);
      else inside.used = true;
    }
    at += line.length + 1;
  }
  // A template literal with such lines must be a program whose line
  // comments they are (its `${...}` are numbers, its `\\` a backslash).
  for (const t of templates) {
    if (!t.used) continue;
    const inner = text.slice(t[0], t[1]).replace(/\$\{[^}]*\}/g, "0").replace(/\\(.)/g, "$1");
    try {
      bad.push(...check(inner, { sourceType: "script", allowReturnOutsideFunction: true }, depth + 1).map((b) => `in the template at ${t[0]}: ${b}`));
    } catch (e) {
      bad.push(`the template at ${t[0]} holds lines starting with // and is not a program: ${e.message}`);
    }
  }
  return bad;
}
const bad = check(source, { sourceType: "module", allowAwaitOutsideFunction: true }, 0);
if (bad.length) { console.log(bad.join("\n")); process.exit(1); }
"#;

#[test]
fn every_line_the_client_strips_is_a_comment() {
    if Command::new("node").arg("--version").output().is_err() {
        assert!(std::env::var_os("CI").is_none(), "CI needs Node");
        eprintln!("skipped: no node on PATH");
        return;
    }
    let source = concat!(
        include_str!("../src/worker/encode.mjs"),
        "\n",
        include_str!("../src/worker/proxy.mjs"),
        "\n",
        include_str!("../src/worker/main.mjs"),
    );
    use std::io::Write;
    let mut child = Command::new("node")
        .args(["--no-deprecation", "-e", CHECK])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run node");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "lines that start with // and are not comments:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
