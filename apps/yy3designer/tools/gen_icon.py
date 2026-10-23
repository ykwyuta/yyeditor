"""Reuse the yy icon frame, YY lettering, sizes and ICO writer."""
import importlib.util
import sys
from pathlib import Path

sys.dont_write_bytecode = True

root = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("yy_icons", root / "tools/gen-icon/gen_icon.py")
icons = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icons)
icons.DRAW = ((249, 115, 22), (154, 52, 18))


def voxel(d, kind, x, y, w, h, t, ot):
    cx = x + w / 2
    d.polygon([(cx, y), (x+w, y+h*.25), (cx, y+h*.5), (x, y+h*.25)], fill=(255, 255, 255, 255))
    d.polygon([(x, y+h*.25), (cx, y+h*.5), (cx, y+h), (x, y+h*.75)], fill=(255, 255, 255, 190))
    d.polygon([(cx, y+h*.5), (x+w, y+h*.25), (x+w, y+h*.75), (cx, y+h)], fill=(255, 255, 255, 125))


icons.draw_symbol = voxel
res = root / "apps/yy3designer/app/res"
res.mkdir(parents=True, exist_ok=True)
images = [icons.render(size, "draw") for size in icons.SIZES]
icons.write_ico(res / "yy3designer.ico", images)
images[-1].save(res / "yy3designer-256.png")
