#!/usr/bin/env python3
"""Generate the scoring-model diagrams embedded in docs/scoring.md.

Pure-stdlib SVG generator (no third-party deps). Re-run after any change to the
scoring defaults in `src-tauri/src/scoring.rs` to keep the published curves in
sync with the model:

    python scripts/gen_scoring_diagrams.py

Writes, into docs/img/:
  - angle-factor-curve.svg   the angle_factor() curve (normal vs crawl speed)
  - speed-factor-curve.svg   the speed_factor() ramp
  - scoring-pipeline.svg     the per-moment scoring pipeline / anatomy

The colours below are the design-system tokens from `src/app.css` (graphite +
amber), hardcoded because a standalone SVG can't read the app's CSS :root
custom properties. Keep them in sync with that file. Each SVG carries its own
dark card background, so it reads on both light and dark GitHub themes.
"""

from __future__ import annotations

import math
from pathlib import Path

# ── Design tokens (src/app.css) ─────────────────────────────────────────────
BG_BODY = "#0f1012"     # --bg-body
BG_CARD = "#16171b"     # between --bg-panel and --bg-card
BG_ELEV = "#1e2024"     # --bg-elevated
BG_TRACK = "#26282d"    # --bg-track
BD_DIM = "#222428"      # --bd-dim
BD_SUBTLE = "#2a2c31"   # --bd-subtle
BD_MUTED = "#34373d"    # --bd-muted
BD_STRONG = "#44484f"   # --bd-strong
TX_HI = "#f2f2f0"       # --tx-hi
TX_MID = "#d7d7d3"      # --tx-mid
TX_LO = "#aaaaa5"       # --tx-lo
TX_DIM = "#8a8b85"      # --tx-dim
TX_XDIM = "#61625e"     # --tx-xdim
AC = "#d2a24c"          # --ac (amber)
AC_BRIGHT = "#ecc274"   # --ac-bright
OK = "#84b577"          # --ok (scoring green)
OK_BRIGHT = "#8fe07f"   # --ok-bright
BAD = "#d56c62"         # --bad
INFO = "#82a7c8"        # --info
GATE_A = "#efa838"
GATE_B = "#38c9b9"
GATE_SPLIT = "#b385ee"

FONT_UI = "'Inter Variable','Segoe UI',system-ui,sans-serif"
FONT_MONO = "'JetBrains Mono','JetBrains Mono Variable','Cascadia Mono',Consolas,monospace"

# ── Scoring defaults (mirror src-tauri/src/scoring.rs) ───────────────────────
MIN_ANGLE = 10.0
SWEET = 45.0
SPIN = 120.0
RISE = 0.085
RISE_SAT = 58.0
ANGLE_POWER = 0.15
LOWSPEED_ADD = 0.22
MIN_SPEED = 1.5
SPEED_CAP = 60.0
SCALE = 10.716
MS_TO_MPH = 2.236936


def angle_factor(a: float, crawl: bool = False) -> float:
    if a < MIN_ANGLE or a > SPIN:
        return 0.0
    if a <= SWEET:
        power = ANGLE_POWER + (LOWSPEED_ADD if crawl else 0.0)
        return (a / SWEET) ** power
    rise_ramp = min((a - SWEET) / (RISE_SAT - SWEET), 1.0)
    return 1.0 + RISE * rise_ramp


def speed_factor(v: float) -> float:
    if v < MIN_SPEED:
        return 0.0
    return min(v, SPEED_CAP) / SPEED_CAP


# ── SVG helpers ──────────────────────────────────────────────────────────────
def n(x: float) -> str:
    return f"{x:.2f}".rstrip("0").rstrip(".")


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def text(x, y, s, size, fill, *, font=FONT_UI, anchor="start", weight=550,
         spacing=None, opacity=None, baseline=None):
    attrs = [
        f'x="{n(x)}"', f'y="{n(y)}"', f'font-family="{font}"',
        f'font-size="{n(size)}"', f'fill="{fill}"', f'text-anchor="{anchor}"',
        f'font-weight="{weight}"',
    ]
    if spacing is not None:
        attrs.append(f'letter-spacing="{n(spacing)}"')
    if opacity is not None:
        attrs.append(f'opacity="{n(opacity)}"')
    if baseline is not None:
        attrs.append(f'dominant-baseline="{baseline}"')
    return f'<text {" ".join(attrs)}>{esc(s)}</text>'


