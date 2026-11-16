// The port of eleventy.config.js. Keep the two in step.

config.add_collection("meta", |api| {
    let posts = api.get_filtered_by_glob("./src/posts/**/*.md").reverse();
    let by_category = object();
    let by_tag = object();
    let categories = [];
    let tags = [];

    for post in posts {
        let category = post.data.category;
        let key = js_string(category);
        if !by_category.contains(key) {
            by_category[key] = [];
        }
        by_category[key].push(post);
        if !categories.contains(category) {
            categories.push(category);
        }

        if truthy(post.data.tags) {
            for tag in post.data.tags {
                let key = js_string(tag);
                if !by_tag.contains(key) {
                    by_tag[key] = [];
                }
                if !tags.contains(tag) {
                    tags.push(tag);
                }
                by_tag[key].push(post);
            }
        }
    }

    let collection = object();
    collection.posts = object();
    collection.posts.categories = by_category;
    collection.posts.tags = by_tag;
    collection.categories = js_sort(categories);
    collection.tags = js_sort(tags);
    collection
});

config.add_collection("posts", |api| {
    api.get_filtered_by_glob("./src/posts/**/*.md").reverse()
});

config.add_collection("postsNewest", |api| {
    api.get_filtered_by_glob("./src/posts/**/*.md")
});

fn urls(items) {
    items.map(|i| i.url)
}

config.add_collection("apiProbe", |api| {
    let o = object();
    o.all = urls(api.get_all());
    o.sorted = urls(api.get_all_sorted());
    o.featured = urls(api.get_filtered_by_tag("featured"));
    o.both = urls(api.get_filtered_by_tags("beta", "Alpha"));
    o.none = urls(api.get_filtered_by_tags());
    o.globs = urls(api.get_filtered_by_glob(["src/extra.md", "./src/posts/2024/**/*.liquid"]));
    o.inputDir = urls(api.get_filtered_by_glob("posts/**/*.md"));
    o.upper = urls(api.get_filtered_by_glob("./SRC/Posts/*/*.MD"));
    o.beta = api.get_filtered_by_tag("beta").slice(0, 2);
    o.count = api.get_all().len();
    o
});

#{
    dir: #{
        input: "src",
    },
}
