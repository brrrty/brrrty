//! Live reload: a script injected into every HTML page opens a WebSocket back to the server,
//! and each rebuild tells the open pages what to do, the way `@11ty/eleventy-dev-server`
//! 2.1.2 does:
//!
//! - a change to stylesheets only, when the page links one of them, swaps the stylesheets in
//!   place;
//! - a change to the page's own template patches the new HTML into the page (morphdom);
//! - any other change reloads the page;
//! - a failed build logs `Build error: …` to the browser console, and the page keeps the
//!   last good build until a fix arrives.
//!
//! Brrrty also logs a build error to pages that connect while it stands, and reports
//! problems writing the output folder the same way.

use std::sync::{Arc, Mutex};

use brrrty_core::{Problem, Severity};
use brrrty_serve::{Handled, HtmlPage, HtmlRewrite, Incoming, Reply, RunningServer, WebSocket};

use super::Rebuild;

// The paths Brrrty answers itself while serving, as the dev server answers `/.11ty/…`. A file
// of the same name in the output folder is hidden; other paths under `/.brrrty/` are not.
const CLIENT_PATH: &str = "/.brrrty/reload-client.js";
const MORPHDOM_PATH: &str = "/.brrrty/morphdom.js";
const SOCKET_PATH: &str = "/.brrrty/live";

const CLIENT_JS: &str = include_str!("reload-client.js");
const MORPHDOM_JS: &str = include_str!("morphdom.js");

/// The tag injected into HTML pages.
pub const SCRIPT_TAG: &str = r#"<script type="module" src="/.brrrty/reload-client.js"></script>"#;

#[derive(Default)]
struct State {
    sockets: Vec<WebSocket>,
    /// The `error` message for the problems that stand, sent to each page that connects.
    error: Option<String>,
}

/// The live-reload channel of one server. Clones share it.
#[derive(Clone, Default)]
pub struct LiveReload(Arc<Mutex<State>>);

impl LiveReload {
    /// Inject the client into `server`'s HTML pages and answer its requests.
    pub fn attach(server: &RunningServer) -> LiveReload {
        let live = LiveReload::default();
        server.set_html_hook(Some(Arc::new(inject_hook)));
        let handler = live.clone();
        server.set_handler(Some(Arc::new(move |req: &Incoming| handler.handle(req))));
        live
    }

    fn handle(&self, req: &Incoming) -> Option<Handled> {
        let js = |body: &str| {
            Some(Handled::Reply(Reply {
                status: 200,
                headers: vec![("Content-Type".into(), "application/javascript".into())],
                body: body.as_bytes().to_vec(),
            }))
        };
        match req.path()? {
            CLIENT_PATH => js(CLIENT_JS),
            MORPHDOM_PATH => js(MORPHDOM_JS),
            SOCKET_PATH => {
                let live = self.clone();
                Some(Handled::WebSocket(Box::new(move |ws| live.connected(ws))))
            }
            _ => None,
        }
    }

