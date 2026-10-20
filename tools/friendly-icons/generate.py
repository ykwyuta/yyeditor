"""Generate yydraw's friendly, approachable ('Irasutoya'-like) vector icon catalog and inventory."""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/yy-draw/assets/friendly-icons.json"
INVENTORY = ROOT / "docs/yydraw-friendly-icons.md"

LINE = "#4a3b32"
SKIN = "#fce2c4"
CHEEK = "#f79bb0"
HAIR = "#6b4c3b"
HAIR_SUB = "#8a624a"
ACCENT = "#e06d53"
PALE = "#fff3e6"
CLOTH = "#6fb3d2"


def path(d, fill="none", stroke=LINE, width=2):
    return f'<path d="{d}" fill="{fill}" stroke="{stroke}" stroke-width="{width}"/>'


def rect(x, y, w, h, fill="#fff", radius=4, stroke=LINE, width=2):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill}" stroke="{stroke}" stroke-width="{width}"/>'


def circle(x, y, r, fill="#fff", stroke=LINE, width=2):
    s = f' stroke="{stroke}" stroke-width="{width}"' if stroke != "none" else ""
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}"{s}/>'


def ellipse(x, y, rx, ry, fill="#fff", stroke=LINE, width=2):
    s = f' stroke="{stroke}" stroke-width="{width}"' if stroke != "none" else ""
    return f'<ellipse cx="{x}" cy="{y}" rx="{rx}" ry="{ry}" fill="{fill}"{s}/>'


def dot(x, y, r=1.8, fill=LINE):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}"/>'


def inset(svg, x, y, scale):
    return f'<g transform="translate({x} {y}) scale({scale})">{svg}</g>'


def backdrop(color=None):
    c = color or PALE
    return circle(32, 33, 26, c, "none")


def cheeks(x=32, y=23, dx=6, rx=2.6, ry=1.5):
    return ellipse(x - dx, y, rx, ry, CHEEK, "none") + ellipse(x + dx, y, rx, ry, CHEEK, "none")


def sparkle(x, y, r=4, fill="#ffd54f"):
    return path(
        f"M{x} {y-r} Q{x} {y} {x+r} {y} Q{x} {y} {x} {y+r} Q{x} {y} {x-r} {y} Q{x} {y} {x} {y-r} Z",
        fill,
        LINE,
        1.5,
    )


def sweat(x, y):
    return path(
        f"M{x} {y} C{x-4} {y+6} {x-3} {y+11} {x} {y+11} C{x+3} {y+11} {x+4} {y+6} {x} {y} Z",
        "#7fd4f5",
        LINE,
        1.6,
    )


def anger_mark(x, y):
    return path(
        f"M{x-5} {y-2} Q{x-1} {y-1} {x-2} {y-5} M{x+5} {y-2} Q{x+1} {y-1} {x+2} {y-5} "
        f"M{x-5} {y+2} Q{x-1} {y+1} {x-2} {y+5} M{x+5} {y+2} Q{x+1} {y+1} {x+2} {y+5}",
        stroke="#e85d5d",
        width=2,
    )


def heart_shape(x, y, s=1, fill="#f58298"):
    return inset(
        path("M0 4 C-5 -3 -13 2 -7 10 L0 16 L7 10 C13 2 5 -3 0 4 Z", fill, LINE, 1.8),
        x,
        y,
        s,
    )


def question_mark(x, y):
    return path(f"M{x-3} {y-4} C{x-3} {y-9} {x+4} {y-9} {x+4} {y-4} C{x+4} {y-1} {x} {y} {x} {y+3}", stroke=ACCENT, width=2.6) + dot(x, y + 7, 1.8, ACCENT)


def exclamation_mark(x, y):
    return path(f"M{x} {y-7} V{y+2}", stroke=ACCENT, width=3) + dot(x, y + 7, 2, ACCENT)


def f(v):
    return f"{round(v, 2):g}"


def person_head(x=32, y=22, r=11, expr="smile", hair="short"):
    s = r / 11.0
    art = ""
    if hair == "long":
        art += path(f"M{f(x-r-1*s)} {f(y+2*s)} V{f(y+13*s)} Q{f(x)} {f(y+17*s)} {f(x+r+1*s)} {f(y+13*s)} V{f(y+2*s)} Z", HAIR_SUB)
    elif hair == "bun":
        art += circle(f(x), f(y - r - 3 * s), f(4.5 * s), HAIR_SUB)
    art += circle(f(x - r + 1 * s), f(y + 1 * s), f(2.8 * s), SKIN) + circle(f(x + r - 1 * s), f(y + 1 * s), f(2.8 * s), SKIN)
    art += circle(f(x), f(y), f(r), SKIN)
    if hair == "short":
        art += path(
            f"M{f(x-r)} {f(y-1*s)} C{f(x-r)} {f(y-r-3*s)} {f(x+r)} {f(y-r-3*s)} {f(x+r)} {f(y-1*s)} Q{f(x+4*s)} {f(y-6*s)} {f(x)} {f(y-3*s)} Q{f(x-5*s)} {f(y-6*s)} {f(x-r)} {f(y-1*s)} Z",
            HAIR,
        )
    else:
        art += path(
            f"M{f(x-r)} {f(y)} C{f(x-r)} {f(y-r-3*s)} {f(x+r)} {f(y-r-3*s)} {f(x+r)} {f(y)} Q{f(x+5*s)} {f(y-6*s)} {f(x-1*s)} {f(y-4*s)} Q{f(x-6*s)} {f(y-6*s)} {f(x-r)} {f(y)} Z",
            HAIR_SUB,
        )
    art += cheeks(round(x, 2), round(y + 2.5 * s, 2), round(r * 0.52, 2), round(2.6 * s, 2), round(1.5 * s, 2))
    ex = r * 0.38
    if expr == "smile":
        art += dot(f(x - ex), f(y - 0.5 * s), f(1.6 * s)) + dot(f(x + ex), f(y - 0.5 * s), f(1.6 * s))
        art += path(f"M{f(x-3.5*s)} {f(y+3.5*s)} Q{f(x)} {f(y+7*s)} {f(x+3.5*s)} {f(y+3.5*s)}", width=1.8)
    elif expr == "laugh":
        art += path(f"M{f(x-ex-2*s)} {f(y)} Q{f(x-ex)} {f(y-3*s)} {f(x-ex+2*s)} {f(y)} M{f(x+ex-2*s)} {f(y)} Q{f(x+ex)} {f(y-3*s)} {f(x+ex+2*s)} {f(y)}", width=1.8)
        art += path(f"M{f(x-4*s)} {f(y+3*s)} H{f(x+4*s)} Q{f(x+3.5*s)} {f(y+8.2*s)} {f(x)} {f(y+8.2*s)} Q{f(x-3.5*s)} {f(y+8.2*s)} {f(x-4*s)} {f(y+3*s)} Z", "#ef7b73", LINE, 1.6)
    elif expr == "troubled":
        art += path(f"M{f(x-ex-2*s)} {f(y-3*s)} L{f(x-ex+2*s)} {f(y-4.5*s)} M{f(x+ex-2*s)} {f(y-4.5*s)} L{f(x+ex+2*s)} {f(y-3*s)}", width=1.6)
        art += dot(f(x - ex), f(y), f(1.5 * s)) + dot(f(x + ex), f(y), f(1.5 * s))
        art += path(f"M{f(x-3.5*s)} {f(y+5.5*s)} Q{f(x-1.5*s)} {f(y+3.5*s)} {f(x)} {f(y+5.5*s)} T{f(x+3.5*s)} {f(y+5.5*s)}", width=1.6)
    elif expr == "cry":
        art += path(f"M{f(x-ex-2*s)} {f(y-3*s)} L{f(x-ex+2*s)} {f(y-4.5*s)} M{f(x+ex-2*s)} {f(y-4.5*s)} L{f(x+ex+2*s)} {f(y-3*s)}", width=1.6)
        art += path(f"M{f(x-ex-2*s)} {f(y)} Q{f(x-ex)} {f(y+2*s)} {f(x-ex+2*s)} {f(y)} M{f(x+ex-2*s)} {f(y)} Q{f(x+ex)} {f(y+2*s)} {f(x+ex+2*s)} {f(y)}", width=1.6)
        art += ellipse(f(x - ex - 2 * s), f(y + 3.5 * s), f(1.5 * s), f(2.5 * s), "#7fd4f5", "none") + ellipse(f(x + ex + 2 * s), f(y + 3.5 * s), f(1.5 * s), f(2.5 * s), "#7fd4f5", "none")
        art += path(f"M{f(x-3.5*s)} {f(y+6*s)} Q{f(x)} {f(y+3*s)} {f(x+3.5*s)} {f(y+6*s)}", width=1.8)
    elif expr == "angry":
        art += path(f"M{f(x-ex-2*s)} {f(y-4.5*s)} L{f(x-ex+2*s)} {f(y-2.5*s)} M{f(x+ex-2*s)} {f(y-2.5*s)} L{f(x+ex+2*s)} {f(y-4.5*s)}", width=1.8)
        art += dot(f(x - ex), f(y), f(1.6 * s)) + dot(f(x + ex), f(y), f(1.6 * s))
        art += path(f"M{f(x-4*s)} {f(y+6*s)} Q{f(x)} {f(y+2*s)} {f(x+4*s)} {f(y+6*s)} Z", "#ef7b73", LINE, 1.6)
    elif expr == "surprised":
        art += path(f"M{f(x-ex-2*s)} {f(y-5*s)} Q{f(x-ex)} {f(y-7*s)} {f(x-ex+2*s)} {f(y-5*s)} M{f(x+ex-2*s)} {f(y-5*s)} Q{f(x+ex)} {f(y-7*s)} {f(x+ex+2*s)} {f(y-5*s)}", width=1.5)
        art += circle(f(x - ex), f(y - 0.5 * s), f(1.8 * s), LINE, "none") + circle(f(x + ex), f(y - 0.5 * s), f(1.8 * s), LINE, "none")
        art += circle(f(x), f(y + 5 * s), f(2.6 * s), "#ef7b73", LINE, 1.6)
    elif expr == "peace":
        art += path(f"M{f(x-ex-2*s)} {f(y-0.5*s)} Q{f(x-ex)} {f(y+2*s)} {f(x-ex+2*s)} {f(y-0.5*s)} M{f(x+ex-2*s)} {f(y-0.5*s)} Q{f(x+ex)} {f(y+2*s)} {f(x+ex+2*s)} {f(y-0.5*s)}", width=1.7)
        art += path(f"M{f(x-3*s)} {f(y+4*s)} Q{f(x)} {f(y+6.5*s)} {f(x+3*s)} {f(y+4*s)}", width=1.7)
    elif expr == "sleepy":
        art += path(f"M{f(x-ex-2*s)} {f(y)} Q{f(x-ex)} {f(y+2*s)} {f(x-ex+2*s)} {f(y)} M{f(x+ex-2*s)} {f(y)} Q{f(x+ex)} {f(y+2*s)} {f(x+ex+2*s)} {f(y)}", width=1.7)
        art += circle(f(x), f(y + 4.8 * s), f(1.8 * s), "#ef7b73", LINE, 1.5)
    elif expr == "determined":
        art += path(f"M{f(x-ex-2*s)} {f(y-4*s)} L{f(x-ex+2*s)} {f(y-3*s)} M{f(x+ex-2*s)} {f(y-3*s)} L{f(x+ex+2*s)} {f(y-4*s)}", width=1.7)
        art += dot(f(x - ex), f(y - 0.5 * s), f(1.6 * s)) + dot(f(x + ex), f(y - 0.5 * s), f(1.6 * s))
        art += path(f"M{f(x-3.5*s)} {f(y+4*s)} Q{f(x)} {f(y+7*s)} {f(x+3.5*s)} {f(y+4*s)}", width=1.8)
    return art


def person_body(x=32, y=34, w=24, h=22, color=None, collar=True):
    c = color or CLOTH
    hw = w / 2
    art = path(f"M{x-hw} {y+h} V{y+9} C{x-hw} {y-2} {x+hw} {y-2} {x+hw} {y+9} V{y+h} Z", c)
    if collar:
        art += path(f"M{x-5} {y+1} L{x} {y+6} L{x+5} {y+1}", "#fff", LINE, 1.6)
    return art


def hand(x, y, r=3.2):
    return circle(x, y, r, SKIN)


def suit_body(x=32, y=34, w=24, h=22, jacket="#4a6fa5", tie="#e65c4f", female=False):
    hw = w / 2
    art = path(f"M{x-hw} {y+h} V{y+9} C{x-hw} {y-2} {x+hw} {y-2} {x+hw} {y+9} V{y+h} Z", jacket)
    art += path(f"M{x-5.5} {y+1} L{x} {y+12} L{x+5.5} {y+1} Z", "#fff", LINE, 1.6)
    if female:
        art += circle(x, y + 6.5, 2.2, tie, LINE, 1.4)
    else:
        art += path(f"M{x} {y+5} L{x+2.2} {y+11} L{x} {y+16} L{x-2.2} {y+11} Z", tie, LINE, 1.3)
    return art


def coat_body(x=32, y=34, w=24, h=22, inner="#6fb3d2"):
    hw = w / 2
    art = path(f"M{x-hw} {y+h} V{y+9} C{x-hw} {y-2} {x+hw} {y-2} {x+hw} {y+9} V{y+h} Z", "#fff")
    art += path(f"M{x-5} {y+1} L{x} {y+11} L{x+5} {y+1} Z", inner, LINE, 1.6)
    art += path(f"M{x} {y+11} V{y+h}", width=1.6)
    return art


def work_body(x=32, y=34, w=24, h=22, color="#5c9ead", stripe="#ffd54f"):
    hw = w / 2
    art = path(f"M{x-hw} {y+h} V{y+9} C{x-hw} {y-2} {x+hw} {y-2} {x+hw} {y+9} V{y+h} Z", color)
    art += path(f"M{x-5} {y+1} L{x} {y+6} L{x+5} {y+1}", "#fff", LINE, 1.6)
    art += path(f"M{x-hw+2} {y+13} H{x+hw-2}", stroke=stripe, width=2.5)
    return art


def glasses(x=32, y=21.5, r=11):
    s = r / 11.0
    ex = r * 0.38
    return (
        circle(f(x - ex), f(y - 0.5 * s), f(3.3 * s), "none", LINE, 1.5)
        + circle(f(x + ex), f(y - 0.5 * s), f(3.3 * s), "none", LINE, 1.5)
        + path(f"M{f(x-ex+3.3*s)} {f(y-0.5*s)} H{f(x+ex-3.3*s)}", width=1.4)
    )


def helmet_hat(x=32, y=14, color="#ffd54f"):
    return path(f"M{x-12} {y+3} C{x-12} {y-8} {x+12} {y-8} {x+12} {y+3} Z", color) + path(
        f"M{x-14} {y+3} H{x+14}", width=2.4
    )


def headset_gear(x=32, y=22, r=11):
    return (
        path(f"M{x-r} {y} C{x-r} {y-r-4} {x+r} {y-r-4} {x+r} {y}", stroke=LINE, width=2)
        + rect(x - r - 2, y - 3, 4, 7, "#4a6fa5", 1.5)
        + rect(x + r - 2, y - 3, 4, 7, "#4a6fa5", 1.5)
        + path(f"M{x+r} {y+4} Q{x+r-2} {y+9} {x+3} {y+8}", width=1.6)
    )


