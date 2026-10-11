#!/usr/bin/env python3
"""Render a Memoria ASCII diagram source (.txt) as a branded SVG.

The source is a plain monospace drawing. The renderer keeps every character on
its grid cell and applies the Memoria diagram style:

  ─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼ ╭ ╮ ╰ ╯   line work, drawn as connected strokes
  ▶ ◀ ▲ ▼                         arrowheads, in the accent color
  ░ ▒ ▓ █                         ordered-dither fills at 25, 50, 75, 100 percent
  «text»                          accent text (each guillemet renders as a space)
  ‹text›                          muted text (each angle quote renders as a space)
  ⟨text⟩                          soft text: ink at 80% opacity (each bracket renders as a space)

Every other character is ink text. The SVG follows the reader's light or dark
color scheme.

A source starts with a header, then a line with three dashes, then the drawing:

  title: One short claim that the diagram makes
  desc: What the diagram shows, for a reader who cannot see it
  ---
  ┌─ DRAWING ─┐

Usage:
  scripts/ascii-diagram.py SOURCE.txt [SOURCE.txt ...]          write SOURCE.svg beside each source
  scripts/ascii-diagram.py --check SOURCE.txt [SOURCE.txt ...]  exit 1 when a SOURCE.svg is not current
"""

import argparse
import sys
from html import escape

CELL_W = 9.0
CELL_H = 18.0
FONT_SIZE = 15
PAD = 2  # cells of padding on every side
PIXEL = 2.0  # one dither pixel, in SVG units

LIGHT = {"bg": "#f4f0e6", "ink": "#24211b", "muted": "#8a8170", "accent": "#c2410c"}
DARK = {"bg": "#13120f", "ink": "#e8e3d4", "muted": "#7f7867", "accent": "#f59e0b"}


def palette_rules(colors):
    return (
        f".bg{{fill:{colors['bg']}}}"
        f".ink{{fill:{colors['ink']}}}"
        f".muted{{fill:{colors['muted']}}}"
        f".accent{{fill:{colors['accent']}}}"
        f".line{{stroke:{colors['ink']}}}"
        f".line-muted{{stroke:{colors['muted']}}}"
        f".line-accent{{stroke:{colors['accent']}}}"
    )


def soft_rules(colors):
    """Soft text is ink at 80% opacity: a step back from ink, brighter than muted."""
    return (
        f".soft{{fill:{colors['ink']};fill-opacity:0.8}}"
        f".line-soft{{stroke:{colors['ink']};stroke-opacity:0.8}}"
    )


def style(soft):
    """The stylesheet. Soft rules appear only in a drawing that uses soft text,
    so every drawing without it renders exactly as before."""
    extra = soft_rules if soft else (lambda colors: "")
    return (
        palette_rules(LIGHT)
        + extra(LIGHT)
        + "@media (prefers-color-scheme: dark){" + palette_rules(DARK) + extra(DARK) + "}"
        + ".line,.line-muted,.line-accent" + (",.line-soft" if soft else "")
        + "{stroke-width:1.5;fill:none;stroke-linecap:square}"
        + 'text{font-family:ui-monospace,"SFMono-Regular","JetBrains Mono",Menlo,Consolas,"DejaVu Sans Mono",monospace;'
        + "font-size:15px;white-space:pre}"
    )

# Directions: N, E, S, W.
BOX = {
    "─": "EW", "│": "NS",
    "┌": "ES", "┐": "WS", "└": "NE", "┘": "NW",
    "╭": "ES", "╮": "WS", "╰": "NE", "╯": "NW",
    "├": "NES", "┤": "NWS", "┬": "EWS", "┴": "EWN", "┼": "NESW",
}
ARROWS = {"▶": "E", "◀": "W", "▲": "N", "▼": "S"}
DITHER = {"░": 1, "▒": 2, "▓": 3, "█": 4}
MODES = {"«": ("accent", "»"), "‹": ("muted", "›"), "⟨": ("soft", "⟩")}

# 2x2 ordered-dither (Bayer) thresholds: pixel (x, y) is lit when level > BAYER[y][x].
BAYER = [[0, 2], [3, 1]]


def parse(text):
    """Return rows of (char, mode) cells. Mode markers become spaces."""
    rows = []
    for raw in text.expandtabs(4).splitlines():
        row, mode, closer = [], "ink", None
        for ch in raw.rstrip():
            if closer is None and ch in MODES:
                mode, closer = MODES[ch]
                row.append((" ", mode))
            elif closer is not None and ch == closer:
                row.append((" ", mode))
                mode, closer = "ink", None
            else:
                row.append((ch, mode))
        rows.append(row)
    while rows and not rows[-1]:
        rows.pop()
    return rows


def fmt(value):
    text = f"{value:.2f}".rstrip("0").rstrip(".")
    return text or "0"


def dither_patterns(soft=False):
    out = []
    for mode in ("accent", "muted", "ink") + (("soft",) if soft else ()):
        for level in (1, 2, 3):
            pixels = []
            for y in range(2):
                for x in range(2):
                    if level > BAYER[y][x]:
                        pixels.append(
                            f'<rect x="{fmt(x * PIXEL)}" y="{fmt(y * PIXEL)}" '
                            f'width="{fmt(PIXEL)}" height="{fmt(PIXEL)}" class="{mode}"/>'
                        )
            size = fmt(2 * PIXEL)
            out.append(
                f'<pattern id="d{level}-{mode}" width="{size}" height="{size}" '
                f'patternUnits="userSpaceOnUse">{"".join(pixels)}</pattern>'
            )
    return out