def line(x1, y1, x2, y2, stroke, w=1.0, dash=None, opacity=None, cap="butt"):
    a = [f'x1="{n(x1)}"', f'y1="{n(y1)}"', f'x2="{n(x2)}"', f'y2="{n(y2)}"',
         f'stroke="{stroke}"', f'stroke-width="{n(w)}"', f'stroke-linecap="{cap}"']
    if dash:
        a.append(f'stroke-dasharray="{dash}"')
    if opacity is not None:
        a.append(f'opacity="{n(opacity)}"')
    return f'<line {" ".join(a)}/>'


def rect(x, y, w, h, *, fill="none", stroke="none", sw=1.0, rx=0, opacity=None):
    a = [f'x="{n(x)}"', f'y="{n(y)}"', f'width="{n(w)}"', f'height="{n(h)}"',
         f'fill="{fill}"', f'stroke="{stroke}"', f'stroke-width="{n(sw)}"',
         f'rx="{n(rx)}"']
    if opacity is not None:
        a.append(f'opacity="{n(opacity)}"')
    return f'<rect {" ".join(a)}/>'


def circle(cx, cy, r, *, fill="none", stroke="none", sw=1.0):
    return (f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{fill}" '
            f'stroke="{stroke}" stroke-width="{n(sw)}"/>')


def polyline(pts, stroke, w=2.5, dash=None, opacity=None):
    d = " ".join(f"{n(x)},{n(y)}" for x, y in pts)
    a = [f'points="{d}"', 'fill="none"', f'stroke="{stroke}"',
         f'stroke-width="{n(w)}"', 'stroke-linejoin="round"', 'stroke-linecap="round"']
    if dash:
        a.append(f'stroke-dasharray="{dash}"')
    if opacity is not None:
        a.append(f'opacity="{n(opacity)}"')
    return f'<polyline {" ".join(a)}/>'


def area(pts, baseline_y, fill, opacity):
    d = "M " + " L ".join(f"{n(x)},{n(y)}" for x, y in pts)
    d += f" L {n(pts[-1][0])},{n(baseline_y)} L {n(pts[0][0])},{n(baseline_y)} Z"
    return f'<path d="{d}" fill="{fill}" opacity="{n(opacity)}"/>'


def svg_doc(w, h, title, desc, body):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" role="img" aria-label="{esc(title)}">\n'
        f'<title>{esc(title)}</title>\n<desc>{esc(desc)}</desc>\n'
        f'{rect(0.5, 0.5, w - 1, h - 1, fill=BG_CARD, stroke=BD_DIM, sw=1, rx=8)}\n'
        f'{body}\n</svg>\n'
    )


def header(title, subtitle):
    out = [
        f'<rect x="20" y="26" width="3" height="20" rx="1.5" fill="{AC}"/>',
        text(32, 36, title, 17, TX_HI, weight=650),
        text(32, 56, subtitle, 12, TX_DIM, weight=500),
    ]
    return "\n".join(out)


