"""Generate yydraw's original, offline business/IT vector catalog and inventory."""
import argparse
import json
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/yy-draw/assets/business-icons.json"
INVENTORY = ROOT / "docs/user/yydraw-business-icons.md"
INK = "#29445f"
ACCENT = "#247ca8"
PALE = "#e4f2fa"


def path(d, fill="none", stroke=INK, width=2):
    return f'<path d="{d}" fill="{fill}" stroke="{stroke}" stroke-width="{width}"/>'


def rect(x, y, w, h, fill=None, radius=3):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill or PALE}" stroke="{INK}" stroke-width="2"/>'


def circle(x, y, r, fill=None):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill or PALE}" stroke="{INK}" stroke-width="2"/>'


def dot(x, y, r=2):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{ACCENT}"/>'


def arrow(x, y, dx, dy):
    ex, ey = x + dx, y + dy
    length = (dx * dx + dy * dy) ** 0.5
    ux, uy = dx / length, dy / length
    return path(f'M{x} {y} L{ex} {ey} M{ex-ux*5-uy*4} {ey-uy*5+ux*4} L{ex} {ey} L{ex-ux*5+uy*4} {ey-uy*5-ux*4}', stroke=ACCENT)


def person(x, y, scale=1):
    return f'<g transform="translate({x} {y}) scale({scale})">' + circle(0, -7, 5, "#fff") + path('M-10 13 V8 C-10-3 10-3 10 8 V13 Z', PALE) + '</g>'


def server(x=16, y=10, w=32, h=12):
    return rect(x, y, w, h) + dot(x + 5, y + h / 2) + path(f'M{x+w-13} {y+h/2} h7')


def database(x=14, y=12, w=36, h=38):
    ry = 6
    return path(f'M{x} {y+ry} V{y+h-ry} C{x} {y+h+2} {x+w} {y+h+2} {x+w} {y+h-ry} V{y+ry}', PALE) + f'<ellipse cx="{x+w/2}" cy="{y+ry}" rx="{w/2}" ry="{ry}" fill="{PALE}" stroke="{INK}" stroke-width="2"/>' + path(f'M{x} {y+h/2} C{x} {y+h/2+8} {x+w} {y+h/2+8} {x+w} {y+h/2}')


def document():
    return path('M16 6 H39 L50 17 V58 H16 Z', PALE) + path('M39 6 V17 H50 M23 26 H42 M23 33 H42 M23 40 H35')


def shield():
    return path('M32 6 L53 14 V30 C53 43 44 51 32 58 C20 51 11 43 11 30 V14 Z', PALE)


def monitor():
    return rect(7, 9, 50, 34) + path('M26 43 V53 H38 V43 M18 54 H46 M8 36 H56')


def building():
    return rect(12, 8, 40, 49) + ''.join(rect(x, y, 6, 6, "#fff", 0) for y in (16, 29) for x in (20, 37)) + rect(27, 44, 10, 13, "#fff", 0)


def check(x=24, y=34):
    return path(f'M{x-5} {y} l5 5 11-12', stroke=ACCENT, width=3)


def gear(x=32, y=32, r=12):
    return circle(x, y, r) + circle(x, y, 4, "#fff") + path(f'M{x} {y-r-6} v6 M{x} {y+r} v6 M{x-r-6} {y} h6 M{x+r} {y} h6 M{x-r-4} {y-r-4} l5 5 M{x+r-1} {y+r-1} l5 5 M{x+r+4} {y-r-4} l-5 5 M{x-r+1} {y+r-1} l-5 5', stroke=ACCENT, width=3)


def chart_axes():
    return path('M10 9 V54 H57')


def bust(style="plain"):
    clothing = "#345b80" if style == "suit" else ("#fff" if style == "coat" else PALE)
    art = face() + path('M16 57 V39 C16 23 48 23 48 39 V57 Z', clothing)
    if style == "suit":
        art += path('M25 28 L32 34 L39 28 L36 48 H28 Z', "#fff")+path('M32 33 L35 38 L32 51 L29 38 Z', ACCENT)+path('M23 29 L26 43 L30 39 M41 29 L38 43 L34 39', stroke="#d8e8f6")
    elif style == "coat":
        art += path('M24 29 L29 39 L25 44 L30 57 M40 29 L35 39 L39 44 L34 57 M22 34 V43 Q22 50 29 47 M42 34 V42', stroke=ACCENT)+circle(42, 46, 3, "#fff")
    elif style == "uniform":
        art += path('M24 29 L32 36 L40 29 M32 36 V57 M39 42 H45', stroke=ACCENT)+dot(32, 44, 1)+dot(32, 51, 1)
    else:
        art += path('M23 36 V57 M41 36 V57', stroke=ACCENT)
    return art


def face(hair="short", sad=False):
    art = ""
    if hair == "long":
        art += path('M19 30 V14 C19-3 45-3 45 14 V30 L39 33 H25 Z', "#493b35")
    art += circle(32, 14, 9, "#f5d1b5")
    if hair == "short":
        art += path('M23 12 C20-1 43-2 42 12 L35 7 L28 11 Z', "#493b35")
    else:
        art += path('M23 11 Q24 1 33 3 Q43 3 42 13 L32 7 L26 12 Z', "#493b35")
    art += dot(28.5, 14, 1)+dot(35.5, 14, 1)
    art += path('M29 20 Q32 16 35 20' if sad else 'M29 18 Q32 21 35 18', stroke=INK, width=1)
    return art


def character(role):
    if role == "criminal":
        return path('M16 59 V38 Q16 27 32 27 Q48 27 48 38 V59 Z', "#ddd")+path('M17 36 H47 M17 44 H47 M17 52 H47', stroke=INK, width=4)+face()+path('M23 6 Q32-2 41 6 L43 10 H21 Z', "#303640")+path('M22 12 Q32 8 42 12 L40 19 L34 17 H30 L24 19 Z', "#303640")+circle(28, 14, 1.5, "#fff")+circle(36, 14, 1.5, "#fff")+circle(40, 49, 8, "#fff")+circle(55, 49, 7, "#fff")+path('M46 46 H49 M46 52 H49')
    if role == "hacker":
        return path('M11 39 V24 C11-5 53-5 53 24 V39 Z', "#303b48")+path('M23 27 V17 C23 3 41 3 41 17 V27 Z', "#f5d1b5")+path('M22 16 H42 V23 H22 Z', "#15202c")+path('M26 19 H30 M34 19 H38', stroke="#69dca4")+rect(7, 32, 50, 26, "#273e43")+path('M3 60 H61 M16 40 L22 44 L16 48 M28 48 H43 M31 38 H47', stroke="#69dca4", width=2)
    if role == "terrorist":
        return path('M12 59 V37 Q12 25 32 25 Q52 25 52 37 V59 Z', "#576348")+path('M18 4 Q32-5 46 4 V25 Q32 34 18 25 Z', "#252c35")+path('M21 11 H43 V18 H21 Z', "#f5d1b5")+dot(27, 14, 1.5)+dot(37, 14, 1.5)+path('M20 29 L26 58 M44 29 L38 58', stroke="#252c35", width=3)+rect(25, 33, 14, 16, "#73815b", 1)+path('M5 38 L18 42 L22 37 H43 V35 H59 V39 H43 V45 H33 L29 57 H23 L25 45 H15 L7 48 Z', "#252c35")
    if role == "victim":
        return face(sad=True)+path('M16 59 V38 Q16 27 32 27 Q48 27 48 38 V59 Z', "#b9cde0")+path('M23 7 H42 V13 H23 Z', "#fff")+path('M25 8 L29 12 M32 8 L36 12', stroke=INK, width=1)+path('M20 30 L40 50 L21 49 Z', "#fff")+path('M40 30 L24 45', stroke=INK, width=2)+path('M40 18 Q45 23 40 25 Q36 24 40 18 Z', "#5da4ce")
    if role == "security":
        return bust("suit")+path('M22 11 H30 V18 H22 Z M34 11 H42 V18 H34 Z M30 14 H34', "#fff")+path('M44 29 L61 35 V47 Q61 57 44 63 Q29 57 29 47 V35 Z', "#367d63")+path('M36 45 L42 51 L55 38', stroke="#fff", width=3)
    if role == "spy":
        return inset(globe(), 27, 27, .56)+path('M9 59 V35 L23 25 H33 L45 35 V59 Z', "#8d8172")+path('M18 26 L24 47 L28 38 L34 47 L40 29 M28 38 V58', stroke=INK)+circle(28, 19, 10, "#f5d1b5")+path('M10 14 L18 9 L21 2 L29 5 L36 2 L40 12 L47 15 Z', "#333b47")+path('M17 18 H26 V24 H17 Z M29 18 H38 V24 H29 Z M26 20 H29', "#252c35")
    if role == "doctor":
        return bust("coat")+path('M22 11 H30 V18 H22 Z M34 11 H42 V18 H34 Z M30 14 H34', "#fff")+path('M24 28 V39 Q24 48 32 48 Q40 48 40 39 V28', stroke="#2d7b88", width=2.5)+circle(32, 50, 3, "#fff")
    if role == "nurse":
        return face("long")+path('M15 59 L19 35 Q32 24 45 35 L49 59 Z', "#fff")+path('M23 29 L32 38 L41 29 M32 38 V58 M19 46 H26', stroke=ACCENT)+path('M20 4 H44 L40 11 H24 Z', "#fff")+path('M32 5 V10 M29 7 H35', stroke=ACCENT, width=2)+rect(38, 43, 7, 9, "#d4ede9", 0)
    if role == "teacher":
        return rect(24, 4, 37, 37, "#315e4a", 1)+path('M29 14 H51 M33 10 V18 M30 26 H52 M41 22 V30 M32 41 V59 M55 41 V59', stroke="#fff", width=2)+inset(bust("suit"), -8, 16, .75)+path('M23 42 L43 20', stroke="#bd8747", width=3)
    if role == "child":
        return rect(15, 29, 34, 22, "#b35c4f", 5)+circle(32, 20, 10, "#f5d1b5")+path('M20 13 Q32-2 44 13 H20 M26 30 H38 V48 H26 Z', "#f0c348")+dot(28, 20, 1)+dot(36, 20, 1)+path('M29 25 Q32 28 35 25', stroke=INK, width=1)+path('M24 30 L17 42 M40 30 L47 42 M24 48 H40 V54 H24 Z', "#fff")+path('M28 54 V61 M36 54 V61', stroke=INK, width=3)
    if role == "guardian":
        return inset(face("long"), -7, 0, 1)+path('M9 59 L12 36 Q25 22 39 36 L43 59 Z', "#b47c8d")+inset(character("child"), 29, 27, .52)+path('M34 40 L43 48', stroke=INK, width=3)
    if role == "salesman":
        return bust("suit")+path('M19 35 L8 47 M45 35 L54 47', stroke=INK, width=3)+rect(36, 43, 26, 17, "#8e6948", 1)+path('M43 43 V38 H55 V43 M36 49 H62 M47 48 H52 V53 H47 Z', "#fff")
    if role == "receptionist":
        return face("long")+path('M17 39 V32 Q32 23 47 32 V39 Z', "#c18b9c")+path('M24 27 L32 34 L40 27', "#fff")+rect(4, 38, 56, 23, "#cbb69b", 1)+path('M4 38 H60', stroke=INK, width=3)+rect(10, 29, 20, 11, "#fff", 1)+path('M40 31 Q45 21 51 31 V35 H39 Z M38 35 H53', PALE)
    if role == "diplomat":
        return bust("suit")+rect(36, 30, 25, 31, "#365c8c", 2)+inset(globe(), 40, 31, .26)+path('M41 52 H56 M41 57 H53', stroke="#fff")+path('M4 9 V28 M4 10 H16 L12 17 L16 23 H4', PALE)
    raise ValueError(role)


def warning():
    return path('M32 5 L60 55 H4 Z', "#fff")+path('M32 23 V37', stroke=ACCENT, width=4)+dot(32, 46, 2.5)


def globe():
    return circle(32, 32, 25, "#fff")+path('M7 32 H57 M13 17 H51 M13 47 H51 M32 7 C15 22 15 42 32 57 C49 42 49 22 32 7', stroke=ACCENT)


def civic_building():
    return path('M4 20 L32 4 L60 20 Z', PALE)+path('M7 56 H57 M4 61 H60')+''.join(rect(x, 24, 7, 28, "#fff", 0) for x in (10, 28, 46))


def scales():
    return path('M32 5 V55 M18 59 H46 M11 17 H53 M16 17 L6 37 H26 Z M48 17 L38 37 H58 Z', PALE)+path('M6 37 Q16 49 26 37 M38 37 Q48 49 58 37', "#fff")+circle(32, 14, 4, "#fff")


def medical_cross():
    return path('M25 7 H39 V25 H57 V39 H39 V57 H25 V39 H7 V25 H25 Z', "#fff")


def executive(symbol):
    return inset(bust("suit"), -4, 1, .94)+inset(symbol, 30, 28, .5)


def organization():
    return rect(23, 5, 18, 14, "#fff")+path('M32 19 V32 M11 32 H53 M11 32 V43 M32 32 V43 M53 32 V43', stroke=ACCENT)+rect(3, 43, 16, 14)+rect(24, 43, 16, 14)+rect(45, 43, 16, 14)


# Small geometric lettering is drawn as paths; no fonts are loaded by the artwork.
LETTERS = {
    "A": "M0 8 L3 0 L6 8 M1 5 H5", "B": "M0 8 V0 H3 Q8 0 5 4 Q8 8 3 8 Z M0 4 H4",
    "C": "M6 1 Q0-2 0 4 Q0 10 6 7", "D": "M0 0 H2 Q7 0 6 4 Q7 8 2 8 H0 Z",
    "E": "M6 0 H0 V8 H6 M0 4 H5", "F": "M6 0 H0 V8 M0 4 H5",
    "G": "M6 1 Q0-2 0 4 Q0 10 6 7 V4 H3", "H": "M0 0 V8 M6 0 V8 M0 4 H6",
    "I": "M0 0 H6 M3 0 V8 M0 8 H6", "J": "M0 0 H6 V6 Q6 10 0 7",
    "K": "M0 0 V8 M6 0 L0 4 L6 8", "L": "M0 0 V8 H6",
    "M": "M0 8 V0 L3 4 L6 0 V8", "N": "M0 8 V0 L6 8 V0",
    "O": "M3 0 Q0 0 0 4 Q0 8 3 8 Q6 8 6 4 Q6 0 3 0 Z",
    "P": "M0 8 V0 H3 Q9 0 5 4 H0", "Q": "M3 0 Q0 0 0 4 Q0 8 3 8 Q6 8 6 4 Q6 0 3 0 Z M3 5 L7 9",
    "R": "M0 8 V0 H3 Q9 0 5 4 H0 M3 4 L6 8", "S": "M6 1 Q0-2 0 2 Q0 4 3 4 Q6 4 6 6 Q6 10 0 7",
    "T": "M0 0 H6 M3 0 V8", "U": "M0 0 V5 Q0 11 6 7 V0",
    "V": "M0 0 L3 8 L6 0", "W": "M0 0 L1 8 L3 4 L5 8 L6 0",
    "X": "M0 0 L6 8 M6 0 L0 8", "Y": "M0 0 L3 4 L6 0 M3 4 V8",
    "Z": "M0 0 H6 L0 8 H6", "5": "M6 0 H0 V4 H3 Q8 4 6 7 Q4 10 0 7",
    "#": "M2 0 L1 8 M5 0 L4 8 M0 3 H6 M0 6 H6",
    "/": "M0 9 L6-1", "-": "M0 4 H6",
}


def word(text, x=32, y=32, scale=1):
    start = x - (len(text) * 9 - 3) * scale / 2
    return ''.join(f'<g transform="translate({start+i*9*scale} {y}) scale({scale})">' +
                   path(LETTERS[c], stroke=ACCENT, width=1.5) + '</g>' for i, c in enumerate(text) if c != ' ')


def tag(text, x=32, y=47):
    w = len(text) * 8 + 10
    return rect(x-w/2, y-4, w, 17, "#fff", 3) + word(text, x, y, .8)


def nameplate(name):
    """Full product names, at a constant letter height; never truncate words."""
    lines = []
    for part in name.split():
        if lines and len(lines[-1]) + len(part) < 12:
            lines[-1] += ' ' + part
        else:
            lines.append(part)
    top = 61 - len(lines) * 8
    art = f'<title>{escape(name)}</title>' + rect(3, top-2, 58, len(lines)*8+3, "#fff", 2)
    for i, line in enumerate(lines):
        width = (len(line) * 9 - 3) * .8
        squeeze = min(1, 52 / width)
        art += f'<g transform="translate(32 {top+i*8}) scale({squeeze} 1)">' + word(line.upper(), 0, 0, .8) + '</g>'
    return art


def elephant():
    # Large ears, two tusks and a curled trunk, readable without the database cylinder.
    return path('M24 12 C11-1 0 11 7 28 Q12 41 23 33 M40 12 C53-1 64 11 57 28 Q52 41 41 33', "#cbe0f1") + path('M22 12 Q32 5 42 12 L41 29 L38 35 V40 Q48 43 49 35 Q55 48 41 49 Q29 49 29 40 V31 L23 28 Z', "#8eb9da") + path('M24 27 Q16 30 20 36 L27 30 M40 27 Q47 30 44 35 L38 31', "#fff") + dot(27, 20, 1.8)+dot(37, 20, 1.8)+path('M32 13 V23 M10 15 Q14 10 21 16 M54 15 Q50 10 43 16', stroke=INK)


def dolphin():
    # A clear side profile with a long snout, dorsal fin, flipper and forked tail.
    return path('M6 27 C15 11 29 8 40 15 L43 5 L50 17 Q55 20 56 25 L62 27 L61 31 L51 31 Q43 35 35 30 L30 40 L26 29 Q20 27 14 32 L10 42 L6 35 L1 39 L4 30 Z', "#b8e4ed") + path('M13 29 Q26 17 42 25 M35 30 L43 29', stroke="#137d98") + dot(52, 24, 1.6) + path('M9 47 Q22 42 34 47 T58 47', stroke="#137d98")


