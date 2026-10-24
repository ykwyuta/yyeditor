"""Optional encrypted refusal fixture; needs pypdf, never needed by yymerge or CI."""
from pathlib import Path
from pypdf import PdfReader, PdfWriter

root = Path(__file__).resolve().parent
writer = PdfWriter()
for page in PdfReader(root / "left.pdf").pages:
    writer.add_page(page)
writer.encrypt("yymerge-demo")
writer.write(root / "password.pdf")
