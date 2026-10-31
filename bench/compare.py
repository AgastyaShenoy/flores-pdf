"""Pixel-compare page-1 and tile renders against MuPDF, and build a side-by-side contact sheet.
usage: python -I compare.py <bench-out dir> <sheet.png>"""
import os, sys
from PIL import Image, ImageChops

out, sheet_path = sys.argv[1], sys.argv[2]
rows = []
for f in sorted(os.listdir(out)):
    d = os.path.join(out, f)
    if not os.path.isdir(d):
        continue
    for kind in ("p1", "tile"):
        ref = Image.open(os.path.join(d, f"mupdf_{kind}.png")).convert("RGB")
        imgs = [ref]
        for eng in ("pdfium", "hayro"):
            img = Image.open(os.path.join(d, f"{eng}_{kind}.png")).convert("RGB")
            if img.size != ref.size:
                print(f"{f:8} {kind:4} {eng:6} size {img.size} vs mupdf {ref.size}")
                img = img.resize(ref.size)
            diff = ImageChops.difference(ref, img).convert("L")
            hist = diff.histogram()
            total = sum(hist)
            bad = sum(hist[32:]) / total * 100
            mean = sum(i * c for i, c in enumerate(hist)) / total
            print(f"{f:8} {kind:4} {eng:6} mean|diff| {mean:5.2f}  pixels off by >32: {bad:5.2f}%")
            imgs.append(img)
        rows.append(imgs)
# contact sheet: each row = mupdf | pdfium | hayro, scaled to 300px tall
H = 300
sheet_rows = []
for imgs in rows:
    scaled = [im.resize((max(1, im.width * H // im.height), H)) for im in imgs]
    w = sum(s.width for s in scaled) + 10 * (len(scaled) - 1)
    row = Image.new("RGB", (w, H), "gray")
    x = 0
    for s in scaled:
        row.paste(s, (x, 0)); x += s.width + 10
    sheet_rows.append(row)
W = max(r.width for r in sheet_rows)
sheet = Image.new("RGB", (W, len(sheet_rows) * (H + 10)), "gray")
for i, r in enumerate(sheet_rows):
    sheet.paste(r, (0, i * (H + 10)))
sheet.save(sheet_path)
print("sheet", sheet.size)
