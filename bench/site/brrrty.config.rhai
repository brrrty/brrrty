// The port of eleventy.config.js. Keep the two in step.
config.add_passthrough_copy("src/images");
config.add_passthrough_copy("src/css");

// Text without tags, with each run of whitespace as one space.
fn plain(html) {
    let text = regex_replace(js_string(html ?? ""), "<[^>]*>", " ", "g");
    text = regex_replace(text, "[ \n\t]+", " ", "g");
    regex_replace(text, "^ | $", "", "g")
}

// Pages of `size` posts for each name, newest first.
fn paged(posts, names, field, size, key) {
    let pages = [];
    for name in names {
        let list = [];
        for post in posts {
            if has(post, field, name) { list.push(post); }
        }
        let total_pages = (list.len() + size - 1) / size;
        for i in 0..total_pages {
            let p = object();
            p[key] = name;
            p.pageNumber = i + 1;
            p.totalPages = total_pages;
            p.total = list.len();
            p.posts = list.slice(i * size, (i + 1) * size);
            pages.push(p);
        }
    }
    pages
}

fn counted(posts, field, names) {
    let out = [];
    for name in names {
        let count = 0;
        for post in posts {
            if has(post, field, name) { count += 1; }
        }
        let c = object();
        c.name = name;
        c.count = count;
        out.push(c);
    }
    out
}

fn has(post, field, name) {
    if field == "tags" { post.data.tags.contains(name) } else { post.data.category == name }
}

fn tag_names(posts) {
    let names = [];
    for post in posts {
        for tag in post.data.tags {
            if !names.contains(tag) { names.push(tag); }
        }
    }
    names.sort()
}

fn category_names(posts) {
    let names = [];
    for post in posts {
        if !names.contains(post.data.category) { names.push(post.data.category); }
    }
    names.sort()
}

config.add_filter("isoDate", |date| luxon_format(date, "yyyy-LL-dd", "utc"));
config.add_filter("readableDate", |date| luxon_format(date, "LLLL d, yyyy", "utc"));
config.add_filter("readingTime", |html| {
    let text = plain(html);
    let words = if text == "" { 0 } else { regex_split(text, " ").len() };
    let minutes = (words.to_float() / 200.0).ceiling();
    if minutes < 1.0 { 1.0 } else { minutes }
});
config.add_filter("excerpt", |html, length| {
    let length = (length ?? 160).to_int();
    let text = plain(html);
    if text.len() <= length {
        text
    } else {
        regex_replace(text.sub_string(0, length), " [^ ]*$", "", "") + "..."
    }
});

config.add_collection("posts", |api| api.get_filtered_by_glob("./src/posts/*.md"));
config.add_collection("tagPages", |api| {
    let posts = api.get_filtered_by_glob("./src/posts/*.md").reverse();
    paged(posts, tag_names(posts), "tags", 10, "tag")
});
config.add_collection("categoryPages", |api| {
    let posts = api.get_filtered_by_glob("./src/posts/*.md").reverse();
    paged(posts, category_names(posts), "category", 20, "category")
});
config.add_collection("tagList", |api| {
    let posts = api.get_filtered_by_glob("./src/posts/*.md");
    counted(posts, "tags", tag_names(posts))
});
config.add_collection("categoryList", |api| {
    let posts = api.get_filtered_by_glob("./src/posts/*.md");
    counted(posts, "category", category_names(posts))
});

config.add_plugin("syntaxhighlight");
config.add_plugin("rss", #{
    type: "atom",
    output_path: "/feed.xml",
    collection: #{ name: "posts", limit: 20 },
    metadata: #{
        language: "en",
        title: "Example Field Notes",
        subtitle: "A synthetic blog for benchmarking static site generators.",
        base: "https://example.com/",
        author: #{ name: "A. Writer", email: "writer@example.com" },
    },
});

#{ dir: #{ input: "src", output: "_site" } }
