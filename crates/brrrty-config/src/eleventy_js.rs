//! What a JavaScript file a build script runs does with 11ty: use its
//! JavaScript API (`import Eleventy from "@11ty/eleventy"`), or spawn its
//! command line (`execFileSync("node", ["node_modules/@11ty/eleventy/cmd.cjs",
//! "--output=x"])`). Read from the file's string literals, never run.

/// A string literal in a JavaScript file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lit {
    start: usize,
    end: usize,
    text: String,
    /// A template literal with `${...}` in it.
    interpolated: bool,
}

/// The string literals of `src`, skipping comments. Regular expression
/// literals aren't told apart, which only matters for a quote inside one.
fn literals(src: &str) -> Vec<Lit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            q @ (b'"' | b'\'' | b'`') => {
                let start = i;
                i += 1;
                let mut text = String::new();
                let mut interpolated = false;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                    } else if q == b'`' && b[i] == b'$' && b.get(i + 1) == Some(&b'{') {
                        interpolated = true;
                    } else if q != b'`' && b[i] == b'\n' {
                        break;
                    }
                    let ch = src[i..].chars().next().unwrap_or(' ');
                    text.push(ch);
                    i += ch.len_utf8();
                }
                out.push(Lit {
                    start,
                    end: (i + 1).min(b.len()),
                    text,
                    interpolated,
                });
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// What 11ty's command line is named by: its `cmd.cjs`, its bin, or the package.
fn names_cli(s: &str) -> bool {
    s.ends_with("@11ty/eleventy/cmd.cjs")
        || s.ends_with("@11ty/eleventy/cmd.js")
        || s.ends_with("/.bin/eleventy")
        || s == "eleventy"
        || s == "@11ty/eleventy"
        || s.starts_with("@11ty/eleventy@")
}

/// What one file does with 11ty.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct JsUse {
    /// It loads `@11ty/eleventy` as a module: the JavaScript API.
    pub api: bool,
    /// Each spawn of 11ty's command line: its arguments when they could be
    /// read from literals, `None` when not.
    pub spawns: Vec<Option<Vec<String>>>,
}

/// The text before `at`, without trailing whitespace.
fn before(src: &str, at: usize) -> &str {
    src[..at].trim_end()
}

/// Functions that start a process: Node's `child_process`, execa, shelljs, zx.
const PROCESS_CALLS: [&str; 13] = [
    "spawn",
    "spawnSync",
    "exec",
    "execSync",
    "execFile",
    "execFileSync",
    "fork",
    "execa",
    "execaSync",
    "execaCommand",
    "execaCommandSync",
    "execaNode",
    "$",
];

/// Calls that only look at a path or print it.
const LOOKS_ONLY: [&str; 16] = [
    "existsSync",
    "exists",
    "statSync",
    "stat",
    "lstatSync",
    "lstat",
    "accessSync",
    "access",
    "readFileSync",
    "realpathSync",
    "log",
    "info",
    "warn",
    "error",
    "debug",
    "assert",
];

