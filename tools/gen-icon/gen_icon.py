#!/usr/bin/env python3
"""yyeditor と yyterm のアイコン（apps/yyeditor/res/yyeditor.ico、apps/yyterm/res/yyterm.ico）を作る。

    python3 tools/gen-icon/gen_icon.py

角丸の四角に白の「YY」を描く。エディタは青地にテキストの行を表す線、ターミナルは黒地に
プロンプト（`>_`）。文字は図形で描くのでフォントに依存しない。各サイズを 4 倍で描いて縮小する。
Pillow が必要。
"""

import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SCALE = 4
EDITOR = ((0x3B, 0x82, 0xF6), (0x1D, 0x4E, 0xD8))
TERMINAL = ((0x37, 0x41, 0x51), (0x11, 0x18, 0x27))
PROMPT = (0x4A, 0xDE, 0x80, 255)
WHITE = (255, 255, 255, 255)
LINE = (255, 255, 255, 170)


def draw_y(d, x, y, w, h, t, a):
    """左上 (x, y)、幅 w・高さ h、線の太さ t、腕の上端の幅 a の「Y」（上端が水平な多角形）。"""
    cx = x + w / 2
    mid = y + h * 0.5
    d.polygon([(x, y), (x + a, y), (cx + t / 2, mid), (cx - t / 2, mid)], fill=WHITE)
    d.polygon([(x + w - a, y), (x + w, y), (cx + t / 2, mid), (cx - t / 2, mid)], fill=WHITE)
    # 腕の合流点から下へ
    d.polygon(
        [(cx - t / 2, mid - t * 0.6), (cx + t / 2, mid - t * 0.6), (cx + t / 2, y + h), (cx - t / 2, y + h)],
        fill=WHITE,
    )


def render(size, terminal=False):
    top_c, bottom_c = TERMINAL if terminal else EDITOR
    n = size * SCALE
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    # 縦のグラデーション
    grad = Image.new("RGBA", (n, n))
    gd = ImageDraw.Draw(grad)
    for yy in range(n):
        f = yy / max(n - 1, 1)
        c = tuple(round(top_c[i] + (bottom_c[i] - top_c[i]) * f) for i in range(3))
        gd.line([(0, yy), (n, yy)], fill=c + (255,))
    mask = Image.new("L", (n, n), 0)
    margin = n * (0.03 if size >= 32 else 0.0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [margin, margin, n - 1 - margin, n - 1 - margin], radius=n * 0.2, fill=255
    )
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    small = size < 32
    # 小さいサイズは「YY」だけを大きく描く
    if small:
        # V の切れ込みがつぶれないよう、腕を細く・文字を幅広にする
        t, a = n * 0.12, n * 0.12
        top, h = n * 0.16, n * 0.68
        w, gap = n * 0.4, n * 0.04
    else:
        t, a = n * 0.1, n * 0.12
        top, h = n * 0.14, n * 0.44
        w, gap = n * 0.3, n * 0.06
    x0 = (n - (w * 2 + gap)) / 2
    draw_y(d, x0, top, w, h, t, a)
    draw_y(d, x0 + w + gap, top, w, h, t, a)
    if not small and terminal:
        # プロンプト「>_」
        lt = n * 0.06
        left, yy = n * 0.2, n * 0.7
        size_p = n * 0.16
        d.line([(left, yy), (left + size_p * 0.7, yy + size_p / 2), (left, yy + size_p)], fill=PROMPT, width=round(lt))
        ux = left + size_p * 0.95
        d.rectangle([ux, yy + size_p - lt, ux + size_p * 0.9, yy + size_p], fill=PROMPT)
    elif not small:
        # テキストの行
        lt = n * 0.055
        left = n * 0.18
        for i, right in enumerate([0.82, 0.66]):
            yy = n * (0.7 + i * 0.12)
            d.rounded_rectangle([left, yy, n * right, yy + lt], radius=lt / 2, fill=LINE)
    return img.resize((size, size), Image.LANCZOS)


def dib(img):
    """アイコン用の 32 ビット DIB（上下反転した BGRA と AND マスク）。"""
    w, h = img.size
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    px = img.load()
    rows = []
    for y in reversed(range(h)):
        rows.append(b"".join(bytes((b, g, r, a)) for r, g, b, a in (px[x, y] for x in range(w))))
    mask_row = ((w + 31) // 32) * 4
    return header + b"".join(rows) + bytes(mask_row * h)


def write_ico(path, images):
    """ICO を書く。256 は PNG、それ以外は BMP（古い読み込み処理でも表示できるように）。"""
    blobs = []
    for img in images:
        if img.size[0] >= 256:
            buf = io.BytesIO()
            img.save(buf, format="PNG", optimize=True)
            blobs.append(buf.getvalue())
        else:
            blobs.append(dib(img))
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for img, blob in zip(images, blobs):
        w = img.size[0]
        out += struct.pack("<BBBBHHII", w % 256, w % 256, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    path.write_bytes(out + b"".join(blobs))


def main():
    root = Path(__file__).resolve().parents[2]
    for app, terminal in [("yyeditor", False), ("yyterm", True)]:
        res = root / "apps" / app / "res"
        res.mkdir(parents=True, exist_ok=True)
        images = [render(s, terminal) for s in SIZES]
        write_ico(res / f"{app}.ico", images)
        images[-1].save(res / f"{app}-256.png")
        print(res / f"{app}.ico")


if __name__ == "__main__":
    main()
