# Writes site/assets/readme/bench-{light,dark}.svg, the README chart. The numbers match the home page.
# Run: python3 bench/chart/readme_chart.py site/assets/readme
import sys
runs = [("MacBook Pro, Apple M4", 0.85, 2.32), ("GitLab shared runner", 1.47, 3.82), ("GitHub Actions runner", 1.87, 4.80)]
themes = {
 "light": dict(fg="#161616", fg2="#525252", fg3="#6b6b6b", line="#e5e5e5", red="#d7141e", redt="#c4111a", other="#a8a8a8"),
 "dark":  dict(fg="#ededed", fg2="#b0b0b0", fg3="#919191", line="#30363d", red="#ef2b33", redt="#ff4d52", other="#5c5c5c"),
}
W, X0, S = 880, 200, 100  # 100 px per second
font = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif"
for name, c in themes.items():
    o = []
    H = 70 + 92*len(runs) + 42
    o.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" font-family="{font}" role="img" aria-labelledby="t d">')
    o.append('<title id="t">Clean build of a 2,272-file blog: Brrrty vs 11ty 3.1.8</title>')
    o.append('<desc id="d">' + "; ".join(f"{e}: Brrrty {b:.2f} s, 11ty {e1:.2f} s" for e, b, e1 in runs) + '. Every output file identical.</desc>')
    o.append(f'<text x="0" y="26" font-size="20" font-weight="700" fill="{c["fg"]}">Clean build of a 2,272-file blog</text>')
    o.append(f'<text x="{W}" y="26" font-size="13" text-anchor="end" fill="{c["fg3"]}">seconds, lower is better</text>')
    # gridlines
    top, bot = 56, 70 + 92*len(runs) - 30
    for k in range(6):
        x = X0 + k*S
        o.append(f'<line x1="{x}" y1="{top}" x2="{x}" y2="{bot}" stroke="{c["line"]}" stroke-width="1"/>')
        o.append(f'<text x="{x}" y="{bot+18}" font-size="12" text-anchor="middle" fill="{c["fg3"]}">{k}{" s" if k==5 else ""}</text>')
    for i, (env, b, e) in enumerate(runs):
        g = 76 + i*92
        o.append(f'<text x="0" y="{g}" font-size="13" font-weight="600" fill="{c["fg2"]}">{env}</text>')
        for j, (who, t, fill, tc, wt) in enumerate([("Brrrty", b, c["red"], c["redt"], 700), ("11ty 3.1.8", e, c["other"], c["fg2"], 400)]):
            y = g + 12 + j*26
            o.append(f'<text x="{X0-12}" y="{y+15}" font-size="14" text-anchor="end" font-weight="{wt}" fill="{c["fg"] if j==0 else c["fg2"]}">{who}</text>')
            o.append(f'<rect x="{X0}" y="{y}" width="{t*S:.1f}" height="20" rx="2" fill="{fill}"/>')
            o.append(f'<text x="{X0 + t*S + 8:.1f}" y="{y+15}" font-size="14" font-weight="{wt}" fill="{tc}" style="font-variant-numeric: tabular-nums">{t:.2f} s</text>')
        o.append(f'<text x="{W}" y="{g+38}" font-size="22" font-weight="800" text-anchor="end" fill="{c["redt"]}">{e/b:.1f}&#215;</text>')
        o.append(f'<text x="{W}" y="{g+56}" font-size="12" text-anchor="end" fill="{c["fg3"]}">faster</text>')
    o.append(f'<text x="0" y="{H-8}" font-size="13" fill="{c["fg2"]}">Every output file is byte-identical to 11ty 3.1.8\'s, compared file by file on the GitLab and GitHub runners.</text>')
    o.append('</svg>')
    open(f"{sys.argv[1]}/bench-{name}.svg", "w").write("\n".join(o) + "\n")
