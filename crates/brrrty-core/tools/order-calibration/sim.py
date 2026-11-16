"""FIFO microtask simulation of 11ty 3.1.8 loading template data (exact
structure from TemplateMap/Template/TemplateLayout/TemplateEngineManager).
The model and step counts are those of src/order.rs; keep them in step.

A process is a generator, run like an async function: spawning it runs it
at once up to its first yield. It yields
  ("hop", n)   continue n microtasks later
  ("use", p)   await shared promise p through one async wrapper: resume at
               settle + 2 if p is pending, or now + 3 if it has settled
and its own promise settles when it returns.
"""
from collections import deque

# Microtasks: engine load per language; engine -> layout.getData() call;
# engine -> end with no layout; layout data -> end; layout file's engine ->
# its front matter. Same as engine_cost, TO_LAYOUT, PLAIN, AFTER_LAYOUT and
# FRONT_MATTER in src/order.rs.
E = {"liquid": 10, "html": 17, "njk": 17, "md": 25}
T_LAYOUT, T_PLAIN, T_AFTER, FM_AFTER = 13, 19, 6, 10


class Promise:
    __slots__ = ("done", "subs")

    def __init__(self):
        self.done = False
        self.subs = []


class Sim:
    def __init__(self):
        self.q = deque()
        self.now = 0

    def later(self, n, fn):
        """run fn n microtasks from now (n >= 1), as a chain of jobs."""
        if n == 1:
            self.q.append((self.now + 1, fn))
        else:
            self.q.append((self.now + 1, lambda: self.later(n - 1, fn)))

    def spawn(self, g):
        p = Promise()
        self.advance(g, p)
        return p

    def advance(self, g, p):
        try:
            ins = next(g)
        except StopIteration:
            p.done = True
            subs, p.subs = p.subs, []
            for fn in subs:
                self.q.append((self.now + 1, fn))
            return
        if ins[0] == "hop":
            self.later(ins[1], lambda: self.advance(g, p))
        else:
            sp = ins[1]
            resume = lambda: self.advance(g, p)
            # thenable job one microtask later subscribes the wrapper
            def thenable():
                if sp.done:
                    # reaction job, then the wrapper settles, then resume
                    self.later(2, resume)
                else:
                    # reaction when sp settles (+1), wrapper settles, resume (+1)
                    sp.subs.append(lambda: self.later(1, resume))
            self.later(1, thenable)

    def run(self):
        while self.q:
            gen, fn = self.q.popleft()
            self.now = gen
            fn()


def simulate(templates, layouts, input_dir="."):
    """templates: dicts with path, ext, order (barrier order), layout (the
    spelled key, or None).
    layouts: spelled key -> (normalized key, file ext, parent spelled key or None).
    Returns [(gen, path)] in completion order."""
    sim = Sim()
    eng, fm, mp, data = {}, {}, {}, {}
    done = []
    cache = {}
    insts = []

    def join(key):
        return key if input_dir in (".", "./", "") else input_dir.rstrip("/") + "/" + key

    def get_template(spelled):
        """TemplateLayout.getTemplate with LayoutCache: an instance per new
        full key, registered under its full key and normalized key."""
        fk = join(spelled)
        if fk in cache:
            return cache[fk]
        norm, ext, parent = layouts[spelled]
        i = len(insts)
        insts.append((ext, parent))
        cache[fk] = i
        cache[norm] = i
        return i

    def shared(cache, key, make):
        if key not in cache:
            cache[key] = sim.spawn(make(key))
        return cache[key]

    def eng_proc(name):
        yield ("hop", E[name])

    def fm_proc(i):
        yield ("use", shared(eng, insts[i][0], eng_proc))
        yield ("hop", FM_AFTER)

    def map_proc(i):
        while True:
            yield ("use", shared(fm, i, fm_proc))
            yield ("hop", 1)
            parent = insts[i][1]
            if parent is None:
                return
            i = get_template(parent)

    def data_proc(key):
        yield ("use", shared(mp, key, map_proc))

    def tmpl(t):
        yield ("use", shared(eng, t["ext"], eng_proc))
        if t["layout"] is not None:
            yield ("hop", T_LAYOUT)
            yield ("use", shared(data, get_template(t["layout"]), data_proc))
            yield ("hop", T_AFTER)
        else:
            yield ("hop", T_PLAIN)
        done.append((sim.now, t["path"]))

    for t in sorted(templates, key=lambda t: t["order"]):
        sim.spawn(tmpl(t))
    sim.run()
    # A page that reads its date from the disk (no date, `Last Modified`,
    # `Created`) finishes after every other page, in the order it asked.
    undated = {t["path"] for t in templates if not t.get("dated", True)}
    return [d for d in done if d[1] not in undated] + [d for d in done if d[1] in undated]
