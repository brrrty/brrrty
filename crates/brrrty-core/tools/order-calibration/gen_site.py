"""Random sites weighted toward data files, layout chains and directory-data
layouts. usage: python3 gen_site.py OUTDIR SEED [undated] [src] [reads] [njk] [dates] [imports] [config]
(writes OUTDIR/site and meta.json)

meta.json lists each template with: path, ext, data-file count, the spelled
layout key it uses (front matter, else directory data), and whether dated.
"""
import json, os, random, sys

out, seed = sys.argv[1], int(sys.argv[2])
flags = sys.argv[3:]
undated = "undated" in flags
# Nunjucks templates and layouts too: Brrrty places them in the order
# without rendering them.
njk = "njk" in flags
# Every way a page gets its date: front matter, a YYYY-MM-DD in the file or
# folder name, `Last Modified` / `Created`, or none (the last three read the
# disk, so those pages come last).
dates = "dates" in flags
# `eleventyImport.collections` in front matter and directory data, and
# templates that paginate a collection: both reorder 11ty's collections graph.
imports = "imports" in flags
# Collections the config adds (`addCollection` in eleventy.config.js, and
# the same in brrrty.config.rhai), and templates that are tagged with,
# paginate, import, or paginate a path into one. These reorder 11ty's
# collections graph: a config collection is computed under `[keys]`.
config = "config" in flags
TAGS = ["t0", "t1", "t2", "t3", "t4"]
rng = random.Random(seed)
CONFIG = []
# The config collections that return an object, `{ x: items, n: count }`.
NESTED = set()
if config:
    # A name may also be a tag: the config's collection wins.
    CONFIG = rng.sample(["c0", "c1", "c2", "t2"], rng.randint(1, 3))
    NESTED = {c for c in CONFIG if c.startswith("c") and rng.random() < 0.5}
# On a config site, a name that is neither a tag nor a config collection is
# refused by design (the JavaScript config may add it), so don't import one.
UNKNOWN = [] if config else ["none"]
site = os.path.join(out, "site", "src") if "src" in flags else os.path.join(out, "site")
os.makedirs(site, exist_ok=True)


