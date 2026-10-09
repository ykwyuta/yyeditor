#!/usr/bin/env python3
"""yyeditor・yyterm・yysftp・yysheet・yyclip・yyfilemanager・yybrowser のアイコンを作る。

    python3 tools/gen-icon/gen_icon.py

角丸の四角に白の「YY」を描く。エディタは青地にテキストの行を表す線、ターミナルは黒地に
プロンプト（`>_`）、ファイル転送は緑地に上下の矢印、スプレッドシートは橙地に表の格子、
クリップボード履歴は紫地にクリップボード、ファイル管理は水色地にフォルダ、ブラウザはローズ地に地球。
どのサイズ（16 px も）でも YY の下に記号を描き、色だけの違いにしない。文字は図形で描くのでフォントに依存しない。各サイズを 4 倍で描いて縮小する。
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
TRANSFER = ((0x10, 0xB9, 0x81), (0x04, 0x78, 0x57))
SHEET = ((0xF5, 0x9E, 0x0B), (0xB4, 0x53, 0x09))
CLIP = ((0xA8, 0x55, 0xF7), (0x6B, 0x21, 0xA8))
FILES = ((0x06, 0xB6, 0xD4), (0x0E, 0x74, 0x90))
BROWSER = ((0xF4, 0x3F, 0x5E), (0x9F, 0x12, 0x39))  # ローズ（ほかの色と重ならない）
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


def draw_symbol(d, kind, x, y, w, h, t, ot):
    """アプリの記号を枠（左上 (x, y)、幅 w・高さ h）の中に描く。t は太い線（行・矢印・プロンプト）、
    ot は輪郭の線（格子・用紙・地球）の太さ。"""
    if kind == "terminal":
        # プロンプト「>_」
        size_p = h
        left = x + (w - size_p * 1.9) / 2
        d.line(
            [(left, y), (left + size_p * 0.65, y + size_p / 2), (left, y + size_p)],
            fill=PROMPT,
            width=round(t),
            joint="curve",
        )
        ux = left + size_p * 0.95
        d.rectangle([ux, y + size_p - t, ux + size_p * 0.95, y + size_p], fill=PROMPT)
    elif kind == "transfer":
        # 上向きと下向きの矢印
        ah = h * 0.55
        for cx, up in [(x + w * 0.3, True), (x + w * 0.7, False)]:
            y0, y1 = y, y + h
            d.rectangle([cx - t / 2, y0 + (ah * 0.6 if up else 0), cx + t / 2, y1 - (0 if up else ah * 0.6)], fill=WHITE)
            tip, base = (y0, y0 + ah) if up else (y1, y1 - ah)
            d.polygon([(cx, tip), (cx - ah * 0.65, base), (cx + ah * 0.65, base)], fill=WHITE)
    elif kind == "sheet":
        # 表の格子（3 列 × 2 行）
        for i in range(4):
            xx = x + (w - ot) * i / 3
            d.rectangle([xx, y, xx + ot, y + h], fill=WHITE)
        for j in range(3):
            yy = y + (h - ot) * j / 2
            d.rectangle([x, yy, x + w, yy + ot], fill=WHITE)
    elif kind == "clip":
        # クリップ付きの用紙
        bw = min(w * 0.6, h * 1.25)
        bx = x + (w - bw) / 2
        d.rounded_rectangle([bx, y + h * 0.2, bx + bw, y + h], radius=ot, outline=WHITE, width=round(ot))
        d.rounded_rectangle([bx + bw * 0.25, y, bx + bw * 0.75, y + h * 0.38], radius=ot / 2, fill=WHITE)
        # 用紙の行
        d.rectangle([bx + bw * 0.25, y + h * 0.58, bx + bw * 0.75, y + h * 0.58 + ot], fill=WHITE)
    elif kind == "files":
        # フォルダ（つまみ付き）
        d.rounded_rectangle([x, y, x + w * 0.45, y + h * 0.4], radius=t / 2, fill=WHITE)
        d.rounded_rectangle([x, y + h * 0.18, x + w, y + h], radius=t / 2, fill=WHITE)
    elif kind == "browser":
        # 地球（円と経線・緯線）
        r = h / 2
        cx, cy = x + w / 2, y + r
        lt = max(1, round(ot))
        d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=WHITE, width=lt)
        if lt < r * 0.25:
            d.ellipse([cx - r * 0.42, cy - r, cx + r * 0.42, cy + r], outline=WHITE, width=lt)
        else:
            # 小さいときは線が重なってつぶれるので、経線は 1 本にする
            d.rectangle([cx - lt / 2, cy - r, cx + lt / 2, cy + r], fill=WHITE)
        d.rectangle([cx - r, cy - lt / 2, cx + r, cy + lt / 2], fill=WHITE)
    else:
        # テキストの行
        for i, right in enumerate([1.0, 0.62]):
            yy = y + (h - t) * i
            d.rounded_rectangle([x, yy, x + w * right, yy + t], radius=t / 2, fill=WHITE)


def render(size, kind="editor"):
    top_c, bottom_c = {
        "editor": EDITOR,
        "terminal": TERMINAL,
        "transfer": TRANSFER,
        "sheet": SHEET,
        "clip": CLIP,
        "files": FILES,
        "browser": BROWSER,
    }[kind]
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
    # どのサイズでも「YY」の下にアプリの記号を描く（色だけの違いにしない）。小さいサイズは YY を
    # 小さめにして、記号を太く大きく描く（16 px でも形で見分けられるように）
    if size < 40:
        t, a = n * 0.11, n * 0.1
        top, h = n * 0.08, n * 0.4
        w, gap = n * 0.34, n * 0.05
        box = (n * 0.14, n * 0.56, n * 0.72, n * 0.36)
        st = n * 0.12
        ot = n * 0.075
    else:
        t, a = n * 0.1, n * 0.12
        top, h = n * 0.14, n * 0.44
        w, gap = n * 0.3, n * 0.06
        box = (n * 0.22, n * 0.65, n * 0.56, n * 0.22)
        st = n * 0.055
        ot = n * 0.03
    x0 = (n - (w * 2 + gap)) / 2
    draw_y(d, x0, top, w, h, t, a)
    draw_y(d, x0 + w + gap, top, w, h, t, a)
    draw_symbol(d, kind, *box, st, ot)
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
    for app, kind in [
        ("yyeditor", "editor"),
        ("yyterm", "terminal"),
        ("yysftp", "transfer"),
        ("yysheet", "sheet"),
        ("yyclip", "clip"),
        ("yyfilemanager", "files"),
        ("yybrowser", "browser"),
    ]:
        res = root / "apps" / app / "res"
        res.mkdir(parents=True, exist_ok=True)
        images = [render(s, kind) for s in SIZES]
        write_ico(res / f"{app}.ico", images)
        images[-1].save(res / f"{app}-256.png")
        print(res / f"{app}.ico")


if __name__ == "__main__":
    main()
