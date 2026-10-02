#!/usr/bin/env python3
"""Render docs/*.md into a static site (stdlib only, Python >= 3.8).

The renderer handles the markdown subset these pages use: headings,
tables, code fences, lists, inline `code` and **bold**, links, and
paragraphs. It deliberately does not aim at full CommonMark — the pages
are written for it, and a smaller renderer is a smaller audit.

    python docs/render.py [docs-dir] [out-dir]

Defaults: `docs/` → `docs/_site`. Every `.md` in the directory renders
to `<name>.html` with a minimal shared chrome; relative `.md` links are
rewritten to `.html` so the site navigates on GitHub Pages and on disk.
"""

import html
import re
import sys
from pathlib import Path

PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — modhash docs</title>
<style>
body{{max-width:56rem;margin:2rem auto;padding:0 1rem;font:16px/1.6 -apple-system,Segoe UI,Roboto,sans-serif;color:#1a1a1a}}
code,pre{{font-family:ui-monospace,Cascadia Code,Consolas,monospace;font-size:.9em}}
pre{{background:#f5f5f5;padding:.8rem;overflow-x:auto;border-radius:6px}}
code{{background:#f5f5f5;padding:.1em .3em;border-radius:4px}}
pre code{{background:none;padding:0}}
table{{border-collapse:collapse;margin:1rem 0}}
th,td{{border:1px solid #d0d0d0;padding:.35rem .7rem;text-align:left}}
th{{background:#f0f0f0}}
h1{{border-bottom:2px solid #ddd;padding-bottom:.3rem}}
a{{color:#0645ad}}
</style></head><body>
{body}
</body></html>
"""


def inline(s: str) -> str:
    s = html.escape(s)
    s = re.sub(r"`([^`]+)`", r"<code>\1</code>", s)
    s = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", s)
    s = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", lambda m: f'<a href="{m.group(2)}">{m.group(1)}</a>', s)
    return s


def render(md: str) -> str:
    out, i, lines = [], 0, md.split("\n")
    while i < len(lines):
        ln = lines[i]
        if ln.startswith("```"):
            i += 1
            buf = []
            while i < len(lines) and not lines[i].startswith("```"):
                buf.append(lines[i])
                i += 1
            i += 1
            out.append("<pre><code>" + html.escape("\n".join(buf)) + "</code></pre>")
            continue
        if ln.startswith("|") and i + 1 < len(lines) and set(lines[i + 1]) <= set("|-: "):
            head = [c.strip() for c in ln.strip("|").split("|")]
            i += 2
            rows = []
            while i < len(lines) and lines[i].startswith("|"):
                rows.append([c.strip() for c in lines[i].strip("|").split("|")])
                i += 1
            t = "<table><tr>" + "".join(f"<th>{inline(c)}</th>" for c in head) + "</tr>"
            for r in rows:
                t += "<tr>" + "".join(f"<td>{inline(c)}</td>" for c in r) + "</tr>"
            out.append(t + "</table>")
            continue
        m = re.match(r"^(#{1,4})\s+(.*)", ln)
        if m:
            lvl, txt = len(m.group(1)), m.group(2)
            out.append(f"<h{lvl}>{inline(txt)}</h{lvl}>")
            i += 1
            continue
        if ln.startswith(("- ", "* ")):
            items = []
            while i < len(lines) and lines[i].startswith(("- ", "* ")):
                items.append(lines[i][2:])
                i += 1
            out.append("<ul>" + "".join(f"<li>{inline(x)}</li>" for x in items) + "</ul>")
            continue
        if re.match(r"^\d+\.\s", ln):
            items = []
            while i < len(lines) and re.match(r"^\d+\.\s", lines[i]):
                items.append(re.sub(r"^\d+\.\s*", "", lines[i]))
                i += 1
            out.append("<ol>" + "".join(f"<li>{inline(x)}</li>" for x in items) + "</ol>")
            continue
        if not ln.strip():
            i += 1
            continue
        buf = [ln]
        i += 1
        while i < len(lines) and lines[i].strip() and not re.match(r"^(#{1,4}\s|\||```|- |\d+\.\s)", lines[i]):
            buf.append(lines[i])
            i += 1
        out.append("<p>" + inline(" ".join(buf)) + "</p>")
    return "\n".join(out)


def title_of(md: str, fallback: str) -> str:
    for ln in md.split("\n"):
        if ln.startswith("# "):
            return html.escape(ln[2:].strip())
    return fallback


def main() -> int:
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("docs")
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else src / "_site"
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    for f in sorted(src.glob("*.md")):
        body = render(f.read_text(encoding="utf-8")).replace('.md"', '.html"')
        (out / (f.stem + ".html")).write_text(
            PAGE.format(title=title_of(f.read_text(encoding="utf-8"), f.stem), body=body),
            encoding="utf-8",
        )
        n += 1
    print(f"rendered {n} page(s) -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