# ── 1. Angle-factor curve ────────────────────────────────────────────────────
def angle_curve_svg() -> str:
    W, H = 760, 432
    PX0, PX1 = 66, 726
    PY0, PY1 = 96, 366
    XMAX = 130.0
    YMAX = 1.15

    def sx(deg):
        return PX0 + (deg / XMAX) * (PX1 - PX0)

    def sy(val):
        return PY1 - (val / YMAX) * (PY1 - PY0)

    body = [header("The angle curve", "How much each degree of slide is worth · angle_factor(angle)")]

    # plot frame
    body.append(rect(PX0, PY0, PX1 - PX0, PY1 - PY0, fill=BG_BODY, stroke=BD_DIM, sw=1, rx=3))

    # y gridlines + labels
    for v in [0.0, 0.25, 0.5, 0.75, 1.0]:
        y = sy(v)
        body.append(line(PX0, y, PX1, y, BD_SUBTLE, 1, opacity=0.7))
        body.append(text(PX0 - 10, y + 3.5, f"{v:.2f}", 10.5, TX_XDIM, font=FONT_MONO, anchor="end"))
    # plateau guide at 1.085
    yp = sy(1.0 + RISE)
    body.append(line(PX0, yp, PX1, yp, OK, 1, dash="2 4", opacity=0.45))
    body.append(text(PX1 - 4, yp - 5, "1.085", 10.5, OK, font=FONT_MONO, anchor="end", weight=600))

    # x gridlines + labels (every 15°)
    deg = 0
    while deg <= 120:
        x = sx(deg)
        if deg > 0:
            body.append(line(x, PY0, x, PY1, BD_SUBTLE, 1, opacity=0.55))
        body.append(text(x, PY1 + 18, f"{deg}", 10.5, TX_XDIM, font=FONT_MONO, anchor="middle"))
        deg += 15

    # vertical guides for the special angles
    for deg, col, op in [(MIN_ANGLE, BAD, 0.6), (SWEET, AC, 0.8), (SPIN, BAD, 0.6)]:
        body.append(line(sx(deg), PY0, sx(deg), PY1, col, 1, dash="3 4", opacity=op))

    # dead-zone baseline segments (no score below the gate / past spin-out)
    body.append(line(PX0, sy(0), sx(MIN_ANGLE), sy(0), BAD, 2.5, opacity=0.55, cap="round"))
    body.append(line(sx(SPIN), sy(0), PX1, sy(0), BAD, 2.5, opacity=0.55, cap="round"))

    # normal-speed curve points
    pts = [(sx(MIN_ANGLE), sy(0.0))]
    a = MIN_ANGLE
    while a < SWEET:
        pts.append((sx(a), sy(angle_factor(a))))
        a += 0.5
    pts.append((sx(SWEET), sy(1.0)))
    pts.append((sx(RISE_SAT), sy(1.0 + RISE)))
    pts.append((sx(SPIN), sy(1.0 + RISE)))

    # area fill under normal curve
    body.append(area(pts[1:], sy(0.0), OK, 0.10))
    # the cliff at spin-out
    body.append(line(sx(SPIN), sy(1.0 + RISE), sx(SPIN), sy(0.0), BAD, 2.0, dash="3 3", opacity=0.7))
    # normal-speed curve
    body.append(polyline(pts, OK, 2.6))

    # crawl-speed curve (below sweet only — they meet at the 45 anchor)
    cpts = []
    a = MIN_ANGLE
    while a <= SWEET:
        cpts.append((sx(a), sy(angle_factor(a, crawl=True))))
        a += 0.5
    body.append(polyline(cpts, OK, 1.8, dash="2 4", opacity=0.7))

    # anchor + plateau markers
    body.append(circle(sx(SWEET), sy(1.0), 3.6, fill=AC, stroke=BG_BODY, sw=1.5))
    body.append(circle(sx(RISE_SAT), sy(1.0 + RISE), 3.2, fill=OK_BRIGHT, stroke=BG_BODY, sw=1.5))

    # callouts
    body.append(text(sx(SWEET) + 8, sy(1.0) + 24, "45° sweet spot", 11, AC_BRIGHT, weight=600))
    body.append(text(sx(SWEET) + 8, sy(1.0) + 38, "factor = 1.00", 10, TX_DIM, font=FONT_MONO))
    body.append(text(sx(66), sy(1.0 + RISE) + 18, "+8.5% by 58°, then flat", 10.5, OK_BRIGHT, weight=600))
    body.append(text(sx(MIN_ANGLE) - 2, sy(0) + 18, "10° gate", 10.5, BAD, anchor="middle", weight=600))
    body.append(text(sx(MIN_ANGLE) - 2, sy(0) + 31, "nothing below", 9.5, TX_XDIM, anchor="middle"))
    body.append(text(sx(SPIN), sy(0) + 18, "120° spin-out", 10.5, BAD, anchor="middle", weight=600))
    body.append(text(sx(MIN_ANGLE) + 8, sy(0.70),
                     "shallow slides already pay ~80%", 10.5, TX_LO, weight=550))

    # axis titles
    body.append(text((PX0 + PX1) / 2, PY1 + 40, "DRIFT ANGLE  (DEGREES)", 10, TX_DIM,
                     anchor="middle", spacing=1.4, weight=600))
    body.append(f'<text x="22" y="{n((PY0 + PY1) / 2)}" font-family="{FONT_UI}" '
                f'font-size="10" fill="{TX_DIM}" text-anchor="middle" font-weight="600" '
                f'letter-spacing="1.4" transform="rotate(-90 22 {n((PY0 + PY1) / 2)})">'
                f'ANGLE FACTOR</text>')

    # legend (header, right side)
    lx, ly = 470, 40
    body.append(line(lx, ly - 4, lx + 26, ly - 4, OK, 2.6))
    body.append(text(lx + 32, ly, "normal speed", 11, TX_MID, weight=550))
    body.append(line(lx + 150, ly - 4, lx + 176, ly - 4, OK, 1.8, dash="2 4", opacity=0.8))
    body.append(text(lx + 182, ly, "crawl (≤3 m/s)", 11, TX_MID, weight=550))

    return svg_doc(W, H, "FH6 drift-zone angle factor curve",
                   "Angle factor vs drift angle: zero below 10 degrees, a concave ramp to 1.0 at "
                   "the 45 degree sweet spot, a rise to +8.5% by 58 degrees held flat to 120 "
                   "degrees, then zero past spin-out. A dashed line shows the crawl-speed depression.",
                   "\n".join(body))