def w(rel, text):
    p = os.path.join(site, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(text)


# Layouts: some plain, some chains, mixed file types.
layouts = {}
names = ["base", "main", "post", "page", "wide", "note"]
rng.shuffle(names)
for i, name in enumerate(names[: rng.randint(1, 5)]):
    ext = rng.choice(["liquid", "liquid", "liquid", "md", "html"] + (["njk", "njk"] if njk else []))
    parent = rng.choice([None, None] + list(layouts)) if layouts else None
    layouts[name] = (ext, parent)
    front = f"---\nlayout: {parent}.{layouts[parent][0]}\n---\n" if parent else ""
    w(f"_includes/{name}.{ext}", front + ("<x>{{ content | safe }}</x>" if ext == "njk" else "<x>{{ content }}</x>"))

dirs = ["", "", "posts/", "docs/", "posts/2020/", "a/b/"] + (["2019-12-31/", "posts/", "posts/"] if dates else [])
dir_layout = {}
dir_files = {}
for d in set(dirs):
    if not d:
        continue
    leaf = d.rstrip("/").split("/")[-1]
    if rng.random() < 0.5:
        data = {"k": 1}
        if layouts and rng.random() < 0.5:
            lname = rng.choice(list(layouts))
            data["layout"] = f"{lname}.{layouts[lname][0]}"
            dir_layout[d] = data["layout"]
        if imports and rng.random() < 0.4:
            data["eleventyImport"] = {"collections": rng.sample(TAGS + CONFIG + ["all"] + UNKNOWN, rng.randint(1, 2))}
        w(f"{d}{leaf}.json", json.dumps(data))
        dir_files[d] = dir_files.get(d, 0) + 1
    if rng.random() < 0.2:
        w(f"{d}{leaf}.11tydata.json", '{"j": 2}')
        dir_files[d] = dir_files.get(d, 0) + 1

meta = []
used = set()
for i in range(rng.randint(4, 14)):
    d = rng.choice(dirs)
    ext = rng.choice(["md", "liquid", "html"] + (["njk"] if njk else []))
    stem = rng.choice(["a", "b", "c", "x", "Post", "z1", "q"]) + str(rng.randint(0, 9))
    mode = rng.choice(["fm", "name", "name", "name", "lastmod", "created", "none"]) if dates else None
    if mode == "name":
        stem = f"2020-0{rng.randint(1, 9)}-1{rng.randint(0, 9)}-{stem}"
    rel = f"{d}{stem}.{ext}"
    if rel.lower() in used:
        continue
    used.add(rel.lower())
    front = {}
    if mode is not None:
        if mode == "fm":
            front["date"] = "2020-01-01"
        elif mode == "lastmod":
            front["date"] = "Last Modified"
        elif mode == "created":
            front["date"] = "Created"
    elif not undated or rng.random() < 0.5:
        front["date"] = "2020-01-01"
    lay = None
    if layouts and rng.random() < 0.6:
        lname = rng.choice(list(layouts))
        spelled = rng.choice([f"{lname}.{layouts[lname][0]}", f"{lname}.{layouts[lname][0]}", lname])
        front["layout"] = spelled
        lay = spelled
    front["tags"] = rng.choice(TAGS + CONFIG)
    if imports and rng.random() < 0.4:
        picks = rng.sample(TAGS + CONFIG + ["all", "[keys]"] + UNKNOWN, rng.randint(1, 3))
        front["eleventyImport"] = "{collections: [" + ", ".join(json.dumps(t) for t in picks) + "]}"
    if config and ext != "njk" and rng.random() < 0.25:
        # A config collection, or a path into one that returns an object.
        # 11ty refuses a template that paginates a collection it is in.
        target = rng.choice([c + ".x" if c in NESTED and rng.random() < 0.7 else c
                             for c in CONFIG if c != front["tags"]] or ["all"])
        front["pagination"] = "{data: collections." + target + ", size: 1, alias: it}"
        front["permalink"] = '"' + d + stem + '-{{ pagination.pageNumber }}/"'
    elif imports and ext != "njk" and rng.random() < 0.08:
        target = rng.choice([t for t in TAGS if t != front["tags"]] + ["all"])
        front["pagination"] = "{data: collections." + target + ", size: 1, alias: it}"
        front["permalink"] = '"' + d + stem + '-{{ pagination.pageNumber }}/"'
    files = 0
    for sfx in [".11tydata.json", ".json"]:
        if rng.random() < 0.25:
            w(f"{d}{stem}{sfx}", '{"m": 1}')
            files += 1
    # directory data from every enclosing folder
    parts = d.rstrip("/").split("/") if d else []
    for k in range(1, len(parts) + 1):
        files += dir_files.get("/".join(parts[:k]) + "/", 0)
        if lay is None and ("/".join(parts[:k]) + "/") in dir_layout:
            pass
    if lay is None:
        for k in range(len(parts), 0, -1):
            dl = dir_layout.get("/".join(parts[:k]) + "/")
            if dl:
                lay = dl
                break
    body = "".join(f"{k}: {v}\n" for k, v in front.items())
    text = "x"
    if imports and ext != "njk" and rng.random() < 0.3:
        text += "{% for c in collections %}{{ c[0] }},{% endfor %}"
    if "reads" in flags and ext != "njk" and rng.random() < 0.4:
        tag = rng.choice(["t0", "t1", "t2", "t3", "t4"])
        text += ("{% for p in collections." + tag + " %}{% if p.inputPath != page.inputPath %}"
                 "({{ p.templateContent | size }}){% endif %}{% endfor %}")
    w(rel, f"---\n{body}---\n{text}\n")
    meta.append({"path": "./" + rel, "files": files, "layout": lay, "dated": front.get("date", "") not in ("", "Last Modified", "Created") or mode == "name"})
if config:
    # Every tag is in use, so importing one is never an unknown name.
    for t in TAGS:
        w(f"tag-{t}.md", f"---\ndate: 2020-01-01\ntags: {t}\n---\nx\n")
        meta.append({"path": f"./tag-{t}.md", "files": 0, "layout": None, "dated": True})
w("keys.liquid", "---\neleventyExcludeFromCollections: true\npermalink: keys.txt\ndate: 2020-01-01\n---\n{% for c in collections %}{{ c[0] }},{% endfor %}"
  + "".join("\n" + c + ": {% for p in collections." + c + " %}{{ p.url }},{% endfor %}"
            + "{% for p in collections." + c + ".x %}{{ p.url }},{% endfor %}{{ collections." + c + ".n }}" for c in CONFIG))


def collection(name):
    """One config collection, in JavaScript and in Rhai."""
    tag, other = rng.sample(TAGS + CONFIG, 2)
    glob = rng.choice([["**/*.md"], ["posts/**"], ["./docs/*"], ["*.liquid", "a/**"], ["posts/2020/**/*.html"]])
    api = rng.choice(["all", "sorted", "tag", "tags", "glob"])
    js = {"all": "api.getAll()", "sorted": "api.getAllSorted()", "tag": f"api.getFilteredByTag({json.dumps(tag)})",
          "tags": f"api.getFilteredByTags({json.dumps(tag)}, {json.dumps(other)})",
          "glob": f"api.getFilteredByGlob({json.dumps(glob)})"}[api]
    rhai = {"all": "api.get_all()", "sorted": "api.get_all_sorted()", "tag": f"api.get_filtered_by_tag({json.dumps(tag)})",
            "tags": f"api.get_filtered_by_tags({json.dumps(tag)}, {json.dumps(other)})",
            "glob": f"api.get_filtered_by_glob({json.dumps(glob)})"}[api]
    if rng.random() < 0.4:
        js, rhai = js + ".reverse()", rhai + ".reverse()"
    if name in NESTED:
        js = f"(() => {{ const x = {js}; return {{ x, n: x.length }}; }})()"
        rhai = f"{{ let o = object(); o.x = {rhai}; o.n = o.x.len(); o }}"
    return (f"  c.addCollection({json.dumps(name)}, (api) => {js});\n",
            f"config.add_collection({json.dumps(name)}, |api| {rhai});\n")


if config:
    made = [collection(n) for n in CONFIG]
    root = os.path.join(out, "site")
    with open(os.path.join(root, "eleventy.config.js"), "w") as f:
        f.write("module.exports = function (c) {\n" + "".join(j for j, _ in made) + "};\n")
    with open(os.path.join(root, "brrrty.config.rhai"), "w") as f:
        f.write("".join(r for _, r in made))
with open(os.path.join(out, "meta.json"), "w") as f:
    json.dump({"templates": meta, "layouts": {k: list(v) for k, v in layouts.items()}}, f)
