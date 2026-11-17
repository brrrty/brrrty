// brrrty.com, built with Brrrty.
config.add_passthrough_copy("assets");
config.add_passthrough_copy(#{ "assets/favicon.ico": "favicon.ico" });
config.add_passthrough_copy("CNAME");

// The docs sidebar, in the order each page's front matter gives.
config.add_collection("docs", |api|
    api.get_filtered_by_tag("docs").sort(|a, b| a.data.order - b.data.order));

// A heading's id: its text without tags, lowercased, with each run of other
// characters as one hyphen.
fn heading_id(html) {
    let text = regex_replace(html, "<[^>]*>", "", "g").to_lower();
    regex_replace(regex_replace(text, "[^a-z0-9]+", "-", "g"), "^-+|-+$", "", "g")
}

// The h2 and h3 headings of rendered HTML, each as #{ level, id, html }. A
// heading with attributes already is left out, as is one without an id.
fn headings(html) {
    let parts = regex_split(html, "<h([23])>");
    let found = [];
    let seen = #{};
    let i = 1;
    while i + 1 < parts.len() {
        let rest = parts[i + 1];
        let end = rest.index_of("</h");
        if end > 0 {
            let inner = rest.sub_string(0, end);
            let id = heading_id(inner);
            if id != "" {
                let n = seen[id] ?? 0;
                seen[id] = n + 1;
                if n > 0 { id += "-" + n; }
                found.push(#{ level: parts[i], id: id, html: inner });
            }
        }
        i += 2;
    }
    found
}

// The on-page contents for the docs' right-hand column.
config.add_filter("toc", |html| headings(html));

// Gives each h2 and h3 the id `toc` lists, makes its text a link to itself, and
// wraps each table in a box that scrolls sideways.
config.add_filter("prose", |html| {
    let parts = regex_split(html, "<h([23])>");
    let ids = headings(html);
    let out = parts[0];
    let i = 1;
    let k = 0;
    while i + 1 < parts.len() {
        let level = parts[i];
        let rest = parts[i + 1];
        let end = rest.index_of("</h");
        if end > 0 && k < ids.len() && ids[k].html == rest.sub_string(0, end) {
            let id = ids[k].id;
            out += "<h" + level + " id=\"" + id + "\"><a class=\"anchor\" href=\"#" + id + "\">"
                + rest.sub_string(0, end) + "</a>" + rest.sub_string(end);
            k += 1;
        } else {
            out += "<h" + level + ">" + rest;
        }
        i += 2;
    }
    // Wide tables scroll inside their own box instead of the page.
    out = regex_replace(out, "<table>", "<div class=\"table-wrap\"><table>", "g");
    regex_replace(out, "</table>", "</table></div>", "g")
});

// Prism highlighting for Markdown fences, as 11ty's syntaxhighlight plugin does it.
config.add_plugin("syntaxhighlight");

// Markdown pages are plain Markdown: the docs show template syntax in code.
#{ markdownTemplateEngine: false, dir: #{ output: "_site" } }
