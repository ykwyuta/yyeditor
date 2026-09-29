#!/usr/bin/env python3
"""プレビューで使う mermaid と KaTeX を npm から取得して crates/yy-preview/assets/ に置く。

    python3 tools/fetch-preview-assets/fetch.py [mermaid のバージョン] [KaTeX のバージョン]

JS・CSS は zlib で圧縮して `.z` として置く（実行ファイルへの埋め込みを小さくするため。
プレビューが最初に要求したときに展開する）。KaTeX のフォントは woff2 だけを置く。
"""

import io
import json
import sys
import tarfile
import urllib.request
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "yy-preview" / "assets"


def package(name, version):
    meta = json.load(urllib.request.urlopen(f"https://registry.npmjs.org/{name}/{version}"))
    data = urllib.request.urlopen(meta["dist"]["tarball"]).read()
    print(f"{name} {meta['version']}")
    return meta["version"], tarfile.open(fileobj=io.BytesIO(data), mode="r:gz")


def read(tar, path):
    return tar.extractfile(f"package/{path}").read()


def put(rel, data, compress):
    dest = OUT / (rel + ".z" if compress else rel)
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(zlib.compress(data, 9) if compress else data)


def main():
    mermaid_ver = sys.argv[1] if len(sys.argv) > 1 else "latest"
    katex_ver = sys.argv[2] if len(sys.argv) > 2 else "latest"
    versions = []

    v, tar = package("mermaid", mermaid_ver)
    versions.append(f"mermaid {v}")
    put("mermaid.min.js", read(tar, "dist/mermaid.min.js"), True)
    put("LICENSE-mermaid.txt", read(tar, "LICENSE"), False)

    v, tar = package("katex", katex_ver)
    versions.append(f"KaTeX {v}")
    put("katex.min.js", read(tar, "dist/katex.min.js"), True)
    put("katex.min.css", read(tar, "dist/katex.min.css"), True)
    put("LICENSE-katex.txt", read(tar, "LICENSE"), False)
    for m in tar.getmembers():
        if m.name.startswith("package/dist/fonts/") and m.name.endswith(".woff2"):
            put("fonts/" + m.name.rsplit("/", 1)[1], tar.extractfile(m).read(), False)

    (OUT / "VERSIONS.txt").write_text("\n".join(versions) + "\n")


if __name__ == "__main__":
    main()
