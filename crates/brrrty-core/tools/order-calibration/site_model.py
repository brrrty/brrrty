"""Read a generated site the way sim.py needs it (input folder = the site):
each template's layout as written (front matter, else directory data), and
each layout file's path in `_includes`, language and parent as written."""
import json, os, re


def front(path):
    text = open(path).read()
    m = re.match(r"---\n(.*?)\n---", text, re.S)
    out = {}
    if m:
        for line in m.group(1).splitlines():
            if ":" in line:
                k, v = line.split(":", 1)
                out[k.strip()] = v.strip()
    return out


def parse(site, ticks_rows, share=True):
    def inst(spelled):
        return spelled.split(".")[0] if share else spelled
    layouts = {}
    inc = os.path.join(site, "_includes")
    if os.path.isdir(inc):
        for f in os.listdir(inc):
            name, ext = f.split(".", 1)
            par = front(os.path.join(inc, f)).get("layout")
            layouts[f] = (f, ext, par)
            layouts.setdefault(name, (f, ext, par))
    templates = []
    for gen, a, start, path in ticks_rows:
        rel = path[2:]
        d, base = os.path.split(rel)
        stem = base.split(".")[0]
        ext = base.rsplit(".", 1)[1].lower()
        lay = front(os.path.join(site, rel)).get("layout")
        parts = d.split("/") if d else []
        if lay is None:
            for k in range(len(parts), 0, -1):
                dd = "/".join(parts[:k])
                for sfx in (".11tydata.json", ".json"):
                    p = os.path.join(site, dd, parts[k - 1] + sfx)
                    if os.path.exists(p):
                        v = json.load(open(p)).get("layout")
                        if v and lay is None:
                            lay = v
        templates.append({"path": path, "ext": ext, "A": a, "start": start,
                          "layout": lay})
    return templates, layouts


def data_files(site, rel):
    """The JSON template and directory data files 11ty reads for `rel`."""
    d, base = os.path.split(rel)
    stem = base.split(".")[0]
    n = sum(os.path.exists(os.path.join(site, d, stem + sfx)) for sfx in (".11tydata.json", ".json"))
    parts = d.split("/") if d else []
    for k in range(1, len(parts) + 1):
        dd = "/".join(parts[:k])
        n += sum(os.path.exists(os.path.join(site, dd, parts[k - 1] + sfx)) for sfx in (".11tydata.json", ".json"))
    return n