/// Where a token sits: inside an array literal or a call's parentheses, with the
/// bracket's position. Brackets inside string literals don't count.
fn enclosing(src: &str, lits: &[Lit], at: usize) -> Option<(u8, usize)> {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut i = at;
    while i > 0 {
        i -= 1;
        if let Some(l) = lits.iter().find(|l| l.start <= i && i < l.end) {
            i = l.start;
            continue;
        }
        match b[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                if depth == 0 {
                    return Some((b[i], i));
                }
                depth -= 1;
            }
            b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// The name of the function whose call's `(` is at `paren`: its last segment
/// (`spawnSync` for `child_process.spawnSync(`).
fn call_name(src: &str, paren: usize) -> &str {
    let head = src[..paren].trim_end();
    let start = head
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .map_or(0, |i| i + 1);
    &head[start..]
}

fn is_process_call(name: &str) -> bool {
    PROCESS_CALLS.contains(&name)
}

/// The literal starting at `pos`, read as an argument: `None` for an
/// interpolation other than `--output`'s value.
fn item_at(lits: &[Lit], pos: usize) -> Option<Option<&Lit>> {
    let lit = lits.iter().find(|l| l.start == pos)?;
    if lit.interpolated && !lit.text.starts_with("--output") {
        return Some(None);
    }
    Some(Some(lit))
}

/// The array items from `pos` (just after an item, or after `[`) to its `]`,
/// all plain literals; then the position after `]`. `None` for anything else.
fn items_to_close(
    src: &str,
    lits: &[Lit],
    mut pos: usize,
    first: bool,
) -> Option<(Vec<String>, usize)> {
    let mut out = Vec::new();
    let mut need_comma = !first;
    loop {
        let rest = src[pos..].trim_start();
        let at = src.len() - rest.len();
        if let Some(r) = rest.strip_prefix(']') {
            return Some((out, src.len() - r.len()));
        }
        let at = if need_comma {
            let r = rest.strip_prefix(',')?.trim_start();
            if let Some(r2) = r.strip_prefix(']') {
                return Some((out, src.len() - r2.len()));
            }
            src.len() - r.len()
        } else {
            at
        };
        let lit = item_at(lits, at)??;
        out.push(lit.text.clone());
        pos = lit.end;
        need_comma = true;
    }
}

/// Whether the array whose `[` is at `open` is used as it is: passed straight
/// to a process call, or a `const` never changed afterwards (no `push`,
/// `unshift`, `splice`, `concat` or index assignment), and nothing is done to
/// the literal itself (`[...].concat(x)`).
fn trusted_array(src: &str, lits: &[Lit], open: usize, close: usize) -> Option<Trust> {
    if src[close..].trim_start().starts_with('.') {
        return None;
    }
    // Up through the literals it sits in (`const tools = [{ cmd, args: [...] }]`) to
    // the call it's passed to, or the `const` that holds it; noting the property
    // names on the way, which mustn't be written to either (`tool.args.push(...)`).
    let mut keys: Vec<String> = Vec::new();
    let mut open = open;
    let binding = regex::Regex::new(r"\bconst\s+([A-Za-z_$][\w$]*)\s*=$").expect("valid");
    let property = regex::Regex::new(r"([A-Za-z_$][\w$]*)\s*:$").expect("valid");
    loop {
        let head = src[..open].trim_end();
        if let Some(c) = binding.captures(head) {
            return match keys.as_slice() {
                [] => {
                    plain_const_ok(src, lits, &c[1], close).map(|calls| Trust { calls, list: None })
                }
                [key] => run_list_ok(src, lits, &c[1], key, close).map(|calls| Trust {
                    calls,
                    list: matching(src, lits, open).map(|end| (open, end)),
                }),
                _ => None,
            };
        }
        let key = property.captures(head).map(|c| c[1].to_string());
        let (b, at) = enclosing(src, lits, open)?;
        match b {
            b'(' if key.is_none() && (head.ends_with('(') || head.ends_with(',')) => {
                return is_process_call(call_name(src, at)).then(|| Trust {
                    calls: vec![at],
                    list: None,
                });
            }
            b'{' if key.is_some() => keys.extend(key),
            b'[' if key.is_none() && (head.ends_with('[') || head.ends_with(',')) => {}
            _ => return None,
        }
        open = at;
    }
}

/// Where `name` is used after `from`, outside string literals.
fn uses(src: &str, lits: &[Lit], name: &str, from: usize) -> Vec<(usize, usize)> {
    let re = regex::Regex::new(&format!(r"(^|[^\w$.]){}\b", regex::escape(name))).expect("valid");
    re.captures_iter(&src[from..])
        .filter_map(|c| {
            let m = c.get(0)?;
            let start = from + m.end() - name.len();
            let inside = lits.iter().any(|l| l.start <= start && start < l.end);
            (!inside).then_some((start, from + m.end()))
        })
        .collect()
}

/// Whether the use at `start..end` is a whole argument of a process call: after
/// its `(` or a `,`, and before a `,` or its `)`. Not spread, not in an
/// expression.
fn whole_spawn_arg(src: &str, lits: &[Lit], start: usize, end: usize) -> bool {
    let pre = src[..start].trim_end();
    let post = src[end..].trim_start();
    (pre.ends_with('(') || (pre.ends_with(',') && !pre.ends_with("...,")))
        && (post.starts_with(',') || post.starts_with(')'))
        && enclosing(src, lits, start)
            .is_some_and(|(b, at)| b == b'(' && is_process_call(call_name(src, at)))
}

/// A `const` argument array: trusted only when every use is a whole argument
/// of a process call (no spread into another array, no other use at all).
fn plain_const_ok(src: &str, lits: &[Lit], name: &str, close: usize) -> Option<Vec<usize>> {
    let all = uses(src, lits, name, close);
    if all.is_empty() || !all.iter().all(|&(s, e)| whole_spawn_arg(src, lits, s, e)) {
        return None;
    }
    all.iter()
        .map(|&(s, _)| enclosing(src, lits, s).map(|(_, p)| p))
        .collect()
}

/// A `const` list of runs (`const tools = [{ cmd, args: [...] }]`), whose
/// arrays are under `key`: trusted only when the list is looped over or
/// destructured into element names, and each element's `key` is only ever
/// passed whole to a process call as `spawnSync(tool.cmd, tool.args)` (or
/// `spawnSync(cmd, args)` with names the loop header destructures). Other
/// properties (`tool.name`, `tool.times.push(...)`) may be read and written.
fn run_list_ok(src: &str, lits: &[Lit], list: &str, key: &str, close: usize) -> Option<Vec<usize>> {
    let ident = r"[A-Za-z_$][\w$]*";
    let loop_head = regex::Regex::new(&format!(
        r"for\s*\(\s*(?:const|let|var)\s+({ident}|\{{[^}}]*\}})\s+of$"
    ))
    .expect("valid");
    let destructure = regex::Regex::new(r"(?:const|let|var)\s+\[([^\]]*)\]\s*=$").expect("valid");
    let mut elements: Vec<String> = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    // Where the names are bound (loop headers, destructuring patterns): uses there
    // are the bindings themselves.
    let mut patterns: Vec<(usize, usize)> = Vec::new();
    let mut calls: Vec<usize> = Vec::new();
    for (s, e) in uses(src, lits, list, close) {
        let pre = src[..s].trim_end();
        let post = src[e..].trim_start();
        if let Some(c) = loop_head.captures(pre) {
            let m = c.get(1).expect("group");
            patterns.push((m.start(), m.end()));
            let pattern = c[1].trim();
            if let Some(inner) = pattern.strip_prefix('{') {
                // `for (const { cmd, args } of tools)`: the name `args` is bound to.
                for part in inner.trim_end_matches('}').split(',') {
                    let (k, v) = part.split_once(':').unwrap_or((part, part));
                    let (k, v) = (k.trim(), v.trim());
                    if k.starts_with("...") || v.contains(['{', '[', '=']) {
                        return None;
                    }
                    if k == key {
                        aliases.push(v.to_string());
                    }
                }
            } else {
                elements.push(pattern.to_string());
            }
        } else if let Some(c) = destructure.captures(pre) {
            let m = c.get(1).expect("group");
            patterns.push((m.start(), m.end()));
            // `const [eleventy, brrrty] = tools`: element names, nothing nested.
            for part in c[1].split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                if part.contains(['{', '[', ':', '=', '.']) {
                    return None;
                }
                elements.push(part.to_string());
            }
        } else if post.starts_with('[') {
            // `tools[0].name`: an element read in place, but never its `key`.
            let end = post.find(']')?;
            let after = post[end + 1..].trim_start();
            let prop = after.strip_prefix('.').map(str::trim_start);
            if prop.is_none_or(|p| {
                p.starts_with(key)
                    && !p[key.len()..]
                        .starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
            }) {
                return None;
            }
        } else {
            return None;
        }
    }
    // Each element name: only property reads and writes, and its `key` only as
    // `spawn(el.x, el.key)`.
    for el in &elements {
        for (s, e) in uses(src, lits, el, close) {
            if patterns.iter().any(|&(a, b)| a <= s && s < b) {
                continue;
            }
            // The element itself, passed somewhere or spread: anything could happen to it.
            let prop = src[e..].strip_prefix('.')?;
            let name_len = prop
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                .unwrap_or(prop.len());
            if &prop[..name_len] != key {
                continue;
            }
            let end = e + 1 + name_len;
            if !whole_spawn_arg(src, lits, s, end) {
                return None;
            }
            // The program before it is the same element's own property.
            let (_, paren) = enclosing(src, lits, s)?;
            let before_arg = src[paren + 1..s].trim().trim_end_matches(',').trim();
            let own = regex::Regex::new(&format!(r"^{}\s*\.\s*{ident}$", regex::escape(el)))
                .expect("valid");
            if !own.is_match(before_arg) {
                return None;
            }
            calls.push(paren);
        }
    }
    // Each destructured `args` name: only as a whole spawn argument.
    for a in &aliases {
        for (s, e) in uses(src, lits, a, close) {
            if patterns.iter().any(|&(a, b)| a <= s && s < b) {
                continue;
            }
            if !whole_spawn_arg(src, lits, s, e) {
                return None;
            }
            calls.push(enclosing(src, lits, s)?.1);
        }
    }
    (!calls.is_empty()).then_some(calls)
}

/// How an argument array is trusted: the process calls that spawn it, and the
/// span of the `const` list of runs it is in, if any.
struct Trust {
    calls: Vec<usize>,
    list: Option<(usize, usize)>,
}

/// One spawn of 11ty's command line that could be read.
struct Read {
    args: Vec<String>,
    /// Items before the program in its array (`--require ./x.cjs` for node).
    before: Vec<String>,
    trust: Trust,
}

/// The position of the bracket that closes the one at `open`.
fn matching(src: &str, lits: &[Lit], open: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        if let Some(l) = lits.iter().find(|l| l.start == i) {
            i = l.end;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// A call's arguments after the program and its argument array: its options
/// (`{ env, cwd }`), as text.
fn call_options(src: &str, lits: &[Lit], paren: usize) -> String {
    call_args_from(src, lits, paren, 2)
}

/// A call's arguments from the `skip`th on, as text.
fn call_args_from(src: &str, lits: &[Lit], paren: usize, skip: usize) -> String {
    let Some(close) = matching(src, lits, paren) else {
        return String::new();
    };
    // Split the arguments at top-level commas.
    let mut parts = Vec::new();
    let mut start = paren + 1;
    let mut i = paren + 1;
    while i < close {
        if let Some(l) = lits.iter().find(|l| l.start == i) {
            i = l.end;
            continue;
        }
        match src.as_bytes()[i] {
            b'(' | b'[' | b'{' => {
                i = matching(src, lits, i).unwrap_or(close);
            }
            b',' => {
                parts.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&src[start..close]);
    parts
        .iter()
        .skip(skip)
        .map(|p| p.trim())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The arguments of the spawn whose program token spans `start..end` (a
/// literal, or a `const` name holding one): `None` when they can't be read
/// with confidence.
fn spawn_args(src: &str, lits: &[Lit], start: usize, end: usize) -> Option<Read> {
    match enclosing(src, lits, start)? {
        // The program in an argument array: the rest of the array.
        (b'[', open) => {
            let (args, close) = items_to_close(src, lits, end, false)?;
            let before = lits
                .iter()
                .filter(|l| l.start > open && l.end <= start)
                .map(|l| l.text.clone())
                .collect();
            let trust = trusted_array(src, lits, open, close)?;
            Some(Read {
                args,
                before,
                trust,
            })
        }
        // The program as a process call's first argument: then an array, or nothing.
        (b'(', paren) => {
            if !is_process_call(call_name(src, paren)) || !src[paren + 1..start].trim().is_empty() {
                return None;
            }
            let direct = |args| {
                Some(Read {
                    args,
                    before: Vec::new(),
                    trust: Trust {
                        calls: vec![paren],
                        list: None,
                    },
                })
            };
            let rest = src[end..].trim_start();
            if rest.starts_with(')') {
                return direct(Vec::new());
            }
            let rest = rest.strip_prefix(',')?.trim_start();
            if rest.starts_with('{') || rest.starts_with(')') {
                return direct(Vec::new());
            }
            let open = src.len() - rest.len();
            rest.strip_prefix('[')?;
            let (args, close) = items_to_close(src, lits, open + 1, true)?;
            let trust = trusted_array(src, lits, open, close)?;
            Some(Read {
                args,
                before: Vec::new(),
                trust,
            })
        }
        _ => None,
    }
}

/// Whether a word names Brrrty's command line.
fn names_brrrty(s: &str) -> bool {
    s == "brrrty"
        || s.ends_with("/.bin/brrrty")
        || s.starts_with("brrrty ")
        || s.contains(" brrrty ")
}

/// Preloads among node's own arguments (`--require ./x.cjs`, `-r x`,
/// `--import=x`), and in a `NODE_OPTIONS` value.
pub(crate) fn preloads(words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let flag = [
            "-r",
            "--require",
            "--import",
            "--loader",
            "--experimental-loader",
        ];
        if flag.contains(&w) {
            out.push(words.get(i + 1).cloned().unwrap_or_default());
            i += 2;
            continue;
        }
        if let Some((f, v)) = w.split_once('=')
            && flag.contains(&f)
        {
            out.push(v.to_string());
        }
        i += 1;
    }
    out
}

/// What a spawn's options change for 11ty: an `env` that can set more than
/// `ELEVENTY_*` variables (`TZ` changes dates), a `cwd` other than the site's
/// folder, a `NODE_OPTIONS` preload. As the `\0` tokens `find` reads.
fn option_tokens(opts: &str) -> Vec<String> {
    let opts = opts.trim();
    let mut out = Vec::new();
    if opts.is_empty() {
        return out;
    }
    if opts.contains("NODE_OPTIONS") {
        out.push("\0preload NODE_OPTIONS set in this file".to_string());
    }
    // Only an inline object whose keys are all known is read; anything else (a
    // variable, a spread, an unknown key) could change 11ty's environment or folder.
    let Some(inner) = opts.strip_prefix('{').and_then(|o| o.strip_suffix('}')) else {
        out.push(format!("\0opts {}", one_line_short(opts)));
        return out;
    };
    for entry in top_level_split(inner) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, value) = match entry.split_once(':') {
            Some((k, v)) => (k.trim().trim_matches(['"', '\'']), Some(v.trim())),
            None => (entry, None),
        };
        match (key, value) {
            ("stdio" | "encoding" | "windowsHide" | "timeout" | "maxBuffer", _) => {}
            ("shell", Some("false")) => {}
            ("env", Some(v)) if env_only_eleventy(v) => {}
            ("env", _) => out.push("\0env set in this file".to_string()),
            ("cwd", Some(v)) => {
                let lit = v.len() >= 2
                    && v.starts_with(['"', '\'', '`'])
                    && v.ends_with(['"', '\'', '`'])
                    && !v.contains("${");
                let dir = v.trim_matches(['"', '\'', '`']).trim_end_matches('/');
                if !lit {
                    out.push("\0cwd set at run time".to_string());
                } else if !(dir.is_empty() || dir == ".") {
                    out.push(format!("\0cwd {}", v.trim_matches(['"', '\'', '`'])));
                }
            }
            ("cwd", None) => out.push("\0cwd set at run time".to_string()),
            _ => out.push(format!("\0opts {}", one_line_short(entry))),
        }
    }
    out
}

/// Whether an `env:` value is an inline object setting only `ELEVENTY_*`
/// variables beside `...process.env`.
fn env_only_eleventy(v: &str) -> bool {
    let Some(inner) = v.strip_prefix('{').and_then(|o| o.strip_suffix('}')) else {
        return false;
    };
    top_level_split(inner)
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .all(|k| {
            k == "...process.env"
                || k.split(':')
                    .next()
                    .is_some_and(|n| n.trim().trim_matches(['"', '\'']).starts_with("ELEVENTY_"))
        })
}

/// `s` split at commas outside brackets and quotes.
fn top_level_split(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0);
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' | '`' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    out.push(&s[start..]);
    out
}

/// `s` on one line, at most 60 characters, for a message.
fn one_line_short(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 60 {
        format!("{}…", flat.chars().take(60).collect::<String>())
    } else {
        flat
    }
}

/// Whether a file does something to its own process that its children inherit, so
/// no spawn in it can be read: writes `process.env`, changes folder, or runs node
/// with its own flags (`fork`, `execArgv`).
fn changes_children(src: &str) -> bool {
    regex::Regex::new(
        r"process\.env\s*(\.\s*[\w$]+|\[[^\]]*\])\s*(=[^=]|\+=|\|\|=|\?\?=)|Object\.assign\s*\(\s*process\.env\b|delete\s+process\.env|process\.chdir\s*\(|\bexecArgv\b|\bfork\s*\(",
    )
    .expect("valid")
    .is_match(src)
}

/// What `src` does with 11ty.
pub(crate) fn analyze(src: &str) -> JsUse {
    let lits = literals(src);
    let mut out = JsUse::default();
    // A spawn needs something that runs a process.
    let runs = ["child_process", "execa", "spawn", "exec", "zx", "$`"]
        .iter()
        .any(|w| src.contains(w));
    let spec =
        regex::Regex::new(r"(\bfrom|\brequire\s*\(|\bimport\s*\(|\bimport)$").expect("valid");
    let assignment =
        regex::Regex::new(r"\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=$").expect("valid");
    // The options of every call that spawns Brrrty: an 11ty run spawned the same way
    // (the same call, or the same options) sees the same environment.
    let brrrty_calls: Vec<(usize, String)> = lits
        .iter()
        .filter(|l| names_brrrty(l.text.trim()))
        .filter_map(|l| {
            let (mut b, mut at) = enclosing(src, &lits, l.start)?;
            if b == b'[' {
                (b, at) = enclosing(src, &lits, at)?;
            }
            (b == b'(' && is_process_call(call_name(src, at)))
                .then(|| (at, call_options(src, &lits, at)))
        })
        .collect();
    let same_env = |calls: &[usize], list: Option<(usize, usize)>| {
        let in_list = list.is_some_and(|(a, b)| {
            lits.iter()
                .any(|l| a < l.start && l.end <= b && names_brrrty(l.text.trim()))
        });
        in_list
            || calls.iter().all(|&c| {
                let opts = call_options(src, &lits, c);
                brrrty_calls.iter().any(|(_, o)| *o == opts)
            })
    };
    let tokens = |read: Read| -> Vec<String> {
        let mut args = read.args;
        for p in preloads(&read.before) {
            args.push(format!("\0preload {p}"));
        }
        let mut opts: Vec<String> = read
            .trust
            .calls
            .iter()
            .flat_map(|&c| option_tokens(&call_options(src, &lits, c)))
            .collect();
        opts.sort();
        opts.dedup();
        args.extend(opts);
        if same_env(&read.trust.calls, read.trust.list) {
            args.push("\0samecall".into());
        }
        args
    };
    for l in &lits {
        let t = l.text.trim();
        let specifier = spec.is_match(before(src, l.start));
        if specifier {
            if t == "@11ty/eleventy" || t.starts_with("@11ty/eleventy/") {
                if t.ends_with("/cmd.cjs") || t.ends_with("/cmd.js") {
                    // Loading cmd.cjs runs the command line with this process's arguments.
                    out.spawns.push(None);
                } else {
                    out.api = true;
                }
            }
            continue;
        }
        if !runs {
            continue;
        }
        let whole = t.contains(' ') && t.split_whitespace().any(names_cli);
        if !names_cli(t) && !whole {
            continue;
        }
        // Built up with `+`: the rest can't be read.
        let joined =
            before(src, l.start).ends_with('+') || src[l.end..].trim_start().starts_with('+');
        // `const cli = "…/cmd.cjs";` and then `cli` once in a spawn: follow it.
        let at = match assignment.captures(before(src, l.start)) {
            Some(c) if before(src, l.start).contains("const") && !joined => {
                let used =
                    regex::Regex::new(&format!(r"\b{}\b", regex::escape(&c[1]))).expect("valid");
                let uses: Vec<_> = used.find_iter(&src[l.end..]).collect();
                match uses.as_slice() {
                    [u] => Some((l.end + u.start(), l.end + u.end())),
                    _ => None,
                }
            }
            Some(_) => None,
            None if joined => None,
            None => Some((l.start, l.end)),
        };
        if whole {
            // A whole command in one string: `execSync("npx @11ty/eleventy --output=x")`,
            // or zx's `` $`…` ``. In a call that only prints it, it's no spawn.
            let ctx = at.and_then(|(s, _)| enclosing(src, &lits, s));
            if let Some((b'(', p)) = ctx
                && matches!(
                    call_name(src, p),
                    "log" | "info" | "warn" | "error" | "debug"
                )
            {
                continue;
            }
            let zx = before(src, l.start).ends_with('$');
            let direct = zx
                || matches!(ctx, Some((b'(', p)) if is_process_call(call_name(src, p))
                    && at.is_some_and(|(s, _)| src[p + 1..s].trim().is_empty()));
            let words: Vec<&str> = t.split_whitespace().collect();
            let k = words.iter().position(|w| names_cli(w)).unwrap_or(0);
            let mut args: Vec<String> = words[k + 1..].iter().map(|w| w.to_string()).collect();
            let unknown = l.interpolated
                && args
                    .iter()
                    .any(|a| a.contains("${") && !a.starts_with("--output"));
            // What runs before the program: `TZ=…`, `NODE_OPTIONS=…`, node's preloads.
            let lead: Vec<String> = words[..k].iter().map(|w| w.to_string()).collect();
            for w in &lead {
                if let Some(v) = w.strip_prefix("TZ=") {
                    args.push(format!("\0env TZ={v}"));
                }
                if let Some(v) = w.strip_prefix("NODE_OPTIONS=") {
                    for p in preloads(&v.split_whitespace().map(str::to_string).collect::<Vec<_>>())
                    {
                        args.push(format!("\0preload {p}"));
                    }
                }
            }
            for p in preloads(&lead) {
                args.push(format!("\0preload {p}"));
            }
            if let Some((b'(', p)) = ctx {
                args.extend(option_tokens(&call_args_from(src, &lits, p, 1)));
                if same_env(&[p], None) {
                    args.push("\0samecall".into());
                }
            }
            out.spawns.push((direct && !unknown).then_some(args));
            continue;
        }
        // An argument of a call that only looks at the file (`fs.existsSync(...)`) or
        // prints it isn't a spawn. `path.join(...)` still is unreadable: its result may
        // be what's spawned.
        if let Some((b'(', p)) = enclosing(src, &lits, l.start)
            && LOOKS_ONLY.contains(&call_name(src, p))
        {
            continue;
        }
        out.spawns.push(
            at.and_then(|(s, e)| spawn_args(src, &lits, s, e))
                .map(tokens),
        );
    }
    if changes_children(src) {
        out.spawns.fill(None);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_and_spawns_are_told_apart() {
        let a = analyze(
            "import Eleventy from \"@11ty/eleventy\";\nconst x = require('@11ty/eleventy/src/Util.js');\n",
        );
        assert!(a.api && a.spawns.is_empty());
        let s = analyze(
            "import { execFileSync } from \"node:child_process\";\n// \"@11ty/eleventy\" in a comment\nexecFileSync(\"node\", [\"node_modules/@11ty/eleventy/cmd.cjs\", `--output=${dir}`, \"--quiet\"], { stdio: \"inherit\" });\n",
        );
        assert!(!s.api);
        assert_eq!(
            s.spawns,
            [Some(vec!["--output=${dir}".into(), "--quiet".into()])]
        );
        let s = analyze("spawn('npx', ['@11ty/eleventy', '--input', 'src'])");
        assert_eq!(s.spawns, [Some(vec!["--input".into(), "src".into()])]);
        let s = analyze("execSync(`npx @11ty/eleventy --output=${out} --quiet`)");
        assert_eq!(
            s.spawns,
            [Some(vec!["--output=${out}".into(), "--quiet".into()])]
        );
        let s = analyze(
            "const cli = \"node_modules/@11ty/eleventy/cmd.cjs\";\nspawnSync(process.execPath, [cli, \"--output=x\"]);\n",
        );
        assert_eq!(s.spawns, [Some(vec!["--output=x".into()])]);
    }

    #[test]
    fn only_arrays_passed_as_they_are_are_read() {
        // A literal array straight into the call, or a `const` left alone.
        let s = analyze(
            "const a = [\"node_modules/@11ty/eleventy/cmd.cjs\", \"--input=src\"];\nspawnSync(\"node\", a);\n",
        );
        assert_eq!(s.spawns, [Some(vec!["--input=src".into()])]);
        let s = analyze("execa(\"eleventy\", [\"--input=src\"], { stdio: \"inherit\" })");
        assert_eq!(s.spawns, [Some(vec!["--input=src".into()])]);
        let s = analyze("$`npx @11ty/eleventy --input=src`");
        assert_eq!(s.spawns, [Some(vec!["--input=src".into()])]);
        for src in [
            // The review's three: a path built with path.join, an array pushed to, a
            // command built with `+`.
            "import path from \"node:path\";\nimport { spawnSync } from \"node:child_process\";\nconst bin = path.join(\"node_modules\", \".bin\", \"eleventy\");\nspawnSync(bin, [\"--input=src\", \"--pathprefix=/b/\"]);\n",
            "import { spawnSync } from \"node:child_process\";\nconst a = [\"node_modules/@11ty/eleventy/cmd.cjs\", \"--input=src\"];\nif (process.env.CI) a.push(\"--pathprefix=/b/\");\nspawnSync(\"node\", a);\n",
            "import { execSync } from \"node:child_process\";\nexecSync(\"npx @11ty/eleventy \" + flags);\n",
            // And their kin.
            "const args = [\"@11ty/eleventy\"]; args.unshift(\"-y\"); spawn(\"npx\", args);",
            "let a = [\"@11ty/eleventy\", \"--input=src\"]; spawn(\"npx\", a);",
            "spawn(\"npx\", [\"@11ty/eleventy\", \"--input=src\"].concat(more));",
            "const a = [\"@11ty/eleventy\"]; a[1] = \"--pathprefix=/b/\"; spawn(\"npx\", a);",
            "spawn(path.resolve(\"node_modules/@11ty/eleventy/cmd.cjs\"), [])",
            "run(\"npx @11ty/eleventy --pathprefix=/b/\"); // run wraps exec",
            "const cmd = \"npx @11ty/eleventy \" + extra; execSync(cmd);",
        ] {
            assert_eq!(analyze(src).spawns, [None], "{src}");
        }
    }

    /// The real blog's `ci/compare.mjs` keeps its runs in a `const` list of objects and
    /// loops over them.
    const TOOLS: &str = "import { spawnSync } from \"node:child_process\";\nconst OUT_11TY = \"_site-11ty\";\nconst tools = [\n  {\n    name: \"11ty\",\n    out: OUT_11TY,\n    cmd: process.execPath,\n    args: [\n      \"--require\", \"./ci/freeze-time.cjs\",\n      \"node_modules/@11ty/eleventy/cmd.cjs\", `--output=${OUT_11TY}`, \"--quiet\",\n    ],\n    times: [],\n  },\n  {\n    name: \"Brrrty\",\n    cmd: \"node_modules/.bin/brrrty\",\n    args: [\"build\", \"--quiet\"],\n  },\n];\nfor (const tool of tools) {\n  const result = spawnSync(tool.cmd, tool.args, { stdio: \"inherit\" });\n  tool.times.push(1);\n}\nconst [eleventy, brrrty] = tools;\nconsole.log(eleventy.times, brrrty.name);\n";

    #[test]
    fn a_const_list_of_runs_is_read() {
        assert_eq!(
            analyze(TOOLS).spawns,
            [Some(vec![
                "--output=${OUT_11TY}".into(),
                "--quiet".into(),
                "\0preload ./ci/freeze-time.cjs".into(),
                "\0samecall".into()
            ])]
        );
        // Written to through the element or a destructured name: not trusted.
        for change in [
            "tool.args.push(\"--pathprefix=/b/\");",
            "tools[0].args = [];",
            "const { args } = tools[0]; args.unshift(\"x\");",
            "tools.push({ cmd: \"x\", args: [] });",
        ] {
            let src = TOOLS.replace("tool.times.push(1);", change);
            assert_eq!(analyze(&src).spawns, [None], "{change}");
        }
        // The review's: flags added at the call, through an alias, a destructured
        // alias, or `Array.prototype.push.call`; and kin.
        let npx = "import { spawnSync } from \"node:child_process\";\nconst tools = [{ cmd: \"npx\", args: [\"@11ty/eleventy\", \"--input=src\"] }];\n";
        let ok = format!("{npx}for (const tool of tools) spawnSync(tool.cmd, tool.args);\n");
        assert_eq!(analyze(&ok).spawns, [Some(vec!["--input=src".into()])]);
        let destructured = format!(
            "{npx}for (const {{ cmd, args }} of tools) spawnSync(cmd, args, {{ stdio: \"inherit\" }});\n"
        );
        assert_eq!(
            analyze(&destructured).spawns,
            [Some(vec!["--input=src".into()])]
        );
        for tail in [
            "for (const tool of tools) spawnSync(tool.cmd, [...tool.args, \"--pathprefix=/b/\"]);",
            "const a = tools[0].args; a.push(\"--pathprefix=/b/\"); for (const tool of tools) spawnSync(tool.cmd, tool.args);",
            "const [{ args: x }] = tools; x.push(\"--pathprefix=/b/\"); for (const tool of tools) spawnSync(tool.cmd, tool.args);",
            "Array.prototype.push.call(tools[0].args, \"--pathprefix=/b/\"); for (const tool of tools) spawnSync(tool.cmd, tool.args);",
            "for (const tool of tools) { addFlags(tool); spawnSync(tool.cmd, tool.args); }",
            "for (const tool of tools) spawnSync(tool.cmd, tool.args.concat(more));",
            "for (const { cmd, args } of tools) spawnSync(cmd, [...args, \"--x\"]);",
            "for (const { cmd, args } of tools) { args.push(\"--x\"); spawnSync(cmd, args); }",
            "tools.forEach((t) => spawnSync(t.cmd, t.args));",
            "for (const tool of tools) spawnSync(other.cmd, tool.args);",
            "for (const tool of tools) console.log(tool.name);",
        ] {
            let src = format!("{npx}{tail}\n");
            assert_eq!(analyze(&src).spawns, [None], "{tail}");
        }
        // A plain const array is held to the same: whole, and only as a spawn argument.
        let plain = "const a = [\"@11ty/eleventy\", \"--input=src\"];\n";
        assert_eq!(
            analyze(&format!("{plain}spawnSync(\"npx\", a);")).spawns,
            [Some(vec!["--input=src".into()])]
        );
        assert_eq!(
            analyze(&format!("{plain}spawnSync(\"npx\", [...a, \"--x\"]);")).spawns,
            [None]
        );
        assert_eq!(
            analyze(&format!(
                "{plain}const b = a; b.push(\"--x\"); spawnSync(\"npx\", a);"
            ))
            .spawns,
            [None]
        );
        // A check that the bin is there doesn't taint the file.
        let s = analyze(&format!(
            "import fs from \"node:fs\";\nif (!fs.existsSync(\"node_modules/.bin/eleventy\")) process.exit(1);\n{TOOLS}"
        ));
        assert_eq!(
            s.spawns,
            [Some(vec![
                "--output=${OUT_11TY}".into(),
                "--quiet".into(),
                "\0preload ./ci/freeze-time.cjs".into(),
                "\0samecall".into()
            ])]
        );
        // A `let` list isn't trusted.
        assert_eq!(
            analyze(&TOOLS.replace("const tools", "let tools")).spawns,
            [None]
        );
    }

    #[test]
    fn a_file_that_changes_what_children_inherit_isnt_read() {
        let spawn = "spawnSync(\"npx\", [\"@11ty/eleventy\", \"--input=src\"]);\n";
        for pre in [
            "process.env.TZ = \"Asia/Tokyo\";",
            "process.env[\"NODE_OPTIONS\"] = \"--require ./p.cjs\";",
            "Object.assign(process.env, { TZ: \"x\" });",
            "process.chdir(\"site\");",
            "const x = { execArgv: [\"-r\", \"./p.cjs\"] };",
            "fork(\"node_modules/@11ty/eleventy/cmd.cjs\", []);",
        ] {
            let src = format!(
                "import {{ spawnSync, fork }} from \"node:child_process\";\n{pre}\n{spawn}"
            );
            assert!(analyze(&src).spawns.iter().all(Option::is_none), "{pre}");
        }
        // Reading process.env is fine.
        let src = format!(
            "const tz = process.env.TZ ?? \"UTC\";\nif (process.env.CI === \"1\") {{}}\n{spawn}"
        );
        assert_eq!(analyze(&src).spawns, [Some(vec!["--input=src".into()])]);
    }

    #[test]
    fn only_known_spawn_options_are_trusted() {
        let with = |opts: &str| {
            analyze(&format!(
                "spawnSync(\"npx\", [\"@11ty/eleventy\"], {opts});"
            ))
            .spawns
            .remove(0)
            .unwrap()
        };
        assert_eq!(
            with("{ stdio: \"inherit\", encoding: \"utf8\", shell: false }"),
            Vec::<String>::new()
        );
        assert_eq!(
            with("{ env: { ...process.env, ELEVENTY_ENV: \"prod\" } }"),
            Vec::<String>::new()
        );
        assert_eq!(with("opts"), ["\0opts opts".to_string()]);
        assert_eq!(
            with("{ ...base, stdio: \"inherit\" }"),
            ["\0opts ...base".to_string()]
        );
        assert_eq!(with("{ shell: true }"), ["\0opts shell: true".to_string()]);
        assert_eq!(with("{ env }"), ["\0env set in this file".to_string()]);
        assert_eq!(with("{ cwd }"), ["\0cwd set at run time".to_string()]);
        assert_eq!(
            with("{ cwd: `${dir}` }"),
            ["\0cwd set at run time".to_string()]
        );
    }

    #[test]
    fn a_mention_without_a_process_is_nothing() {
        assert_eq!(analyze("console.log(\"eleventy\")"), JsUse::default());
    }

    #[test]
    fn spawns_whose_arguments_cant_be_read() {
        for src in [
            "spawn('node', ['node_modules/@11ty/eleventy/cmd.cjs', ...args])",
            "spawn('node', ['node_modules/@11ty/eleventy/cmd.cjs', flag])",
            "spawn('npx', ['@11ty/eleventy', `--input=${dir}`])",
            "execSync(`npx @11ty/eleventy --pathprefix=${p}`)",
            "const cli = 'node_modules/.bin/eleventy'; exec(cli); exec(cli);",
            "require('@11ty/eleventy/cmd.cjs')",
        ] {
            assert_eq!(analyze(src).spawns, [None], "{src}");
        }
    }
}
