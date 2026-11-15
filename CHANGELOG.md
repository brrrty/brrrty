# Changelog

## 0.0.2 - 2026-11-15

Behavior change:

- Ignore files now follow 11ty's rules. Each line in `.gitignore`, `.eleventyignore` and `<input>/.eleventyignore` is a path from that file's own folder, not a gitignore pattern that matches at any depth. So a root `.gitignore` line `drafts` ignores `./drafts` but not `src/drafts`. A new `ignore-line-anchored` warning points at lines that would have matched under the old rules, with the full path to write instead.

Changes:

- `<input>/.eleventyignore` is now read.
- Symlinked files and folders in the input folder are followed and built at the link's path.
- Page dates and slugs come from the whole path, like 11ty: a date in a folder name counts, and `index` takes its slug from its folder. An impossible path date like `2026-13-45` is a `date-invalid` error for that page, and the rest of the site still builds.
- Permalinks with no file extension are allowed when the page sets `eleventyAllowMissingExtension` to a truthy value, and for `_redirects`, `.htaccess` and `_headers`.
- Two passthrough sources that copy to the same output file are now a `passthrough-collision` error.
- An input folder outside the project root, through `--input ../x` or a symlink, now builds and honors its `.eleventyignore`.
- Problem paths are shown relative to the project root.

New warnings:

- `ignore-line-anchored`: an ignore line matches nothing where it is anchored, but would have matched pages as a gitignore pattern.
- `ignore-line-invalid`: an ignore line is not a valid pattern, so it is skipped and the build goes on.
- `template-plugin-only`: files such as `.webc`, `.pug` or `.mdx` are skipped because they are templates only when a plugin adds them.
- `symlink-loop` and `symlink-broken`: a link that points back into its own folder, or at a missing file, is skipped.

## 0.0.1 - 2026-11-15

First release: builds plain Markdown and HTML sites the way 11ty does. Templates coming next.
