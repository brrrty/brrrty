"""Random sites weighted toward data files, layout chains and directory-data
layouts. usage: python3 gen_site.py OUTDIR SEED [undated] [src] [reads] [njk]
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
rng = random.Random(seed)
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

dirs = ["", "", "posts/", "docs/", "posts/2020/", "a/b/"]
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
    rel = f"{d}{stem}.{ext}"
    if rel.lower() in used:
        continue
    used.add(rel.lower())
    front = {}
    if not undated or rng.random() < 0.5:
        front["date"] = "2020-01-01"
    lay = None
    if layouts and rng.random() < 0.6:
        lname = rng.choice(list(layouts))
        spelled = rng.choice([f"{lname}.{layouts[lname][0]}", f"{lname}.{layouts[lname][0]}", lname])
        front["layout"] = spelled
        lay = spelled
    front["tags"] = rng.choice(["t0", "t1", "t2", "t3", "t4"])
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
    if "reads" in flags and ext != "njk" and rng.random() < 0.4:
        tag = rng.choice(["t0", "t1", "t2", "t3", "t4"])
        text += ("{% for p in collections." + tag + " %}{% if p.inputPath != page.inputPath %}"
                 "({{ p.templateContent | size }}){% endif %}{% endfor %}")
    w(rel, f"---\n{body}---\n{text}\n")
    meta.append({"path": "./" + rel, "files": files, "layout": lay, "dated": "date" in front})
w("keys.liquid", "---\neleventyExcludeFromCollections: true\npermalink: keys.txt\ndate: 2020-01-01\n---\n{% for c in collections %}{{ c[0] }},{% endfor %}")
with open(os.path.join(out, "meta.json"), "w") as f:
    json.dump({"templates": meta, "layouts": {k: list(v) for k, v in layouts.items()}}, f)
