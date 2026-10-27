"""Original PDF comparison fixtures. Requires only the Python standard library."""
from pathlib import Path

root = Path(__file__).resolve().parent


def document(changed=False, changed_from=0, count_override=None):
    count = count_override or (3 if changed else 2)
    font = count * 2 + 3
    kids = " ".join(f"{i * 2 + 3} 0 R" for i in range(count))
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
               f"<< /Type /Pages /Kids [{kids}] /Count {count} >>".encode()]
    for i in range(count):
        rotation = 90 if i == 1 else 0
        objects.append((f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 400] /Rotate {rotation} "
                        f"/Resources << /Font << /F1 {font} 0 R >> >> /Contents {i * 2 + 4} 0 R >>").encode())
        colour = "0.05 0.55 0.60" if changed and i >= changed_from else "0.12 0.35 0.75"
        title = "Updated document" if changed and i >= changed_from else "Baseline document"
        content = (f"0 0 0 rg BT /F1 28 Tf 40 340 Td ({title}) Tj ET\n"
                   f"BT /F1 16 Tf 40 300 Td (Page {i + 1} - YY series PDF comparison example) Tj ET\n"
                   f"{colour} rg 40 130 200 120 re f\n"
                   "0.85 0.2 0.15 rg 300 130 120 120 re f\n"
                   "0 0 0 rg BT /F1 16 Tf 40 85 Td (Compare colours, text, rotation and added pages.) Tj ET\n")
        objects.append(f"<< /Length {len(content)} >>\nstream\n{content}endstream".encode())
    objects.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    out = bytearray(b"%PDF-1.4\n% Original YY series fixture\n")
    offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(out))
        out.extend(f"{i} 0 obj\n".encode() + obj + b"\nendobj\n")
    xref = len(out)
    out.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        out.extend(f"{offset:010} 00000 n \n".encode())
    out.extend((f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\n"
                f"startxref\n{xref}\n%%EOF\n").encode())
    return out


if __name__ == "__main__":
    (root / "left.pdf").write_bytes(document())
    (root / "right.pdf").write_bytes(document(True))
    (root / "later.pdf").write_bytes(document(True, changed_from=1, count_override=2))