def message(x=22, y=24, w=20, h=14):
    return rect(x, y, w, h, "#fff", 1) + path(f'M{x} {y} L{x+w/2} {y+h*.6} L{x+w} {y}', stroke=ACCENT)


def queue():
    return rect(7, 16, 50, 31) + message(12, 24, 10, 14)+message(27, 24, 10, 14)+message(42, 24, 10, 14)+arrow(0, 32, 7, 0)+arrow(57, 32, 7, 0)


def topic():
    return circle(25, 31, 12)+message(17, 26, 16, 11)+path('M37 31 H44 M44 12 V51', stroke=ACCENT)+message(47, 6, 14, 12)+message(47, 25, 14, 12)+message(47, 45, 14, 12)+path('M44 12 H47 M44 31 H47 M44 51 H47', stroke=ACCENT)


def padlock():
    return path('M22 26 V19 C22 3 42 3 42 19 V26', stroke=ACCENT, width=3)+rect(15, 26, 34, 29)+circle(32, 38, 4, "#fff")+path('M32 42 V48')


def disk():
    return rect(7, 7, 50, 48)+circle(32, 28, 16, "#fff")+circle(32, 28, 4)+path('M35 30 L48 43 M14 49 H25', stroke=ACCENT)+dot(49, 49)


def cluster():
    return server(3, 4, 24, 14)+server(37, 4, 24, 14)+server(20, 43, 24, 14)+path('M15 18 V30 H49 V18 M32 30 V43', stroke=ACCENT)+circle(32, 30, 5, "#fff")


def clock():
    return circle(32, 32, 24, "#fff")+path('M32 16 V32 L43 39', stroke=ACCENT, width=3)+path('M32 8 V12 M56 32 H52 M32 56 V52 M8 32 H12')


def key_shape():
    return circle(18, 24, 11, "#fff")+circle(18, 24, 4)+path('M26 32 L49 55 L57 47 L50 40 L46 44 L39 37 L43 33 L36 26 L31 31', PALE)


def copies():
    return rect(6, 8, 35, 35, "#fff")+rect(22, 23, 35, 35)+path('M29 33 H49 M29 41 H49 M29 49 H42', stroke=ACCENT)


def product(name, artwork):
    return inset(artwork, 6, 0, .8) + nameplate(name)


def phone(notch=False, wide=False):
    x, w = (10, 44) if wide else (17, 30)
    return rect(x, 4, w, 56, "#fff", 6) + rect(x+4, 11, w-8, 39, radius=2) + (
        path('M26 8 H38', width=3) if notch else dot(32, 8, 1)) + path('M27 55 H37')


def robot():
    return path('M20 22 Q20 8 32 8 Q44 8 44 22 Z', PALE) + path('M22 10 L18 5 M42 10 L46 5') + rect(19, 26, 26, 25) + path('M13 29 V45 M51 29 V45 M25 51 V58 M39 51 V58', stroke=ACCENT, width=5) + dot(27, 17)+dot(37, 17)


def inset(svg, x, y, scale):
    return f'<g transform="translate({x} {y}) scale({scale})">{svg}</g>'


def bubble():
    return path('M7 8 H57 V43 H31 L17 56 V43 H7 Z', PALE)


def envelope():
    return rect(5, 16, 54, 37, "#fff") + path('M6 18 L32 38 L58 18 M6 51 L23 34 M58 51 L41 34', stroke=ACCENT)


def headset():
    return path('M12 31 V24 C12-2 52-2 52 24 V31', stroke=ACCENT, width=4) + rect(7, 25, 10, 18, "#fff") + rect(47, 25, 10, 18, "#fff") + path('M52 43 V51 H38') + dot(35, 51, 3)


def brain():
    return path('M32 13 C22 2 12 11 13 20 C2 22 4 37 12 39 C8 52 24 59 32 50 C40 59 56 52 52 39 C60 37 62 22 51 20 C52 11 42 2 32 13 Z', PALE) + path('M32 13 V50 M20 17 L24 26 L15 34 L23 43 M44 17 L40 26 L49 34 L41 43', stroke=ACCENT) + dot(24, 26, 3)+dot(40, 26, 3)


def network():
    points = [(12, 12), (12, 32), (12, 52), (33, 21), (33, 43), (54, 32)]
    return ''.join(path(f'M{a[0]} {a[1]} L{b[0]} {b[1]}', stroke=ACCENT) for a in points[:3] for b in points[3:5]) + ''.join(path(f'M{x} {y} L54 32') for x, y in points[3:5]) + ''.join(circle(x, y, 5, "#fff") for x, y in points)


def file_rows(kind):
    artwork = path('M16 6 H39 L50 17 V58 H16 Z', PALE) + path('M39 6 V17 H50')
    if kind == "FIX":
        artwork += path('M21 24 H45 M21 31 H45 M21 38 H45 M27 22 V41 M35 22 V41 M41 22 V41', stroke=ACCENT)
    elif kind in ("CSV", "TSV"):
        artwork += path('M21 23 H45 M21 30 H45 M21 37 H45 M29 22 V40 M38 22 V40', stroke=ACCENT)
        artwork += path('M25 26 l-1 2 M34 33 l-1 2') if kind == "CSV" else arrow(22, 26, 20, 0)
    elif kind == "JSON":
        artwork += path('M27 23 Q22 23 24 29 L21 32 L24 35 Q22 41 27 41 M38 23 Q43 23 41 29 L44 32 L41 35 Q43 41 38 41', stroke=ACCENT)
    elif kind == "XML":
        artwork += path('M27 24 L21 31 L27 38 M38 24 L44 31 L38 38 M35 23 L30 40', stroke=ACCENT)
    return artwork + (nameplate({'FIX': 'Fixed Length', 'XLS': 'Excel'}[kind]) if kind in ('FIX', 'XLS') else tag(kind, 34, 47))


def adult(style="plain"):
    art = circle(32, 15, 8, "#fff") + path('M19 54 V38 Q19 26 32 26 Q45 26 45 38 V54 Z', PALE)
    if style == "senior":
        art += path('M25 10 Q32 2 39 10 M24 17 H30 M34 17 H40 M30 17 H34 M25 17 V20 H30 V17 M34 17 V20 H39 V17 M46 38 Q54 32 54 41 V59', stroke=ACCENT) + path('M27 54 V60 M37 54 V60')
    elif style == "student":
        art += path('M15 8 L32 2 L49 8 L32 14 Z', PALE)+path('M48 8 V21 M25 33 H39 V49 H25 Z', "#fff")
    elif style == "teen":
        art += path('M21 10 Q28-1 41 8 M23 32 V49 M41 32 V49 M27 54 V60 M37 54 V60')+rect(27, 36, 10, 14, "#fff", 1)
    elif style == "care":
        art += path('M24 5 H40 V12 H24 Z', PALE)+path('M32 5 V11 M29 8 H35', stroke=ACCENT)+path('M28 33 H36 M32 29 V37', stroke=ACCENT, width=3)
    else:
        art += path('M32 29 V45 M27 54 V60 M37 54 V60')
    return art


def dish():
    return path('M12 10 Q4 44 44 50 Z', PALE) + path('M12 10 L44 50 M28 31 L47 17 M44 14 L50 20 M27 48 L21 59 H48 M38 50 L42 59', stroke=ACCENT) + path('M43 5 Q60 7 59 23 M45 11 Q54 12 53 21', stroke=ACCENT)


def satellite():
    return '<g transform="rotate(-30 32 32)">' + rect(24, 20, 16, 25, "#fff") + rect(3, 23, 17, 19) + rect(44, 23, 17, 19) + path('M11 23 V42 M3 32 H20 M52 23 V42 M44 32 H61 M20 32 H24 M40 32 H44 M32 20 V11 M27 10 Q32 15 37 10 M32 45 V51', stroke=ACCENT) + '</g>'


def ad_frame():
    return rect(5, 8, 54, 45, "#fff") + path('M5 19 H59') + dot(12, 13)+dot(19, 13)


