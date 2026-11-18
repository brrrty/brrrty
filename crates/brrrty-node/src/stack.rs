//! Where a JavaScript error happened, from V8's stack text: the first frame
//! inside the project and outside `node_modules`, else the first frame.

use std::path::{Path, PathBuf};

/// One stack frame's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub file: PathBuf,
    pub line: u32,
    pub column: Option<u32>,
}

/// The frames of `stack`, in order, that name a file. Forms read:
/// `at fn (file:///abs/x.js:14:9)`, `at file:///abs/x.js:14:9`,
/// `at /abs/x.js:14:9`, `at fn (/abs/x.js:14:9)`, `at async fn (...)`.
pub fn frames(stack: &str) -> Vec<Frame> {
    stack
        .lines()
        .filter_map(|l| l.trim().strip_prefix("at "))
        .filter_map(|rest| {
            // The place is inside the last parentheses, or is the rest.
            let place = match (rest.rfind('('), rest.ends_with(')')) {
                (Some(open), true) => &rest[open + 1..rest.len() - 1],
                _ => rest,
            };
            place_of(place, true)
        })
        .collect()
}

/// `file:///abs/x.js:14:9` or `/abs/x.js:14:9`; the column is optional
/// unless `need_column`.
fn place_of(text: &str, need_column: bool) -> Option<Frame> {
    let path_text = |p: &str| -> Option<PathBuf> {
        if let Some(rest) = p.strip_prefix("file://") {
            let decoded = percent_decode(rest.split('?').next().unwrap_or(rest));
            Some(PathBuf::from(decoded))
        } else if p.starts_with('/') {
            Some(PathBuf::from(p))
        } else {
            None
        }
    };
    let (head, last) = text.rsplit_once(':')?;
    let last: u32 = last.parse().ok()?;
    if let Some((file, line)) = head.rsplit_once(':')
        && let Ok(line) = line.parse::<u32>()
        && let Some(file) = path_text(file)
    {
        return Some(Frame {
            file,
            line,
            column: Some(last),
        });
    }
    if need_column {
        return None;
    }
    Some(Frame {
        file: path_text(head)?,
        line: last,
        column: None,
    })
}

/// A SyntaxError's place, from the first line of `util.inspect(error)`:
/// `file:///abs/x.mjs:3` (ES modules) or `/abs/x.js:3` (CommonJS).
pub fn syntax_header(inspect: &str) -> Option<Frame> {
    let first = inspect.lines().next()?.trim();
    if first.contains(' ') {
        return None;
    }
    place_of(first, false)
}

/// The frame to report: the first inside `root` and outside `node_modules`,
/// else the first.
pub fn pick(frames: &[Frame], root: &Path) -> Option<Frame> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    frames
        .iter()
        .find(|f| {
            let file = f.file.canonicalize().unwrap_or_else(|_| f.file.clone());
            file.starts_with(&root) && !file.components().any(|c| c.as_os_str() == "node_modules")
        })
        .or_else(|| frames.first())
        .cloned()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(file: &str, line: u32, column: Option<u32>) -> Frame {
        Frame {
            file: file.into(),
            line,
            column,
        }
    }

    #[test]
    fn every_frame_form_is_read() {
        let stack = "TypeError: x\n    at fn (file:///abs/a%20b.js:14:9)\n    at file:///abs/c.mjs:2:3\n    at /abs/d.js:4:5\n    at g (/abs/e.cjs:6:7)\n    at async Promise.all (index 0)\n    at node:internal/modules/esm/loader:1:1\n    at async h (file:///abs/f.js?x=1:8:9)";
        assert_eq!(
            frames(stack),
            vec![
                f("/abs/a b.js", 14, Some(9)),
                f("/abs/c.mjs", 2, Some(3)),
                f("/abs/d.js", 4, Some(5)),
                f("/abs/e.cjs", 6, Some(7)),
                f("/abs/f.js", 8, Some(9)),
            ]
        );
    }

    #[test]
    fn syntax_error_headers_are_read() {
        assert_eq!(
            syntax_header("file:///abs/x.mjs:3\nexport default {\n^\n\nSyntaxError: ..."),
            Some(f("/abs/x.mjs", 3, None))
        );
        assert_eq!(
            syntax_header("/abs/x.js:3\n}}\n^"),
            Some(f("/abs/x.js", 3, None))
        );
        assert_eq!(syntax_header("SyntaxError: Unexpected token"), None);
    }

    #[test]
    fn the_first_project_frame_outside_node_modules_wins() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let in_nm = root.join("node_modules/lib/index.js");
        let mine = root.join("src/_data/x.js");
        let outside = PathBuf::from("/elsewhere/y.js");
        let list = vec![
            Frame {
                file: outside.clone(),
                line: 1,
                column: Some(1),
            },
            Frame {
                file: in_nm,
                line: 2,
                column: Some(2),
            },
            Frame {
                file: mine.clone(),
                line: 3,
                column: Some(3),
            },
        ];
        assert_eq!(pick(&list, &root).unwrap().file, mine);
        assert_eq!(pick(&list[..2], &root).unwrap().file, outside);
        assert_eq!(pick(&[], &root), None);
    }
}