def animal_head(kind, x=32, y=24, r=11, expr="smile"):
    art = ""
    if kind == "dog":
        art += ellipse(x - r, y + 2, 4, 7.5, "#c98b5b") + ellipse(x + r, y + 2, 4, 7.5, "#c98b5b")
        art += circle(x, y, r, "#f7dfc4")
        art += ellipse(x, y + 3, 5.5, 4, "#fff", "none")
        art += ellipse(x, y + 1, 2.3, 1.6, LINE, "none")
    elif kind == "cat":
        art += path(f"M{x-r+1} {y-4} L{x-r-2} {y-r-3} L{x-3} {y-r+1} Z M{x+r-1} {y-4} L{x+r+2} {y-r-3} L{x+3} {y-r+1} Z", "#fff6ea")
        art += path(f"M{x-r+2} {y-5} L{x-r} {y-r} L{x-5} {y-r+2} Z M{x+r-2} {y-5} L{x+r} {y-r} L{x+5} {y-r+2} Z", CHEEK, "none")
        art += circle(x, y, r, "#fff6ea")
        art += path(f"M{x-r-2} {y+1} H{x-7} M{x-r-2} {y+4} H{x-7} M{x+7} {y+1} H{x+r+2} M{x+7} {y+4} H{x+r+2}", width=1.4)
        art += ellipse(x, y + 1, 1.8, 1.3, "#e87a90", "none")
    elif kind == "rabbit":
        art += ellipse(x - 5, y - r - 5, 3.3, 8, "#fff") + ellipse(x + 5, y - r - 5, 3.3, 8, "#fff")
        art += ellipse(x - 5, y - r - 5, 1.6, 5, CHEEK, "none") + ellipse(x + 5, y - r - 5, 1.6, 5, CHEEK, "none")
        art += circle(x, y, r, "#fff")
        art += ellipse(x, y + 1, 1.8, 1.3, "#f08ca2", "none")
    elif kind == "bear":
        art += circle(x - r + 2, y - r + 3, 4.2, "#c68b59") + circle(x + r - 2, y - r + 3, 4.2, "#c68b59")
        art += circle(x, y, r, "#c68b59")
        art += ellipse(x, y + 3, 5, 4, "#f7e3cc", "none")
        art += ellipse(x, y + 1, 2.2, 1.5, LINE, "none")
    elif kind == "panda":
        art += circle(x - r + 2, y - r + 3, 4.2, LINE) + circle(x + r - 2, y - r + 3, 4.2, LINE)
        art += circle(x, y, r, "#fff")
        art += ellipse(x - 4.5, y - 1, 3.2, 2.5, "#6b5b53", "none") + ellipse(x + 4.5, y - 1, 3.2, 2.5, "#6b5b53", "none")
        art += ellipse(x, y + 1.5, 2, 1.4, LINE, "none")
    elif kind == "fox":
        art += path(f"M{x-r+1} {y-3} L{x-r-2} {y-r-4} L{x-2} {y-r+1} Z M{x+r-1} {y-3} L{x+r+2} {y-r-4} L{x+2} {y-r+1} Z", "#f2a455")
        art += circle(x, y, r, "#f2a455")
        art += path(f"M{x-r+1} {y+2} Q{x} {y+9} {x+r-1} {y+2} Q{x} {y+12} {x-r+1} {y+2} Z", "#fff7e8", "none")
        art += ellipse(x, y + 1.5, 2, 1.4, LINE, "none")
    art += cheeks(x, y + 2.8, r * 0.55)
    eye_color = "#fff" if kind == "panda" else LINE
    if expr == "smile":
        art += dot(x - 4.5, y - 1, 1.5, eye_color) + dot(x + 4.5, y - 1, 1.5, eye_color)
        art += path(f"M{x-3} {y+3.5} Q{x} {y+6} {x+3} {y+3.5}", width=1.6)
    elif expr == "laugh":
        art += path(f"M{x-6} {y-1} Q{x-4.5} {y-3.5} {x-3} {y-1} M{x+3} {y-1} Q{x+4.5} {y-3.5} {x+6} {y-1}", stroke=eye_color, width=1.6)
        art += path(f"M{x-3.5} {y+3} H{x+3.5} Q{x} {y+8} {x-3.5} {y+3} Z", "#ef7b73", LINE, 1.5)
    elif expr == "troubled":
        art += dot(x - 4.5, y - 1, 1.5, eye_color) + dot(x + 4.5, y - 1, 1.5, eye_color)
        art += path(f"M{x-6} {y-4} L{x-2.5} {y-5} M{x+2.5} {y-5} L{x+6} {y-4}", width=1.5)
        art += path(f"M{x-3} {y+5} Q{x} {y+2.5} {x+3} {y+5}", width=1.5)
    return art


def animal_body(kind, x=32, y=35, w=20, h=20):
    colors = {
        "dog": "#f7dfc4",
        "cat": "#fff6ea",
        "rabbit": "#ffffff",
        "bear": "#c68b59",
        "panda": "#ffffff",
        "fox": "#f2a455",
    }
    c = colors.get(kind, "#fff")
    hw = w / 2
    art = path(f"M{x-hw} {y+h} V{y+8} C{x-hw} {y-2} {x+hw} {y-2} {x+hw} {y+8} V{y+h} Z", c)
    art += ellipse(x, y + 11, hw * 0.55, 6, "#fff", "none")
    return art