def render(title, desc, text):
    rows = parse(text)
    soft = any(mode == "soft" for row in rows for _, mode in row)
    cols = max((len(r) for r in rows), default=0)
    width = (cols + 2 * PAD) * CELL_W
    height = (len(rows) + 2 * PAD) * CELL_H

    def cx(c):
        return (c + PAD) * CELL_W

    def cy(r):
        return (r + PAD) * CELL_H

    fills, lines, arrows, texts = [], {}, [], []
    for r, row in enumerate(rows):
        # Dither fills: merge horizontal runs of the same glyph and mode.
        c = 0
        while c < len(row):
            ch, mode = row[c]
            if ch in DITHER:
                start = c
                while c < len(row) and row[c] == (ch, mode):
                    c += 1
                level = DITHER[ch]
                fill = f"url(#d{level}-{mode})" if level < 4 else None
                cls = f' class="{mode}"' if fill is None else ""
                fill_attr = f' fill="{fill}"' if fill else ""
                fills.append(
                    f'<rect x="{fmt(cx(start))}" y="{fmt(cy(r) + 1)}" '
                    f'width="{fmt((c - start) * CELL_W)}" height="{fmt(CELL_H - 2)}"{cls}{fill_attr}/>'
                )
                continue
            c += 1
        # Line work, arrows, and text.
        run = None
        for c, (ch, mode) in enumerate(row):
            x0, y0 = cx(c), cy(r)
            mx, my = x0 + CELL_W / 2, y0 + CELL_H / 2
            special = ch in BOX or ch in ARROWS or ch in DITHER or ch == " "
            if ch in BOX:
                ends = {"N": (mx, y0), "E": (x0 + CELL_W, my), "S": (mx, y0 + CELL_H), "W": (x0, my)}
                for d in BOX[ch]:
                    ex, ey = ends[d]
                    lines.setdefault(mode, []).append(f"M{fmt(mx)} {fmt(my)}L{fmt(ex)} {fmt(ey)}")
            elif ch in ARROWS:
                s = 4.5
                pts = {
                    "E": [(mx + s, my), (mx - s, my - s), (mx - s, my + s)],
                    "W": [(mx - s, my), (mx + s, my - s), (mx + s, my + s)],
                    "N": [(mx, my - s), (mx - s, my + s), (mx + s, my + s)],
                    "S": [(mx, my + s), (mx - s, my - s), (mx + s, my - s)],
                }[ARROWS[ch]]
                arrow_mode = "accent" if mode == "ink" else mode
                arrows.append(
                    f'<polygon points="{" ".join(f"{fmt(px)},{fmt(py)}" for px, py in pts)}" class="{arrow_mode}"/>'
                )
            if special:
                if run:
                    texts.append(run)
                    run = None
                continue
            if run and run["mode"] == mode and run["end"] == c:
                run["chars"].append(ch)
                run["xs"].append(x0)
                run["end"] = c + 1
            else:
                if run:
                    texts.append(run)
                run = {"mode": mode, "row": r, "chars": [ch], "xs": [x0], "end": c + 1}
        if run:
            texts.append(run)

    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{fmt(width)}" height="{fmt(height)}" '
        f'viewBox="0 0 {fmt(width)} {fmt(height)}" role="img" aria-labelledby="title desc">',
        f'<title id="title">{escape(title)}</title>',
        f'<desc id="desc">{escape(desc)}</desc>',
        f"<style>{style(soft)}</style>",
        f'<defs>{"".join(dither_patterns(soft))}</defs>',
        f'<rect class="bg" width="{fmt(width)}" height="{fmt(height)}" rx="8"/>',
    ]
    out.extend(fills)
    for mode, segments in sorted(lines.items()):
        cls = {"ink": "line", "muted": "line-muted", "accent": "line-accent", "soft": "line-soft"}[mode]
        out.append(f'<path class="{cls}" d="{"".join(segments)}"/>')
    out.extend(arrows)
    for run in texts:
        baseline = cy(run["row"]) + CELL_H * 0.72
        xs = " ".join(fmt(x) for x in run["xs"])
        out.append(
            f'<text class="{run["mode"]}" x="{xs}" y="{fmt(baseline)}">{escape("".join(run["chars"]))}</text>'
        )
    out.append("</svg>")
    return "\n".join(out) + "\n"


def split_source(path, text):
    """Return (title, desc, drawing) from a diagram source."""
    header, sep, drawing = text.partition("\n---\n")
    if not sep:
        raise SystemExit(f"{path}: the header must end with a line of three dashes")
    fields = {}
    for line in header.splitlines():
        key, colon, value = line.partition(":")
        if colon:
            fields[key.strip()] = value.strip()
    for key in ("title", "desc"):
        if not fields.get(key):
            raise SystemExit(f"{path}: the header needs a nonempty {key}")
    return fields["title"], fields["desc"], drawing


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("sources", nargs="+", metavar="SOURCE.txt")
    parser.add_argument("--check", action="store_true", help="Exit 1 when a SOURCE.svg is not current")
    args = parser.parse_args()

    status = 0
    for source in args.sources:
        if not source.endswith(".txt"):
            raise SystemExit(f"{source}: a diagram source ends with .txt")
        output = source[: -len(".txt")] + ".svg"
        with open(source, encoding="utf-8") as handle:
            svg = render(*split_source(source, handle.read()))
        if args.check:
            try:
                with open(output, encoding="utf-8") as handle:
                    current = handle.read()
            except FileNotFoundError:
                current = None
            if current != svg:
                print(f"{output} is not the current render of {source}", file=sys.stderr)
                status = 1
            continue
        with open(output, "w", encoding="utf-8") as handle:
            handle.write(svg)
    return status


if __name__ == "__main__":
    sys.exit(main())