    fn connected(&self, ws: WebSocket) {
        let mut state = self.lock();
        let mut ok = ws.send_text(r#"{"type":"connected"}"#).is_ok();
        if let Some(error) = &state.error {
            ok = ok && ws.send_text(error).is_ok();
        }
        if ok {
            state.sockets.push(ws);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How many pages are connected.
    pub fn clients(&self) -> usize {
        let mut state = self.lock();
        state.sockets.retain(WebSocket::is_open);
        state.sockets.len()
    }

    fn broadcast(state: &mut State, message: &str) {
        state.sockets.retain(|ws| ws.send_text(message).is_ok());
    }

    /// Tell the open pages about a rebuild.
    pub fn rebuilt(&self, r: &Rebuild) {
        let mut state = self.lock();
        if !r.ok {
            let message = error_message(&r.problems);
            Self::broadcast(&mut state, &message);
            state.error = Some(message);
            return;
        }
        Self::broadcast(&mut state, &reload_message(r));
        state.error = None;
        if !r.output_problems.is_empty() {
            let message = error_message(&r.output_problems);
            Self::broadcast(&mut state, &message);
            state.error = Some(message);
        }
    }

    /// Log `problems` in the open pages, and in each page that connects until the next good
    /// build. For a first build that failed, before any rebuild.
    pub fn show_problems(&self, problems: &[Problem]) {
        let mut state = self.lock();
        let message = error_message(problems);
        Self::broadcast(&mut state, &message);
        state.error = Some(message);
    }
}

fn inject_hook(page: &HtmlPage) -> HtmlRewrite {
    if !page.is_navigation {
        return HtmlRewrite {
            body: page.body.to_string(),
            headers: Vec::new(),
        };
    }
    // The server's own error page forbids every script; allow the client from this server.
    let headers = if page.is_builtin_error {
        vec![(
            "Content-Security-Policy".to_string(),
            "script-src 'self'".to_string(),
        )]
    } else {
        Vec::new()
    };
    HtmlRewrite {
        body: inject(page.body, SCRIPT_TAG),
        headers,
    }
}

/// Put `script` into `html` where the dev server does: before the first `</head>`, else after
/// an import map, else after the first `</title>`, else before the first `</body>`, else
/// before the first `</html>`, else after the first `<!doctype html>`, else at the end. The
/// tags are matched as written in lowercase, as the dev server matches them.
pub fn inject(html: &str, script: &str) -> String {
    let insert_at = |i: usize| format!("{}{script}{}", &html[..i], &html[i..]);
    if let Some(i) = html.find("</head>") {
        return insert_at(i);
    }
    if let Some(end) = import_map_end(html) {
        return insert_at(end);
    }
    if let Some(i) = html.find("</title>") {
        return insert_at(i + "</title>".len());
    }
    if let Some(i) = html.find("</body>") {
        return insert_at(i);
    }
    if let Some(i) = html.find("</html>") {
        return insert_at(i);
    }
    if let Some(i) = html.find("<!doctype html>") {
        return insert_at(i + "<!doctype html>".len());
    }
    format!("{html}{script}")
}

/// The end of the first import map, as the dev server's
/// `/<script type=\\?importmap\\?[^>]*>(\n|.)*?<\/script>/i` finds it: `type=importmap`
/// unquoted (or with backslashes), in any case, up to the nearest `</script>` with no
/// carriage return or line separator before it.
fn import_map_end(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("<script type=") {
        let start = from + found;
        from = start + 1;
        let mut rest = &lower[start + "<script type=".len()..];
        rest = rest.strip_prefix('\\').unwrap_or(rest);
        let Some(after) = rest.strip_prefix("importmap") else {
            continue;
        };
        let after = after.strip_prefix('\\').unwrap_or(after);
        let Some(close) = after.find('>') else {
            continue;
        };
        let body_start = lower.len() - after.len() + close + 1;
        let Some(end) = lower[body_start..].find("</script>") else {
            continue;
        };
        let body = &html[body_start..body_start + end];
        if body.contains(['\r', '\u{2028}', '\u{2029}']) {
            continue;
        }
        return Some(body_start + end + "</script>".len());
    }
    None
}

fn reload_message(r: &Rebuild) -> String {
    let mut out = String::from(r#"{"type":"reload","css":"#);
    out.push_str(if r.css_only { "true" } else { "false" });
    out.push_str(r#","stylesheets":["#);
    for (i, s) in r.stylesheets.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_string(&mut out, s);
    }
    out.push_str(r#"],"pages":["#);
    for (i, p) in r.pages.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(r#"{"url":"#);
        json_string(&mut out, &p.url);
        out.push_str(r#","content":"#);
        json_string(&mut out, &p.content);
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn error_message(problems: &[Problem]) -> String {
    let errors: Vec<&Problem> = problems
        .iter()
        .filter(|p| matches!(p.severity, Severity::Error))
        .collect();
    let shown: Vec<&Problem> = if errors.is_empty() {
        problems.iter().collect()
    } else {
        errors
    };
    let text = if shown.is_empty() {
        "the build failed".to_string()
    } else {
        shown
            .iter()
            .map(|p| describe(p))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut out = String::from(r#"{"type":"error","message":"#);
    json_string(&mut out, &text);
    out.push('}');
    out
}

/// `path:line:column: message`, as the terminal shows it.
fn describe(p: &Problem) -> String {
    let location = match (&p.path, p.line, p.column) {
        (Some(path), Some(l), Some(c)) => format!("{}:{l}:{c}: ", path.display()),
        (Some(path), Some(l), None) => format!("{}:{l}: ", path.display()),
        (Some(path), None, _) => format!("{}: ", path.display()),
        (None, ..) => String::new(),
    };
    format!("{location}{}", p.message)
}

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "<S>";

    #[test]
    fn the_script_goes_where_the_dev_server_puts_it() {
        let cases = [
            // Before </head>, even when </title> and </body> are there too.
            (
                "<html><head><title>t</title></head><body>x</body></html>",
                "<html><head><title>t</title><S></head><body>x</body></html>",
            ),
            // Only the first </head>.
            ("a</head>b</head>", "a<S></head>b</head>"),
            // After an unquoted import map when there is no </head>.
            (
                "<SCRIPT TYPE=importmap>{}</script><title>t</title>",
                "<SCRIPT TYPE=importmap>{}</script><S><title>t</title>",
            ),
            // A quoted type is not what the dev server's pattern matches.
            (
                "<script type=\"importmap\">{}</script><title>t</title>x",
                "<script type=\"importmap\">{}</script><title>t</title><S>x",
            ),
            // After </title>.
            (
                "<!doctype html><title>x</title><p>title only</p>\n",
                "<!doctype html><title>x</title><S><p>title only</p>\n",
            ),
            // Before </body>, then </html>.
            (
                "<html><body><p>b</p></body></html>",
                "<html><body><p>b</p><S></body></html>",
            ),
            ("<p>a</p></html>", "<p>a</p><S></html>"),
            // After a lowercase doctype only.
            ("<!doctype html><p>d", "<!doctype html><S><p>d"),
            ("<!DOCTYPE html><p>d", "<!DOCTYPE html><p>d<S>"),
            // Uppercase closing tags are not matched.
            (
                "<HEAD></HEAD><BODY>x</BODY>",
                "<HEAD></HEAD><BODY>x</BODY><S>",
            ),
            // Otherwise at the end, even for an empty page.
            ("<p>no head no body</p>\n", "<p>no head no body</p>\n<S>"),
            ("", "<S>"),
        ];
        for (html, want) in cases {
            assert_eq!(inject(html, S), want, "{html}");
        }
    }

    #[test]
    fn an_import_map_with_a_carriage_return_is_not_matched() {
        assert_eq!(
            inject("<script type=importmap>{\r}</script>x", S),
            "<script type=importmap>{\r}</script>x<S>"
        );
        assert_eq!(
            inject("<script type=importmap>{\n}</script>x", S),
            "<script type=importmap>{\n}</script><S>x"
        );
    }

    #[test]
    fn messages_are_json() {
        let r = Rebuild {
            ok: true,
            changed_paths: Vec::new(),
            css_only: false,
            problems: Vec::new(),
            pages: vec![super::super::ChangedPage {
                url: "/a/".into(),
                content: "<p class=\"x\">é\n\u{1}</p>".into(),
            }],
            stylesheets: vec!["/s.css".into()],
            output_problems: Vec::new(),
        };
        assert_eq!(
            reload_message(&r),
            r#"{"type":"reload","css":false,"stylesheets":["/s.css"],"pages":[{"url":"/a/","content":"<p class=\"x\">é\n\u0001</p>"}]}"#
        );
        let p = Problem::error("x", "bad \"thing\"")
            .with_path("src/a.md")
            .at(3, 7);
        let q = Problem::error("y", "no path");
        assert_eq!(
            error_message(&[Problem::warning("w", "just a warning"), p, q]),
            r#"{"type":"error","message":"src/a.md:3:7: bad \"thing\"\nno path"}"#
        );
        // Warnings alone, such as a failed write to the output folder, are shown.
        assert_eq!(
            error_message(&[Problem::warning("w", "cannot write _site/a")]),
            r#"{"type":"error","message":"cannot write _site/a"}"#
        );
        assert_eq!(
            error_message(&[]),
            r#"{"type":"error","message":"the build failed"}"#
        );
    }
}