# ── 2. Speed-factor curve ────────────────────────────────────────────────────
def speed_curve_svg() -> str:
    W, H = 760, 392
    PX0, PX1 = 66, 726
    PY0, PY1 = 92, 312
    XMAX = 80.0
    YMAX = 1.1

    def sx(v):
        return PX0 + (v / XMAX) * (PX1 - PX0)

    def sy(val):
        return PY1 - (val / YMAX) * (PY1 - PY0)

    body = [header("The speed curve", "A straight line · speed_factor(speed) = min(speed, 60) / 60")]
    body.append(rect(PX0, PY0, PX1 - PX0, PY1 - PY0, fill=BG_BODY, stroke=BD_DIM, sw=1, rx=3))

    # y grid
    for v in [0.0, 0.25, 0.5, 0.75, 1.0]:
        y = sy(v)
        body.append(line(PX0, y, PX1, y, BD_SUBTLE, 1, opacity=0.7))
        body.append(text(PX0 - 10, y + 3.5, f"{v:.2f}", 10.5, TX_XDIM, font=FONT_MONO, anchor="end"))

    # x grid (every 10 m/s) with mph secondary
    v = 0
    while v <= 80:
        x = sx(v)
        if v > 0:
            body.append(line(x, PY0, x, PY1, BD_SUBTLE, 1, opacity=0.55))
        body.append(text(x, PY1 + 18, f"{v}", 10.5, TX_XDIM, font=FONT_MONO, anchor="middle"))
        body.append(text(x, PY1 + 31, f"{round(v * MS_TO_MPH)}", 9, TX_XDIM,
                         font=FONT_MONO, anchor="middle", opacity=0.75))
        v += 10

    # cap guide
    body.append(line(sx(SPEED_CAP), PY0, sx(SPEED_CAP), PY1, AC, 1, dash="3 4", opacity=0.7))

    # curve: 0 below min speed, linear to cap, flat after
    pts = [(sx(0), sy(0)), (sx(MIN_SPEED), sy(0))]
    pts.append((sx(MIN_SPEED), sy(speed_factor(MIN_SPEED))))
    pts.append((sx(SPEED_CAP), sy(1.0)))
    pts.append((sx(XMAX), sy(1.0)))
    body.append(area(pts[1:], sy(0), OK, 0.10))
    body.append(polyline(pts, OK, 2.6))

    # markers
    body.append(circle(sx(SPEED_CAP), sy(1.0), 3.6, fill=AC, stroke=BG_BODY, sw=1.5))
    body.append(circle(sx(MIN_SPEED), sy(0), 3.2, fill=BAD, stroke=BG_BODY, sw=1.5))

    # callouts
    body.append(text(sx(MIN_SPEED) + 10, sy(0) - 22, "1.5 m/s floor", 10.5, BAD, weight=600))
    body.append(text(sx(MIN_SPEED) + 10, sy(0) - 9, "(nothing scores below)", 9.5, TX_XDIM))
    body.append(text(sx(SPEED_CAP) - 8, sy(1.0) - 12, "saturates at 60 m/s", 11, AC_BRIGHT, anchor="end", weight=600))
    body.append(text(sx(SPEED_CAP) - 8, sy(1.0) + 2, "(~134 mph) — more speed, no more credit", 9.5, TX_DIM, anchor="end"))
    body.append(text(sx(28), sy(speed_factor(28)) - 10, "linear: twice the speed, twice the rate", 10.5, TX_LO, anchor="middle", weight=550))

    # axis titles
    body.append(text((PX0 + PX1) / 2, PY1 + 49, "SPEED  —  M/S (TOP)  ·  MPH (BELOW)", 10, TX_DIM,
                     anchor="middle", spacing=1.2, weight=600))
    body.append(f'<text x="22" y="{n((PY0 + PY1) / 2)}" font-family="{FONT_UI}" '
                f'font-size="10" fill="{TX_DIM}" text-anchor="middle" font-weight="600" '
                f'letter-spacing="1.4" transform="rotate(-90 22 {n((PY0 + PY1) / 2)})">'
                f'SPEED FACTOR</text>')

    return svg_doc(W, H, "FH6 drift-zone speed factor curve",
                   "Speed factor vs speed: zero below 1.5 m/s, then a straight line rising to 1.0 "
                   "at the 60 m/s (about 134 mph) cap, flat beyond.",
                   "\n".join(body))