def make_catalog():
    entries = []
    categories = []

    def category(name, accent, pale, cloth, items):
        global ACCENT, PALE, CLOTH
        ACCENT, PALE, CLOTH = accent, pale, cloth
        rows = []
        for slug, name_ja, keywords, draw in items:
            svg = (
                '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">'
                '<g stroke-linecap="round" stroke-linejoin="round">'
                + draw()
                + "</g></svg>"
            )
            entries.append(
                dict(
                    vendor="イラスト",
                    category=name,
                    name=name_ja,
                    keywords=keywords,
                    source="yydraw:friendly-icons/v1",
                    file=f"friendly/{slug}.svg",
                    svg=svg,
                )
            )
            rows.append((name_ja, keywords, slug))
        categories.append((name, rows))

    category("01 表情・感情", "#e56b55", "#fff1e6", "#73b3d6", [
        ("smile-person", "笑顔・にっこり", "smile happy friendly 笑顔 にっこり 嬉しい 人物 イラスト",
         lambda: backdrop()+person_body()+person_head(expr="smile")+sparkle(13, 14)+sparkle(51, 16, 3)),
        ("laugh-person", "大笑い・爆笑", "laugh joyful cheerful 大笑い 爆笑 楽しい 喜ぶ 人物",
         lambda: backdrop()+person_body(color="#f4a261")+person_head(expr="laugh")+hand(17, 36)+hand(47, 36)+path("M11 16 L16 19 M53 16 L48 19 M12 23 L17 24 M52 23 L47 24", stroke=ACCENT, width=2.2)),
        ("troubled-person", "困る・悩む", "troubled worried confused 困る 悩む 困惑 どうしよう 人物",
         lambda: backdrop()+person_body(color="#8ab6d6")+person_head(expr="troubled")+hand(21, 27)+sweat(50, 13)+path("M11 14 Q8 20 13 24", stroke="#7393a7", width=2)),
        ("crying-person", "泣く・悲しい", "cry crying sad tears 泣く 悲しい 涙 ショック 人物",
         lambda: backdrop()+person_body(color="#88a2c4")+person_head(expr="cry")+hand(23, 27)+hand(41, 27)+sweat(13, 22)),
        ("angry-person", "怒る・プンプン", "angry mad rage 怒る プンプン 激怒 イライラ 人物",
         lambda: backdrop()+person_body(color="#e57373")+person_head(expr="angry")+hand(18, 41)+hand(46, 41)+anger_mark(49, 13)),
        ("surprised-person", "驚く・びっくり", "surprised shocked astonished 驚く びっくり 仰天 ハッとする 人物",
         lambda: backdrop()+person_body(color="#f6bd60")+person_head(expr="surprised")+hand(16, 30)+hand(48, 30)+exclamation_mark(52, 16)+path("M12 13 L17 18 M10 21 H16", stroke=ACCENT, width=2.2)),
        ("idea-person", "ひらめき・納得", "idea eureka lightbulb ひらめき 思いつく 納得 発見 人物",
         lambda: backdrop()+person_body(color="#81c995")+person_head(expr="smile")+hand(47, 26)+path("M47 36 L47 29", width=2.5)+circle(49, 13, 5.5, "#ffe066")+rect(47, 18, 4, 3, "#d9d9d9", 1)+path("M49 4 V6 M57 9 L55 10 M41 9 L43 10", stroke=ACCENT, width=2)),
        ("question-person", "疑問・はてな", "question doubt thinking 疑問 はてな 考える なぜ 人物",
         lambda: backdrop()+person_body(color="#95a5cd")+person_head(expr="troubled", hair="long")+hand(40, 29)+question_mark(50, 15)),
        ("relieved-person", "安心・ほっとする", "relieved peace calm 安心 ほっとする 安堵 リラックス 人物",
         lambda: backdrop()+person_body(color="#98d4a6")+person_head(expr="peace", hair="long")+hand(28, 41)+hand(36, 41)+heart_shape(49, 12, 0.55)),
        ("sleepy-person", "居眠り・うとうと", "sleepy tired nap 居眠り うとうと 眠い 疲れ 人物",
         lambda: backdrop()+person_body(color="#b39ddb")+person_head(expr="sleepy")+ellipse(47, 15, 6, 4.5, "#fff")+circle(41, 20, 2, "#fff")+path("M45 13 H49 L45 17 H49", stroke=ACCENT, width=1.6)),
    ])

    category("02 ポーズ・挨拶・案内", "#d97254", "#fff3e8", "#64b5c4", [
        ("bowing-person", "お辞儀・感謝", "bow bowing polite thanks お辞儀 挨拶 お礼 丁寧 ペコリ",
         lambda: backdrop()+path("M18 55 L21 37 C23 28 41 28 43 37 L46 55 Z", CLOTH)+person_head(32, 26, 11, "peace")+hand(27, 50)+hand(37, 50)+path("M14 17 Q19 12 24 14 M50 17 Q45 12 40 14", stroke=ACCENT, width=2)),
        ("guide-person", "案内・こちら", "guide point introduce 案内 紹介 こちら ガイド 説明",
         lambda: backdrop()+person_body(28, 34)+person_head(28, 22, 10, "smile", "long")+path("M39 40 L51 33", stroke=CLOTH, width=5)+hand(53, 31)+sparkle(53, 18, 3.5)),
        ("ok-sign-person", "OKサイン・まる", "ok good approve circle OK まる 了解 バッチリ",
         lambda: backdrop()+person_body(26, 35, 22, 21)+person_head(26, 23, 10, "laugh")+circle(48, 24, 9, "none", "#e85d5d", 4)+hand(39, 34)),
        ("ng-sign-person", "NG・ばつ", "ng no cross stop NG ばつ ダメ 禁止 注意",
         lambda: backdrop()+person_body(26, 35, 22, 21, "#8d99ae")+person_head(26, 23, 10, "troubled")+path("M20 41 L32 51 M32 41 L20 51", stroke=LINE, width=3)+path("M41 16 L55 30 M55 16 L41 30", stroke="#e63946", width=4)),
        ("cheering-person", "応援・ガッツポーズ", "cheer guts pose support 応援 ガッツポーズ やったー 達成",
         lambda: backdrop()+person_body(32, 35, 22, 21, "#f4a261")+person_head(32, 23, 10, "laugh")+path("M20 40 L14 27 M44 40 L50 27", stroke="#f4a261", width=5)+hand(14, 25)+hand(50, 25)+sparkle(13, 13, 3)),
        ("waving-person", "手を振る・挨拶", "wave hello goodbye 手を振る こんにちは さようなら 挨拶",
         lambda: backdrop()+person_body(29, 35)+person_head(29, 22, 10, "smile")+path("M40 39 L49 25", stroke=CLOTH, width=5)+hand(50, 23)+path("M45 14 Q52 13 55 19 M49 11 Q56 11 58 17", stroke=ACCENT, width=1.8)),
        ("pointing-person", "指差し確認・注目", "point check finger 指差し 確認 注目 ポイント チェック",
         lambda: backdrop()+person_body(28, 35, 22, 21, "#5c9ead")+person_head(28, 22, 10, "determined")+path("M39 39 L50 28", stroke="#5c9ead", width=5)+hand(51, 26)+path("M51 23 V17", stroke=SKIN, width=3.5)+sparkle(51, 11, 3)),
        ("apology-person", "ごめんなさい・謝る", "apology sorry excuse ごめんなさい 謝る すみません お詫び",
         lambda: backdrop()+person_body(32, 35, 22, 21, "#90a4ae")+person_head(32, 22, 10, "troubled")+path("M32 37 V49", stroke=SKIN, width=4)+hand(32, 37)+sweat(49, 14)+sweat(15, 16)),
        ("banzai-person", "万歳・大喜び", "banzai hooray celebrate 万歳 バンザイ 大喜び 成功",
         lambda: backdrop()+person_body(32, 36, 22, 20, "#e76f51")+person_head(32, 24, 10, "laugh", "long")+path("M21 38 L13 18 M43 38 L51 18", stroke="#e76f51", width=5)+hand(12, 16)+hand(52, 16)+sparkle(32, 8, 3.5)),
        ("fighting-pose-person", "やる気・ファイト", "fighting motivated energetic やる気 ファイト 頑張る 意気込み",
         lambda: backdrop()+person_body(32, 35, 22, 21, "#e07a5f")+person_head(32, 22, 10, "determined")+path("M21 15 H43", stroke="#e63946", width=3)+hand(18, 35)+hand(46, 35)+path("M11 27 L15 21 M53 27 L49 21", stroke=ACCENT, width=2.2)),
    ])

    category("03 コミュニケーション・交流", "#d46a7e", "#fdedf0", "#76b4bd", [
        ("conversation-pair", "雑談・おしゃべり", "chat conversation talk 雑談 おしゃべり 会話 相談",
         lambda: backdrop()+inset(person_body()+person_head(expr="smile"), -10, 8, 0.78)+inset(person_body(color="#f28b82")+person_head(expr="laugh", hair="long"), 18, 8, 0.78)+rect(23, 6, 18, 11, "#fff", 4)+dot(28, 11.5, 1.3)+dot(32, 11.5, 1.3)+dot(36, 11.5, 1.3)),
        ("handshake-pair", "握手・合意", "handshake agreement partnership 握手 合意 仲良し 協力",
         lambda: backdrop()+inset(person_body()+person_head(expr="smile"), -11, 7, 0.78)+inset(person_body(color="#81c995")+person_head(expr="smile", hair="long"), 19, 7, 0.78)+path("M23 44 H41", stroke=SKIN, width=5)+circle(32, 44, 4, SKIN)+heart_shape(32, 9, 0.55)),
        ("praise-pair", "褒める・称賛", "praise compliment good job 褒める 称賛 拍手 評価",
         lambda: backdrop()+inset(person_body()+person_head(expr="smile"), -11, 8, 0.78)+inset(person_body(color="#f6bd60")+person_head(expr="laugh", hair="long"), 19, 8, 0.78)+sparkle(32, 12, 4)+sparkle(48, 12, 3)+path("M24 38 L29 34", stroke=SKIN, width=4)),
        ("consultation-pair", "相談・カウンセリング", "consultation advice listen 相談 悩み相談 アドバイス 傾聴",
         lambda: backdrop()+inset(person_body(color="#90a4ae")+person_head(expr="troubled"), -11, 8, 0.78)+inset(person_body(color="#81c995")+person_head(expr="smile", hair="long"), 19, 8, 0.78)+rect(8, 49, 48, 6, "#d7b38c", 2)+sweat(11, 14)),
        ("high-five-pair", "ハイタッチ・協力", "high five teamwork celebration ハイタッチ 協力 チームワーク 達成",
         lambda: backdrop()+inset(person_body()+person_head(expr="laugh"), -11, 9, 0.76)+inset(person_body(color="#f4a261")+person_head(expr="laugh", hair="long"), 19, 9, 0.76)+path("M22 39 L32 23 L42 39", stroke=SKIN, width=4.5)+sparkle(32, 11, 4.5)),
        ("gift-giving", "贈り物・プレゼント渡し", "gift present thank you 贈り物 プレゼント 感謝 お祝い",
         lambda: backdrop()+person_body(23, 35, 20, 20)+person_head(23, 22, 9.5, "smile", "long")+rect(36, 31, 16, 15, "#f28b82", 2)+path("M44 31 V46 M36 38 H52 M44 31 C40 26 37 31 44 31 C51 31 48 26 44 31", stroke="#ffe066", width=2.2)+heart_shape(46, 13, 0.5)),
        ("helping-hand", "助け合い・サポート", "help support kindness 助け合い サポート 手助け 親切",
         lambda: backdrop()+inset(person_body(color="#81c995")+person_head(expr="smile"), -10, 6, 0.78)+inset(person_body(color="#73b3d6")+person_head(expr="peace", hair="long"), 18, 10, 0.75)+path("M24 41 H38", stroke=SKIN, width=4.5)+heart_shape(32, 10, 0.55)),
        ("megaphone-cheer", "メガホンで応援・告知", "megaphone cheer announce メガホン 応援 告知 呼びかけ",
         lambda: backdrop()+person_body(24, 35, 20, 20, "#f4a261")+person_head(24, 22, 9.5, "laugh")+path("M35 26 L53 18 V34 L35 28 Z", "#ffca3a")+path("M56 19 L60 16 M57 26 H61 M56 33 L60 36", stroke=ACCENT, width=2.2)),
        ("whisper-secret", "内緒話・ひそひそ", "whisper secret rumor 内緒話 ひそひそ 秘密 噂話",
         lambda: backdrop()+person_body(26, 35, 22, 20, "#b39ddb")+person_head(26, 22, 10, "surprised", "long")+hand(36, 26)+rect(43, 11, 14, 11, "#fff", 4)+dot(48, 16.5, 1.4)+dot(52, 16.5, 1.4)),
        ("harmony-circle", "仲良し・輪になる", "harmony friends together 仲良し 仲間 輪 団結",
         lambda: backdrop()+inset(person_body(color="#73b3d6")+person_head(expr="smile"), -12, 12, 0.65)+inset(person_body(color="#f28b82")+person_head(expr="laugh", hair="long"), 22, 12, 0.65)+inset(person_body(color="#81c995")+person_head(expr="smile"), 5, 3, 0.68)+heart_shape(32, 4, 0.45)),
    ])

    category("04 仕事・オフィス風景", "#3d85c6", "#eaf4fc", "#5b9bd5", [
        ("presentation-scene", "プレゼン・発表", "presentation pitch explain プレゼン 発表 説明 セミナー",
         lambda: backdrop()+rect(31, 10, 26, 22, "#fff", 2)+path("M36 25 L42 18 L47 22 L53 15", stroke=ACCENT, width=2.5)+person_body(19, 36, 18, 19)+person_head(19, 24, 8.5, "smile")+path("M27 39 L39 24", stroke=LINE, width=2.2)),
        ("pc-work-happy", "パソコン作業・順調", "pc work laptop smooth パソコン作業 順調 デスクワーク 仕事",
         lambda: backdrop()+person_body(32, 31, 22, 20)+person_head(32, 19, 9.5, "smile")+path("M14 53 H50", stroke=LINE, width=2.5)+rect(22, 38, 20, 14, "#e6eef5", 2)+sparkle(49, 15, 3.5)+sparkle(15, 17, 3)),
        ("pc-trouble", "パソコンで困る・フリーズ", "pc trouble error crash パソコン トラブル フリーズ 困る エラー",
         lambda: backdrop()+person_body(32, 31, 22, 20, "#8d99ae")+person_head(32, 19, 9.5, "troubled")+rect(22, 38, 20, 14, "#e6eef5", 2)+path("M29 42 L35 48 M35 42 L29 48", stroke="#e63946", width=2.2)+sweat(50, 14)+path("M13 13 L16 19 L12 21 L16 27", stroke="#e63946", width=2)),
        ("busy-multitask", "猫の手も借りたい・多忙", "busy multitask rush 多忙 忙しい 残業 焦る 締め切り",
         lambda: backdrop()+person_body(32, 33, 22, 20, "#e07a5f")+person_head(32, 20, 9.5, "surprised")+rect(8, 36, 11, 15, "#fff", 1)+rect(45, 34, 11, 17, "#fff", 1)+sweat(48, 11)+sweat(16, 12)+path("M11 41 H16 M11 45 H16 M48 39 H53 M48 43 H53", width=1.5)),
        ("phone-call-smile", "電話応対・連絡", "phone call contact 電話 電話応対 連絡 通話",
         lambda: backdrop()+person_body(29, 35, 22, 20)+person_head(29, 22, 10, "smile", "long")+rect(40, 18, 7, 13, "#495057", 2)+hand(41, 27)+path("M49 14 Q55 17 54 24 M52 11 Q59 15 58 24", stroke=ACCENT, width=2)),
        ("business-card-exchange", "名刺交換・初対面", "business card exchange greeting 名刺交換 挨拶 初対面 商談",
         lambda: backdrop()+inset(person_body()+person_head(expr="smile"), -12, 8, 0.76)+inset(person_body(color="#6c757d")+person_head(expr="smile"), 20, 8, 0.76)+rect(26, 36, 12, 8, "#fff", 1)+sparkle(32, 14, 3.5)),
        ("brainstorming-board", "アイデア出し・付箋会議", "brainstorming sticky notes workshop アイデア出し 付箋 ブレスト 会議",
         lambda: backdrop()+rect(11, 10, 42, 30, "#fff", 3)+rect(16, 15, 9, 8, "#ffd54f", 1)+rect(28, 15, 9, 8, "#f79bb0", 1)+rect(40, 15, 8, 8, "#90e0ef", 1)+rect(21, 26, 10, 8, "#a8e6cf", 1)+person_head(32, 48, 7.5, "laugh")+sparkle(53, 10, 3)),
        ("coffee-break", "休憩・ほっと一息", "coffee break tea rest 休憩 コーヒー ほっと一息 リフレッシュ",
         lambda: backdrop()+person_body(28, 35, 22, 20, "#81c995")+person_head(28, 22, 10, "peace", "long")+rect(41, 34, 11, 11, "#fff", 2)+path("M52 36 H55 Q57 39 55 42 H52 M44 29 Q46 25 44 22 M49 29 Q51 25 49 22", stroke=ACCENT, width=1.8)),
        ("commuting-bag", "通勤・外出", "commute outing walk 通勤 外出 出社 外回り",
         lambda: backdrop()+person_body(27, 34, 20, 21)+person_head(27, 21, 9.5, "smile")+rect(39, 38, 15, 12, "#b5835a", 2)+path("M43 38 V35 H50 V38", stroke=LINE, width=2)+sparkle(14, 18, 3)),
        ("payday-bonus", "給料日・ボーナス", "payday bonus salary 給料日 ボーナス 収入 嬉しい",
         lambda: backdrop()+person_body(26, 35, 20, 20, "#f4a261")+person_head(26, 22, 9.5, "laugh")+rect(39, 24, 15, 23, "#fff", 2)+circle(46.5, 36, 5, "#ffd54f")+path("M44 34 L46.5 37 L49 34 M46.5 37 V40 M44 37 H49", width=1.5)+sparkle(48, 13, 3.5)),
    ])

    category("05 テレワーク・学習・作業", "#459373", "#e8f7f0", "#5ab190", [
        ("remote-work-home", "自宅テレワーク", "remote work wfh telework テレワーク 在宅ワーク リモート",
         lambda: backdrop()+path("M8 28 L32 9 L56 28 V54 H8 Z", "#fff")+person_body(32, 38, 18, 16)+person_head(32, 27, 8, "smile")+rect(24, 44, 16, 10, "#dce8f2", 2)),
        ("online-meeting-wave", "オンライン会議・画面越し", "online meeting video call オンライン会議 リモート会議 通話",
         lambda: backdrop()+rect(9, 11, 46, 34, "#fff", 4)+path("M25 45 V53 M18 53 H46", width=2.2)+person_body(32, 32, 18, 13)+person_head(32, 22, 7.5, "smile", "long")+hand(44, 25)),
        ("studying-desk", "勉強・資格試験", "study exam learning 勉強 学習 受験 資格 読書",
         lambda: backdrop()+person_body(32, 33, 22, 19)+person_head(32, 20, 9.5, "determined")+path("M14 48 L32 53 L50 48 L32 43 Z", "#fff")+path("M32 43 V53", width=1.8)+sparkle(49, 14, 3.5)),
        ("reading-book", "読書・マニュアル確認", "reading book manual 読書 本 マニュアル 調べる",
         lambda: backdrop()+person_body(32, 34, 22, 20)+person_head(32, 21, 9.5, "smile", "long")+path("M17 36 L32 40 L47 36 V52 L32 56 L17 52 Z", "#fff")+path("M32 40 V56", width=2)+hand(18, 44)+hand(46, 44)),
        ("writing-notes", "メモ・記録をとる", "memo note writing メモ 記録 ノート 筆記",
         lambda: backdrop()+rect(14, 12, 30, 41, "#fff", 3)+path("M20 22 H38 M20 30 H38 M20 38 H31", stroke="#6fb3d2", width=2.2)+path("M36 42 L50 22 L55 27 L41 47 L34 49 Z", "#ffd54f")+sparkle(50, 12, 3)),
        ("crafts-diy", "工作・DIY・修理", "diy craft repair tool 工作 修理 工具 ものづくり",
         lambda: backdrop()+person_body(28, 35, 20, 20, "#f4a261")+person_head(28, 22, 9.5, "smile")+path("M17 16 C19 9 37 9 39 16 Z", "#ffd54f")+path("M45 22 L53 30 M43 34 L55 20", stroke="#7f8c8d", width=4)+hand(43, 35)),
        ("delivery-box", "荷物受取・宅配", "delivery package parcel 宅配 荷物 配達 受け取り",
         lambda: backdrop()+person_body(26, 35, 20, 20)+person_head(26, 21, 9.5, "smile")+rect(32, 33, 22, 18, "#deb887", 2)+path("M40 33 V41 H46 V33", "#fff")+heart_shape(48, 16, 0.5)),
        ("cleaning-sweep", "掃除・整理整頓", "cleaning tidy broom 掃除 整理整頓 清掃 片付け",
         lambda: backdrop()+person_body(26, 35, 20, 20, "#73b3d6")+person_head(26, 22, 9.5, "smile", "bun")+path("M45 14 L39 42", stroke="#b5835a", width=3)+path("M33 42 H47 L49 54 H31 Z", "#ffd54f")+sparkle(50, 24, 3.5)+sparkle(14, 16, 3)),
        ("cooking-apron", "料理・自炊", "cooking kitchen meal 料理 自炊 エプロン 家事",
         lambda: backdrop()+person_body(28, 35, 22, 20, "#f28b82")+person_head(28, 22, 9.5, "laugh", "bun")+rect(21, 39, 14, 15, "#fff", 2)+ellipse(46, 41, 9, 5, "#495057")+path("M34 41 H38", width=3)+heart_shape(48, 18, 0.5)),
        ("laundry-hang", "洗濯・家事", "laundry washing housework 洗濯 家事 干す 暮らし",
         lambda: backdrop()+path("M8 18 H56", width=2)+path("M16 18 H34 L38 26 L33 29 L31 25 V44 H19 V25 L17 29 L12 26 Z", "#fff")+rect(39, 18, 12, 20, "#f79bb0", 2)+sparkle(48, 45, 3.5)+sparkle(14, 48, 3)),
    ])

    category("06 暮らし・買い物・健康", "#e07a5f", "#fff2ec", "#f28b82", [
        ("shopping-bag", "買い物・ショッピング", "shopping bag retail 買い物 ショッピング お出かけ 買出し",
         lambda: backdrop()+person_body(25, 35, 20, 20)+person_head(25, 22, 9.5, "smile", "long")+rect(37, 31, 17, 20, "#ffd54f", 2)+path("M41 31 V26 C41 21 50 21 50 26 V31", width=2)+path("M42 26 L46 19 L51 26", "#81c995")),
        ("shopping-cart-push", "カートで買い物", "shopping cart supermarket カート スーパー 買い物 まとめ買い",
         lambda: backdrop()+path("M10 22 H17 L22 43 H48 L53 27 H19", "#fff")+path("M20 34 H50 M31 27 V43 M41 27 V43", stroke=ACCENT, width=1.6)+circle(25, 50, 4, "#495057")+circle(45, 50, 4, "#495057")+rect(26, 17, 10, 10, "#81c995", 2)+rect(38, 19, 9, 8, "#ffd54f", 2)),
        ("savings-piggybank", "貯金・節約", "savings piggy bank money 貯金 節約 豚の貯金箱 家計",
         lambda: backdrop()+ellipse(32, 36, 18, 14, "#f9bec7")+ellipse(14, 36, 3.5, 5, "#f79bb0")+path("M22 24 L18 17 L27 22 Z M40 24 L44 17 L35 22 Z", "#f9bec7")+path("M23 48 V54 M41 48 V54 M28 24 H36", width=2.5)+dot(22, 33, 1.8)+circle(32, 14, 5.5, "#ffd54f")+sparkle(50, 18, 3.5)),
        ("wallet-coins", "お財布・お小遣い", "wallet purse money お財布 財布 支払い お小遣い",
         lambda: backdrop()+path("M13 26 C13 17 51 17 51 26 L54 48 C54 53 10 53 10 48 Z", "#e76f51")+path("M16 24 H48", width=2.5)+circle(28, 19, 3, "#ffd54f")+circle(36, 19, 3, "#ffd54f")+circle(47, 44, 7, "#ffd54f")+sparkle(14, 15, 3)),
        ("exercise-stretch", "運動・ストレッチ", "exercise stretch fitness 運動 ストレッチ 体操 健康",
         lambda: backdrop()+person_body(32, 36, 20, 19, "#81c995")+person_head(32, 23, 9.5, "laugh")+path("M20 38 L12 20 M44 38 L52 20", stroke="#81c995", width=5)+hand(12, 18)+hand(52, 18)+sparkle(32, 8, 3.5)),
        ("good-sleep", "快眠・ぐっすり", "sleep bed rest 快眠 睡眠 ぐっすり 休養",
         lambda: backdrop()+rect(9, 24, 46, 28, "#fff", 5)+person_head(24, 33, 8.5, "peace")+rect(28, 26, 25, 24, "#90e0ef", 4)+path("M42 12 H48 L42 18 H48 M50 7 H54 L50 11 H54", stroke=ACCENT, width=1.8)),
        ("catch-cold-mask", "マスク・風邪予防", "mask cold prevention マスク 風邪 予防 体調不良",
         lambda: backdrop()+person_body(32, 36, 22, 19, "#73b3d6")+person_head(32, 22, 10.5, "troubled")+rect(24, 22, 16, 8, "#fff", 3)+path("M24 24 L21 22 M40 24 L43 22 M24 28 L21 27 M40 28 L43 27", width=1.4)+sweat(49, 14)),
        ("first-aid-box", "救急箱・手当て", "first aid medical box 救急箱 手当て 薬 応急処置",
         lambda: backdrop()+rect(11, 21, 42, 31, "#fff", 4)+path("M24 21 V15 H40 V21", width=2.5)+path("M29 29 H35 V34 H40 V40 H35 V45 H29 V40 H24 V34 H29 Z", "#e63946", LINE, 1.6)+heart_shape(49, 12, 0.5)),
        ("hot-spring-bath", "お風呂・温泉", "bath onsen relax お風呂 温泉 入浴 リラックス",
         lambda: backdrop()+path("M8 40 H56 L52 54 H12 Z", "#deb887")+path("M10 40 Q20 36 32 40 T54 40", "#90e0ef")+person_head(32, 29, 9.5, "peace")+rect(26, 16, 12, 4, "#fff", 1)+path("M16 25 Q18 19 16 14 M48 25 Q50 19 48 14", stroke=ACCENT, width=2)),
        ("gardening-water", "園芸・水やり", "gardening plant watering 園芸 水やり 植物 育てる",
         lambda: backdrop()+path("M16 42 H32 L29 55 H19 Z", "#e07a5f")+path("M24 42 V28", stroke="#4caf50", width=3)+ellipse(18, 32, 5, 3, "#81c995")+ellipse(30, 30, 5, 3, "#81c995")+circle(24, 22, 5, "#f79bb0")+circle(24, 22, 2, "#ffd54f")+rect(40, 20, 14, 11, "#73b3d6", 2)+path("M40 25 L33 21", stroke="#73b3d6", width=3)),
    ])

    category("07 動物キャラクター（案内・喜び）", "#d9822b", "#fff5e6", "#f4a261", [
        ("dog-guide", "案内する犬", "dog puppy guide 案内 犬 いぬ わんこ ガイド",
         lambda: backdrop()+animal_body("dog", 28, 35)+animal_head("dog", 28, 22, 10, "smile")+path("M38 40 L50 32", stroke="#f7dfc4", width=5)+circle(51, 31, 3, "#f7dfc4")+sparkle(51, 18, 3.5)),
        ("cat-smile", "にっこり猫", "cat kitten smile 猫 ねこ にゃんこ 笑顔",
         lambda: backdrop()+animal_body("cat", 32, 36)+animal_head("cat", 32, 24, 10, "laugh")+path("M42 49 Q54 45 51 35", stroke="#fff6ea", width=4)+path("M42 49 Q54 45 51 35", width=2)+heart_shape(49, 13, 0.55)),
        ("rabbit-cheer", "応援するうさぎ", "rabbit bunny cheer うさぎ ウサギ 応援 ピョン",
         lambda: backdrop()+animal_body("rabbit", 32, 38, 18, 17)+animal_head("rabbit", 32, 26, 9.5, "laugh")+circle(16, 32, 4, "#fff")+circle(48, 32, 4, "#fff")+sparkle(14, 16, 3)+sparkle(50, 16, 3)),
        ("bear-bow", "お辞儀するくま", "bear bow polite くま クマ お辞儀 挨拶",
         lambda: backdrop()+animal_body("bear", 32, 36)+animal_head("bear", 32, 25, 10, "smile")+circle(25, 48, 3.5, "#c68b59")+circle(39, 48, 3.5, "#c68b59")+path("M14 18 Q19 13 23 15 M50 18 Q45 13 41 15", stroke=ACCENT, width=2)),
        ("panda-wave", "手を振るパンダ", "panda wave hello パンダ 手を振る 歓迎",
         lambda: backdrop()+animal_body("panda", 29, 36)+animal_head("panda", 29, 23, 10, "smile")+path("M39 40 L49 28", stroke=LINE, width=5)+circle(50, 27, 3, LINE)+sparkle(51, 15, 3.5)),
        ("chick-hatched", "ひよこ・初心者", "chick bird beginner ひよこ ヒヨコ 初心者 デビュー",
         lambda: backdrop()+circle(32, 27, 11, "#ffe066")+cheeks(32, 28, 6)+dot(28, 25, 1.5)+dot(36, 25, 1.5)+path("M30 27 L34 27 L32 31 Z", "#f4a261")+path("M15 36 L22 41 L27 35 L32 41 L37 35 L42 41 L49 36 L45 53 H19 Z", "#fff")+sparkle(49, 15, 3)),
        ("owl-professor", "ふくろう博士・知恵", "owl wise professor ふくろう フクロウ 博士 解説 知恵",
         lambda: backdrop()+ellipse(32, 35, 15, 17, "#b08968")+ellipse(32, 39, 10, 11, "#f5ebe0", "none")+circle(26, 27, 5, "#fff")+circle(38, 27, 5, "#fff")+dot(26, 27, 1.8)+dot(38, 27, 1.8)+path("M30 31 L34 31 L32 35 Z", "#f4a261")+path("M18 17 L32 11 L46 17 L32 22 Z", "#495057")),
        ("squirrel-acorn", "りす・蓄える", "squirrel acorn collect りす リス どんぐり 集める",
         lambda: backdrop()+path("M41 49 C57 49 58 24 44 26 C38 27 40 39 41 49 Z", "#d9822b")+animal_body("fox", 27, 36, 18, 18)+animal_head("fox", 27, 23, 9.5, "smile")+circle(35, 43, 5, "#8d5524")+path("M30 41 Q35 36 40 41 Z", "#5c3a21")),
        ("penguin-walk", "ぺんぎん・お散歩", "penguin cute walk ペンギン ぺんぎん 歩く",
         lambda: backdrop()+ellipse(32, 34, 15, 19, "#5c768d")+ellipse(32, 35, 11, 15, "#fff", "none")+cheeks(32, 27, 6)+dot(28, 24, 1.6)+dot(36, 24, 1.6)+path("M29 26 L35 26 L32 30 Z", "#ffd54f")+ellipse(25, 53, 4, 2, "#ffd54f")+ellipse(39, 53, 4, 2, "#ffd54f")),
        ("hamster-eat", "ハムスター・もぐもぐ", "hamster cute eat ハムスター もぐもぐ 小動物",
         lambda: backdrop()+circle(23, 18, 4, "#f4a261")+circle(41, 18, 4, "#f4a261")+ellipse(32, 34, 16, 17, "#fff")+path("M17 28 C19 15 45 15 47 28 Q32 22 17 28 Z", "#f4a261")+cheeks(32, 33, 8, 3.5, 2.2)+dot(26, 28, 1.6)+dot(38, 28, 1.6)+path("M29 40 L35 40 L32 46 Z", "#5c4033")),
    ])

    category("08 動物キャラクター（リアクション・作業）", "#c97a63", "#fdf0eb", "#e07a5f", [
        ("dog-pc-work", "パソコン作業する犬", "dog pc work 犬 パソコン 仕事 テレワーク",
         lambda: backdrop()+animal_body("dog", 32, 33)+animal_head("dog", 32, 20, 9.5, "smile")+rect(21, 39, 22, 14, "#dce8f2", 2)+sparkle(50, 15, 3)),
        ("cat-question", "はてな猫・疑問", "cat question curious 猫 はてな 疑問 なぜ",
         lambda: backdrop()+animal_body("cat", 28, 36)+animal_head("cat", 28, 24, 10, "troubled")+question_mark(49, 16)),
        ("rabbit-idea", "ひらめくうさぎ", "rabbit idea lightbulb うさぎ ひらめき 電球 思いつく",
         lambda: backdrop()+animal_body("rabbit", 28, 38, 18, 17)+animal_head("rabbit", 28, 26, 9.5, "laugh")+circle(49, 16, 6, "#ffe066")+rect(47, 22, 4, 3, "#d9d9d9", 1)+path("M49 6 V8 M57 12 L55 13 M41 12 L43 13", stroke=ACCENT, width=2)),
        ("bear-troubled", "困るくま", "bear troubled sweat くま 困る 悩む 焦る",
         lambda: backdrop()+animal_body("bear", 32, 36)+animal_head("bear", 32, 24, 10, "troubled")+circle(22, 30, 3.5, "#c68b59")+sweat(50, 14)),
        ("cat-kotatsu-relax", "くつろぐ猫", "cat relax nap 猫 くつろぐ 休憩 のんびり",
         lambda: backdrop()+path("M10 38 H54 L58 53 H6 Z", "#f28b82")+rect(14, 33, 36, 6, "#deb887", 2)+animal_head("cat", 32, 24, 9.5, "smile")+rect(43, 41, 7, 8, "#fff", 1)+path("M46 37 Q48 34 46 32", stroke=ACCENT, width=1.6)),
        ("dog-delivery", "お届けする犬", "dog delivery mail 犬 お届け 配達 手紙",
         lambda: backdrop()+animal_body("dog", 26, 36)+animal_head("dog", 26, 23, 9.5, "smile")+rect(36, 32, 18, 13, "#fff", 2)+path("M36 33 L45 40 L54 33", stroke=ACCENT, width=2)+heart_shape(45, 16, 0.45)),
        ("rabbit-hurry", "急ぐうさぎ", "rabbit hurry run うさぎ 急ぐ 走る ダッシュ",
         lambda: backdrop()+animal_body("rabbit", 35, 38, 18, 17)+animal_head("rabbit", 35, 26, 9.5, "smile")+path("M8 28 H18 M6 36 H16 M9 44 H19", stroke=ACCENT, width=2.2)+sweat(51, 16)),
        ("bear-ok-sign", "OKサインのくま", "bear ok circle くま OK まる バッチリ",
         lambda: backdrop()+animal_body("bear", 25, 36, 18, 18)+animal_head("bear", 25, 23, 9.5, "laugh")+circle(48, 25, 8.5, "none", "#e63946", 4)),
        ("fox-smart", "きつね・工夫", "fox clever smart きつね キツネ 工夫 提案",
         lambda: backdrop()+animal_body("fox", 28, 36)+animal_head("fox", 28, 23, 9.5, "smile")+rect(38, 32, 14, 18, "#fff", 2)+path("M41 37 H49 M41 42 H49", stroke="#6fb3d2", width=1.8)+sparkle(49, 16, 3.5)),
        ("elephant-water", "ぞう・優しいサポート", "elephant gentle water ぞう ゾウ 優しい 応援",
         lambda: backdrop()+ellipse(15, 28, 6, 10, "#9ec3db")+ellipse(45, 28, 6, 10, "#9ec3db")+circle(30, 28, 12, "#9ec3db")+cheeks(30, 30, 6)+dot(25, 26, 1.6)+dot(35, 26, 1.6)+path("M30 30 C30 43 44 43 44 34", "#9ec3db")+sweat(50, 16)+heart_shape(48, 40, 0.5)),
    ])

    category("09 感情マーク・漫符・評価", "#e66c55", "#fff2e8", "#f4a261", [
        ("hanamaru-stamp", "花丸・たいへんよくできました", "hanamaru flower circle great 花丸 はなまる 合格 満点 よくできました",
         lambda: backdrop()+path("M32 9 Q38 4 42 11 Q50 9 50 17 Q58 20 53 27 Q59 34 52 39 Q55 47 47 49 Q45 57 38 53 Q32 60 26 53 Q19 57 17 49 Q9 47 12 39 Q5 34 11 27 Q6 20 14 17 Q14 9 22 11 Q26 4 32 9 Z", "#fff", "#e63946", 2.5)+path("M25 35 C20 26 30 19 36 25 C43 32 33 44 24 39 C17 34 23 20 35 20 C45 20 46 35 40 42", stroke="#e63946", width=2.5)),
        ("lightbulb-flash", "電球・ひらめきマーク", "lightbulb idea flash 電球 ひらめき アイデア 発想",
         lambda: backdrop()+path("M22 39 C16 32 18 16 32 16 C46 16 48 32 42 39 L39 45 H25 Z", "#ffe066")+rect(25, 45, 14, 7, "#ced4da", 2)+path("M32 6 V11 M12 16 L16 19 M52 16 L48 19 M9 30 H14 M55 30 H50", stroke=ACCENT, width=2.6)),
        ("beginner-wakaba", "初心者マーク・若葉", "beginner wakaba leaf 初心者 若葉マーク 入門 はじめて",
         lambda: backdrop()+path("M32 18 L50 9 V41 L32 53 Z", "#52b788")+path("M32 18 L14 9 V41 L32 53 Z", "#ffd54f")+sparkle(51, 47, 3)),
        ("crown-ranking", "王冠・ランキング1位", "crown number one ranking 王冠 1位 ランキング 優勝 おすすめ",
         lambda: backdrop()+path("M11 45 L15 21 L26 32 L32 17 L38 32 L49 21 L53 45 Z", "#ffd54f")+rect(11, 45, 42, 6, "#f4a261", 2)+circle(15, 19, 2.5, "#e63946")+circle(32, 15, 3, "#e63946")+circle(49, 19, 2.5, "#e63946")),
        ("trophy-award", "トロフィー・表彰", "trophy award prize トロフィー 表彰 受賞 達成",
         lambda: backdrop()+path("M19 14 H45 V29 C45 40 19 40 19 29 Z", "#ffd54f")+path("M19 18 H12 C10 27 15 31 19 30 M45 18 H52 C54 27 49 31 45 30", width=2.2)+path("M32 38 V47", width=3)+rect(21, 47, 22, 7, "#b5835a", 2)+sparkle(32, 23, 3.5, "#fff")),
        ("medal-ribbon", "メダル・金賞", "medal gold ribbon メダル 金メダル 褒章 ご褒美",
         lambda: backdrop()+path("M20 9 H29 L33 25 H25 Z", "#e63946")+path("M44 9 H35 L31 25 H39 Z", "#5b9bd5")+circle(32, 39, 15, "#ffd54f")+circle(32, 39, 11, "#ffe885")+sparkle(32, 39, 5, "#fff")),
        ("sparkles-clean", "キラキラ・輝き", "sparkles shine clean キラキラ 輝き ピカピカ 素敵",
         lambda: backdrop()+sparkle(26, 28, 13)+sparkle(46, 17, 7, "#fff")+sparkle(44, 43, 8)+dot(14, 16, 2, ACCENT)+dot(16, 46, 2, ACCENT)),
        ("sweat-panic-mark", "焦り・びっくり漫符", "sweat panic surprise 焦り 汗 びっくり 漫符 エフェクト",
         lambda: backdrop()+sweat(22, 16)+sweat(38, 12)+sweat(48, 26)+path("M14 40 L24 34 M16 49 L28 42 M36 48 L48 44", stroke=ACCENT, width=3)),
        ("heart-warm", "ハート・まごころ", "heart love warm care ハート まごころ 感謝 安心 好意",
         lambda: backdrop()+heart_shape(32, 17, 1.8)+sparkle(14, 18, 3.5)+sparkle(50, 16, 3.5)+heart_shape(48, 42, 0.5, "#f79bb0")),
        ("thumbs-up-good", "いいね・グッドサイン", "thumbs up good like いいね グッド 高評価 おすすめ",
         lambda: backdrop()+rect(12, 30, 10, 20, "#6fb3d2", 2)+path("M22 32 H28 L33 15 C36 13 40 16 38 22 L36 28 H48 C52 28 53 33 49 35 C52 37 51 41 48 42 C50 44 49 48 45 49 H22 Z", SKIN)+sparkle(48, 15, 4)),
    ])

    category("10 イベント・お祝い・季節", "#d66882", "#fdedf2", "#f28b82", [
        ("gift-box-ribbon", "プレゼントボックス", "gift present box ribbon プレゼント ギフト 贈り物 特典",
         lambda: backdrop()+rect(14, 26, 36, 27, "#f28b82", 3)+rect(11, 20, 42, 8, "#f59b95", 2)+rect(29, 20, 6, 33, "#ffd54f", 0)+path("M32 20 C22 9 15 19 29 20 M32 20 C42 9 49 19 35 20", "#ffd54f")),
        ("party-popper", "クラッカー・お祝い", "party popper celebrate クラッカー お祝い パーティー 祝賀",
         lambda: backdrop()+path("M12 52 L24 22 L42 40 Z", "#ffd54f")+path("M18 37 L29 46 M15 44 L22 49", stroke="#e63946", width=2)+path("M36 22 Q45 12 53 16 M42 30 Q52 24 56 30 M28 16 Q32 7 40 9", stroke=ACCENT, width=2.5)+sparkle(48, 12, 3.5)+dot(52, 22, 2.2, "#52b788")),
        ("birthday-cake", "お祝いケーキ", "cake birthday anniversary ケーキ 誕生日 記念日 お祝い",
         lambda: backdrop()+rect(12, 34, 40, 19, "#fff", 3)+path("M12 41 Q17 45 22 41 T32 41 T42 41 T52 41", stroke="#f79bb0", width=2.5)+rect(22, 22, 4, 12, "#90e0ef", 1)+rect(30, 22, 4, 12, "#ffd54f", 1)+rect(38, 22, 4, 12, "#f79bb0", 1)+circle(24, 18, 2.5, "#ff9f1c")+circle(32, 18, 2.5, "#ff9f1c")+circle(40, 18, 2.5, "#ff9f1c")),
        ("cherry-blossom-sakura", "桜・春のお祝い", "sakura cherry blossom spring 桜 さくら 春 入学 新生活",
         lambda: backdrop()+path("M32 32 C24 14 29 9 32 14 C35 9 40 14 32 32 C49 21 55 25 50 29 C55 32 51 37 32 32 C46 47 42 53 38 49 C36 54 31 51 32 32 C23 51 18 54 26 49 C22 53 18 47 32 32 C13 37 9 32 14 29 C9 25 15 21 32 32 Z", "#fbc4d5")+circle(32, 32, 4, "#ffd54f")),
        ("bouquet-flowers", "花束・感謝", "bouquet flowers congrats 花束 フラワー 送別 感謝 祝福",
         lambda: backdrop()+path("M16 28 L32 55 L48 28 Z", "#f9dcc4")+circle(24, 24, 6, "#f28b82")+circle(40, 24, 6, "#ffd54f")+circle(32, 19, 7, "#f79bb0")+circle(32, 19, 2.5, "#fff")+path("M26 44 Q32 41 38 44", stroke="#e63946", width=3)),
        ("new-year-daruma", "だるま・必勝祈願", "daruma goal wish だるま 達磨 目標達成 祈願 合格",
         lambda: backdrop()+ellipse(32, 35, 19, 19, "#e63946")+ellipse(32, 29, 13, 9, SKIN)+circle(26, 28, 3, "#fff")+circle(38, 28, 3, "#fff")+dot(26, 28, 1.8)+path("M21 23 Q26 20 29 23 M35 23 Q38 20 43 23 M25 44 V50 M32 43 V51 M39 44 V50", stroke="#ffd54f", width=2.2)),
        ("lucky-bag-fukubukuro", "福袋・お楽しみ", "lucky bag fukubukuro bargain 福袋 お得 キャンペーン 特集",
         lambda: backdrop()+path("M20 22 L14 11 H50 L44 22 C54 29 53 53 42 54 H22 C11 53 10 29 20 22 Z", "#e63946")+path("M19 22 H45", stroke="#ffd54f", width=3.5)+rect(24, 31, 16, 15, "#fff", 2)+path("M28 36 H36 M32 36 V43 M28 40 H36", stroke="#e63946", width=2)),
        ("confetti-banner", "くす玉・祝達成", "kusudama confetti celebration くす玉 祝 達成 オープン 記念",
         lambda: backdrop()+path("M7 26 A14 14 0 0 1 30 15 L23 36 Z", "#ffd54f")+path("M57 26 A14 14 0 0 0 34 15 L41 36 Z", "#ffd54f")+rect(26, 30, 12, 24, "#fff", 2)+path("M29 36 H35 M32 36 V48", stroke="#e63946", width=2)+dot(16, 44, 2, "#f28b82")+dot(48, 43, 2, "#6fb3d2")),
        ("umbrella-rain", "傘・雨の日", "umbrella rain weather 傘 雨 梅雨 天気 備え",
         lambda: backdrop()+path("M10 34 C10 14 54 14 54 34 Q46 31 39 34 Q32 31 25 34 Q18 31 10 34 Z", "#73b3d6")+path("M32 16 V49 C32 54 25 54 25 49", width=2.5)+sweat(16, 12)+sweat(48, 12)),
        ("snowman-winter", "雪だるま・冬", "snowman winter snow 雪だるま 冬 季節 イベント",
         lambda: backdrop()+circle(32, 42, 13, "#fff")+circle(32, 24, 9.5, "#fff")+rect(24, 10, 16, 7, "#e63946", 2)+path("M23 31 H41 L39 40 H34 V33", "#e63946")+dot(29, 23, 1.5)+dot(35, 23, 1.5)+cheeks(32, 25, 5.5, 2, 1.2)),
    ])

    category("11 食事・カフェ・休憩小物", "#d9822b", "#fff5e6", "#f4a261", [
        ("onigiri-riceball", "おにぎり・軽食", "onigiri rice ball lunch おにぎり おむすび 昼食 軽食",
         lambda: backdrop()+path("M32 12 C42 12 53 39 48 48 C43 54 21 54 16 48 C11 39 22 12 32 12 Z", "#fff")+rect(24, 36, 16, 15, "#3d405b", 2)+cheeks(32, 30, 7)+dot(28, 27, 1.5)+dot(36, 27, 1.5)),
        ("bento-lunchbox", "お弁当・ランチ", "bento lunch box meal お弁当 弁当 ランチ 食事",
         lambda: backdrop()+rect(9, 16, 46, 34, "#e63946", 4)+rect(13, 20, 21, 26, "#fff", 2)+circle(23.5, 33, 3.5, "#e63946")+circle(43, 26, 5, "#ffd54f")+circle(43, 39, 5, "#81c995")),
        ("coffee-cup-warm", "温かいコーヒー・カフェ", "coffee cup cafe warm コーヒー カフェ マグカップ 一息",
         lambda: backdrop()+path("M44 29 H51 C56 29 56 41 51 41 H44", width=2.5)+path("M16 25 H44 V41 C44 50 16 50 16 41 Z", "#fff")+ellipse(30, 50, 19, 4, "#fff")+ellipse(30, 25, 14, 3.5, "#8d5524")+path("M25 17 Q28 12 25 8 M35 17 Q38 12 35 8", stroke=ACCENT, width=2)),
        ("green-tea-yunomi", "お茶・湯呑み", "green tea japanese tea yunomi お茶 緑茶 湯呑み おもてなし",
         lambda: backdrop()+path("M18 22 H46 L43 49 C42 54 22 54 21 49 Z", "#a8dadc")+ellipse(32, 22, 14, 4, "#81c995")+path("M18 35 H45 M27 14 Q30 9 27 5 M37 14 Q40 9 37 5", stroke=ACCENT, width=2)),
        ("shortcake-sweet", "ショートケーキ・おやつ", "shortcake dessert sweet ショートケーキ おやつ スイーツ ご褒美",
         lambda: backdrop()+path("M12 29 L38 15 L52 29 V49 H12 Z", "#fff")+path("M12 29 L52 29 M12 39 H52", stroke="#f79bb0", width=3)+path("M32 10 C27 10 26 20 32 21 C38 20 37 10 32 10 Z", "#e63946")),
        ("bread-bakery", "パン・朝食", "bread toast bakery パン 食パン 朝食 ベーカリー",
         lambda: backdrop()+path("M17 26 C11 26 12 13 23 14 C28 11 36 11 41 14 C52 13 53 26 47 26 V50 H17 Z", "#f4a261")+path("M21 27 C17 27 18 18 25 18 H39 C46 18 47 27 43 27 V46 H21 Z", "#fff8eb", "none")+rect(28, 29, 8, 8, "#ffd54f", 1)),
        ("apple-fruit", "りんご・果物", "apple fruit fresh りんご リンゴ 果物 フルーツ 健康",
         lambda: backdrop()+path("M32 21 C20 15 11 27 15 41 C18 52 27 54 32 50 C37 54 46 52 49 41 C53 27 44 15 32 21 Z", "#e63946")+path("M32 21 V12", width=2.5)+path("M33 14 C40 10 45 14 39 18 Z", "#81c995")+ellipse(23, 30, 2.5, 5, "#ff9eb5", "none")),
        ("ramen-noodles", "ラーメン・温かい食事", "ramen noodles soup ラーメン 麺 温かい 食事",
         lambda: backdrop()+path("M10 31 H54 C52 48 42 53 32 53 C22 53 12 48 10 31 Z", "#e63946")+ellipse(32, 31, 22, 6, "#ffd54f")+circle(24, 30, 4, "#fff")+circle(24, 30, 2, "#f4a261")+rect(35, 26, 9, 6, "#81c995", 1)+path("M22 18 Q25 13 22 9 M32 18 Q35 13 32 9 M42 18 Q45 13 42 9", stroke=ACCENT, width=2)),
        ("cheers-glasses", "乾杯・懇親会", "cheers toast party drink 乾杯 懇親会 打ち上げ 飲み物",
         lambda: backdrop()+rect(13, 24, 15, 24, "#ffd54f", 2)+path("M12 22 C12 16 29 16 29 22 Z", "#fff")+rect(36, 24, 15, 24, "#ffd54f", 2)+path("M35 22 C35 16 52 16 52 22 Z", "#fff")+sparkle(32, 12, 4)),
        ("water-bottle-hydration", "水筒・水分補給", "water bottle hydration 水筒 マイボトル 水分補給 熱中症対策",
         lambda: backdrop()+rect(21, 18, 22, 36, "#73b3d6", 5)+rect(25, 10, 14, 8, "#f28b82", 2)+sweat(49, 22)+sparkle(14, 24, 3.5)),
    ])

    category("12 乗り物・街・暮らしの建物", "#4d908e", "#e8f6f5", "#64b5c4", [
        ("family-car", "乗用車・マイカー", "car automobile drive 車 乗用車 マイカー ドライブ 移動",
         lambda: backdrop()+path("M10 36 L17 22 H45 L53 36 H56 V47 H8 V36 Z", "#f28b82")+path("M20 25 H31 V35 H15 Z M34 25 H43 L48 35 H34 Z", "#e0f4ff")+circle(19, 47, 6, "#495057")+circle(19, 47, 2.5, "#fff")+circle(45, 47, 6, "#495057")+circle(45, 47, 2.5, "#fff")),
        ("delivery-truck", "トラック・配送車", "truck delivery transport トラック 配送 運送 引っ越し",
         lambda: backdrop()+rect(8, 20, 31, 25, "#81c995", 2)+path("M39 27 H49 L55 36 V45 H39 Z", "#ffd54f")+rect(42, 30, 8, 6, "#e0f4ff", 1)+circle(19, 46, 5.5, "#495057")+circle(46, 46, 5.5, "#495057")),
        ("commuter-train", "電車・鉄道", "train railway commute 電車 鉄道 通勤 駅",
         lambda: backdrop()+rect(14, 11, 36, 38, "#fff", 6)+rect(14, 29, 36, 7, "#81c995", 0)+rect(19, 16, 11, 10, "#90e0ef", 2)+rect(34, 16, 11, 10, "#90e0ef", 2)+circle(22, 41, 2.5, "#ffd54f")+circle(42, 41, 2.5, "#ffd54f")+path("M10 55 H54 M20 49 L16 55 M44 49 L48 55", width=2.2)),
        ("city-bus", "バス・路線バス", "bus transit tour バス 路線バス 送迎 交通",
         lambda: backdrop()+rect(8, 19, 48, 27, "#73b3d6", 5)+rect(13, 24, 10, 9, "#fff", 1)+rect(27, 24, 10, 9, "#fff", 1)+rect(41, 24, 10, 9, "#fff", 1)+circle(19, 47, 5.5, "#495057")+circle(45, 47, 5.5, "#495057")),
        ("bicycle-ride", "自転車・サイクリング", "bicycle bike eco 自転車 サイクリング エコ 移動",
         lambda: backdrop()+circle(18, 43, 9, "none", LINE, 2.5)+circle(46, 43, 9, "none", LINE, 2.5)+path("M18 43 L28 28 H42 L31 43 Z M42 28 L46 43 M25 24 H31 M40 22 H46", stroke="#e63946", width=2.5)+rect(45, 23, 8, 7, "#deb887", 1)),
        ("airplane-travel", "飛行機・旅行・出張", "airplane flight travel 飛行機 旅行 出張 空の旅",
         lambda: backdrop()+path("M10 38 L26 31 L19 15 L26 13 L38 27 L51 22 C56 20 58 26 53 29 L14 44 L8 35 Z", "#fff")+path("M26 38 L22 49 L28 48 L36 35", "#73b3d6")),
        ("cozy-house", "マイホーム・一戸建て", "house home residence 家 マイホーム 一戸建て 住まい",
         lambda: backdrop()+rect(14, 28, 36, 26, "#fff8eb", 2)+path("M8 30 L32 11 L56 30 Z", "#e66c55")+rect(28, 39, 9, 15, "#b5835a", 1)+rect(19, 34, 7, 7, "#90e0ef", 1)+rect(39, 34, 7, 7, "#90e0ef", 1)),
        ("apartment-mansion", "マンション・集合住宅", "apartment condo building マンション アパート 集合住宅 賃貸",
         lambda: backdrop()+rect(14, 11, 36, 44, "#fff", 2)+rect(12, 9, 40, 5, "#73b3d6", 1)+"".join(rect(x, y, 7, 6, "#90e0ef", 1) for y in (18, 28, 38) for x in (20, 37))+rect(28, 46, 8, 9, "#b5835a", 1)),
        ("town-park", "公園・憩いの場", "park trees bench 公園 広場 緑 地域 憩い",
         lambda: backdrop()+rect(20, 32, 6, 20, "#8d5524", 1)+circle(23, 22, 11, "#81c995")+circle(16, 26, 7, "#52b788")+circle(30, 26, 7, "#52b788")+rect(34, 42, 18, 4, "#deb887", 1)+path("M37 46 V52 M49 46 V52 M35 37 H51 V42", width=2)),
        ("mailbox-post", "郵便ポスト・投函", "mailbox post letter ポスト 郵便 投函 手紙 案内",
         lambda: backdrop()+rect(28, 42, 8, 13, "#495057", 1)+rect(18, 16, 28, 27, "#e63946", 4)+rect(15, 14, 34, 5, "#e63946", 2)+rect(24, 23, 16, 4, "#fff", 1)+path("M29 33 H35 M32 33 V40 M28 36 H36", stroke="#fff", width=2)),
    ])

    category("13 ビジネス人物（基本ポーズ・案内）", "#3b6e9c", "#ebf3fa", "#4a6fa5", [
        ("biz-smile", "ビジネスマン・笑顔", "businessman suit smile 社員 ビジネスマン スーツ 笑顔 人物",
         lambda: backdrop()+suit_body()+person_head(expr="smile")+sparkle(13, 15, 3.5)+sparkle(51, 16, 3)),
        ("biz-woman-smile", "ビジネスウーマン・笑顔", "businesswoman suit smile 女性社員 ビジネスウーマン スーツ 笑顔 人物",
         lambda: backdrop()+suit_body(jacket="#52799c", tie="#f28b82", female=True)+person_head(expr="smile", hair="long")+sparkle(14, 15, 3)+sparkle(50, 15, 3.5)),
        ("biz-bow", "お辞儀する社員・ビジネスマナー", "business bow polite お辞儀 挨拶 ビジネスマナー 謝罪 お礼 社員",
         lambda: backdrop()+suit_body(32, 36, 24, 20)+person_head(32, 25, 10.5, "peace")+hand(26, 51)+hand(38, 51)+path("M14 17 Q19 12 24 14 M50 17 Q45 12 40 14", stroke=ACCENT, width=2)),
        ("biz-guide-right", "案内する社員・こちら", "business guide point 案内 紹介 こちら 社員 ガイド 説明",
         lambda: backdrop()+suit_body(27, 35, 22, 20)+person_head(27, 22, 10, "smile")+path("M38 40 L51 32", stroke="#4a6fa5", width=5)+hand(52, 31)+sparkle(52, 18, 3.5)),
        ("biz-ok-sign", "OKサインを出す社員", "business ok approve OK まる 承認 了解 社員 スーツ",
         lambda: backdrop()+suit_body(25, 35, 22, 20, "#52799c", "#f28b82", True)+person_head(25, 22, 10, "laugh", "long")+circle(48, 24, 9, "none", "#e63946", 4)+hand(38, 34)),
        ("biz-ng-sign", "NGサインを出す社員", "business ng reject cross NG ばつ 却下 禁止 注意 社員",
         lambda: backdrop()+suit_body(25, 35, 22, 20, "#6c7a89")+person_head(25, 22, 10, "troubled")+path("M41 16 L55 30 M55 16 L41 30", stroke="#e63946", width=4)+sweat(13, 15)),
        ("biz-guts-pose", "ガッツポーズの社員・達成", "business cheer success ガッツポーズ 達成 成約 やったー 社員",
         lambda: backdrop()+suit_body(32, 35, 22, 20)+person_head(32, 22, 10, "laugh")+path("M20 40 L14 27 M44 40 L50 27", stroke="#4a6fa5", width=5)+hand(14, 25)+hand(50, 25)+sparkle(13, 13, 3.5)),
        ("biz-troubled", "困る・悩む社員", "business troubled worried 困る 悩む 課題 問題 社員 スーツ",
         lambda: backdrop()+suit_body(32, 35, 22, 20, "#6c7a89")+person_head(32, 22, 10, "troubled")+hand(22, 28)+sweat(50, 14)+question_mark(14, 17)),
        ("biz-idea", "ひらめく社員・企画", "business idea eureka ひらめき 企画 提案 思いつく 社員",
         lambda: backdrop()+suit_body(28, 35, 22, 20)+person_head(28, 22, 10, "smile")+hand(43, 27)+circle(49, 14, 5.5, "#ffe066")+rect(47, 19, 4, 3, "#d9d9d9", 1)+path("M49 5 V7 M57 10 L55 11 M41 10 L43 11", stroke="#e67e22", width=2)),
        ("biz-apology", "謝罪する社員・お詫び", "business apology sorry 謝罪 お詫び すみません 反省 社員",
         lambda: backdrop()+suit_body(32, 35, 22, 20, "#5c6b73")+person_head(32, 22, 10, "cry")+hand(27, 46)+hand(37, 46)+sweat(50, 14)+sweat(14, 15)),
    ])

    category("14 営業・商談・顧客折衝", "#2d7d8a", "#e6f5f7", "#3f7ea6", [
        ("biz-sales-visit", "外回り・訪問営業", "sales visit bag 営業 外回り 訪問 商談 カバン",
         lambda: backdrop()+suit_body(26, 34, 20, 21)+person_head(26, 21, 9.5, "smile")+rect(38, 37, 16, 13, "#8d5524", 2)+path("M42 37 V34 H50 V37", width=2)+sparkle(49, 16, 3.5)),
        ("biz-proposal", "企画提案・プレゼンする社員", "proposal pitch presentation 提案 企画 プレゼン 営業 説明",
         lambda: backdrop()+rect(32, 9, 25, 21, "#fff", 2)+path("M36 24 L42 17 L47 20 L53 13", stroke="#e65c4f", width=2.4)+suit_body(20, 35, 19, 20)+person_head(20, 23, 9, "smile")+path("M29 39 L40 25", width=2.2)),
        ("biz-negotiation", "商談・打ち合わせ", "negotiation meeting deal 商談 打ち合わせ 面談 交渉",
         lambda: backdrop()+inset(suit_body()+person_head(expr="smile"), -12, 8, 0.76)+inset(suit_body(jacket="#52799c", tie="#f28b82", female=True)+person_head(expr="smile", hair="long"), 20, 8, 0.76)+rect(10, 49, 44, 5, "#d7b38c", 2)+sparkle(32, 13, 3.5)),
        ("biz-contract-sign", "契約締結・サインする人", "contract sign agreement 契約 サイン 合意 捺印 成約",
         lambda: backdrop()+suit_body(24, 35, 20, 20)+person_head(24, 22, 9.5, "smile")+rect(37, 25, 17, 23, "#fff", 2)+path("M40 31 H51 M40 36 H51 M40 41 H46", stroke="#6fb3d2", width=1.8)+circle(49, 42, 3, "#e63946")),
        ("biz-handshake-deal", "成約・ビジネスの握手", "handshake deal partner 握手 成約 合意 パートナー 取引",
         lambda: backdrop()+inset(suit_body()+person_head(expr="laugh"), -11, 7, 0.78)+inset(suit_body(jacket="#3d5a80", tie="#f4a261")+person_head(expr="laugh"), 19, 7, 0.78)+path("M23 44 H41", stroke=SKIN, width=5)+circle(32, 44, 4, SKIN)+sparkle(32, 10, 4)),
        ("biz-estimate-explain", "見積もり・料金説明する社員", "estimate quote price 見積 見積書 料金 説明 提案",
         lambda: backdrop()+suit_body(23, 35, 20, 20, "#52799c", "#f28b82", True)+person_head(23, 22, 9.5, "smile", "long")+rect(36, 20, 19, 27, "#fff", 2)+circle(45.5, 30, 5, "#ffd54f")+path("M40 40 H51 M40 44 H48", width=1.6)),
        ("biz-reception-greet", "受付・来客案内", "receptionist welcome visitor 受付 来客 案内 おもてなし",
         lambda: backdrop()+suit_body(32, 32, 20, 16, "#5c9ead", "#f28b82", True)+person_head(32, 20, 9.5, "smile", "bun")+rect(10, 45, 44, 10, "#deb887", 2)+rect(15, 37, 9, 8, "#fff", 1)+sparkle(49, 16, 3.5)),
        ("biz-see-off", "見送り・感謝のお辞儀", "see off thank you 見送り お礼 感謝 顧客対応",
         lambda: backdrop()+suit_body(28, 36, 22, 19, "#52799c", "#f28b82", True)+person_head(28, 24, 10, "peace", "long")+heart_shape(48, 15, 0.55)+sparkle(13, 18, 3)),
        ("biz-phone-sales", "電話営業・架電する社員", "phone sales call 電話営業 テレアポ 架電 連絡",
         lambda: backdrop()+suit_body(28, 35, 22, 20)+person_head(28, 22, 10, "laugh")+rect(40, 18, 7, 13, "#495057", 2)+hand(41, 27)+path("M49 14 Q55 17 54 24 M52 11 Q59 15 58 24", stroke=ACCENT, width=2)),
        ("biz-closing-joy", "受注・目標達成で喜ぶ営業", "order won target achieved 受注 目標達成 成約 喜び 営業",
         lambda: backdrop()+suit_body(26, 35, 20, 20)+person_head(26, 22, 9.5, "laugh")+path("M16 39 L11 24", stroke="#4a6fa5", width=5)+hand(11, 22)+circle(47, 36, 10, "#fff")+circle(47, 36, 6, "#e63946")+circle(47, 36, 2.5, "#ffd54f")+sparkle(47, 14, 4)),
    ])

    category("15 会議・チームワーク・職場コミュニケーション", "#4b7f52", "#eaf6ec", "#529471", [
        ("biz-horensou", "報連相（報告・連絡・相談）", "report contact consult 報連相 報告 連絡 相談 上司 部下",
         lambda: backdrop()+inset(suit_body()+person_head(expr="smile"), -11, 8, 0.78)+inset(suit_body(jacket="#3d5a80")+person_head(expr="smile")+glasses(), 19, 8, 0.78)+rect(22, 34, 11, 14, "#fff", 1)+rect(25, 6, 14, 10, "#fff", 3)+dot(29, 11, 1.3)+dot(35, 11, 1.3)),
        ("biz-one-on-one", "1on1・面談する上司と部下", "one on one interview mentoring 1on1 面談 評価 相談 上司",
         lambda: backdrop()+inset(suit_body(jacket="#52799c", tie="#f28b82", female=True)+person_head(expr="smile", hair="long"), -11, 8, 0.78)+inset(suit_body()+person_head(expr="peace"), 19, 8, 0.78)+rect(9, 49, 46, 5, "#d7b38c", 2)+heart_shape(32, 11, 0.5)),
        ("biz-whiteboard-meeting", "ホワイトボード会議・議論", "whiteboard meeting discussion 会議 ホワイトボード 議論 ファシリテーター",
         lambda: backdrop()+rect(14, 8, 36, 24, "#fff", 2)+path("M20 15 H36 M20 21 H31", stroke="#4a6fa5", width=2)+rect(39, 14, 6, 6, "#ffd54f", 1)+inset(suit_body()+person_head(expr="smile"), -12, 21, 0.65)+inset(suit_body(jacket="#52799c", tie="#f28b82", female=True)+person_head(expr="laugh", hair="long"), 20, 21, 0.65)),
        ("biz-team-scrum", "チーム団結・円陣", "teamwork scrum unity チーム 団結 協力 プロジェクト 仲間",
         lambda: backdrop()+inset(suit_body()+person_head(expr="laugh"), -12, 12, 0.65)+inset(suit_body(jacket="#52799c", tie="#f28b82", female=True)+person_head(expr="laugh", hair="long"), 22, 12, 0.65)+inset(suit_body(jacket="#3d5a80", tie="#f4a261")+person_head(expr="smile"), 5, 3, 0.68)+sparkle(32, 5, 3.5)),
        ("biz-onboarding-mentor", "新人指導・OJTトレーナー", "ojt mentor training 新人教育 OJT 指導 先輩 新入社員",
         lambda: backdrop()+inset(suit_body(jacket="#52799c", tie="#f28b82", female=True)+person_head(expr="smile", hair="long"), -11, 8, 0.78)+inset(suit_body()+person_head(expr="smile"), 19, 8, 0.78)+path("M48 8 L55 4 V17 L48 22 Z", "#52b788")+path("M48 8 L41 4 V17 L48 22 Z", "#ffd54f")),
        ("biz-praise-boss", "部下を褒める・評価する上司", "praise feedback boss 褒める 評価 フィードバック 上司 承認",
         lambda: backdrop()+inset(suit_body(jacket="#3d5a80")+person_head(expr="smile")+glasses(), -11, 8, 0.78)+inset(suit_body()+person_head(expr="laugh"), 19, 8, 0.78)+sparkle(32, 11, 4)+sparkle(50, 12, 3)),
        ("biz-information-sharing", "情報共有・資料を配る人", "share information handout 情報共有 資料 配布 共有 伝達",
         lambda: backdrop()+suit_body(25, 35, 20, 20)+person_head(25, 22, 9.5, "smile")+rect(36, 26, 14, 18, "#fff", 2)+rect(41, 31, 14, 18, "#fff", 2)+path("M44 37 H51 M44 42 H51", stroke="#4a6fa5", width=1.8)+sparkle(48, 16, 3)),
        ("biz-opinion-raise-hand", "発言・挙手する社員", "raise hand opinion speak 挙手 発言 意見 質問 会議",
         lambda: backdrop()+suit_body(29, 36, 22, 19)+person_head(29, 23, 10, "laugh")+path("M40 39 L48 16", stroke="#4a6fa5", width=5)+hand(48, 13)+sparkle(15, 16, 3)),
        ("biz-remote-conference", "Web会議に参加する社員", "web conference online meeting Web会議 オンライン会議 リモート 社員",
         lambda: backdrop()+rect(9, 10, 46, 34, "#fff", 4)+path("M25 44 V53 M18 53 H46", width=2.2)+suit_body(32, 31, 18, 13)+person_head(32, 21, 7.5, "smile")+headset_gear(32, 21, 7.5)),
        ("biz-applause-celebrate", "拍手で称える・表彰", "applause clap celebrate 拍手 称賛 表彰 お祝い 職場",
         lambda: backdrop()+suit_body(32, 35, 22, 20, "#52799c", "#f28b82", True)+person_head(32, 22, 10, "laugh", "long")+hand(25, 38)+hand(39, 38)+path("M16 33 L21 36 M15 40 L20 40 M48 33 L43 36 M49 40 L44 40", stroke=ACCENT, width=2)+sparkle(32, 7, 3.5)),
    ])

    category("16 役職・経営・マネジメントの人物", "#7b5ea7", "#f2ecf9", "#5e548e", [
        ("biz-president-ceo", "社長・経営者", "president ceo executive 社長 経営者 代表 トップ リーダー",
         lambda: backdrop()+suit_body(32, 35, 24, 20, "#2b3a55", "#d9a05b")+person_head(32, 22, 10.5, "smile")+path("M23 9 L26 4 L32 8 L38 4 L41 9 Z", "#ffd54f")+sparkle(51, 16, 3.5)),
        ("biz-executive-director", "役員・重役", "director board executive 役員 取締役 重役 幹部",
         lambda: backdrop()+suit_body(32, 35, 24, 20, "#3d405b", "#e07a5f")+person_head(32, 22, 10.5, "smile")+glasses(32, 22, 10.5)+rect(42, 41, 11, 12, "#fff", 1)),
        ("biz-department-manager", "部長・管理職", "manager director leader 部長 課長 管理職 マネージャー",
         lambda: backdrop()+suit_body(28, 35, 22, 20, "#3d5a80", "#4ea8de")+person_head(28, 22, 10, "determined")+glasses(28, 22, 10)+rect(39, 33, 14, 18, "#fff", 2)+path("M42 39 H50 M42 44 H48", width=1.8)),
        ("biz-project-leader", "プロジェクトリーダー・旗振り役", "project leader pm flag プロジェクトリーダー PM 推進 旗振り役",
         lambda: backdrop()+suit_body(25, 35, 20, 20)+person_head(25, 22, 9.5, "laugh")+path("M41 53 V11", width=2.5)+path("M41 11 H57 L52 19 L57 27 H41 Z", "#e65c4f")+hand(41, 36)),
        ("biz-female-manager", "女性管理職・キャリアウーマン", "female manager leader 女性管理職 キャリア リーダー 役職",
         lambda: backdrop()+suit_body(28, 35, 22, 20, "#5e548e", "#f4a261", True)+person_head(28, 22, 10, "smile", "bun")+rect(39, 32, 15, 19, "#fff", 2)+path("M42 44 L46 38 L51 41", stroke="#e65c4f", width=2)+sparkle(49, 16, 3.5)),
        ("biz-new-employee", "新入社員・フレッシュマン", "new employee freshman rookie 新入社員 新人 フレッシャーズ 若手",
         lambda: backdrop()+suit_body(27, 35, 21, 20, "#4a6fa5", "#52b788")+person_head(27, 22, 10, "laugh")+path("M48 16 L56 11 V26 L48 32 Z", "#52b788")+path("M48 16 L40 11 V26 L48 32 Z", "#ffd54f")+sparkle(14, 15, 3.5)),
        ("biz-veteran-advisor", "ベテラン社員・顧問", "veteran senior advisor ベテラン 熟練 顧問 シニア社員",
         lambda: backdrop()+suit_body(32, 35, 23, 20, "#495057", "#8d99ae")+person_head(32, 22, 10.5, "peace")+path("M21 20 C21 9 43 9 43 20 Q36 15 32 18 Q27 15 21 20 Z", "#ced4da")+glasses(32, 22, 10.5)),
        ("biz-secretary-schedule", "スケジュール管理する秘書", "secretary assistant schedule 秘書 スケジュール 補佐 アシスタント",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#6d597a", "#f79bb0", True)+person_head(25, 22, 9.5, "smile", "long")+rect(37, 22, 18, 21, "#fff", 2)+path("M37 28 H55 M42 20 V24 M50 20 V24", width=1.8)+dot(42, 33, 1.5, "#e63946")+dot(48, 37, 1.5, "#4a6fa5")),
        ("biz-auditor-check", "チェック・監査する担当者", "auditor inspector check 監査 チェック 確認 審査 評価",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#3d5a80")+person_head(25, 22, 9.5, "determined")+rect(37, 26, 17, 22, "#fff", 2)+path("M41 37 L45 41 L51 33", stroke="#2a9d8f", width=2.6)),
        ("biz-vision-speech", "方針発表・スピーチする経営者", "speech vision policy 方針発表 スピーチ 訓示 演台 経営者",
         lambda: backdrop()+suit_body(32, 28, 20, 16, "#2b3a55", "#e65c4f")+person_head(32, 17, 9, "laugh")+path("M16 41 H48 L44 55 H20 Z", "#b5835a")+path("M25 36 L22 41 M39 36 L42 41", width=2)+sparkle(50, 14, 3.5)),
    ])

    category("17 IT・エンジニア・クリエイティブ職の人物", "#2b8a78", "#e6f6f2", "#3a9689", [
        ("biz-programmer-coding", "プログラマー・開発エンジニア", "programmer developer coding プログラマー エンジニア 開発 コーディング",
         lambda: backdrop()+person_body(32, 32, 22, 19, "#3a9689")+person_head(32, 19, 9.5, "smile")+glasses(32, 19, 9.5)+rect(21, 38, 22, 15, "#dce8f2", 2)+path("M28 43 L25 45.5 L28 48 M36 43 L39 45.5 L36 48", stroke="#2b8a78", width=1.8)),
        ("biz-system-admin", "システム管理者・運用保守", "sysadmin operations server システム管理者 運用 保守 インフラ",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#457b9d")+person_head(24, 22, 9.5, "smile")+rect(38, 16, 17, 11, "#e6eef5", 2)+rect(38, 30, 17, 11, "#e6eef5", 2)+dot(42, 21.5, 1.5, "#2a9d8f")+dot(42, 35.5, 1.5, "#2a9d8f")+hand(36, 38)),
        ("biz-security-guard-it", "情報セキュリティ担当者", "security officer shield 情報セキュリティ 監視 防御 担当者",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#2b3a55", "#2a9d8f")+person_head(25, 22, 9.5, "determined")+path("M46 21 L57 26 V36 C57 44 52 49 46 53 C40 49 35 44 35 36 V26 Z", "#52b788")+path("M42 36 L45 40 L51 32", stroke="#fff", width=2.5)),
        ("biz-data-analyst", "データアナリスト・分析担当", "data analyst chart bi データアナリスト 分析 マーケティング 担当者",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#52799c", "#f4a261", True)+person_head(24, 22, 9.5, "smile", "long")+glasses(24, 22, 9.5)+rect(36, 22, 19, 22, "#fff", 2)+rect(40, 34, 3, 7, "#6fb3d2", 0.5)+rect(45, 30, 3, 11, "#f4a261", 0.5)+rect(50, 26, 3, 15, "#e65c4f", 0.5)),
        ("biz-ui-designer", "Webデザイナー・クリエイター", "designer creator ui Webデザイナー デザイナー クリエイター 制作",
         lambda: backdrop()+person_body(25, 35, 20, 20, "#e07a5f")+person_head(25, 22, 9.5, "smile", "bun")+ellipse(46, 38, 10, 8, "#fff")+circle(43, 35, 2, "#e63946", "none")+circle(49, 35, 2, "#ffd54f", "none")+circle(47, 41, 2, "#4ea8de", "none")+sparkle(49, 17, 3.5)),
        ("biz-helpdesk-operator", "ヘルプデスク・サポート窓口", "helpdesk support operator ヘルプデスク サポート オペレーター 問い合わせ",
         lambda: backdrop()+suit_body(30, 35, 22, 20, "#4ea8de", "#f28b82", True)+person_head(30, 22, 10, "smile", "long")+headset_gear(30, 22, 10)+heart_shape(50, 15, 0.5)),
        ("biz-qa-tester", "テスト・品質確認するエンジニア", "qa tester quality check テスト 品質確認 検証 バグチェック",
         lambda: backdrop()+person_body(25, 35, 20, 20, "#529471")+person_head(25, 22, 9.5, "smile")+circle(46, 31, 8, "#fff")+path("M52 37 L57 42", width=3)+path("M43 31 L45.5 34 L50 28", stroke="#2a9d8f", width=2.2)),
        ("biz-ai-prompt-user", "AIを活用するビジネスパーソン", "ai user prompt assistant AI活用 生成AI 業務効率化 社員",
         lambda: backdrop()+suit_body(25, 35, 20, 20)+person_head(25, 22, 9.5, "laugh")+rect(38, 22, 16, 14, "#fff", 4)+dot(43, 28, 1.5)+dot(49, 28, 1.5)+path("M44 32 H48 M46 17 V22", width=1.8)+sparkle(50, 12, 3.5)),
        ("biz-network-trouble-fix", "障害対応・復旧に走るエンジニア", "incident response fix trouble 障害対応 復旧 トラブル対応 エンジニア",
         lambda: backdrop()+person_body(27, 35, 20, 20, "#457b9d")+person_head(27, 22, 9.5, "determined")+helmet_hat(27, 13, "#ffd54f")+path("M44 26 L53 35 M43 36 L54 25", stroke="#7f8c8d", width=3.5)+sweat(13, 18)),
        ("biz-release-deploy", "リリース・公開を完了した担当者", "release deploy launch リリース 公開 完了 ローンチ 開発",
         lambda: backdrop()+person_body(25, 35, 20, 20, "#3a9689")+person_head(25, 22, 9.5, "laugh")+path("M46 39 V22 Q46 13 50 9 Q54 13 54 22 V39 Z", "#fff")+path("M46 31 L42 38 H46 M54 31 L58 38 H54", "#e65c4f")+circle(50, 24, 2.5, "#6fb3d2")+sparkle(14, 15, 3.5)),
    ])

    category("18 バックオフィス・管理部門の人物", "#8a6d3b", "#fbf4e8", "#6c809a", [
        ("biz-accounting-calc", "経理・電卓を叩く担当者", "accounting finance calculator 経理 会計 電卓 決算 精算",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#52799c", "#f4a261", True)+person_head(24, 22, 9.5, "smile", "long")+rect(37, 24, 17, 23, "#fff", 2)+rect(40, 27, 11, 5, "#dce8f2", 1)+dot(42, 36, 1.3)+dot(46, 36, 1.3)+dot(50, 36, 1.3)+dot(42, 41, 1.3)+dot(46, 41, 1.3)+dot(50, 41, 1.3)),
        ("biz-hr-interviewer", "人事・採用面接官", "hr interviewer recruitment 人事 採用 面接 選考 担当者",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#4a6fa5")+person_head(25, 22, 9.5, "smile")+rect(37, 24, 17, 23, "#fff", 2)+circle(45.5, 31, 3, SKIN)+path("M41 40 C41 35 50 35 50 40", "#6fb3d2")+heart_shape(48, 13, 0.5)),
        ("biz-legal-contract", "法務・契約書を確認する担当者", "legal compliance contract 法務 コンプライアンス 契約確認 規約",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#3d5a80")+person_head(24, 22, 9.5, "determined")+glasses(24, 22, 9.5)+path("M47 16 V42 M39 23 H55 M40 23 L37 31 H43 Z M54 23 L51 31 H57 Z", "#ffd54f")),
        ("biz-general-affairs", "総務・備品管理の担当者", "general affairs admin supply 総務 備品 管理 庶務 サポート",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#529471", "#f28b82", True)+person_head(25, 22, 9.5, "smile", "bun")+rect(35, 33, 20, 16, "#deb887", 2)+path("M42 33 V39 H48 V33", "#fff")+sparkle(49, 17, 3)),
        ("biz-pr-announcer", "広報・プレスリリース担当者", "public relations pr media 広報 宣伝 プレスリリース 発信",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#e07a5f", "#ffd54f", True)+person_head(24, 22, 9.5, "laugh", "long")+path("M35 27 L52 19 V35 L35 29 Z", "#ffd54f")+sparkle(54, 14, 3.5)),
        ("biz-approval-hanko", "稟議・承認印を押す上司", "approval stamp hanko workflow 承認 決裁 稟議 ハンコ 押印",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#3d5a80")+person_head(24, 22, 9.5, "smile")+rect(36, 26, 18, 22, "#fff", 2)+circle(47, 40, 4.5, "none", "#e63946", 2)+dot(47, 40, 1.5, "#e63946")),
        ("biz-document-filing", "書類整理・ファイリングする社員", "filing document binder 書類整理 ファイリング 保管 事務",
         lambda: backdrop()+suit_body(24, 35, 20, 20)+person_head(24, 22, 9.5, "smile")+rect(36, 22, 7, 25, "#6fb3d2", 1)+rect(43, 22, 7, 25, "#f4a261", 1)+rect(50, 22, 7, 25, "#81c995", 1)+circle(39.5, 41, 1.5, "#fff")+circle(46.5, 41, 1.5, "#fff")+circle(53.5, 41, 1.5, "#fff")),
        ("biz-expense-receipt", "経費精算・領収書を提出する社員", "expense receipt claim 経費精算 領収書 精算 申請",
         lambda: backdrop()+suit_body(24, 35, 20, 20)+person_head(24, 22, 9.5, "smile")+path("M37 20 H54 V46 L51 43 L48 46 L45 43 L42 46 L39 43 L37 46 Z", "#fff")+circle(45.5, 31, 4.5, "#ffd54f")),
        ("biz-compliance-ethics", "コンプライアンス・法令遵守", "compliance ethics rule コンプライアンス 法令遵守 ルール 啓発",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#4a6fa5")+person_head(24, 22, 9.5, "smile")+rect(37, 21, 17, 24, "#fff", 2)+path("M41 21 V45", width=1.6)+path("M44 32 L47 35 L52 28", stroke="#2a9d8f", width=2.2)),
        ("biz-payroll-payslip", "給与計算・労務担当者", "payroll labor salary 労務 給与計算 勤怠 人事労務",
         lambda: backdrop()+suit_body(24, 35, 20, 20, "#52799c", "#f28b82", True)+person_head(24, 22, 9.5, "smile", "long")+rect(36, 26, 19, 16, "#fff", 2)+circle(45.5, 34, 4.5, "#ffd54f")+sparkle(49, 16, 3.5)),
    ])

    category("19 現場・作業・店舗・物流の人物", "#c87d32", "#fdf3e7", "#5c9ead", [
        ("biz-factory-helmet", "現場作業員・安全第一", "factory worker helmet safety 作業員 工場 現場 ヘルメット 安全第一",
         lambda: backdrop()+work_body(32, 35, 22, 20)+person_head(32, 23, 9.5, "smile")+helmet_hat(32, 14)+path("M30 10 H34 M32 8 V12", stroke="#2a9d8f", width=2)+sparkle(50, 18, 3)),
        ("biz-site-supervisor", "現場監督・図面確認", "site supervisor blueprint 現場監督 施工管理 図面 点検",
         lambda: backdrop()+work_body(25, 35, 20, 20, "#457b9d")+person_head(25, 23, 9.5, "determined")+helmet_hat(25, 14)+rect(37, 28, 18, 16, "#fff", 1)+path("M40 33 H52 M40 38 H48", stroke="#4ea8de", width=1.8)),
        ("biz-warehouse-picker", "倉庫スタッフ・在庫検品", "warehouse staff inventory 倉庫 物流 検品 在庫管理 ピッキング",
         lambda: backdrop()+work_body(24, 35, 20, 20, "#e07a5f")+person_head(24, 22, 9.5, "smile")+rect(36, 32, 19, 17, "#deb887", 2)+rect(42, 32, 7, 6, "#fff", 0.5)+sparkle(48, 18, 3)),
        ("biz-delivery-driver", "配達員・配送ドライバー", "delivery driver courier 配達員 ドライバー 配送 物流 宅配",
         lambda: backdrop()+work_body(25, 35, 20, 20, "#529471")+person_head(25, 22, 9.5, "laugh")+path("M16 15 C18 8 34 8 36 15 H40 L36 17 H16 Z", "#529471")+rect(36, 33, 18, 15, "#deb887", 2)),
        ("biz-shop-clerk-welcome", "店舗スタッフ・いらっしゃいませ", "shop clerk retail welcome 店員 店舗 接客 いらっしゃいませ 販売員",
         lambda: backdrop()+person_body(32, 35, 22, 20, "#f4a261")+rect(24, 38, 16, 17, "#fff", 1)+person_head(32, 22, 10, "laugh", "long")+hand(20, 44)+hand(44, 44)+sparkle(14, 15, 3.5)+sparkle(50, 15, 3.5)),
        ("biz-cashier-register", "レジ接客・会計スタッフ", "cashier register payment レジ 会計 店舗 接客 決済",
         lambda: backdrop()+person_body(24, 35, 20, 20, "#81c995")+person_head(24, 22, 9.5, "smile", "bun")+rect(37, 31, 18, 17, "#e6eef5", 2)+rect(40, 24, 12, 7, "#fff", 1)+sparkle(49, 15, 3)),
        ("biz-cafe-restaurant-staff", "飲食・ホールスタッフ", "waiter cafe restaurant staff 飲食店 カフェ ホール 接客 スタッフ",
         lambda: backdrop()+person_body(25, 35, 20, 20, "#e76f51")+rect(19, 38, 12, 17, "#fff", 1)+person_head(25, 22, 9.5, "smile")+ellipse(46, 36, 9, 3, "#ced4da")+path("M41 35 H51 L49 29 H43 Z", "#fff")+hand(46, 39)),
        ("biz-cleaning-staff", "清掃・ビルメンテナンススタッフ", "cleaning maintenance staff 清掃 ビルメン 衛生 スタッフ",
         lambda: backdrop()+work_body(25, 35, 20, 20, "#6fb3d2", "#fff")+person_head(25, 22, 9.5, "smile")+path("M46 15 L40 43", stroke="#b5835a", width=3)+rect(34, 43, 14, 7, "#90e0ef", 2)+sparkle(51, 23, 3.5)),
        ("biz-security-guard", "警備員・施設警備", "security guard patrol 警備員 守衛 施設警備 安全",
         lambda: backdrop()+suit_body(27, 35, 21, 20, "#2b3a55", "#ffd54f")+person_head(27, 23, 9.5, "determined")+path("M17 16 H37 L35 10 H19 Z", "#2b3a55")+path("M46 22 V46", stroke="#e63946", width=4)+hand(44, 38)),
        ("biz-maintenance-mechanic", "保守点検・メンテナンス技術者", "maintenance technician repair 保守点検 メンテナンス 修理 技術者",
         lambda: backdrop()+work_body(25, 35, 20, 20, "#457b9d")+person_head(25, 22, 9.5, "smile")+rect(36, 34, 19, 14, "#e63946", 2)+path("M41 34 V30 H50 V34 M36 40 H55", width=1.8)+sparkle(48, 17, 3)),
    ])

    category("20 医療・福祉・教育・公共の人物", "#358f80", "#e7f7f4", "#4ea8de", [
        ("biz-doctor-explain", "説明する医師・ドクター", "doctor physician medical 医師 医者 ドクター 診察 説明",
         lambda: backdrop()+coat_body(26, 35, 22, 20, "#4ea8de")+person_head(26, 22, 10, "smile")+rect(40, 26, 15, 20, "#fff", 2)+path("M45 32 H50 M47.5 29.5 V34.5", stroke="#e63946", width=2.2)),
        ("biz-nurse-smile", "優しい看護師・ナース", "nurse healthcare smile 看護師 ナース 医療 ケア",
         lambda: backdrop()+person_body(28, 35, 22, 20, "#fff")+person_head(28, 22, 10, "smile", "long")+rect(38, 32, 14, 18, "#f79bb0", 2)+heart_shape(48, 15, 0.55)),
        ("biz-pharmacist-medicine", "お薬を渡す薬剤師", "pharmacist medicine prescription 薬剤師 薬局 調剤 服薬指導",
         lambda: backdrop()+coat_body(25, 35, 20, 20, "#81c995")+person_head(25, 22, 9.5, "smile", "bun")+rect(38, 26, 16, 20, "#fff", 2)+path("M43 33 H49 M46 30 V36", stroke="#2a9d8f", width=2.2)+hand(36, 39)),
        ("biz-care-worker", "介護スタッフ・ヘルパー", "care worker helper welfare 介護士 ヘルパー 福祉 ケア",
         lambda: backdrop()+person_body(26, 35, 21, 20, "#f4a261")+person_head(26, 22, 9.5, "laugh")+heart_shape(46, 24, 0.9)+sparkle(14, 16, 3)),
        ("biz-teacher-lesson", "授業・講義する先生・講師", "teacher instructor lecture 先生 講師 教員 授業 セミナー",
         lambda: backdrop()+rect(30, 10, 26, 20, "#2b6e46", 2)+suit_body(20, 35, 19, 20)+person_head(20, 23, 9, "smile")+path("M28 38 L41 22", stroke="#b5835a", width=2.5)+path("M35 17 H49 M35 23 H45", stroke="#fff", width=1.8)),
        ("biz-nursery-teacher", "保育士・幼稚園の先生", "nursery teacher childcare 保育士 幼稚園 先生 子育て",
         lambda: backdrop()+person_body(28, 35, 22, 20, "#ffd54f")+rect(21, 38, 14, 17, "#fff", 2)+person_head(28, 22, 10, "laugh", "bun")+circle(48, 36, 6, "#f79bb0")+circle(48, 36, 2, "#fff")+sparkle(48, 16, 3.5)),
        ("biz-civil-servant", "自治体職員・窓口相談員", "civil servant municipal window 自治体 公務員 役所 窓口 相談員",
         lambda: backdrop()+suit_body(32, 31, 20, 17, "#4a6fa5", "#52b788")+person_head(32, 19, 9.5, "smile")+rect(9, 46, 46, 9, "#deb887", 2)+rect(42, 33, 12, 13, "#fff", 1)),
        ("biz-counselor-listen", "カウンセラー・相談員", "counselor advisor listen カウンセラー 相談員 メンタルヘルス 傾聴",
         lambda: backdrop()+person_body(26, 35, 21, 20, "#81c995")+person_head(26, 22, 9.5, "peace", "long")+rect(38, 31, 14, 18, "#fff", 2)+heart_shape(47, 14, 0.6)),
        ("biz-researcher-lab", "研究者・白衣の科学者", "researcher scientist lab 研究者 科学者 実験 開発 白衣",
         lambda: backdrop()+coat_body(24, 35, 20, 20, "#5e548e")+person_head(24, 22, 9.5, "smile")+glasses(24, 22, 9.5)+path("M43 22 H49 M45 22 V30 L39 43 H53 L47 30 V22", "#90e0ef")+sparkle(49, 13, 3)),
        ("biz-lawyer-badge", "弁護士・士業の専門家", "lawyer attorney legal expert 弁護士 税理士 社労士 士業 専門家",
         lambda: backdrop()+suit_body(25, 35, 21, 20, "#2b3a55", "#e63946")+circle(31, 41, 2.5, "#ffd54f")+person_head(25, 22, 9.5, "determined")+rect(39, 26, 15, 21, "#8d5524", 2)+sparkle(48, 15, 3.5)),
    ])

    category("21 顧客・ユーザー・生活者のリアクション", "#d97254", "#fff2ec", "#73b3d6", [
        ("user-satisfied", "満足する顧客・高評価ユーザー", "satisfied customer happy user 満足 顧客 ユーザー 好評 笑顔",
         lambda: backdrop()+person_body(27, 35, 21, 20, "#f4a261")+person_head(27, 22, 10, "laugh", "long")+path("M48 14 L50 20 L56 20 L51 24 L53 30 L48 26 L43 30 L45 24 L40 20 L46 20 Z", "#ffd54f")+sparkle(14, 16, 3.5)),
        ("user-dissatisfied", "不満・クレームの顧客", "dissatisfied customer complaint 不満 クレーム 困る 顧客 課題",
         lambda: backdrop()+person_body(27, 35, 21, 20, "#e57373")+person_head(27, 22, 10, "angry")+anger_mark(48, 16)+path("M43 30 L51 25 L48 34 L55 30", stroke="#e63946", width=2.2)),
        ("user-confused-ui", "操作に迷う・使い方がわからない人", "confused user usability 迷う わからない 操作 ユーザー 疑問",
         lambda: backdrop()+person_body(26, 35, 20, 20, "#8d99ae")+person_head(26, 22, 9.5, "troubled")+rect(39, 28, 12, 19, "#fff", 2)+question_mark(48, 15)+sweat(13, 16)),
        ("user-smartphone-tap", "スマホを操作するユーザー", "smartphone user mobile app スマホ操作 ユーザー アプリ 利用者",
         lambda: backdrop()+person_body(25, 35, 20, 20, "#73b3d6")+person_head(25, 22, 9.5, "smile")+rect(39, 24, 13, 22, "#fff", 3)+rect(41, 27, 9, 14, "#90e0ef", 1)+hand(38, 37)+sparkle(49, 14, 3.5)),
        ("user-laptop-apply", "PCで申し込む・検索するユーザー", "pc user online apply 申し込み 検索 Web利用 ユーザー",
         lambda: backdrop()+person_body(32, 32, 22, 19, "#81c995")+person_head(32, 19, 9.5, "laugh", "long")+rect(21, 38, 22, 15, "#fff", 2)+circle(32, 45, 3.5, "#ffd54f")+sparkle(49, 15, 3.5)),
        ("user-compare-choose", "比較・検討して選ぶ顧客", "compare choose customer 比較 検討 選ぶ 迷う 顧客",
         lambda: backdrop()+person_body(32, 36, 20, 19, "#95a5cd")+person_head(32, 23, 9.5, "smile")+rect(8, 22, 11, 13, "#f28b82", 2)+rect(45, 22, 11, 13, "#6fb3d2", 2)+hand(15, 38)+hand(49, 38)),
        ("user-repeat-fan", "リピーター・ファン顧客", "repeat customer fan loyal リピーター ファン 常連 愛用者",
         lambda: backdrop()+person_body(27, 35, 21, 20, "#f28b82")+person_head(27, 22, 10, "laugh", "long")+heart_shape(47, 16, 0.85)+heart_shape(49, 34, 0.55)),
        ("user-family-customer", "ファミリー層の顧客・家族", "family customer household ファミリー 家族 世帯 顧客層",
         lambda: backdrop()+inset(person_body(color="#6fb3d2")+person_head(expr="smile"), -12, 7, 0.75)+inset(person_body(color="#f28b82")+person_head(expr="smile", hair="long"), 20, 7, 0.75)+inset(person_body(color="#ffd54f")+person_head(expr="laugh"), 5, 19, 0.62)),
        ("user-senior-smile", "シニア層の顧客・高齢者", "senior customer elderly シニア 高齢者 顧客 安心",
         lambda: backdrop()+person_body(32, 35, 22, 20, "#81c995")+person_head(32, 22, 10, "smile")+path("M22 20 C22 9 42 9 42 20 Q36 15 32 18 Q27 15 22 20 Z", "#e9ecef")+glasses(32, 22, 10)+sparkle(49, 15, 3.5)),
        ("user-student-youth", "若年層・学生ユーザー", "student youth user 学生 若者 Z世代 ユーザー",
         lambda: backdrop()+suit_body(26, 35, 20, 20, "#3d5a80", "#e63946")+person_head(26, 22, 9.5, "laugh")+rect(38, 31, 15, 18, "#ffd54f", 2)+sparkle(48, 16, 3.5)),
    ])

    category("22 働き方・キャリア・コンプライアンスの人物", "#5c768d", "#edf2f7", "#5b8e7d", [
        ("biz-work-life-balance", "ワークライフバランス・定時退社", "work life balance on time ワークライフバランス 定時退社 充実 働き方",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#4a6fa5")+person_head(25, 22, 9.5, "laugh")+circle(47, 23, 9, "#fff")+path("M47 18 V23 L51 25", width=2)+heart_shape(48, 40, 0.55)),
        ("biz-parental-leave", "育休・子育てと仕事の両立", "parental leave childcare work 育休 育児休業 両立 共働き 子育て",
         lambda: backdrop()+person_body(26, 35, 21, 20, "#5b8e7d")+person_head(26, 21, 9.5, "smile")+circle(41, 35, 6.5, SKIN)+cheeks(41, 36, 3.5, 1.6, 1)+dot(39, 34, 1.2)+dot(43, 34, 1.2)+path("M34 43 C34 38 48 38 48 43 V53 H34 Z", "#ffd54f")+heart_shape(48, 14, 0.55)),
        ("biz-career-up", "キャリアアップ・昇進して喜ぶ人", "career up promotion step up キャリアアップ 昇進 成長 ステップアップ",
         lambda: backdrop()+suit_body(23, 35, 19, 20, "#52799c", "#f28b82", True)+person_head(23, 22, 9.5, "laugh", "long")+path("M36 51 H43 V42 H50 V33 H57", stroke="#e65c4f", width=3)+sparkle(48, 16, 4)),
        ("biz-job-change-recruit", "転職・求職活動する人", "job change recruit career 転職 求人 応募 キャリア",
         lambda: backdrop()+suit_body(25, 35, 20, 20)+person_head(25, 22, 9.5, "determined")+rect(37, 25, 17, 22, "#fff", 2)+path("M41 32 H50 M41 37 H50 M41 42 H47", width=1.8)+sparkle(48, 14, 3.5)),
        ("biz-overwork-tired", "残業・過労で疲れた社員", "overtime tired burnout 残業 過労 疲労 働きすぎ 注意",
         lambda: backdrop()+suit_body(32, 33, 22, 19, "#6c7a89")+person_head(32, 20, 9.5, "sleepy")+rect(21, 39, 22, 14, "#dce8f2", 2)+sweat(50, 15)+path("M13 13 Q9 19 14 24", stroke="#6c7a89", width=2)),
        ("biz-harassment-no", "ハラスメント防止・NOサイン", "harassment prevention stop ハラスメント 防止 パワハラ 対策 職場環境",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#4a6fa5")+person_head(25, 22, 9.5, "determined")+hand(41, 29)+circle(48, 24, 8, "#fff", "#e63946", 3)+path("M43 19 L53 29", stroke="#e63946", width=3)),
        ("biz-info-leak-warn", "情報漏洩・うっかりミスに焦る社員", "information leak mistake panic 情報漏洩 うっかり ミス 紛失 注意",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#6c7a89")+person_head(25, 22, 9.5, "surprised")+rect(39, 32, 15, 18, "#fff", 2)+exclamation_mark(48, 16)+sweat(13, 15)),
        ("biz-password-lock-user", "セキュリティ意識・鍵を守る社員", "security awareness password lock セキュリティ 鍵 パスワード 対策 社員",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#3d5a80")+person_head(25, 22, 9.5, "smile")+path("M42 32 V27 C42 21 52 21 52 27 V32", width=2.5)+rect(39, 32, 16, 15, "#ffd54f", 2)+dot(47, 39, 2)),
        ("biz-health-checkup", "健康診断・健康経営", "health checkup wellness 健康診断 検診 健康経営 体調管理",
         lambda: backdrop()+suit_body(25, 35, 20, 20, "#529471")+person_head(25, 22, 9.5, "smile")+rect(38, 25, 16, 22, "#fff", 2)+heart_shape(46, 30, 0.5)+path("M42 41 L45 44 L51 38", stroke="#2a9d8f", width=2.2)),
        ("biz-diversity-team", "ダイバーシティ・多様な人材", "diversity inclusion global ダイバーシティ 多様性 チーム 協働",
         lambda: backdrop()+inset(suit_body(jacket="#4a6fa5")+person_head(expr="smile"), -12, 11, 0.66)+inset(suit_body(jacket="#e07a5f", tie="#ffd54f", female=True)+person_head(expr="laugh", hair="long"), 22, 11, 0.66)+inset(person_body(color="#529471")+person_head(expr="smile", hair="bun"), 5, 3, 0.68)+heart_shape(32, 4, 0.45)),
    ])

    return entries, categories


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail when generated files differ")
    args = parser.parse_args()
    entries, categories = make_catalog()
    assert len(entries) == 220 and len({e["file"] for e in entries}) == 220
    assert len({e["svg"] for e in entries}) == 220
    assert len(categories) == 22
    assert all(len(rows) == 10 for _, rows in categories)
    inventory = [
        "# yydraw イラストアイコン220種",
        "",
        "説明資料、業務マニュアル、プレゼン、提案書、案内図などで使いやすい、親しみやすいタッチの独自SVGアイコン集です。",
        "左のカテゴリで「イラスト / 01 …」～「イラスト / 22 …」を選び、名前または検索語で絞り込めます。",
        "名前と画像を表示し、ドラッグして配置します。配置した図柄だけが文書に保存されます。",
        "図柄はすべてパスで描かれており、外部フォントに依存しません。",
        "",
        "公式クラウド素材1,834種・業務・ITアイコン376種と合わせて2,430種です。",
        "このリポジトリと同じGPL-3.0-or-laterで提供します。",
        "",
        "再生成: `python tools/friendly-icons/generate.py`。整合性確認: `python tools/friendly-icons/generate.py --check`。",
        "",
    ]
    for name, rows in categories:
        inventory += [f"## {name}", "", "| 名前 | 検索語 |", "| --- | --- |"]
        inventory += [f"| {label} | {keywords} |" for label, keywords, _ in rows]
        inventory.append("")
    files = [
        (OUT, json.dumps(entries, ensure_ascii=False, indent=2) + "\n"),
        (INVENTORY, "\n".join(inventory)),
    ]
    for filename, content in files:
        if args.check:
            if not filename.exists() or filename.read_text(encoding="utf-8") != content:
                raise SystemExit(f"Generated file is out of date: {filename}")
        else:
            filename.parent.mkdir(parents=True, exist_ok=True)
            filename.write_text(content, encoding="utf-8", newline="\n")
    print(f"Friendly illustration catalog: {len(entries)} unique icons in {len(categories)} categories")


if __name__ == "__main__":
    main()