def make_catalog():
    entries = []
    categories = []

    def category(name, accent, pale, items):
        global ACCENT, PALE
        ACCENT, PALE = accent, pale
        rows = []
        for slug, name_ja, keywords, draw in items:
            svg = '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><g stroke-linecap="round" stroke-linejoin="round">' + draw() + '</g></svg>'
            entries.append(dict(vendor="業務・IT", category=name, name=name_ja,
                                keywords=keywords, source="yydraw:business-icons/v1",
                                file=f"business/{slug}.svg", svg=svg))
            rows.append((name_ja, keywords, slug))
        categories.append((name, rows))

    category("01 コンピューティング", "#247ca8", "#e4f2fa", [
        ("server", "サーバー", "server サーバ", lambda: server(12, 8, 40, 14)+server(12, 25, 40, 14)+server(12, 42, 40, 14)),
        ("rack", "ラック", "rack datacenter データセンター", lambda: rect(13, 5, 38, 52, "#fff")+server(18, 11, 28, 10)+server(18, 25, 28, 10)+server(18, 39, 28, 10)+path('M18 57 V60 M46 57 V60')),
        ("desktop", "デスクトップPC", "desktop workstation client 端末", lambda: monitor()+rect(17, 57, 30, 4, "#fff", 1)),
        ("laptop", "ノートPC", "laptop notebook mobile パソコン", lambda: rect(12, 10, 40, 30)+rect(17, 15, 30, 20, "#fff", 1)+path('M12 40 L5 52 Q5 56 10 56 H54 Q59 56 59 52 L52 40 Z', PALE)+path('M26 50 H38')),
        ("virtual-machine", "仮想マシン", "virtual machine vm 仮想化", lambda: rect(8, 8, 35, 35, "#fff")+rect(21, 21, 35, 35)+path('M28 33 H48 M28 40 H48 M28 47 H39')+path('M9 49 V56 H16', stroke=ACCENT)),
        ("container", "コンテナ", "container docker pod kubernetes k8s", lambda: path('M8 17 L32 6 L56 17 V47 L32 58 L8 47 Z', PALE)+path('M8 17 L32 28 L56 17 M32 28 V58 M20 12 L44 23 M20 23 V51 M44 23 V51')),
        ("application", "アプリケーション", "application app software system ソフトウェア", lambda: rect(7, 9, 50, 46)+path('M7 20 H57')+dot(13, 15)+dot(20, 15)+path('M26 29 L19 36 L26 43 M38 29 L45 36 L38 43 M34 27 L30 45', stroke=ACCENT)),
        ("batch", "バッチ処理", "batch job bulk 一括処理", lambda: rect(9, 7, 34, 38, "#fff")+rect(16, 14, 34, 38)+path('M23 25 H43 M23 32 H37')+arrow(23, 42, 19, 0)+circle(48, 48, 10, "#fff")+path('M48 42 V48 L52 51', stroke=ACCENT)),
        ("mainframe", "メインフレーム", "mainframe host legacy 汎用機 ホスト", lambda: rect(6, 9, 24, 47)+rect(34, 9, 24, 47)+path('M11 19 H25 M11 26 H25 M11 33 H25 M39 19 H53 M39 26 H53 M39 33 H53')+circle(18, 45, 4, "#fff")+circle(46, 45, 4, "#fff")),
        ("edge-device", "エッジ機器", "edge iot device sensor センサー", lambda: rect(16, 19, 32, 32)+rect(23, 26, 18, 18, "#fff")+path('M24 13 V19 M32 13 V19 M40 13 V19 M24 51 V57 M32 51 V57 M40 51 V57 M10 27 H16 M10 35 H16 M10 43 H16 M48 27 H54 M48 35 H54 M48 43 H54')+dot(32, 35, 4)),
    ])
    category("02 ネットワーク", "#207f84", "#e0f3f0", [
        ("router", "ルーター", "router routing ルータ", lambda: rect(8, 19, 48, 27)+arrow(19, 27, 26, 0)+arrow(45, 38, -26, 0)+path('M17 46 V55 M47 46 V55 M12 55 H52')),
        ("switch", "スイッチ", "switch switching hub ハブ", lambda: rect(6, 18, 52, 30)+''.join(rect(x, 28, 7, 9, "#fff", 0) for x in (12, 23, 34, 45))+path('M13 12 V18 M51 12 V18')+dot(13, 42)+dot(23, 42)),
        ("firewall", "ファイアウォール", "firewall fw 境界防御", lambda: rect(9, 11, 46, 43)+path('M9 25 H55 M9 40 H55 M24 11 V25 M41 11 V25 M17 25 V40 M34 25 V40 M49 25 V40 M24 40 V54 M41 40 V54')+arrow(2, 32, 8, 0)),
        ("load-balancer", "ロードバランサー", "load balancer lb 負荷分散", lambda: rect(5, 25, 14, 14)+path('M19 32 H31 M31 13 V51')+arrow(31, 13, 13, 0)+arrow(31, 32, 13, 0)+arrow(31, 51, 13, 0)+rect(46, 6, 12, 14)+rect(46, 25, 12, 14)+rect(46, 44, 12, 14)),
        ("vpn", "VPN", "vpn tunnel encrypted 専用接続 トンネル", lambda: rect(5, 22, 15, 22)+rect(44, 22, 15, 22)+path('M20 27 H44 M20 39 H44', stroke=ACCENT)+path('M27 27 V21 C27 14 37 14 37 21 V27')+rect(24, 27, 16, 16, "#fff")+dot(32, 34)),
        ("gateway", "ゲートウェイ", "gateway gw proxy プロキシ", lambda: rect(21, 7, 22, 50)+path('M27 15 H37 V49 H27 Z', "#fff")+arrow(3, 25, 25, 0)+arrow(36, 40, 25, 0)),
        ("internet", "インターネット", "internet wan globe 外部ネットワーク", lambda: circle(32, 32, 25)+path('M7 32 H57 M12 18 H52 M12 46 H52 M32 7 C16 17 16 47 32 57 C48 47 48 17 32 7 M32 7 V57')),
        ("wifi", "無線LAN", "wifi wireless wlan access point AP", lambda: path('M6 20 Q32-2 58 20 M13 29 Q32 12 51 29 M21 38 Q32 28 43 38', stroke=ACCENT, width=4)+dot(32, 48, 5)),
        ("dns", "DNS", "dns domain name resolver 名前解決", lambda: circle(20, 21, 13)+path('M7 21 H33 M20 8 C11 15 11 27 20 34 C29 27 29 15 20 8')+rect(31, 35, 27, 19)+arrow(22, 42, 9, 0)+path('M38 42 H51 M38 48 H46')),
        ("subnet", "サブネット", "subnet vlan segment lan セグメント", lambda: rect(5, 5, 54, 54, "#fff")+rect(11, 12, 16, 15)+rect(37, 12, 16, 15)+rect(24, 40, 16, 12)+path('M19 27 V33 H45 V27 M32 33 V40')),
    ])
    category("03 データ・連携", "#6457a3", "#eeeafa", [
        ("database", "データベース", "database db sql rdb データ管理", lambda: database()),
        ("data-warehouse", "データウェアハウス", "data warehouse dwh bi 集約", lambda: path('M5 24 L32 7 L59 24 V56 H5 Z', PALE)+path('M5 24 H59')+rect(14, 32, 14, 16, "#fff", 1)+rect(36, 32, 14, 16, "#fff", 1)+path('M14 40 H28 M36 40 H50')),
        ("data-lake", "データレイク", "data lake raw unstructured 非構造化", lambda: path('M7 23 L18 9 L29 19 L41 8 L57 23 V47 Q32 61 7 47 Z', PALE)+path('M7 31 Q20 23 32 31 T57 31 M7 41 Q20 33 32 41 T57 41 M17 49 Q32 43 47 49', stroke=ACCENT)),
        ("file-storage", "ファイルストレージ", "file storage nas folder 共有フォルダー", lambda: path('M7 15 H26 L32 22 H57 V52 H7 Z', PALE)+path('M7 27 H57')+rect(17, 33, 20, 13, "#fff", 1)+path('M23 38 H31 M23 42 H29')),
        ("backup", "バックアップ", "backup restore recovery 復元", lambda: database(7, 12, 30, 37)+path('M37 21 C55 16 64 39 49 49', stroke=ACCENT, width=3)+path('M49 40 V49 H58', stroke=ACCENT, width=3)+dot(19, 42)),
        ("object-storage", "オブジェクトストレージ", "object blob bucket オブジェクト", lambda: path('M10 14 H54 L49 55 H15 Z', PALE)+path('M10 14 Q32 4 54 14 Q32 24 10 14')+path('M25 29 L33 25 L41 29 V39 L33 43 L25 39 Z', "#fff")+path('M25 29 L33 33 L41 29 M33 33 V43')),
        ("event-stream", "イベントストリーム", "event stream queue message kafka メッセージ キュー", lambda: arrow(4, 15, 55, 0)+arrow(4, 49, 55, 0)+rect(8, 25, 10, 14)+rect(27, 25, 10, 14)+rect(46, 25, 10, 14)+path('M18 32 H27 M37 32 H46')),
        ("etl", "ETL・データ変換", "etl elt integration transform データ連携 変換", lambda: database(5, 19, 17, 30)+path('M24 12 H48 L39 28 V38 L32 43 V28 Z', PALE)+arrow(41, 45, 15, 0)+rect(49, 51, 10, 7, "#fff", 1)),
        ("data-mart", "データマート", "data mart department analytics 部門データ", lambda: database(6, 10, 23, 27)+path('M29 25 H38 V47')+database(37, 7, 20, 20)+database(37, 35, 20, 20)+path('M38 17 H37 M38 47 H37')),
        ("data-catalog", "データカタログ", "data catalog metadata lineage メタデータ", lambda: rect(9, 8, 42, 47)+path('M18 8 V55 M14 17 H22 M14 29 H22 M14 41 H22')+circle(37, 29, 8, "#fff")+path('M43 35 L52 44', stroke=ACCENT, width=3)),
    ])
    category("04 セキュリティ", "#287c63", "#e1f2e9", [
        ("identity", "ユーザー認証", "identity authentication login idp 認証 sso", lambda: rect(5, 10, 54, 44)+person(22, 31, .8)+check(43, 33)),
        ("key", "暗号鍵", "key encryption kms secret 暗号 秘密", lambda: circle(19, 23, 12)+circle(19, 23, 4, "#fff")+path('M28 32 L50 54 L57 47 L51 41 L47 45 L42 40 L46 36 L39 29 L35 33', PALE)),
        ("security-shield", "セキュリティ保護", "shield protection secure 防御", lambda: shield()+check(29, 33)),
        ("lock", "アクセス制限", "lock access control restriction acl ロック", lambda: path('M20 28 V19 C20 3 44 3 44 19 V28', stroke=ACCENT, width=4)+rect(13, 28, 38, 29)+circle(32, 40, 4, "#fff")+path('M32 44 V49')),
        ("certificate", "電子証明書", "certificate tls ssl pki 証明書", lambda: document()+circle(42, 45, 10, "#fff")+path('M36 53 L34 61 L42 57 L49 61 L48 53', PALE)+check(41, 44)),
        ("mfa", "多要素認証", "mfa otp two factor 2fa 二要素", lambda: rect(7, 13, 26, 38)+person(20, 29, .65)+rect(38, 7, 19, 49, "#fff")+path('M43 13 H52 M44 50 H51')+dot(44, 29)+dot(51, 29)+check(46, 37)),
        ("audit", "監査", "audit compliance evidence コンプライアンス 証跡", lambda: document()+circle(43, 42, 11, "#fff")+path('M50 50 L59 59', stroke=ACCENT, width=4)+check(42, 40)),
        ("vulnerability", "脆弱性", "vulnerability bug cve security risk 脆弱性診断", lambda: shield()+circle(32, 32, 9, "#fff")+path('M32 23 V41 M27 20 L24 16 M37 20 L40 16 M23 28 H18 M41 28 H46 M23 36 H18 M41 36 H46')),
        ("soc", "SOC・脅威監視", "soc siem threat security operations 脅威", lambda: monitor()+path('M11 27 L20 27 L25 18 L31 33 L37 23 L43 27 H53', stroke=ACCENT)+dot(45, 16, 3)),
        ("authorization", "権限・ロール", "authorization permission role iam rbac アクセス権", lambda: person(18, 24, .9)+path('M18 37 V47 H41')+rect(37, 11, 20, 15, "#fff")+rect(37, 39, 20, 15)+path('M41 47 V26')+check(46, 18)),
    ])
    category("05 開発・運用", "#367b9a", "#e3f0f6", [
        ("monitoring", "稼働監視", "monitoring observability health ヘルスチェック", lambda: monitor()+path('M12 28 H21 L27 17 L33 32 L40 23 L45 28 H53', stroke=ACCENT)),
        ("alert", "アラート", "alert notification alarm 通知 警報", lambda: path('M13 44 L18 37 V25 C18 7 46 7 46 25 V37 L51 44 Z', PALE)+path('M26 51 Q32 59 38 51')+path('M32 20 V31', stroke=ACCENT, width=3)+dot(32, 37)),
        ("logs", "ログ", "logs logging log 記録", lambda: rect(8, 7, 48, 50)+path('M8 19 H56 M15 28 H19 M25 28 H49 M15 37 H19 M25 37 H44 M15 46 H19 M25 46 H47')+dot(14, 13)),
        ("metrics", "メトリクス", "metrics performance gauge 性能 指標", lambda: path('M9 48 A25 25 0 1 1 55 48 Z', PALE)+path('M16 36 L20 38 M23 20 L25 25 M41 20 L39 25 M48 36 L44 38')+path('M32 43 L43 28', stroke=ACCENT, width=3)+circle(32, 43, 4, "#fff")),
        ("tracing", "分散トレーシング", "trace tracing span apm 分散追跡", lambda: path('M10 10 V54 M10 17 H17 M10 31 H25 M10 45 H36')+rect(17, 12, 35, 10)+rect(25, 26, 27, 10)+rect(36, 40, 20, 10)),
        ("scheduler", "ジョブスケジューラー", "scheduler cron job schedule 定期実行", lambda: rect(7, 11, 41, 43)+path('M7 23 H48 M17 6 V17 M37 6 V17')+circle(43, 43, 15, "#fff")+path('M43 33 V43 L50 47', stroke=ACCENT)+dot(17, 32)+dot(28, 32)),
        ("ci-cd", "CI・CD", "ci cd cicd pipeline deployment devops デプロイ", lambda: path('M30 20 C13-1 1 21 15 34 C29 47 35 17 49 30 C64 43 47 65 33 45', stroke=ACCENT, width=4)+path('M25 14 L30 20 L23 23 M39 41 L33 45 L40 50', stroke=ACCENT, width=3)+dot(12, 24, 3)+dot(52, 41, 3)),
        ("testing", "テスト・品質保証", "test qa quality verification 品質", lambda: rect(8, 9, 40, 46)+path('M16 20 H39 M16 30 H28 M16 40 H26')+circle(45, 43, 13, "#fff")+check(43, 42)),
        ("incident", "障害対応", "incident outage problem 障害 インシデント", lambda: path('M32 6 L60 54 H4 Z', PALE)+path('M32 23 V37', stroke=ACCENT, width=4)+dot(32, 45, 2.5)),
        ("helpdesk", "ヘルプデスク", "helpdesk support service desk サポート", lambda: person(32, 28, 1.2)+path('M12 31 V24 C12 0 52 0 52 24 V31', stroke=ACCENT, width=3)+rect(8, 25, 8, 16, "#fff")+rect(48, 25, 8, 16, "#fff")+path('M52 41 V46 H39')+dot(37, 46, 3)),
    ])
    category("06 組織・拠点", "#a16b30", "#fff0dc", [
        ("employee", "担当者", "person employee staff user 担当 社員", lambda: person(32, 29, 1.7)),
        ("team", "チーム", "team group organization 部署 組織", lambda: person(15, 32, .9)+person(49, 32, .9)+person(32, 34, 1.25)),
        ("headquarters", "本社", "headquarters office company enterprise 会社 オフィス", lambda: building()+path('M23 3 H41 M32 3 V8', stroke=ACCENT)),
        ("branch-office", "支店・営業所", "branch office site location 拠点", lambda: rect(6, 9, 21, 31)+rect(38, 31, 20, 27)+path('M12 17 H20 M12 24 H20 M44 40 H52 M44 47 H52 M16 40 V51 H38', stroke=ACCENT)),
        ("factory", "工場", "factory manufacturing plant 製造", lambda: path('M6 28 L21 18 V28 L36 18 V28 H56 V57 H6 Z', PALE)+path('M44 28 V7 H52 V28')+rect(13, 38, 8, 10, "#fff", 0)+rect(28, 38, 8, 10, "#fff", 0)+rect(43, 38, 7, 10, "#fff", 0)),
        ("store", "店舗", "store shop retail 小売 店", lambda: rect(9, 25, 46, 32)+path('M5 25 L12 9 H52 L59 25 Z', PALE)+path('M17 9 L14 25 M27 9 L26 25 M37 9 L38 25 M47 9 L50 25')+rect(16, 36, 15, 21, "#fff", 0)+rect(39, 36, 10, 10, "#fff", 0)),
        ("home-office", "在宅勤務", "home office remote work 在宅 テレワーク", lambda: path('M5 28 L32 6 L59 28 M12 23 V56 H52 V23', PALE)+rect(20, 30, 24, 16, "#fff")+path('M27 46 V51 M21 51 H43')),
        ("remote-site", "遠隔拠点", "remote site geographic location リモート 地域", lambda: path('M18 7 C-1 7-1 30 18 45 C37 30 37 7 18 7 Z', PALE)+circle(18, 20, 5, "#fff")+path('M47 26 C31 26 31 44 47 58 C63 44 63 26 47 26 Z', PALE)+circle(47, 37, 4, "#fff")+path('M14 49 Q23 62 32 48 Q35 40 37 43', stroke=ACCENT)),
        ("partner", "パートナー企業", "partner vendor supplier alliance 取引先 ベンダー", lambda: path('M5 22 L15 13 L28 19 L38 14 L59 28 L49 48 L37 52 L17 40 Z', PALE)+path('M15 13 L9 29 L17 40 M38 14 L27 27 L31 32 L39 26 L53 36 M21 35 L35 47 M27 31 L42 43', stroke=ACCENT)),
        ("customer", "顧客", "customer client consumer 顧客 利用者", lambda: person(24, 30, 1.3)+path('M48 10 L51 18 L59 18 L53 24 L55 32 L48 27 L41 32 L43 24 L37 18 L45 18 Z', PALE)),
    ])
    category("07 業務プロセス", "#a36b3e", "#fff0e2", [
        ("task", "作業・タスク", "task work activity 作業", lambda: rect(9, 10, 46, 45)+rect(18, 6, 28, 10, "#fff")+path('M19 28 H45 M19 37 H40 M19 46 H34')),
        ("workflow", "ワークフロー", "workflow process flow 業務フロー", lambda: rect(6, 8, 20, 14)+rect(38, 42, 20, 14)+path('M16 22 V34 H48')+arrow(48, 34, 0, 8)+path('M26 15 H48 V27', stroke=ACCENT)),
        ("approval", "承認", "approval signoff permission 決裁 稟議", lambda: document()+path('M31 48 V40 H48 V48 M28 48 H52 V55 H28 Z', "#fff")+check(38, 32)),
        ("decision", "判断・分岐", "decision branch condition 判断 分岐 条件", lambda: path('M32 5 L53 24 L32 43 L11 24 Z', PALE)+path('M32 43 V57 M11 24 H4 V48', stroke=ACCENT)+path('M28 18 Q28 11 35 14 Q42 18 33 23 V27', stroke=ACCENT)+dot(33, 33)),
        ("business-document", "業務文書", "document paper specification 文書 仕様書", lambda: document()+path('M23 47 H43 M23 53 H37', stroke=ACCENT)),
        ("form", "申請フォーム", "form request input 申請 入力", lambda: rect(8, 7, 48, 50)+rect(15, 15, 34, 8, "#fff", 1)+rect(15, 29, 13, 8, "#fff", 1)+rect(36, 29, 13, 8, "#fff", 1)+rect(31, 44, 18, 7, None, 1)+path('M16 47 H24')),
        ("contract", "契約", "contract agreement legal 契約書 合意", lambda: document()+path('M23 48 Q28 38 30 48 T38 48 T46 46', stroke=ACCENT, width=2.5)+path('M48 30 L55 25 L59 29 L49 39 L43 41 Z', "#fff")),
        ("checklist", "チェックリスト", "checklist todo completion 点検 完了", lambda: rect(9, 7, 46, 50)+path('M16 18 L19 21 L24 14 M16 32 L19 35 L24 28 M16 46 L19 49 L24 42', stroke=ACCENT)+path('M31 18 H47 M31 32 H47 M31 46 H47')),
        ("automation", "業務自動化", "automation rpa robot 自動化", lambda: rect(10, 17, 44, 34)+path('M32 8 V17 M24 57 H40 M19 51 V57 M45 51 V57 M5 28 V41 M59 28 V41')+dot(32, 7, 3)+circle(23, 30, 4, "#fff")+circle(41, 30, 4, "#fff")+path('M23 42 H41', stroke=ACCENT)),
        ("handover", "業務引き継ぎ", "handover transfer handoff 引継ぎ 移管", lambda: person(13, 28, .85)+person(51, 39, .85)+rect(26, 22, 13, 19, "#fff", 1)+arrow(20, 11, 26, 0)+arrow(44, 55, -26, 0)),
    ])
    category("08 プロジェクト", "#845c9f", "#f2e9f8", [
        ("roadmap", "ロードマップ", "roadmap planning plan 計画", lambda: path('M16 58 V46 Q16 33 32 33 Q49 33 49 20 V5', stroke=ACCENT, width=4)+circle(16, 48, 5, "#fff")+circle(32, 33, 5, "#fff")+circle(49, 16, 5, "#fff")),
        ("gantt", "ガントチャート", "gantt schedule timeline 工程表", lambda: rect(6, 8, 52, 48, "#fff")+path('M6 19 H58 M17 8 V56')+rect(21, 24, 19, 6, None, 1)+rect(29, 35, 24, 6, None, 1)+rect(40, 46, 13, 6, None, 1)),
        ("milestone", "マイルストーン", "milestone checkpoint flag 節目", lambda: path('M5 48 H59 M17 48 V8 M18 10 H48 L42 20 L48 30 H18', PALE)+circle(17, 48, 5, "#fff")+dot(39, 48, 3)),
        ("kanban", "カンバン", "kanban agile scrum board アジャイル", lambda: rect(5, 8, 54, 48, "#fff")+path('M23 8 V56 M41 8 V56 M5 19 H59')+rect(10, 25, 8, 8, None, 1)+rect(10, 39, 8, 8, None, 1)+rect(28, 25, 8, 8, None, 1)+rect(46, 25, 8, 8, None, 1)+check(49, 44)),
        ("resources", "リソース配分", "resource allocation staffing capacity 要員 工数", lambda: person(15, 19, .7)+person(48, 19, .7)+rect(6, 40, 20, 15)+rect(38, 40, 20, 15)+path('M15 30 V40 M48 30 V40 M26 47 H38', stroke=ACCENT)+dot(12, 47)+dot(20, 47)+dot(44, 47)),
        ("budget", "予算・コスト", "budget cost finance money 費用 予算", lambda: rect(6, 17, 38, 34)+path('M6 26 H44')+circle(43, 39, 15, "#fff")+path('M36 31 L43 40 L50 31 M43 40 V50 M36 40 H50 M36 45 H50', stroke=ACCENT)),
        ("risk", "リスク管理", "risk issue mitigation リスク 課題", lambda: rect(6, 8, 39, 47)+path('M14 18 H36 M14 27 H30 M14 36 H24')+path('M44 28 L60 56 H28 Z', "#fff")+path('M44 38 V45', stroke=ACCENT, width=3)+dot(44, 51)),
        ("meeting", "会議", "meeting workshop discussion ワークショップ 打合せ", lambda: rect(15, 24, 34, 20)+circle(32, 11, 5, "#fff")+circle(32, 56, 5, "#fff")+circle(6, 34, 4, "#fff")+circle(58, 34, 4, "#fff")+path('M32 16 V24 M32 44 V51 M10 34 H15 M49 34 H54')),
        ("dependencies", "依存関係", "dependency dependencies critical path wbs 依存", lambda: rect(5, 8, 17, 15)+rect(42, 8, 17, 15)+rect(23, 42, 18, 15)+path('M13 23 V32 H50 V23')+arrow(32, 32, 0, 10)),
        ("deliverable", "成果物", "deliverable release delivery 成果 納品", lambda: path('M7 23 L32 11 L57 23 V49 L32 61 L7 49 Z', PALE)+path('M7 23 L32 35 L57 23 M32 35 V61 M19 17 L44 29')+circle(46, 13, 10, "#fff")+check(44, 12)),
    ])
    category("09 戦略・ビジネス", "#a45e59", "#fceae5", [
        ("vision", "ビジョン", "vision future direction 将来像 方針", lambda: path('M4 32 Q32 1 60 32 Q32 63 4 32 Z', PALE)+circle(32, 32, 12, "#fff")+circle(32, 32, 5)+path('M32 4 V10 M32 54 V60', stroke=ACCENT)),
        ("kpi-target", "目標・KPI", "target goal kpi okr 目標", lambda: circle(30, 34, 24)+circle(30, 34, 16, "#fff")+circle(30, 34, 7)+arrow(56, 8, -26, 26)),
        ("swot", "SWOT分析", "swot strength weakness opportunity threat 強み 弱み 機会 脅威", lambda: rect(7, 7, 50, 50, "#fff")+path('M32 7 V57 M7 32 H57')+path('M19 13 V25 M13 19 H25 M39 19 H51', stroke=ACCENT, width=3)+path('M13 49 L19 37 L25 49 Z', PALE)+path('M40 40 L50 50 M50 40 L40 50', stroke=ACCENT, width=3)),
        ("customer-journey", "カスタマージャーニー", "customer journey experience cx touchpoint 顧客体験", lambda: person(11, 14, .55)+path('M8 36 C17 17 31 52 42 31 L55 16', stroke=ACCENT, width=3)+circle(12, 32, 4, "#fff")+circle(30, 38, 4, "#fff")+circle(46, 26, 4, "#fff")+path('M50 13 L56 14 L57 21', stroke=ACCENT)),
        ("value-chain", "バリューチェーン", "value chain supply chain 価値連鎖 サプライチェーン", lambda: path('M5 18 H20 L30 32 L20 46 H5 L15 32 Z', PALE)+path('M22 18 H37 L47 32 L37 46 H22 L32 32 Z', "#fff")+path('M39 18 H49 L59 32 L49 46 H39 L49 32 Z', PALE)),
        ("portfolio", "ポートフォリオ", "portfolio investment product business 事業 製品", lambda: rect(6, 16, 52, 41)+path('M21 16 V7 H43 V16 M6 31 H58')+rect(26, 27, 12, 9, "#fff", 1)+path('M16 43 H25 M16 49 H35', stroke=ACCENT)),
        ("transformation", "変革・DX", "transformation dx change digital デジタル変革", lambda: rect(5, 16, 18, 30)+path('M10 24 H18 M10 31 H18 M10 38 H18')+arrow(24, 31, 16, 0)+path('M43 13 L57 20 V43 L43 50 L33 44 V37', PALE)+path('M43 29 L57 20 M43 29 V50')),
        ("business-model", "ビジネスモデル", "business model canvas revenue model 収益モデル", lambda: rect(5, 10, 54, 44, "#fff")+path('M18 10 V42 M31 10 V42 M45 10 V42 M5 42 H59 M31 26 H59 M31 42 V54')+dot(11, 25, 3)+dot(24, 25, 3)+dot(38, 18, 3)+dot(51, 34, 3)),
        ("competitive-analysis", "競合比較", "competitive competitor benchmark positioning 競合 ベンチマーク", lambda: path('M11 54 V12 H53 M11 54 H57 M11 54 L49 16', stroke=ACCENT)+circle(25, 37, 6)+circle(42, 39, 4, "#fff")+circle(41, 22, 7, "#fff")),
        ("stakeholders", "ステークホルダー", "stakeholder influence interest relationship 利害関係者", lambda: circle(32, 32, 10)+path('M32 22 V13 M22 32 H13 M42 32 H51 M32 42 V51')+person(32, 7, .4)+person(7, 31, .4)+person(57, 31, .4)+person(32, 55, .4)),
    ])
    category("10 分析・報告", "#416b9c", "#e8eefb", [
        ("report", "レポート", "report reporting presentation 資料 報告書", lambda: document()+path('M24 52 V45 H29 V52 M34 52 V39 H39 V52', PALE)),
        ("dashboard", "ダッシュボード", "dashboard analytics cockpit 分析画面", lambda: rect(5, 8, 54, 48, "#fff")+rect(11, 14, 18, 15)+rect(35, 14, 18, 15)+rect(11, 35, 42, 15)+path('M16 44 L24 39 L32 45 L41 40 L48 43', stroke=ACCENT)),
        ("bar-chart", "棒グラフ", "bar chart histogram 棒グラフ 比較", lambda: chart_axes()+rect(17, 33, 8, 21, None, 1)+rect(31, 23, 8, 31, None, 1)+rect(45, 12, 8, 42, None, 1)),
        ("line-chart", "折れ線グラフ", "line chart trend time series 傾向 推移", lambda: chart_axes()+path('M16 43 L27 29 L37 36 L52 17', stroke=ACCENT, width=3)+dot(16, 43, 3)+dot(27, 29, 3)+dot(37, 36, 3)+dot(52, 17, 3)),
        ("pie-chart", "円グラフ", "pie chart share ratio 構成比", lambda: path('M29 9 A24 24 0 1 0 53 33 H29 Z', PALE)+path('M35 5 V27 H57 A22 22 0 0 0 35 5 Z', "#fff")+path('M29 33 L13 51', stroke=ACCENT)),
        ("scatter-plot", "散布図", "scatter plot correlation distribution 相関 分布", lambda: chart_axes()+''.join(dot(x, y, r) for x, y, r in [(18, 43, 3), (23, 34, 3), (31, 37, 3), (34, 26, 3), (45, 21, 4), (49, 13, 3), (51, 31, 3)])),
        ("funnel", "ファネル", "funnel conversion pipeline sales コンバージョン", lambda: path('M5 9 H59 L49 23 H15 Z', PALE)+path('M17 28 H47 L39 40 H25 Z', "#fff")+path('M27 45 H37 V57 H27 Z', PALE)),
        ("matrix", "評価マトリクス", "matrix evaluation prioritization priority 評価 優先順位", lambda: rect(7, 7, 50, 50, "#fff")+path('M7 23 H57 M7 40 H57 M23 7 V57 M40 7 V57')+circle(31, 31, 4)+check(47, 31)+dot(31, 48, 3)),
        ("insight", "インサイト・仮説", "insight hypothesis idea innovation 知見 仮説 アイデア", lambda: path('M21 43 C21 35 12 32 14 21 C17 1 47 1 50 21 C52 32 43 35 43 43 Z', PALE)+path('M23 49 H41 M26 55 H38 M29 43 V30 L23 25 M35 43 V30 L41 25', stroke=ACCENT)),
        ("survey", "アンケート・調査", "survey research questionnaire interview 調査 ヒアリング", lambda: rect(6, 8, 37, 47)+path('M13 19 H22 M13 29 H22 M13 39 H22')+rect(28, 15, 8, 8, "#fff", 1)+rect(28, 25, 8, 8, "#fff", 1)+path('M38 34 H59 V50 H50 L43 58 V50 H38 Z', PALE)+dot(44, 42)+dot(51, 42)),
    ])
    category("11 モバイル端末", "#287e91", "#e4f4f8", [
        ("smartphone", "スマートフォン", "smartphone mobile phone スマホ 携帯電話", lambda: phone()+rect(24, 20, 7, 7, "#fff", 1)+rect(34, 20, 7, 7, "#fff", 1)+rect(24, 31, 7, 7, "#fff", 1)+rect(34, 31, 7, 7, "#fff", 1)),
        ("android-phone", "Androidスマートフォン", "android アンドロイド smartphone スマートフォン スマホ", lambda: phone()+inset(robot(), 20, 16, .38)),
        ("iphone", "iPhone", "iphone ios apple アイフォン アイフォーン スマートフォン スマホ", lambda: phone(True)+circle(32, 30, 8, "#fff")+path('M27 32 Q32 22 37 32 M29 37 H35', stroke=ACCENT)),
        ("tablet", "タブレット", "tablet slate タブレット端末", lambda: phone(wide=True)+path('M18 23 H45 M18 29 H40 M18 35 H43', stroke=ACCENT)),
        ("android-tablet", "Androidタブレット", "android tablet アンドロイド タブレット", lambda: phone(wide=True)+inset(robot(), 15, 12, .53)),
        ("ipad", "iPad", "ipad ipados apple アイパッド タブレット", lambda: phone(True, True)+path('M19 40 L31 23 L40 34 L46 28 M19 43 H45', stroke=ACCENT)+dot(22, 19)),
        ("smartwatch", "スマートウォッチ", "smartwatch wearable watch 腕時計 ウェアラブル", lambda: rect(23, 2, 18, 60, "#fff", 6)+rect(14, 16, 36, 32, radius=9)+path('M50 26 H54 V34 H50 M32 23 V32 L39 36', stroke=ACCENT)+circle(32, 32, 12, "none")),
        ("feature-phone", "フィーチャーフォン", "feature phone keypad ガラケー 携帯電話", lambda: rect(17, 4, 30, 56, "#fff", 5)+rect(22, 12, 20, 20)+''.join(dot(x, y) for y in (39, 46, 53) for x in (24, 32, 40))),
        ("rugged-handheld", "業務用ハンディ端末", "rugged handheld barcode scanner PDA ハンディ 倉庫", lambda: rect(14, 4, 36, 55, radius=5)+rect(21, 12, 22, 27, "#fff", 1)+path('M25 17 V30 M29 17 V30 M35 17 V30 M39 17 V30', stroke=ACCENT)+rect(21, 45, 9, 7, "#fff", 1)+rect(35, 45, 9, 7, "#fff", 1)),
        ("foldable-phone", "折りたたみスマートフォン", "foldable folding dual screen 折りたたみ 二画面 スマホ", lambda: path('M7 11 L30 5 L57 11 V55 L33 60 L7 54 Z', "#fff")+path('M30 5 L33 60 M12 18 L25 15 V47 L12 44 Z M38 15 L52 18 V47 L39 50 Z', PALE)),
    ])
    category("12 モバイル機能・通信", "#326b9f", "#e8f0fc", [
        ("mobile-app", "モバイルアプリ", "mobile app application スマホアプリ", lambda: phone()+rect(23, 20, 18, 20, "#fff", 5)+path('M28 30 L31 34 L37 26', stroke=ACCENT)),
        ("app-distribution", "アプリ配信", "app store distribution install download アプリストア インストール", lambda: phone()+path('M32 18 V36 M26 30 L32 36 L38 30 M24 41 H40', stroke=ACCENT, width=3)),
        ("push-notification", "プッシュ通知", "push notification alert 通知 お知らせ", lambda: phone()+path('M24 36 V30 Q24 20 32 20 Q40 20 40 30 V36 Z', "#fff")+dot(32, 40)+circle(47, 13, 10, "#fff")+dot(47, 13, 4)),
        ("mobile-payment", "スマホ決済", "mobile payment wallet cashless 電子マネー キャッシュレス", lambda: phone()+rect(25, 23, 33, 22, "#fff", 3)+path('M25 30 H58 M43 36 L47 41 L53 34', stroke=ACCENT)),
        ("qr-scanner", "QRコード読み取り", "qr code scan scanner 二次元コード QRコード", lambda: path('M6 21 V7 H20 M44 7 H58 V21 M58 43 V57 H44 M20 57 H6 V43', stroke=ACCENT, width=3)+rect(15, 16, 12, 12, "#fff", 0)+rect(37, 16, 12, 12, "#fff", 0)+rect(15, 38, 12, 12, "#fff", 0)+path('M38 37 H44 V43 H50 V50 H37 V45 M19 20 H23 V24 H19 Z M41 20 H45 V24 H41 Z', PALE)),
        ("phone-biometrics", "生体認証端末", "biometric fingerprint face id 指紋 顔認証 スマホ", lambda: phone()+path('M23 31 C23 16 41 16 41 31 M27 33 C25 21 38 20 37 31 L35 40 M31 29 Q34 24 33 33 L30 42 M23 35 L25 40', stroke=ACCENT)),
        ("nfc-phone", "NFC・近距離通信", "nfc contactless tap 非接触 近距離通信 タッチ", lambda: inset(phone(), -7, 0, 1)+path('M41 21 Q53 32 41 43 M48 15 Q65 32 48 49', stroke=ACCENT, width=3)+dot(38, 32, 3)),
        ("mobile-location", "スマホ位置情報", "mobile location map geolocation 位置情報 地図", lambda: phone()+path('M32 18 C18 18 21 34 32 43 C43 34 46 18 32 18 Z', "#fff")+circle(32, 26, 4)),
        ("mobile-5g", "5G通信", "5g cellular wireless mobile 第五世代 移動体通信", lambda: path('M32 27 V58 M22 58 H42 M15 42 Q3 28 16 14 M49 42 Q61 28 48 14 M23 37 Q15 28 23 21 M41 37 Q49 28 41 21', stroke=ACCENT)+circle(32, 28, 4, "#fff")+tag('5G', 32, 5)),
        ("mobile-device-management", "モバイル端末管理", "mdm emm mobile device management 端末管理", lambda: inset(phone(), -9, 0, .9)+inset(shield(), 27, 26, .52)+inset(check(), 29, 26, .52)+path('M39 11 H54 V23', stroke=ACCENT)),
    ])
    category("13 開発言語", "#aa6933", "#fff0df", [
        ("java", "Java", "java jvm openjdk ジャバ 開発言語", lambda: path('M13 26 H45 V39 Q45 50 29 50 Q13 50 13 39 Z', PALE)+path('M45 28 H51 Q62 36 45 41 M10 54 H49 M22 20 C13 13 31 13 24 5 M34 21 C26 13 44 13 37 4', stroke=ACCENT)+tag('JAVA', 32, 45)),
        ("rust", "Rust", "rust rustlang cargo ラスト 開発言語", lambda: gear(32, 27, 17)+path('M19 36 L11 41 L7 34 M45 36 L53 41 L57 34 M18 23 L10 16 M46 23 L54 16', stroke=ACCENT)+circle(27, 25, 2, "#fff")+circle(37, 25, 2, "#fff")+tag('RUST', 32, 47)),
        ("python", "Python", "python py pip パイソン 開発言語", lambda: rect(7, 7, 50, 43, "#fff")+path('M7 17 H57 M21 26 L15 32 L21 38 M43 26 L49 32 L43 38 M35 23 L29 41', stroke=ACCENT)+nameplate('Python')),
        ("javascript", "JavaScript", "javascript js ecmascript node nodejs ジャバスクリプト", lambda: rect(7, 7, 50, 45)+path('M22 23 L15 30 L22 37 M42 23 L49 30 L42 37', stroke=ACCENT)+nameplate('JavaScript')),
        ("typescript", "TypeScript", "typescript ts deno タイプスクリプト 開発言語", lambda: rect(7, 7, 50, 45, "#fff")+path('M20 24 L14 30 L20 36 M42 24 L48 30 L42 36', stroke=ACCENT)+check(31, 30)+nameplate('TypeScript')),
        ("csharp", "C#", "c# csharp dotnet .net シーシャープ 開発言語", lambda: path('M32 5 L56 18 V45 L32 58 L8 45 V18 Z', PALE)+word('C#', 32, 23, 1.8)+path('M19 45 H45', stroke=ACCENT)),
        ("golang", "Go", "go golang gopher ゴー 開発言語", lambda: path('M8 17 H28 M4 25 H22 M8 33 H20', stroke=ACCENT, width=3)+rect(24, 8, 32, 46, "#fff", 13)+circle(32, 20, 6)+circle(47, 20, 6)+dot(33, 20)+dot(46, 20)+tag('GO', 39, 39)),
        ("kotlin", "Kotlin", "kotlin kt android コトリン 開発言語", lambda: rect(8, 7, 48, 49, "#fff")+path('M15 14 H48 L32 31 L48 49 H15 Z', PALE)+path('M15 14 L32 31 L15 49', stroke=ACCENT)+nameplate('Kotlin')),
        ("swift", "Swift", "swift ios apple スウィフト 開発言語", lambda: circle(32, 29, 24)+path('M13 22 L35 34 L23 14 L49 35 L43 44 L22 42 Z', "#fff")+nameplate('Swift')),
        ("cobol", "COBOL", "cobol mainframe legacy コボル 汎用機 開発言語", lambda: path('M8 9 H49 L57 17 V48 H8 Z', PALE)+''.join(rect(x, y, 4, 6, "#fff", 0) for x, y in [(15, 18), (24, 18), (42, 18), (15, 29), (33, 29), (42, 29)])+tag('COBOL', 32, 46)),
    ])
    category("14 データベース製品", "#6157a2", "#efebfa", [
        ("postgresql", "PostgreSQL", "postgresql postgres pgsql ポスグレ ポストグレス データベース", lambda: elephant()+nameplate("PostgreSQL")),
        ("mysql", "MySQL", "mysql マイエスキューエル データベース", lambda: dolphin()+nameplate("MySQL")),
        ("mariadb", "MariaDB", "mariadb maria マリアデービー データベース", lambda: database()+path('M17 32 Q29 21 40 29 L49 24 L46 36 L37 36 L31 41 L27 35 Z', "#fff")+nameplate('MariaDB')),
        ("sqlite", "SQLite", "sqlite embedded local エスキューライト 組込 データベース", lambda: database(12, 10, 35, 42)+path('M30 38 Q34 17 54 7 Q53 27 35 40 L28 47 M35 35 L44 18', "#fff")+nameplate('SQLite')),
        ("sql-server", "SQL Server", "sql server mssql microsoft マイクロソフト データベース", lambda: database()+path('M19 23 L44 31 L19 40 L44 24 M19 31 H44', stroke=ACCENT)+nameplate('SQL Server')),
        ("oracle-database", "Oracle Database", "oracle database db オラクル データベース", lambda: database()+rect(20, 25, 24, 13, "#fff", 6)+nameplate('Oracle Database')),
        ("redis", "Redis", "redis cache key value レディス キャッシュ データベース", lambda: path('M7 38 L32 27 L57 38 L32 50 Z M7 30 L32 19 L57 30 L32 42 Z M7 22 L32 11 L57 22 L32 34 Z', PALE)+path('M22 22 H42 M32 17 V27', stroke=ACCENT)+nameplate('Redis')),
        ("mongodb", "MongoDB", "mongodb mongo nosql document モンゴ データベース", lambda: database()+path('M32 19 Q15 32 32 42 Q49 32 32 19 Z', "#fff")+path('M32 25 V44', stroke=ACCENT)+nameplate('MongoDB')),
        ("elasticsearch", "Elasticsearch", "elasticsearch elastic search エラスティック 全文検索 データベース", lambda: database()+circle(31, 29, 9, "#fff")+path('M37 36 L47 44 M26 29 H36 M31 24 V34', stroke=ACCENT)+nameplate('Elasticsearch')),
        ("database-replication", "データベースレプリケーション", "database replication replica primary standby 同期 複製 レプリカ", lambda: database(4, 6, 25, 31)+database(35, 27, 25, 31)+arrow(35, 12, 19, 11)+arrow(29, 52, -20, -10)),
    ])
    category("15 AI・機械学習", "#8653a0", "#f4eafa", [
        ("artificial-intelligence", "AI・人工知能", "ai artificial intelligence 人工知能 エーアイ", lambda: brain()+tag('AI', 32, 46)),
        ("generative-ai", "生成AI", "generative ai genai 生成 人工知能", lambda: inset(brain(), 2, 10, .75)+path('M49 4 L52 13 L61 16 L52 19 L49 28 L46 19 L37 16 L46 13 Z', "#fff")+path('M51 40 V52 M45 46 H57', stroke=ACCENT, width=3)),
        ("large-language-model", "大規模言語モデル", "llm large language model 生成ai 言語モデル", lambda: path('M6 13 Q19 5 32 14 Q45 5 58 13 V49 Q45 41 32 50 Q19 41 6 49 Z', PALE)+path('M32 14 V50 M12 22 H25 M12 29 H25 M39 22 H52 M39 29 H52')+tag('LLM', 32, 46)),
        ("ai-chatbot", "AIチャットボット", "ai chatbot chat bot 対話 チャットボット", lambda: bubble()+rect(19, 17, 26, 19, "#fff", 5)+dot(26, 25)+dot(38, 25)+path('M27 31 H37 M32 12 V17', stroke=ACCENT)),
        ("ai-agent", "AIエージェント", "ai agent autonomous assistant 自律 エージェント", lambda: inset(robot(), 0, 3, .8)+inset(gear(), 30, 30, .5)+arrow(38, 9, 18, 0)+path('M56 9 V21', stroke=ACCENT)),
        ("neural-network", "ニューラルネットワーク", "neural network deep learning ニューラル 深層学習 ai", lambda: network()),
        ("machine-learning", "機械学習・モデル学習", "ml machine learning training model 機械学習 学習 ai", lambda: inset(database(), -2, 15, .6)+inset(brain(), 27, 1, .55)+arrow(22, 26, 18, -10)+path('M46 42 L40 53 H21', stroke=ACCENT)+check(47, 47)),
        ("image-recognition", "画像認識", "computer vision image recognition ai 画像認識 検出", lambda: rect(8, 10, 48, 43, "#fff")+path('M15 43 L27 29 L37 39 L46 26 L51 43 Z', PALE)+circle(21, 22, 4)+path('M3 20 V5 H18 M46 5 H61 V20 M61 44 V59 H46 M18 59 H3 V44', stroke=ACCENT)),
        ("speech-recognition", "音声認識", "speech voice recognition stt asr ai 音声 文字起こし", lambda: rect(23, 6, 18, 30, "#fff", 9)+path('M16 25 V31 Q16 47 32 47 Q48 47 48 31 V25 M32 47 V58 M23 58 H41 M4 24 V34 M10 20 V38 M54 20 V38 M60 24 V34', stroke=ACCENT)),
        ("rag-retrieval", "RAG・検索拡張生成", "rag retrieval augmented generation vector embedding ai 検索拡張生成 ベクトル", lambda: inset(database(), -4, 3, .65)+inset(document(), 31, 21, .55)+arrow(22, 33, 22, 0)+circle(46, 16, 11, "#fff")+path('M41 16 L45 20 L52 12', stroke=ACCENT)),
    ])
    category("16 顧客対応・メール", "#247d7b", "#e0f4f0", [
        ("call-center", "コールセンター", "call center contact centre コールセンター コンタクトセンター 電話", lambda: inset(building(), 10, 5, .67)+headset()+path('M22 55 H42', stroke=ACCENT)),
        ("call-operator", "電話オペレーター", "operator agent receptionist オペレーター 電話受付", lambda: adult()+inset(headset(), 4, -2, .87)),
        ("interactive-voice-response", "自動音声応答・IVR", "ivr interactive voice response auto attendant 自動音声 応答", lambda: rect(23, 5, 18, 17)+path('M32 22 V33 M11 33 H53 M11 33 V44 M32 33 V44 M53 33 V44', stroke=ACCENT)+circle(11, 51, 7, "#fff")+circle(32, 51, 7, "#fff")+circle(53, 51, 7, "#fff")+word('IVR', 32, 9, .55)),
        ("call-queue", "着信キュー", "call queue acd waiting routing 着信待ち 呼分配", lambda: inset(headset(), 0, 0, .67)+path('M9 51 H55', stroke=ACCENT)+circle(16, 51, 6, "#fff")+circle(34, 51, 6, "#fff")+circle(52, 51, 6, "#fff")+arrow(44, 20, 13, 0)),
        ("computer-telephony", "CTI・電話連携", "cti computer telephony integration 電話連携", lambda: inset(monitor(), -3, -3, .8)+inset(phone(), 31, 22, .58)+path('M26 45 V55 H41', stroke=ACCENT)),
        ("customer-chat", "チャット窓口", "chat messaging live support チャット メッセージ", lambda: bubble()+dot(20, 26, 3)+dot(32, 26, 3)+dot(44, 26, 3)),
        ("email", "メール", "email e-mail mail メール 電子メール Eメール", lambda: envelope()+path('M22 13 L32 5 L42 13', stroke=ACCENT)),
        ("mail-server", "メールサーバー", "mail server smtp imap pop3 メールサーバ 配信", lambda: server(7, 5, 42, 14)+server(7, 22, 42, 14)+inset(envelope(), 22, 26, .62)+dot(12, 46, 3)),
        ("mail-newsletter", "メルマガ・一斉配信", "newsletter bulk mail mailing campaign メルマガ 一斉送信", lambda: inset(envelope(), 15, -1, .52)+inset(envelope(), 0, 22, .52)+inset(envelope(), 30, 22, .52)+path('M32 27 V34 M16 34 H48 M16 34 V39 M48 34 V39', stroke=ACCENT)),
        ("video-conference", "ビデオ会議", "video conference meeting webinar ビデオ会議 ウェビナー", lambda: monitor()+person(23, 26, .7)+person(42, 26, .7)+path('M14 48 H20 M44 48 H50', stroke=ACCENT)),
    ])
    category("17 人物・ライフステージ", "#b27745", "#fff0e4", [
        ("baby", "赤ちゃん", "baby infant newborn 赤ちゃん 乳児 ベビー", lambda: circle(32, 25, 18, "#fff")+path('M29 9 Q24 2 34 5 Q40 8 33 12 M20 48 Q32 37 44 48 L50 59 H14 Z', PALE)+dot(26, 24)+dot(38, 24)+circle(32, 33, 4)),
        ("child", "子供", "child children kid 子供 子ども こども 児童", lambda: circle(32, 20, 10, "#fff")+path('M23 14 Q31 5 41 14 M22 34 L14 43 M42 34 L50 43 M23 32 H41 V49 H23 Z M27 49 V59 M37 49 V59', PALE)+dot(28, 20, 1)+dot(36, 20, 1)+path('M29 25 Q32 28 35 25', stroke=ACCENT)),
        ("school-student", "学生", "student school pupil 学生 生徒 児童 学校", lambda: adult("student")),
        ("teenager", "若者・青少年", "teenager youth young 青少年 若者 ティーン", lambda: adult("teen")),
        ("adult-person", "成人", "adult person grownup 大人 成人", lambda: adult()+path('M22 33 L32 40 L42 33', stroke=ACCENT)),
        ("senior-person", "高齢者", "senior elderly older person aged 老人 高齢者 お年寄り シニア", lambda: adult("senior")),
        ("senior-couple", "高齢者夫婦", "senior elderly couple 老夫婦 高齢者 世帯 老人", lambda: inset(adult("senior"), -7, 13, .75)+inset(adult("senior"), 25, 13, .75)+path('M25 48 Q32 54 39 48', stroke=ACCENT)),
        ("family", "家族", "family household parents children 家族 世帯 親子 子供", lambda: inset(adult(), -5, 8, .8)+inset(adult(), 21, 8, .8)+person(32, 44, .85)+path('M24 58 H40', stroke=ACCENT)),
        ("caregiver", "介護・医療スタッフ", "caregiver nurse medical care 介護 看護師 介護士", lambda: adult("care")),
        ("wheelchair-user", "車椅子利用者", "wheelchair accessibility disability 車椅子 車いす バリアフリー", lambda: circle(29, 10, 6, "#fff")+circle(25, 42, 17)+path('M29 20 V36 H43 L51 52 H59 M29 25 H44 M19 41 H32 M29 36 L26 44', stroke=ACCENT, width=3)),
    ])
    category("18 ファイル形式・帳票", "#5a7594", "#e9f0f8", [
        ("fixed-length-file", "固定長ファイル", "fixed length file fixed-width record flat file 固定長 固定幅 レコード", lambda: file_rows('FIX')),
        ("csv-file", "CSVファイル", "csv comma separated values delimited カンマ区切り CSV ファイル", lambda: file_rows('CSV')),
        ("tsv-file", "TSVファイル", "tsv tab separated values delimited タブ区切り TSV ファイル", lambda: file_rows('TSV')),
        ("json-file", "JSONファイル", "json javascript object notation ジェイソン ファイル", lambda: file_rows('JSON')),
        ("xml-file", "XMLファイル", "xml markup structured エックスエムエル ファイル", lambda: file_rows('XML')),
        ("pdf-file", "PDFファイル", "pdf portable document 電子文書 ファイル", lambda: file_rows('PDF')+path('M24 29 L31 21 L39 34 L25 37 Z', "#fff")),
        ("spreadsheet-file", "表計算ファイル", "xlsx xls excel spreadsheet workbook エクセル 表計算 ファイル", lambda: file_rows('XLS')+path('M22 24 H44 M22 30 H44 M22 36 H44 M28 22 V40 M37 22 V40', stroke=ACCENT)),
        ("business-report", "帳票", "report form business statement 帳票 定型帳票 報告", lambda: document()+rect(22, 24, 21, 15, "#fff", 0)+path('M22 29 H43 M22 34 H43 M29 24 V39 M37 24 V39 M23 46 H43 M34 52 H43', stroke=ACCENT)),
        ("invoice", "請求書", "invoice billing bill receipt 請求書 明細 帳票", lambda: document()+path('M23 22 H43 M23 29 H43 M23 36 H43 M35 21 V40', stroke=ACCENT)+circle(43, 48, 11, "#fff")+path('M38 42 L43 48 L48 42 M43 48 V55 M38 49 H48', stroke=ACCENT)),
        ("report-printer", "帳票印刷", "printer print spool report 帳票 印刷 プリンター", lambda: rect(17, 4, 30, 19, "#fff", 0)+rect(6, 21, 52, 29)+rect(17, 36, 30, 24, "#fff", 0)+path('M23 43 H41 M23 49 H41 M23 55 H35')+dot(49, 29, 3)),
    ])
    category("19 広告・メディア", "#a65269", "#fde9ef", [
        ("web-advertising", "Web広告", "web advertising advertisement ads online internet WEB広告 ウェブ広告 ネット広告", lambda: ad_frame()+rect(12, 24, 40, 22)+word('AD', 32, 29, 1.2)+path('M49 42 L60 48 L54 50 L52 57 Z', "#fff")),
        ("display-advertising", "ディスプレイ広告", "display banner advertising ad バナー ディスプレイ広告", lambda: ad_frame()+rect(11, 24, 29, 23)+rect(44, 24, 10, 23)+path('M16 40 L23 29 L28 36 L34 30 L37 40 Z', "#fff")),
        ("search-advertising", "検索広告", "search advertising paid sem ppc listing 検索広告 リスティング", lambda: ad_frame()+rect(11, 24, 42, 9)+circle(17, 28, 2, "#fff")+path('M19 30 L22 33 M14 41 H43 M14 47 H35', stroke=ACCENT)+tag('AD', 46, 43)),
        ("video-advertising", "動画広告", "video advertising preroll commercial 動画広告 CM", lambda: ad_frame()+path('M26 25 L42 35 L26 45 Z', PALE)+tag('AD', 48, 47)),
        ("social-advertising", "SNS広告", "social media sns advertising influencer SNS広告 ソーシャル", lambda: rect(12, 5, 40, 54, "#fff", 7)+circle(22, 15, 4)+path('M31 13 H44 M18 24 H46 V43 H18 Z', PALE)+path('M21 50 H34', stroke=ACCENT)+path('M41 48 Q45 43 48 48 Q51 53 41 57 Q31 53 34 48 Q37 43 41 48 Z', "#fff")),
        ("television", "テレビ", "television tv broadcast テレビ 受像機 放送", lambda: rect(5, 16, 54, 37, radius=5)+rect(11, 22, 35, 24, "#fff", 3)+path('M20 4 L32 16 L44 4 M17 53 L12 60 M47 53 L52 60')+circle(52, 28, 2, "#fff")+circle(52, 39, 2, "#fff")),
        ("radio", "ラジオ", "radio audio broadcast am fm ラジオ 音声放送", lambda: rect(5, 20, 54, 35)+path('M14 20 L47 4 M12 27 H51 M30 34 V47 M36 34 V47 M42 34 V47 M48 34 V47', stroke=ACCENT)+circle(17, 40, 7, "#fff")),
        ("newspaper", "新聞", "newspaper news press print 新聞 ニュース 紙媒体", lambda: path('M10 8 H57 V53 Q57 59 50 59 H12 Q5 59 5 52 V19 H10 Z', "#fff")+path('M10 8 V52 Q10 59 17 59 M17 17 H49 M17 24 H49 M35 32 H49 M35 39 H49 M17 49 H49')+rect(17, 31, 12, 12)),
        ("digital-signage", "デジタルサイネージ", "digital signage outdoor ooh dooh 電子看板 サイネージ", lambda: rect(11, 4, 42, 43, "#fff")+rect(17, 10, 30, 30)+path('M23 47 V59 M41 47 V59 M15 60 H49 M22 33 L30 20 L41 33 Z', PALE)+dot(23, 17, 3)),
        ("web-ad-analytics", "広告効果測定", "advertising analytics conversion ctr roas web 広告効果 計測", lambda: ad_frame()+path('M14 44 V29 M25 44 V36 M36 44 V24', stroke=ACCENT, width=5)+circle(46, 44, 10, "#fff")+path('M53 51 L60 58 M42 44 H50 M46 40 V48', stroke=ACCENT)),
    ])
    category("20 衛星・通信インフラ", "#47749c", "#e5f1fb", [
        ("satellite", "人工衛星", "satellite space orbital 人工衛星 衛星 宇宙", lambda: satellite()),
        ("satellite-dish", "衛星アンテナ", "satellite dish antenna parabolic パラボラ 衛星アンテナ", lambda: dish()),
        ("ground-station", "衛星地上局", "satellite ground station earth station teleport 衛星 地上局", lambda: rect(4, 34, 31, 25)+path('M9 42 H16 M23 42 H30 M17 59 V49 H24 V59')+inset(dish(), 24, 1, .6)+path('M43 31 V59', stroke=ACCENT)),
        ("gps-satellite", "GPS・測位衛星", "gps gnss navigation satellite 位置 測位衛星 衛星", lambda: inset(satellite(), -1, -4, .65)+path('M44 29 C23 29 26 48 44 60 C62 48 65 29 44 29 Z', "#fff")+circle(44, 39, 5)+path('M31 21 L40 30', stroke=ACCENT)),
        ("earth-observation", "地球観測", "earth observation remote sensing satellite リモートセンシング 衛星 地球観測", lambda: circle(26, 39, 21)+path('M6 39 H46 M26 18 Q12 39 26 60 Q40 39 26 18', stroke=ACCENT)+inset(satellite(), 27, -4, .5)+path('M40 21 L31 31 M49 24 L38 35', stroke=ACCENT)),
        ("weather-satellite", "気象衛星", "weather satellite meteorology forecast 気象衛星 天気 予報", lambda: inset(satellite(), -1, -4, .64)+path('M22 53 C10 56 7 40 17 37 C16 22 39 21 42 35 C59 31 66 52 52 54 Z', "#fff")+path('M25 57 L22 62 M38 57 L35 62 M51 57 L48 62', stroke=ACCENT)),
        ("rocket", "ロケット", "rocket launch space vehicle ロケット 打ち上げ 宇宙", lambda: path('M23 43 V23 Q23 10 32 3 Q41 10 41 23 V43 Z', PALE)+path('M23 28 L13 42 V53 L23 47 M41 28 L51 42 V53 L41 47 M23 19 H41 M27 48 L25 60 L32 54 L39 60 L37 48', "#fff")+circle(32, 30, 5)),
        ("broadcast-tower", "放送塔", "broadcast tower tv television transmitter テレビ 放送塔 電波塔", lambda: path('M32 17 L14 59 M32 17 L50 59 M21 42 H43 M17 52 H47 M24 35 H40 M26 29 L44 52 M38 29 L20 52', stroke=ACCENT)+circle(32, 13, 4, "#fff")+path('M21 5 Q13 13 21 21 M43 5 Q51 13 43 21 M13 2 Q1 13 13 25 M51 2 Q63 13 51 25', stroke=ACCENT)),
        ("submarine-cable", "海底ケーブル", "submarine subsea underwater fiber cable 海底ケーブル 光ケーブル", lambda: path('M3 13 Q10 6 17 13 T31 13 T45 13 T59 13 M3 23 Q10 16 17 23 T31 23 T45 23 T59 23', stroke=ACCENT)+path('M4 59 Q17 52 27 57 Q40 48 60 57', PALE)+path('M5 33 C22 33 16 51 32 46 S43 31 59 35', stroke=ACCENT, width=5)+circle(32, 46, 7, "#fff")+circle(32, 46, 3)),
        ("cellular-base-station", "携帯基地局", "cellular mobile base station 4g 5g antenna 携帯 基地局", lambda: path('M28 9 H36 V59 H28 Z', PALE)+rect(16, 12, 8, 23, "#fff", 1)+rect(40, 12, 8, 23, "#fff", 1)+path('M24 21 H28 M36 21 H40 M18 59 H46 M9 9 Q0 23 9 37 M55 9 Q64 23 55 37', stroke=ACCENT)),
    ])
    category("21 メッセージング製品", "#69539a", "#f0eafa", [
        ("ibm-mq", "IBM MQ", "ibm mq message queue messaging websphere メッセージキュー", lambda: product('IBM MQ', server(7, 5, 50, 16)+queue())),
        ("apache-kafka", "Apache Kafka", "apache kafka broker event streaming カフカ メッセージ", lambda: product('Apache Kafka', path('M17 13 L32 29 L17 46 M32 29 L52 14 M32 29 L52 46', stroke=ACCENT, width=3)+circle(17, 13, 7, "#fff")+circle(17, 46, 7, "#fff")+circle(32, 29, 7)+circle(52, 14, 7, "#fff")+circle(52, 46, 7, "#fff"))),
        ("rabbitmq", "RabbitMQ", "rabbitmq rabbit mq amqp ラビット メッセージブローカー", lambda: product('RabbitMQ', path('M17 27 V7 Q17 0 23 7 V21 H31 V7 Q31 0 37 7 V27 Q53 29 51 46 H15 Q10 29 17 27 Z', PALE)+dot(29, 33)+dot(40, 33)+path('M33 39 L37 39 L35 43 Z', "#fff"))),
        ("activemq-artemis", "Apache ActiveMQ Artemis", "apache activemq artemis jms amqp メッセージブローカー", lambda: product('Apache ActiveMQ Artemis', circle(32, 28, 23)+arrow(9, 28, 43, 0)+message(22, 18, 20, 20))),
        ("nats-messaging", "NATS", "nats messaging jetstream publish subscribe メッセージング", lambda: product('NATS', path('M5 11 H40 L59 27 L40 43 H5 L24 27 Z', PALE)+path('M16 11 L35 27 L16 43', stroke=ACCENT))),
        ("apache-pulsar", "Apache Pulsar", "apache pulsar pubsub messaging パルサー メッセージ", lambda: product('Apache Pulsar', circle(32, 27, 14)+path('M32 1 V9 M32 45 V53 M6 27 H14 M50 27 H58 M12 7 L19 14 M45 40 L52 47 M52 7 L45 14 M19 40 L12 47', stroke=ACCENT, width=3)+message(23, 21, 18, 12))),
    ])
    category("22 キュー・トピック", "#317990", "#e4f2fa", [
        ("message-queue", "メッセージキュー", "queue message mq キュー 待ち行列 メッセージ", lambda: queue()),
        ("message-topic", "メッセージトピック", "topic message トピック メッセージ 配信", lambda: topic()),
        ("message-producer", "メッセージ送信者", "producer publisher sender メッセージ 送信 プロデューサー", lambda: server(4, 7, 26, 17)+path('M17 24 V39 H35', stroke=ACCENT)+arrow(35, 39, 12, 0)+message(46, 32, 15, 15)),
        ("message-consumer", "メッセージ受信者", "consumer subscriber receiver メッセージ 受信 コンシューマー", lambda: message(3, 10, 17, 15)+arrow(20, 18, 17, 0)+path('M37 18 V38 H46', stroke=ACCENT)+server(36, 38, 25, 17)),
        ("publish-subscribe", "発行・購読", "publish subscribe pubsub fanout ファンアウト 発行 購読", lambda: message(2, 25, 17, 14)+path('M19 32 H32 M32 10 V54', stroke=ACCENT)+arrow(32, 10, 12, 0)+arrow(32, 32, 12, 0)+arrow(32, 54, 12, 0)+rect(46, 4, 14, 12)+rect(46, 26, 14, 12)+rect(46, 48, 14, 12)),
        ("point-to-point", "一対一メッセージ通信", "point to point direct messaging 一対一 メッセージ", lambda: rect(3, 20, 16, 25)+rect(45, 20, 16, 25)+arrow(19, 32, 26, 0)+message(24, 6, 16, 12)),
        ("dead-letter-queue", "デッドレターキュー", "dead letter queue dlq error poison message デッドレター エラーキュー", lambda: inset(queue(), 0, -3, .9)+path('M46 32 L62 59 H30 Z', "#fff")+path('M46 42 V49', stroke=ACCENT, width=3)+dot(46, 54)),
        ("fifo-queue", "先入れ先出しキュー", "fifo first in first out ordering キュー 先入れ先出し", lambda: inset(queue(), 0, -1, 1)+path('M13 52 H51 M13 49 V55 M51 49 V55', stroke=ACCENT)+arrow(13, 9, 38, 0)),
        ("delayed-queue", "遅延キュー", "delay delayed scheduled message queue 遅延 キュー", lambda: inset(queue(), -1, -2, .85)+inset(clock(), 31, 29, .48)),
        ("message-broker", "メッセージブローカー", "broker exchange message router メッセージ ブローカー 中継", lambda: server(18, 19, 28, 23)+arrow(1, 31, 17, 0)+arrow(46, 31, 17, 0)+message(20, 2, 24, 13)+path('M32 15 V19 M32 42 V58', stroke=ACCENT)),
    ])
    category("23 メッセージ配送・信頼性", "#466ba0", "#e8effb", [
        ("message-acknowledgement", "受信確認", "ack acknowledgement receipt message メッセージ 受信確認 応答", lambda: message(7, 8, 39, 28)+circle(45, 44, 14, "#fff")+check(43, 43)+arrow(28, 53, -19, 0)),
        ("message-retry", "メッセージ再送", "retry redelivery resend メッセージ 再送 リトライ", lambda: message(20, 22, 24, 19)+path('M13 24 A23 23 0 1 1 14 48 M13 13 V24 H24', stroke=ACCENT, width=3)),
        ("retry-backoff", "再送間隔の調整", "exponential backoff retry 再送 間隔 バックオフ", lambda: path('M6 51 H58 M10 46 V37 H20 V27 H36 V13 H57', stroke=ACCENT, width=3)+message(5, 11, 17, 12)+path('M13 58 V54 M27 58 V54 M49 58 V54')),
        ("message-ordering", "メッセージ順序制御", "ordering sequence message 順序 制御 メッセージ", lambda: message(5, 8, 17, 12)+message(23, 26, 17, 12)+message(41, 44, 17, 12)+path('M14 20 V32 H23 M32 38 V50 H41', stroke=ACCENT)+arrow(33, 10, 23, 23)),
        ("message-priority", "優先キュー", "priority queue message 優先 キュー メッセージ", lambda: inset(queue(), 0, 12, .85)+path('M43 2 L47 13 L59 13 L49 21 L53 32 L43 25 L33 32 L37 21 L27 13 H39 Z', "#fff")),
        ("message-deduplication", "重複メッセージ除去", "deduplication duplicate message 重複 排除 メッセージ", lambda: message(4, 5, 20, 15)+message(4, 31, 20, 15)+path('M24 13 H32 V39 H24 M32 26 H43', stroke=ACCENT)+message(43, 18, 18, 15)+path('M8 53 L19 61 M19 53 L8 61', stroke=ACCENT)),
        ("idempotent-consumer", "冪等な受信処理", "idempotent idempotency consumer message 冪等 べき等 受信", lambda: message(3, 6, 18, 13)+message(3, 26, 18, 13)+path('M21 13 H30 V33 H21 M30 23 H39', stroke=ACCENT)+rect(39, 15, 21, 30)+check(47, 29)+path('M41 53 H58 M41 59 H58', stroke=ACCENT)),
        ("transactional-outbox", "トランザクショナルアウトボックス", "transactional outbox database message アウトボックス 送信保証", lambda: database(5, 5, 31, 40)+message(20, 27, 23, 16)+arrow(43, 35, 17, 0)+path('M12 52 H30 L36 45', stroke=ACCENT)),
        ("event-replay", "イベント再生", "event replay reprocess イベント 再生 再処理", lambda: path('M9 15 H53 V49 H9 Z', PALE)+path('M21 24 L14 32 L21 40 V24 M41 24 L27 32 L41 40 Z', "#fff")+path('M17 7 Q32-1 47 7', stroke=ACCENT)),
        ("message-retention", "メッセージ保持", "message retention durable persistence メッセージ 保持 永続化", lambda: message(5, 7, 34, 23)+inset(disk(), 27, 26, .52)+path('M12 30 V49 H27', stroke=ACCENT)),
    ])
    category("24 バックアップ方式", "#417b79", "#e3f3ed", [
        ("full-backup", "フルバックアップ", "full backup complete バックアップ 完全 全体", lambda: disk()+check(29, 29)+path('M15 58 H49', stroke=ACCENT, width=3)),
        ("incremental-backup", "増分バックアップ", "incremental backup delta 増分 バックアップ", lambda: inset(disk(), -1, 3, .65)+rect(39, 8, 18, 12)+rect(39, 26, 18, 12)+rect(39, 44, 18, 12)+path('M26 23 H32 V50 H39 M32 14 H39 M32 32 H39', stroke=ACCENT)),
        ("differential-backup", "差分バックアップ", "differential backup difference 差分 バックアップ", lambda: inset(disk(), -2, 2, .65)+path('M33 15 H47 V21 M33 32 H47 V38 M33 49 H47', stroke=ACCENT)+rect(43, 20, 16, 10)+rect(39, 37, 20, 10)+rect(35, 51, 24, 10)),
        ("storage-snapshot", "スナップショット", "snapshot point in time storage バックアップ スナップショット", lambda: copies()+circle(39, 36, 11, "#fff")+path('M33 30 H45 V42 H33 Z M39 26 V29 M39 43 V46', stroke=ACCENT)),
        ("continuous-backup", "継続的バックアップ", "continuous backup cdp data protection 継続 バックアップ", lambda: inset(database(), -3, 8, .72)+inset(disk(), 29, 9, .55)+arrow(25, 7, 28, 0)+arrow(53, 57, -28, 0)),
        ("offline-backup", "オフラインバックアップ", "offline air gap backup removable オフライン バックアップ エアギャップ", lambda: inset(disk(), -2, 4, .85)+path('M45 10 V28 H58 V10 M48 5 V10 M55 5 V10 M52 28 V37 M43 45 L60 55', stroke=ACCENT)+path('M46 58 L58 43', stroke=ACCENT)),
        ("immutable-backup", "変更不可バックアップ", "immutable backup worm object lock イミュータブル 改ざん防止 バックアップ", lambda: inset(disk(), -1, -2, .8)+inset(padlock(), 31, 28, .52)),
        ("encrypted-backup", "暗号化バックアップ", "encrypted encryption backup 暗号化 バックアップ", lambda: inset(disk(), -2, 3, .75)+inset(key_shape(), 27, 19, .6)),
        ("remote-backup", "遠隔バックアップ", "remote offsite backup 遠隔 オフサイト バックアップ", lambda: inset(disk(), -2, 2, .6)+inset(building(), 32, 22, .52)+arrow(25, 17, 24, 0)+path('M49 17 V26', stroke=ACCENT)),
        ("multi-region-backup", "複数リージョンバックアップ", "multi region geographic backup geo 複数リージョン 地理分散 バックアップ", lambda: circle(32, 32, 25)+path('M7 32 H57 M32 7 C15 22 15 42 32 57 C49 42 49 22 32 7', stroke=ACCENT)+inset(disk(), -1, 13, .43)+inset(disk(), 36, 13, .43)+arrow(22, 29, 18, 0)),
    ])
    category("25 復旧・バックアップ運用", "#3775a0", "#e4f0fa", [
        ("backup-restore", "バックアップから復元", "restore backup recovery バックアップ 復元 リストア", lambda: inset(disk(), 0, 3, .7)+inset(database(), 31, 20, .52)+arrow(23, 48, 20, 0)+path('M43 48 V39', stroke=ACCENT)),
        ("point-in-time-recovery", "指定時点への復旧", "point in time recovery pitr rollback 時点 復旧", lambda: inset(database(), -2, 3, .8)+inset(clock(), 29, 27, .52)+path('M12 12 L7 18 H18', stroke=ACCENT)),
        ("disaster-recovery", "災害復旧", "disaster recovery dr bcp 災害 復旧 事業継続", lambda: inset(building(), -2, 7, .75)+path('M41 5 L35 22 H45 L38 39 L59 14 H47 L52 5 Z', PALE)+arrow(27, 55, 29, 0)),
        ("failover", "フェイルオーバー", "failover standby switchover 切替 障害 待機", lambda: server(3, 7, 25, 18)+server(36, 40, 25, 18)+path('M8 10 L22 22 M22 10 L8 22', stroke=ACCENT)+path('M28 17 H48 V36', stroke=ACCENT)+arrow(48, 30, 0, 10)),
        ("failback", "フェイルバック", "failback return primary 復帰 切戻し フェイルバック", lambda: server(3, 7, 25, 18)+server(36, 40, 25, 18)+path('M48 40 V17 H32', stroke=ACCENT)+arrow(38, 17, -10, 0)+check(12, 17)),
        ("recovery-drill", "復旧訓練", "recovery drill rehearsal disaster test 復旧 訓練 演習", lambda: inset(document(), -1, -1, .8)+inset(clock(), 29, 28, .5)+check(25, 27)+path('M16 50 H27', stroke=ACCENT)),
        ("backup-schedule", "バックアップスケジュール", "backup schedule calendar scheduled バックアップ 計画 定期", lambda: rect(5, 10, 42, 42, "#fff")+path('M5 22 H47 M15 5 V15 M37 5 V15')+dot(15, 31)+dot(27, 31)+inset(disk(), 30, 30, .5)),
        ("backup-retention-policy", "バックアップ保存期限", "backup retention policy lifecycle expiry バックアップ 保存期限 世代管理", lambda: inset(disk(), -1, 4, .7)+rect(35, 9, 23, 20, "#fff")+path('M35 16 H58 M42 4 V12 M52 4 V12 M40 22 H53', stroke=ACCENT)+arrow(28, 52, 25, 0)),
        ("backup-verification", "バックアップ検証", "backup verification integrity checksum バックアップ 検証 整合性", lambda: disk()+circle(44, 43, 15, "#fff")+check(42, 42)),
        ("archive-retrieval", "アーカイブ復元", "archive retrieval cold storage restore アーカイブ 復元 取り出し", lambda: rect(6, 17, 52, 36)+rect(4, 9, 56, 10, "#fff", 1)+rect(24, 25, 16, 9, "#fff", 1)+path('M32 45 V59', stroke=ACCENT)+arrow(32, 59, 0, -15)),
    ])
    category("26 暗号化・鍵管理", "#467d60", "#e6f4e8", [
        ("encryption-at-rest", "保存データの暗号化", "encryption at rest storage disk 暗号化 保存 データ", lambda: inset(database(), -2, 2, .9)+inset(padlock(), 29, 28, .5)),
        ("encryption-in-transit", "通信データの暗号化", "encryption in transit tls transport 暗号化 通信 経路", lambda: rect(2, 35, 13, 21)+rect(49, 35, 13, 21)+path('M15 46 H49', stroke=ACCENT, width=3)+inset(padlock(), 13, -2, .6)),
        ("end-to-end-encryption", "エンドツーエンド暗号化", "end to end encryption e2ee 暗号化 端末間", lambda: inset(phone(), -10, 3, .7)+inset(phone(), 29, 3, .7)+path('M22 31 H41', stroke=ACCENT)+inset(padlock(), 19, 21, .4)),
        ("public-key-encryption", "公開鍵暗号", "public key asymmetric encryption rsa ecc 公開鍵 暗号化", lambda: inset(key_shape(), -2, -1, .75)+inset(key_shape(), 24, 22, .65)+path('M40 8 H57 M48 1 V15 M7 53 H22', stroke=ACCENT, width=3)),
        ("symmetric-encryption", "共通鍵暗号", "symmetric shared key encryption aes 共通鍵 暗号化", lambda: rect(3, 32, 15, 25)+rect(46, 32, 15, 25)+inset(key_shape(), 13, -3, .6)+path('M18 44 H46', stroke=ACCENT)+path('M24 40 L20 44 L24 48 M40 40 L44 44 L40 48', stroke=ACCENT)),
        ("key-rotation", "暗号鍵のローテーション", "key rotation renewal encryption 暗号鍵 更新 ローテーション", lambda: inset(key_shape(), 6, 1, .8)+path('M8 28 A25 25 0 1 0 47 10 M47 2 V10 H55', stroke=ACCENT, width=3)),
        ("key-management", "暗号鍵管理", "key management kms keystore 暗号鍵 管理", lambda: rect(5, 6, 54, 50)+inset(key_shape(), 8, 4, .73)+path('M5 18 H59')+dot(12, 12)+dot(20, 12)),
        ("hardware-security-module", "ハードウェアセキュリティモジュール", "hardware security module hsm cryptographic ハードウェア 暗号装置", lambda: server(5, 18, 54, 32)+inset(padlock(), 21, 6, .68)+path('M13 50 V57 M51 50 V57 M13 11 V18 M51 11 V18', stroke=ACCENT)),
        ("digital-signature", "デジタル署名", "digital signature signing verification 電子署名 デジタル署名", lambda: inset(document(), -1, -2, .87)+path('M17 44 Q21 32 26 43 T36 40', stroke=ACCENT)+inset(key_shape(), 34, 29, .45)),
        ("data-tokenization", "データのトークン化", "tokenization token sensitive data トークン化 置換 個人情報", lambda: rect(4, 10, 23, 22, "#fff")+path('M9 17 H22 M9 24 H18')+arrow(20, 42, 24, 0)+path('M46 28 L59 35 V51 L46 58 L33 51 V35 Z', PALE)+dot(46, 43, 4)),
    ])
    category("27 データ保護・認証", "#397d82", "#e2f3f2", [
        ("data-masking", "データマスキング", "masking redaction privacy データ マスキング 秘匿", lambda: document()+path('M21 26 H44 M21 33 H44', stroke=ACCENT, width=5)+path('M23 46 H41 M23 52 H35')),
        ("data-anonymization", "データ匿名化", "anonymization anonymous privacy 個人情報 匿名化", lambda: person(23, 25, 1.2)+arrow(33, 17, 22, 0)+circle(45, 43, 14, "#fff")+path('M38 38 L52 48 M52 38 L38 48', stroke=ACCENT)),
        ("secret-vault", "シークレット保管庫", "secret vault credentials credential store シークレット 保管庫 資格情報", lambda: rect(7, 6, 50, 51)+rect(14, 13, 36, 36, "#fff")+circle(32, 31, 10)+path('M32 21 V41 M22 31 H42 M10 17 H14 M10 43 H14', stroke=ACCENT)),
        ("password-hashing", "パスワードハッシュ化", "password hashing hash bcrypt argon パスワード ハッシュ化", lambda: rect(4, 6, 56, 20, "#fff")+dot(16, 16, 3)+dot(27, 16, 3)+dot(38, 16, 3)+dot(49, 16, 3)+arrow(32, 27, 0, 12)+rect(10, 42, 44, 17)+word('#', 32, 46, 1)),
        ("certificate-rotation", "証明書更新", "certificate rotation renewal tls ssl 証明書 更新", lambda: inset(document(), 7, 1, .72)+path('M9 33 A24 24 0 1 0 48 12 M48 4 V12 H56', stroke=ACCENT, width=3)+check(33, 33)),
        ("data-loss-prevention", "情報漏洩防止", "data loss prevention dlp exfiltration 情報漏洩 防止", lambda: inset(document(), -3, 2, .75)+inset(shield(), 27, 23, .58)+path('M40 40 L52 51 M52 40 L40 51', stroke=ACCENT)),
        ("secure-erasure", "データ完全消去", "secure erase erasure sanitization wipe データ 消去 完全消去", lambda: inset(disk(), -2, -2, .82)+path('M29 42 L43 28 L59 44 L45 58 H29 L21 50 Z', "#fff")+path('M35 36 L51 52 M30 58 H60', stroke=ACCENT)),
        ("zero-trust-access", "ゼロトラストアクセス", "zero trust ztna verify access ゼロトラスト アクセス", lambda: inset(shield(), 9, 1, .72)+person(12, 43, .7)+server(43, 36, 18, 16)+path('M20 40 H44', stroke=ACCENT)+check(31, 23)),
        ("mutual-tls", "相互認証通信", "mutual tls mtls authentication certificate 相互認証 通信", lambda: inset(document(), -4, 4, .55)+inset(document(), 34, 4, .55)+arrow(22, 22, 20, 0)+arrow(42, 45, -20, 0)+inset(padlock(), 20, 22, .4)),
        ("data-access-audit", "データアクセス監査", "data access audit trail database アクセス 監査 証跡", lambda: inset(database(), -2, -2, .76)+inset(document(), 28, 23, .6)+path('M43 48 L47 52 L56 42', stroke=ACCENT)),
    ])
    category("28 クラスタ構成", "#5c66a0", "#ebeefa", [
        ("server-cluster", "サーバークラスタ", "server cluster node compute サーバ クラスタ クラスター", lambda: cluster()),
        ("database-cluster", "データベースクラスタ", "database cluster distributed db データベース クラスタ", lambda: inset(database(), -2, -1, .5)+inset(database(), 34, -1, .5)+inset(database(), 16, 33, .5)+path('M18 25 L32 39 L47 25 M18 25 H47', stroke=ACCENT)),
        ("kubernetes-cluster", "Kubernetesクラスタ", "kubernetes k8s cluster pod node クバネティス クラスタ", lambda: path('M32 3 L57 17 V47 L32 61 L7 47 V17 Z', PALE)+inset(cluster(), 8, 7, .75)),
        ("active-active-cluster", "アクティブ・アクティブ構成", "active active cluster multi master クラスタ 同時稼働", lambda: server(3, 9, 25, 19)+server(36, 9, 25, 19)+check(12, 20)+check(45, 20)+arrow(12, 45, 40, 0)+arrow(52, 55, -40, 0)),
        ("active-passive-cluster", "稼働系・待機系構成", "active passive standby cluster クラスタ 稼働系 待機系", lambda: server(3, 9, 25, 19)+server(36, 9, 25, 19)+check(12, 20)+path('M46 14 V24 M52 14 V24', stroke=ACCENT)+arrow(12, 48, 40, 0)),
        ("cluster-quorum", "クラスタの多数決", "cluster quorum majority consensus クラスタ クォーラム 多数決", lambda: inset(cluster(), -1, 0, 1)+check(10, 12)+check(44, 12)+path('M27 49 L38 55 M38 49 L27 55', stroke=ACCENT)),
        ("leader-election", "リーダー選出", "leader election consensus cluster クラスタ リーダー 選出", lambda: inset(cluster(), 0, 4, .9)+path('M25 11 L21 1 L30 6 L34 0 L38 6 L47 1 L43 11 Z', "#fff")),
        ("database-sharding", "データベースシャーディング", "sharding shard partition cluster データベース シャード 分割 クラスタ", lambda: inset(database(), -2, -1, .52)+inset(database(), 34, -1, .52)+inset(database(), 16, 34, .52)+path('M10 12 H19 M46 17 H55 M28 50 H37', stroke=ACCENT, width=4)),
        ("cluster-replication", "クラスタ間の複製", "cluster replication replica sync クラスタ レプリケーション 複製", lambda: inset(cluster(), -2, 8, .55)+inset(cluster(), 33, 8, .55)+arrow(22, 8, 20, 0)+arrow(42, 56, -20, 0)),
        ("cluster-autoscaling", "クラスタ自動拡張", "cluster autoscaling scale out クラスタ 自動拡張 オートスケール", lambda: inset(cluster(), -1, 9, .72)+rect(42, 6, 19, 19, "#fff")+path('M46 15 H57 M52 10 V21 M44 38 L58 52 M50 52 H58 V44', stroke=ACCENT)),
    ])
    category("29 可用性・負荷制御", "#4c7d77", "#e8f3ee", [
        ("horizontal-scaling", "水平スケーリング", "horizontal scaling scale out in 水平 拡張 スケールアウト", lambda: server(3, 19, 24, 18)+server(37, 19, 24, 18)+arrow(24, 50, -20, 0)+arrow(40, 50, 20, 0)+path('M27 28 H37', stroke=ACCENT)),
        ("vertical-scaling", "垂直スケーリング", "vertical scaling scale up down 垂直 拡張 スケールアップ", lambda: server(7, 28, 28, 23)+arrow(49, 30, 0, -24)+arrow(49, 38, 0, 24)+path('M12 21 H29 V8 H12 Z', "#fff")),
        ("service-health-check", "サービス正常性確認", "health check probe liveness readiness サービス ヘルスチェック 正常性", lambda: inset(monitor(), -1, -1, .9)+path('M12 29 H20 L26 18 L32 37 L39 24 H49', stroke=ACCENT, width=3)+check(46, 50)),
        ("cluster-heartbeat", "クラスタ死活監視", "heartbeat keepalive cluster failure クラスタ ハートビート 死活監視", lambda: server(3, 7, 23, 17)+server(38, 7, 23, 17)+path('M14 24 V43 H23 L28 32 L35 55 L42 40 H50 V24', stroke=ACCENT, width=3)),
        ("circuit-breaker", "サーキットブレーカー", "circuit breaker resilience isolation サーキットブレーカー 遮断", lambda: path('M3 32 H20 M44 32 H61 M21 31 L40 11', stroke=ACCENT, width=4)+circle(20, 32, 4, "#fff")+circle(44, 32, 4, "#fff")+path('M12 49 H52 M23 45 V54 M41 45 V54')),
        ("bulkhead-isolation", "障害の隔離", "bulkhead isolation fault containment 障害 隔離 分離", lambda: rect(4, 8, 56, 48, "#fff")+path('M23 8 V56 M42 8 V56', stroke=ACCENT, width=3)+server(8, 23, 11, 17)+server(27, 23, 11, 17)+server(46, 23, 10, 17)+path('M27 22 L38 43 M38 22 L27 43', stroke=ACCENT)),
        ("rate-limiting", "流量制限", "rate limit limiting throttle quota 流量 制限 スロットリング", lambda: path('M3 12 H61 L43 31 V51 L25 60 V31 Z', PALE)+message(11, 16, 13, 10)+message(29, 16, 13, 10)+path('M25 37 H43', stroke=ACCENT, width=4)),
        ("backpressure", "バックプレッシャー", "backpressure flow control consumer バックプレッシャー 流量制御", lambda: inset(queue(), -1, 4, .95)+arrow(51, 9, -38, 0)+path('M45 44 V58 M51 44 V58', stroke=ACCENT, width=3)),
        ("cache-cluster", "分散キャッシュクラスタ", "cache cluster distributed memory キャッシュ クラスタ 分散", lambda: cluster()+path('M27 17 L23 30 H31 L28 41 L41 24 H33 L37 17 Z', "#fff")),
        ("session-replication", "セッション情報の複製", "session replication state cluster セッション 複製 クラスタ", lambda: rect(3, 7, 25, 25)+rect(36, 7, 25, 25)+person(16, 21, .52)+person(49, 21, .52)+arrow(12, 45, 40, 0)+arrow(52, 56, -40, 0)),
    ])
    category("30 イベント・ストリーム連携", "#8a619b", "#f3eafa", [
        ("event-driven-architecture", "イベント駆動構成", "event driven architecture eda イベント 駆動 非同期", lambda: message(23, 23, 18, 14)+path('M32 23 V12 H13 M41 30 H51 V50 M23 30 H12 V50', stroke=ACCENT)+rect(3, 4, 18, 13)+rect(3, 49, 18, 13)+rect(43, 49, 18, 13)),
        ("change-data-capture", "データ変更の取得", "change data capture cdc database データ変更 変更データ キャプチャ", lambda: inset(database(), -2, 4, .72)+path('M12 15 H22 V24 H12 Z', "#fff")+arrow(27, 32, 18, 0)+message(44, 24, 17, 17)),
        ("schema-registry", "スキーマレジストリ", "schema registry avro protobuf compatibility スキーマ レジストリ", lambda: copies()+path('M29 32 V49 M36 32 H49 M36 40 H49 M36 48 H44', stroke=ACCENT)+circle(15, 17, 8, "#fff")+check(13, 16)),
        ("event-sourcing", "イベント履歴による状態管理", "event sourcing append log state イベントソーシング 履歴 状態", lambda: message(4, 5, 15, 10)+message(24, 5, 15, 10)+message(44, 5, 15, 10)+path('M12 15 V27 H52 V15 M32 27 V38', stroke=ACCENT)+inset(database(), 17, 32, .47)),
        ("command-query-separation", "コマンド・参照分離", "cqrs command query responsibility segregation コマンド 参照 分離", lambda: server(20, 3, 24, 14)+path('M32 17 V28 M12 28 H52 M12 28 V40 M52 28 V40', stroke=ACCENT)+rect(2, 41, 20, 18)+inset(database(), 37, 34, .4)+path('M8 49 L13 54 L18 47', stroke=ACCENT)),
        ("stream-processing", "ストリーム処理", "stream processing real time flink spark ストリーム リアルタイム 処理", lambda: message(2, 9, 14, 11)+message(2, 27, 14, 11)+message(2, 45, 14, 11)+inset(gear(), 15, 14, .6)+arrow(16, 32, 15, 0)+arrow(49, 32, 12, 0)+path('M19 14 H34 M19 50 H34', stroke=ACCENT)),
        ("consumer-group", "コンシューマーグループ", "kafka consumer group subscribers コンシューマー グループ 受信", lambda: inset(queue(), 1, -2, .62)+path('M20 29 V40 M8 40 H54 M8 40 V48 M31 40 V48 M54 40 V48', stroke=ACCENT)+server(1, 48, 16, 13)+server(23, 48, 16, 13)+server(46, 48, 16, 13)),
        ("message-partition", "トピックのパーティション", "kafka topic partition shard トピック パーティション 分割", lambda: rect(4, 5, 56, 54, "#fff")+path('M4 23 H60 M4 41 H60')+message(9, 10, 12, 8)+message(28, 10, 12, 8)+message(9, 28, 12, 8)+message(28, 46, 12, 8)+message(46, 46, 10, 8)),
        ("consumer-offset", "読み取り位置", "kafka consumer offset cursor checkpoint オフセット 読み取り位置", lambda: inset(queue(), 0, -1, 1)+path('M31 6 V50 M25 54 L31 60 L37 54 Z', "#fff")+path('M31 7 V47', stroke=ACCENT, width=3)),
        ("message-encryption", "メッセージ暗号化", "message encryption payload secure メッセージ 暗号化 ペイロード", lambda: message(4, 8, 45, 31)+inset(padlock(), 29, 25, .52)+path('M10 48 H27 M10 55 H21', stroke=ACCENT)),
    ])
    category("31 AIサービス・開発支援", "#377e78", "#e3f3ee", [
        ("openai", "OpenAI", "openai オープンエーアイ ai service 人工知能", lambda: product('OpenAI', brain())),
        ("chatgpt", "ChatGPT", "chatgpt openai chat assistant チャットジーピーティー 対話 ai", lambda: product('ChatGPT', bubble()+path('M18 22 H45 M18 30 H36', stroke=ACCENT)+path('M48 4 V16 M42 10 H54', stroke=ACCENT, width=3))),
        ("openai-api", "OpenAI API", "openai api developer integration オープンエーアイ 開発 api ai", lambda: product('OpenAI API', rect(5, 7, 54, 40, "#fff")+path('M5 18 H59 M23 25 L16 32 L23 39 M41 25 L48 32 L41 39 M35 23 L29 41', stroke=ACCENT))),
        ("openai-codex", "OpenAI Codex", "openai codex coding agent コーデックス コード生成 開発支援 ai", lambda: product('OpenAI Codex', rect(5, 7, 54, 40)+path('M15 24 L23 31 L15 38 M29 38 H45', stroke=ACCENT, width=3)+path('M46 8 L48 15 L55 17 L48 19 L46 26 L44 19 L37 17 L44 15 Z', "#fff"))),
        ("gemini", "Gemini", "gemini google ジェミニ ジェミナイ ai service", lambda: product('Gemini', path('M32 2 Q38 22 59 27 Q38 32 32 53 Q26 32 5 27 Q26 22 32 2 Z', PALE)+path('M50 3 L52 9 L58 11 L52 13 L50 19 L48 13 L42 11 L48 9 Z', "#fff"))),
        ("google-ai-studio", "Google AI Studio", "google ai studio gemini developer グーグル エーアイ スタジオ 開発", lambda: product('Google AI Studio', rect(4, 5, 56, 43, "#fff")+path('M4 16 H60 M32 20 L36 29 L45 33 L36 37 L32 46 L28 37 L19 33 L28 29 Z', PALE)+dot(11, 11)+dot(18, 11))),
        ("claude", "Claude", "claude anthropic assistant クロード アンソロピック 対話 ai", lambda: product('Claude', circle(32, 26, 10, "#fff")+path('M32 2 V12 M32 40 V50 M8 26 H18 M46 26 H56 M15 9 L22 16 M42 36 L49 43 M49 9 L42 16 M22 36 L15 43', stroke=ACCENT, width=3))),
        ("claude-code", "Claude Code", "claude code anthropic coding agent クロードコード コード生成 開発支援 ai", lambda: product('Claude Code', rect(4, 5, 56, 43)+path('M4 16 H60 M14 24 L22 31 L14 38 M28 38 H38', stroke=ACCENT, width=3)+inset(gear(), 36, 17, .35))),
        ("github-copilot", "GitHub Copilot", "github copilot coding assistant ギットハブ コパイロット 開発支援 ai", lambda: product('GitHub Copilot', rect(11, 13, 42, 30, "#fff", 12)+circle(24, 26, 8)+circle(42, 26, 8)+path('M16 8 L10 13 M48 8 L54 13 M26 39 H38', stroke=ACCENT))),
        ("microsoft-copilot", "Microsoft Copilot", "microsoft copilot office assistant マイクロソフト コパイロット ai", lambda: product('Microsoft Copilot', path('M10 10 H31 L42 28 L32 46 H11 L22 28 Z', PALE)+path('M32 10 H53 L42 28 L53 46 H32 L22 28 Z', "#fff")+path('M22 28 H42', stroke=ACCENT))),
    ])
    category("32 セキュリティ・脅威の人物", "#536b8c", "#e8eef8", [
        ("criminal-person", "犯罪者", "criminal offender perpetrator 犯罪者 加害者 犯人", lambda: character("criminal")),
        ("cyber-attacker", "サイバー攻撃者", "cyber attacker hacker threat actor ハッカー 攻撃者", lambda: character("hacker")),
        ("malicious-insider", "内部不正者", "malicious insider employee threat 内部不正者 内部脅威 不正", lambda: bust("suit")+rect(4, 32, 19, 27, "#fff", 2)+circle(13, 39, 3)+path('M8 47 L18 55 M18 47 L8 55 M13 27 V32', stroke=ACCENT)),
        ("fraudster", "詐欺師", "fraudster scammer fraud social engineering 詐欺師 詐欺 なりすまし", lambda: inset(bust("suit"), -5, 0, .94)+inset(document(), 31, 27, .51)+path('M22 14 Q32 8 42 14 L39 20 H25 Z', "#fff")+dot(28, 15, 1)+dot(36, 15, 1)+path('M46 40 L56 51 M56 40 L46 51', stroke=ACCENT)),
        ("terrorist-person", "テロリスト", "terrorist terrorism violent actor テロリスト テロ 脅威", lambda: character("terrorist")),
        ("victim-person", "被害者", "victim affected person survivor 被害者 被害者者 被災者", lambda: character("victim")),
        ("security-specialist", "セキュリティスペシャリスト", "security specialist expert professional analyst セキュリティスペシャリスト セキュリティ専門家", lambda: character("security")),
        ("incident-responder", "インシデント対応担当者", "incident responder csirt response specialist インシデント 対応担当者", lambda: inset(bust("uniform"), -5, 0, .94)+inset(clock(), 31, 27, .49)+path('M7 22 L11 15 L15 22 M11 16 V29', stroke=ACCENT)),
        ("digital-forensics-specialist", "デジタル鑑識担当者", "digital forensics investigator specialist デジタル鑑識 フォレンジック", lambda: inset(bust(), -5, 0, .94)+circle(44, 41, 11, "#fff")+path('M51 49 L61 59 M39 43 C35 32 52 31 49 42 M42 44 Q39 36 46 37 V47', stroke=ACCENT)),
        ("police-officer", "警察官", "police officer law enforcement 警察官 警官 捜査官", lambda: bust("uniform")+path('M21 5 Q32-1 43 5 L40 10 H24 Z', PALE)+path('M39 39 L45 42 V49 L39 53 L34 49 V42 Z', "#fff")+dot(32, 7, 1.5)),
    ])
    category("33 安全保障・公共組織", "#626c9d", "#eceefa", [
        ("foreign-intelligence-agency", "海外諜報機関", "foreign intelligence overseas agency espionage 海外諜報機関 外国 情報機関 諜報", lambda: character("spy")),
        ("domestic-intelligence-agency", "国内諜報機関", "domestic intelligence agency security 国内諜報機関 情報機関 公安", lambda: civic_building()+inset(shield(), 20, 22, .43)+path('M8 10 L15 4 L22 10', stroke=ACCENT)),
        ("investigative-agency", "捜査機関", "investigation investigative agency law enforcement 捜査機関 調査機関", lambda: inset(civic_building(), -1, 3, .84)+circle(45, 42, 12, "#fff")+path('M53 51 L62 60 M41 42 H49 M45 38 V46', stroke=ACCENT)),
        ("police-station", "警察署", "police station law enforcement organization 警察署 交番", lambda: building()+path('M32 12 L36 21 L46 22 L39 29 L41 39 L32 34 L23 39 L25 29 L18 22 L28 21 Z', "#fff")),
        ("government-agency", "政府機関", "government agency ministry public administration 政府機関 官庁 省庁 行政", lambda: civic_building()+path('M32 4 V0 M22 1 H42', stroke=ACCENT)),
        ("embassy", "大使館", "embassy diplomatic mission foreign 大使館 外交 使節", lambda: inset(civic_building(), -2, 8, .83)+path('M48 4 V36 M48 5 H61 L57 12 L61 19 H48', PALE)+inset(globe(), 32, 36, .43)),
        ("consulate", "領事館", "consulate consular visa passport 領事館 査証", lambda: inset(building(), -3, 2, .86)+rect(38, 28, 22, 31, "#fff", 2)+circle(49, 39, 6)+path('M43 49 H55 M43 54 H51', stroke=ACCENT)),
        ("diplomat", "外交官", "diplomat ambassador envoy diplomatic 外交官 大使 外交", lambda: character("diplomat")),
        ("international-organization", "国際機関", "international organization multinational 国際機関 国際組織", lambda: globe()+person(10, 51, .5)+person(54, 51, .5)+person(32, 53, .5)+path('M12 48 H52', stroke=ACCENT)),
        ("humanitarian-organization", "人道支援団体", "humanitarian organization aid relief ngo 人道支援 団体 NGO", lambda: path('M32 19 C21 4 3 16 32 43 C61 16 43 4 32 19 Z', PALE)+path('M5 35 L15 48 H28 M59 35 L49 48 H36 M9 58 L19 50 H45 L55 58', stroke=ACCENT, width=3)),
    ])
    category("34 医療・ケアの人物と組織", "#317e7d", "#e4f5f0", [
        ("hospital", "病院", "hospital healthcare medical institution 病院 医療機関", lambda: rect(6, 19, 52, 40, "#fff")+rect(20, 4, 24, 55)+inset(medical_cross(), 21, 5, .34)+path('M27 59 V43 H37 V59 M11 29 H16 M11 38 H16 M48 29 H53 M48 38 H53', stroke=ACCENT)),
        ("medical-clinic", "診療所", "clinic medical practice outpatient 診療所 クリニック 医院", lambda: path('M4 27 L32 6 L60 27 M10 24 V59 H54 V24', PALE)+inset(medical_cross(), 20, 15, .38)+rect(26, 42, 13, 17, "#fff", 0)),
        ("emergency-medical-care", "救急医療", "emergency medical urgent care ambulance 救急医療 救急 救命", lambda: rect(3, 22, 36, 29, "#fff")+path('M39 31 H51 L61 42 V51 H39 Z', PALE)+circle(15, 51, 6, "#fff")+circle(49, 51, 6, "#fff")+inset(medical_cross(), 11, 24, .33)+path('M19 16 V9 H31 V16', stroke=ACCENT)),
        ("physician", "医師", "physician doctor medical professional 医師 医者 ドクター", lambda: character("doctor")),
        ("nurse", "看護師", "nurse nursing medical professional 看護師 看護婦 ナース", lambda: character("nurse")),
        ("pharmacist", "薬剤師", "pharmacist pharmacy medicine 薬剤師 調剤 薬局", lambda: inset(bust("coat"), -5, 0, .94)+rect(38, 34, 21, 22, "#fff", 2)+path('M37 30 H60 V35 H37 Z M43 41 H54 M48 37 V48', PALE)),
        ("patient", "患者", "patient medical inpatient outpatient 患者 入院 外来", lambda: bust()+path('M25 8 H39 V14 H25 Z', PALE)+path('M29 8 L35 14 M35 8 L29 14 M21 39 H43 M21 46 H43 M21 53 H43', stroke=ACCENT)),
        ("paramedic", "救急救命士", "paramedic emergency technician emt 救急救命士 救命士", lambda: inset(bust("uniform"), -4, 0, .94)+rect(36, 36, 25, 21, "#fff", 2)+path('M43 36 V30 H54 V36')+inset(medical_cross(), 40, 38, .27)),
        ("physical-therapist", "理学療法士", "physical therapist physiotherapy rehabilitation 理学療法士 リハビリ", lambda: inset(bust("coat"), -8, 5, .85)+circle(48, 23, 5, "#fff")+path('M48 30 L43 41 L53 48 M43 41 L37 55 M48 31 L57 37', stroke=ACCENT, width=3)),
        ("clinical-laboratory-technologist", "臨床検査技師", "clinical laboratory technologist lab technician 臨床検査技師 検査技師", lambda: inset(bust("coat"), -7, 0, .92)+path('M42 25 L52 31 L45 43 L36 38 Z', "#fff")+path('M49 36 Q64 49 49 55 H35 M39 47 H53 M42 55 V60 M34 60 H56', stroke=ACCENT)),
    ])
    category("35 教育・子育ての人物と組織", "#8e7345", "#fff2db", [
        ("teacher", "先生・教員", "teacher educator instructor professor 先生 教員 教師 講師", lambda: character("teacher")),
        ("school-child", "児童", "school child pupil student elementary 児童 小学生 子供 子ども", lambda: character("child")),
        ("guardian", "保護者", "guardian parent carer 保護者 親 父母 保護者者", lambda: character("guardian")),
        ("childcare-worker", "保育士", "childcare worker nursery teacher 保育士 保育 教育", lambda: inset(bust(), -5, 0, .94)+rect(34, 43, 12, 13, "#fff", 1)+rect(48, 43, 12, 13, "#fff", 1)+path('M47 27 L59 40 H35 Z', PALE)),
        ("school-principal", "校長", "school principal headteacher headmaster 校長 園長 学校長", lambda: inset(bust("suit"), 0, -1, .8)+path('M10 38 H54 L49 59 H15 Z', "#fff")+path('M24 44 L32 40 L40 44 L32 48 Z M27 48 V52 H37 V48', PALE)),
        ("school-building", "学校", "school elementary junior high education 学校 小学校 中学校 高校", lambda: rect(5, 23, 54, 36, "#fff")+path('M18 23 V16 L32 5 L46 16 V59 H18 Z', PALE)+circle(32, 22, 6, "#fff")+path('M32 18 V22 H36 M27 59 V43 H37 V59 M10 32 H14 M50 32 H54', stroke=ACCENT)),
        ("nursery-school", "保育園", "nursery preschool kindergarten daycare 保育園 幼稚園 こども園", lambda: path('M4 26 L32 5 L60 26 H54 V59 H10 V26 Z', PALE)+rect(18, 34, 11, 13, "#fff", 1)+rect(35, 34, 11, 13, "#fff", 1)+path('M23 56 H41', stroke=ACCENT)+circle(32, 19, 5, "#fff")),
        ("university", "大学", "university college campus academy 大学 学園 キャンパス", lambda: inset(civic_building(), 7, 14, .78)+path('M9 10 L32 2 L55 10 L32 19 Z', PALE)+path('M16 13 V21 Q32 30 48 21 V13 M54 10 V25', stroke=ACCENT)),
        ("school-counselor", "スクールカウンセラー", "school counselor counsellor guidance スクールカウンセラー 学校相談", lambda: inset(bust(), -6, 5, .86)+path('M33 6 H60 V27 H48 L40 35 V27 H33 Z', "#fff")+path('M39 14 H54 M39 20 H50', stroke=ACCENT)+person(49, 47, .72)),
        ("parents-association", "保護者会", "parents association pta school family 保護者会 PTA 父母会", lambda: person(12, 19, .8)+person(52, 19, .8)+person(32, 17, .8)+rect(7, 37, 50, 17, "#fff")+path('M12 54 V61 M52 54 V61 M26 39 H38 V50 H26 Z', PALE)),
    ])
    category("36 営業・顧客支援・管理職種", "#3c7594", "#e8f3fb", [
        ("sales-representative", "営業マン", "sales representative salesperson salesman account executive 営業マン 営業 営業担当者", lambda: character("salesman")),
        ("receptionist", "受付嬢", "receptionist front desk concierge 受付嬢 受付 受付係 受付担当者", lambda: character("receptionist")),
        ("customer-success-manager", "カスタマーサクセス担当者", "customer success manager csm カスタマーサクセス 顧客支援", lambda: executive(path('M4 32 Q32 2 60 32 Q32 62 4 32 Z', "#fff")+check(29, 32))+path('M10 9 H17 M13 5 V13', stroke=ACCENT)),
        ("business-consultant", "コンサルタント", "consultant advisor consulting コンサルタント コンサル アドバイザー", lambda: inset(bust("suit"), -6, 0, .94)+rect(34, 27, 27, 25, "#fff")+path('M39 46 L44 38 L49 43 L57 33 M45 52 V60 M54 52 V60', stroke=ACCENT)),
        ("human-resources-staff", "人事担当者", "human resources hr personnel 人事担当者 人事 労務", lambda: executive(person(20, 28, 1)+person(46, 28, 1)+path('M10 51 H56', stroke=ACCENT))),
        ("recruiter", "採用担当者", "recruiter recruitment hiring talent 採用担当者 採用 リクルーター", lambda: inset(bust("suit"), -6, 0, .94)+circle(45, 40, 11, "#fff")+person(45, 39, .5)+path('M52 48 L61 59', stroke=ACCENT, width=3)),
        ("accounting-staff", "経理担当者", "accountant accounting finance bookkeeper 経理担当者 経理 会計", lambda: inset(bust("suit"), -6, 0, .94)+rect(36, 29, 24, 31, "#fff", 2)+rect(40, 33, 16, 7, radius=0)+''.join(dot(x, y, 1.5) for y in (46, 53) for x in (42, 49, 56))),
        ("legal-staff", "法務担当者", "legal counsel lawyer corporate law 法務担当者 法務 法律", lambda: executive(scales())),
        ("public-relations-staff", "広報担当者", "public relations pr communications spokesperson 広報担当者 広報 宣伝", lambda: executive(path('M7 22 H25 L54 7 V51 L25 36 H7 Z', "#fff")+path('M17 36 L22 57 H32 L27 37 M59 15 V42', stroke=ACCENT))),
        ("executive-assistant", "秘書・役員補佐", "secretary executive assistant personal assistant 秘書 役員補佐 補佐", lambda: inset(bust("suit"), -6, 0, .94)+rect(34, 30, 27, 27, "#fff")+path('M34 38 H61 M41 25 V34 M54 25 V34 M40 46 H45 M50 46 H55', stroke=ACCENT)),
    ])
    category("37 経営・ガバナンス", "#997044", "#fff1df", [
        ("board-director", "取締役", "director board member executive 取締役 役員", lambda: executive(document())),
        ("representative-director", "代表取締役", "representative director company president 代表取締役 社長", lambda: inset(bust("suit"), -6, 0, .94)+inset(document(), 31, 25, .5)+path('M42 53 L57 36 L61 40 L46 57 H42 Z', "#fff")),
        ("board-of-directors", "取締役会", "board of directors governance board meeting 取締役会 経営会議", lambda: person(10, 23, .7)+person(32, 12, .8)+person(54, 23, .7)+path('M17 32 L47 32 L57 52 H7 Z', PALE)+path('M15 52 V61 M49 52 V61 M27 38 H37 V46 H27 Z', "#fff")),
        ("outside-director", "社外取締役", "outside independent director board 社外取締役 独立役員", lambda: inset(bust("suit"), -6, 0, .94)+inset(building(), 34, 26, .43)+path('M39 10 H59 V23 M53 4 L59 10 L53 16', stroke=ACCENT)),
        ("statutory-auditor", "監査役", "statutory auditor corporate audit supervisor 監査役 監査委員", lambda: executive(document()+check(31, 36))+circle(11, 11, 6, "#fff")+check(9, 10)),
        ("chief-officer", "CxO・経営責任者", "cxo chief officer executive management CxO 経営責任者 最高責任者", lambda: inset(bust("suit"), 3, 7, .9)+rect(2, 3, 13, 10, "#fff")+rect(49, 3, 13, 10, "#fff")+path('M15 8 H24 M40 8 H49 M8 13 V28 M56 13 V28', stroke=ACCENT)),
        ("chief-executive-officer", "最高経営責任者（CEO）", "ceo chief executive officer 最高経営責任者 経営 CEO", lambda: executive(organization())+path('M20 3 H35 M24 0 V6 M31 0 V6', stroke=ACCENT)),
        ("chief-financial-officer", "最高財務責任者（CFO）", "cfo chief financial officer 最高財務責任者 財務 CFO", lambda: executive(circle(26, 32, 19, "#fff")+circle(42, 42, 16, "#fff")+path('M20 21 L26 30 L32 21 M26 30 V42 M20 33 H32 M37 47 L47 37', stroke=ACCENT))),
        ("chief-technology-officer", "最高技術責任者（CTO）", "cto chief technology officer 最高技術責任者 技術 CTO", lambda: executive(gear())+path('M10 17 H16 M13 14 V20', stroke=ACCENT)),
        ("chief-information-security-officer", "最高情報セキュリティ責任者（CISO）", "ciso chief information security officer 最高情報セキュリティ責任者 CISO", lambda: executive(shield()+inset(key_shape(), 15, 11, .6))+path('M10 12 H18 M14 8 V16', stroke=ACCENT)),
    ])
    category("38 経営責任者・企業組織", "#815b95", "#f3eafa", [
        ("chief-information-officer", "最高情報責任者（CIO）", "cio chief information officer 最高情報責任者 情報システム CIO", lambda: executive(database()+path('M20 27 H44 M20 34 H44', stroke=ACCENT))),
        ("chief-operating-officer", "最高執行責任者（COO）", "coo chief operating officer 最高執行責任者 業務執行 COO", lambda: executive(gear()+check(29, 31))),
        ("chief-human-resources-officer", "最高人事責任者（CHRO）", "chro chief human resources officer 最高人事責任者 人事 CHRO", lambda: executive(organization()+person(32, 9, .55))),
        ("chief-data-officer", "最高データ責任者（CDO）", "cdo chief data officer 最高データ責任者 データ CDO", lambda: executive(database()+path('M20 42 L28 32 L36 37 L45 24', stroke=ACCENT, width=3))),
        ("chief-product-officer", "最高製品責任者（CPO）", "cpo chief product officer 最高製品責任者 プロダクト CPO", lambda: executive(path('M6 20 L32 7 L58 20 V48 L32 61 L6 48 Z', "#fff")+path('M6 20 L32 33 L58 20 M32 33 V61 M19 14 L45 27', stroke=ACCENT))),
        ("chief-legal-officer", "最高法務責任者（CLO）", "clo chief legal officer general counsel 最高法務責任者 法務 CLO", lambda: executive(scales()+circle(32, 14, 8, "#fff")+check(30, 13))),
        ("chief-risk-officer", "最高リスク管理責任者（CRO）", "cro chief risk officer 最高リスク管理責任者 リスク CRO", lambda: executive(shield()+inset(warning(), 15, 15, .56))),
        ("business-division", "事業部", "business division department unit 事業部 部門 部署", lambda: rect(20, 5, 24, 16, "#fff")+path('M32 21 V32 M12 32 H52 M12 32 V44 M52 32 V44', stroke=ACCENT)+rect(3, 44, 19, 16)+rect(42, 44, 19, 16)+path('M25 11 H39 M8 50 H17 M47 50 H56')),
        ("subsidiary-company", "子会社", "subsidiary company group affiliate 子会社 グループ会社 関係会社", lambda: inset(building(), -3, -1, .66)+inset(building(), 35, 27, .45)+path('M25 21 H48 V30 M43 25 L48 30 L53 25', stroke=ACCENT)),
        ("holding-company", "持株会社", "holding company group parent company 持株会社 ホールディングス 親会社", lambda: inset(civic_building(), 12, -1, .63)+path('M32 39 V45 M11 45 H53 M11 45 V50 M53 45 V50', stroke=ACCENT)+rect(3, 50, 16, 11, "#fff")+rect(24, 50, 16, 11, "#fff")+rect(45, 50, 16, 11, "#fff")),
    ])
    return entries, categories


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail when generated files differ")
    args = parser.parse_args()
    entries, categories = make_catalog()
    assert len(entries) == 376 and len({e["file"] for e in entries}) == 376
    assert len({e["svg"] for e in entries}) == 376
    assert len(categories) == 38
    assert all(len(rows) == (6 if name == "21 メッセージング製品" else 10) for name, rows in categories)
    inventory = ["# yydraw 業務・ITアイコン376種", "",
                 "システム構成図、業務フロー、プロジェクト計画、戦略・分析資料向けの独自SVGアイコンです。",
                 "左のカテゴリで「業務・IT / 01 …」～「業務・IT / 38 …」を選び、名前または検索語で絞り込めます。",
                 "名前と画像を表示し、ドラッグして配置します。配置した図柄だけが文書に保存されます。",
                 "ラベルは他のyyシリーズと同じフォントを使用します。図柄内の製品名も省略せずパスで描き、外部フォントに依存しません。", "",
                 "公式クラウドアイコン1,834種・イラストアイコン220種と合わせて2,430種です。公式クラウド素材とは別の独自図柄で、",
                 "このリポジトリと同じGPL-3.0-or-laterで提供します。製品名のアイコンも公式ロゴではなく、用途を表す独自イラストです。", "",
                 "一覧の検索語は、アプリのアイコン検索で使用できます。", ""]
    for name, rows in categories:
        inventory += [f"## {name}", "", "| 名前 | 検索語 |", "| --- | --- |"]
        inventory += [f"| {label} | {keywords} |" for label, keywords, _ in rows]
        inventory.append("")
    files = [(OUT, json.dumps(entries, ensure_ascii=False, indent=2) + "\n"),
             (INVENTORY, "\n".join(inventory))]
    for filename, content in files:
        if args.check:
            if not filename.exists() or filename.read_text(encoding="utf-8") != content:
                raise SystemExit(f"Generated file is out of date: {filename}")
        else:
            filename.parent.mkdir(parents=True, exist_ok=True)
            filename.write_text(content, encoding="utf-8", newline="\n")
    print(f"Business/IT catalog: {len(entries)} unique icons in {len(categories)} categories")


if __name__ == "__main__":
    main()