# ── 3. Per-moment pipeline ───────────────────────────────────────────────────
def pipeline_svg() -> str:
    W, H = 860, 360
    body = [header("Anatomy of a scored moment",
                   "What happens to every telemetry packet, 64 times a second")]

    cards = [
        ("TELEMETRY PACKET", "every 1/64 s", ["position · velocity", "slip · surface"], TX_MID, None),
        ("PER-MOMENT RATE", "points / second", ["base_rate", "× angle_factor", "× speed_factor"], OK, "mono"),
        ("INTEGRATE", "over the run", ["× dt", "sum every", "scoring moment"], TX_MID, "mono"),
        ("SCALE → SCORE", "one calibration", ["raw × 10.716", "= in-game", "magnitude"], AC_BRIGHT, "mono"),
    ]
    margin = 28
    gap = 30
    cw = (W - 2 * margin - 3 * gap) / 4
    cy, ch = 100, 96
    centers = []
    for i, (titlec, sub, lines, accent, mono) in enumerate(cards):
        x = margin + i * (cw + gap)
        centers.append((x, x + cw))
        bar = OK if i == 1 else (AC if i == 3 else BD_STRONG)
        body.append(rect(x, cy, cw, ch, fill=BG_ELEV, stroke=BD_MUTED, sw=1, rx=4))
        body.append(f'<rect x="{n(x)}" y="{n(cy)}" width="3.5" height="{ch}" rx="1.5" fill="{bar}"/>')
        body.append(text(x + 16, cy + 24, titlec, 11.5, TX_HI, weight=650, spacing=0.3))
        body.append(text(x + 16, cy + 39, sub, 9.5, TX_DIM, weight=500))
        font = FONT_MONO if mono else FONT_UI
        for j, ln in enumerate(lines):
            body.append(text(x + 16, cy + 58 + j * 13.5, ln, 10.5 if mono else 10,
                             accent if j == 0 and mono else TX_LO, font=font,
                             weight=600 if mono and j == 0 else 500))
        if i < 3:
            ax = x + cw + gap / 2
            body.append(f'<path d="M {n(x + cw + 6)},{n(cy + ch / 2)} L {n(x + cw + gap - 6)},{n(cy + ch / 2)}" '
                        f'stroke="{AC}" stroke-width="1.6" marker-end="url(#arrow)"/>')

    # arrowhead marker
    body.insert(0, f'<defs><marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" '
                    f'markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
                    f'<path d="M 0 1 L 9 5 L 0 9 z" fill="{AC}"/></marker></defs>')

    # gate caption strip
    gy = cy + ch + 34
    body.append(f'<rect x="28" y="{n(gy - 18)}" width="{W - 56}" height="40" rx="4" '
                f'fill="{BG_BODY}" stroke="{BD_DIM}" stroke-width="1"/>')
    body.append(text(44, gy + 1, "A moment scores only while drifting:", 11, TX_MID, weight=600))
    cond = "speed ≥ 1.5 m/s   ·   angle 10°–120°   ·   rear slip ≥ 1.0   ·   ≥ 2 wheels on tarmac †"
    body.append(text(44, gy + 16, cond, 10.5, OK, font=FONT_MONO, weight=550))
    body.append(text(W - 44, gy + 16, "† winter only", 9.5, TX_DIM, anchor="end"))

    # worked example
    ey = gy + 52
    body.append(text(44, ey, "Example", 10, TX_DIM, spacing=1.2, weight=650))
    ex = "30° at 25 m/s on tarmac →  1000 × 0.94 × 0.42 ≈ 392 pts/s  →  × dt  →  × 10.716"
    body.append(text(108, ey, ex, 11, TX_LO, font=FONT_MONO, weight=500))

    return svg_doc(W, H, "FH6 drift scoring pipeline",
                   "Each telemetry packet flows through four stages: the packet, the per-moment "
                   "rate (base rate times angle factor times speed factor), integration over time, "
                   "and a final scale to in-game magnitude. A moment scores only while the car is "
                   "drifting with at least two wheels on tarmac (winter only).",
                   "\n".join(body))


def main():
    out = Path(__file__).resolve().parent.parent / "docs" / "img"
    out.mkdir(parents=True, exist_ok=True)
    files = {
        "angle-factor-curve.svg": angle_curve_svg(),
        "speed-factor-curve.svg": speed_curve_svg(),
        "scoring-pipeline.svg": pipeline_svg(),
    }
    for name, content in files.items():
        (out / name).write_text(content, encoding="utf-8")
        print(f"wrote {out / name}  ({len(content)} bytes)")


if __name__ == "__main__":
    main()
