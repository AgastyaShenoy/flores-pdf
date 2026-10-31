"""Time MuPDF (PyMuPDF) and PDFium (pypdfium2) on the same workload as hayro-bench.

usage: python -I bench_c.py <engine: mupdf|pdfium> <pdf> <max_pages> <png_out_dir>
Prints one JSON line with timings in ms.
"""
import json
import os
import sys
import time

DPI_SCALE = 150 / 72  # page render at 150 DPI
ZOOM_SCALE = 300 / 72  # re-render of an already-loaded page at 300 DPI
TILE_SCALE = 4.0  # 400% zoom
TILE_PX = 1024


def ms(t0):
    return (time.perf_counter() - t0) * 1000


def bench_mupdf(path, max_pages, out_dir):
    import pymupdf

    r = {"engine": "mupdf " + pymupdf.mupdf_version}
    t0 = time.perf_counter()
    doc = pymupdf.open(path)
    n = doc.page_count
    sizes = [doc.page_cropbox(i) if hasattr(doc, "page_cropbox") else doc[i].rect for i in range(n)]
    r["open_ms"] = ms(t0)
    r["pages"] = n
    count = min(n, max_pages)
    m = pymupdf.Matrix(DPI_SCALE, DPI_SCALE)

    per = []
    t_all = time.perf_counter()
    for i in range(count):
        t0 = time.perf_counter()
        pix = doc[i].get_pixmap(matrix=m, alpha=False)
        per.append(ms(t0))
        if i == 0:
            pix.save(os.path.join(out_dir, "mupdf_p1.png"))
    r["render_total_ms"] = ms(t_all)
    r["first_page_ms"] = per[0]
    r["render_pages"] = count

    # zoom change on an already-parsed page: Sumatra replays the cached display list
    page = doc[0]
    dl = page.get_displaylist()
    t0 = time.perf_counter()
    dl.get_pixmap(matrix=pymupdf.Matrix(ZOOM_SCALE, ZOOM_SCALE), alpha=False)
    r["rezoom_cached_ms"] = ms(t0)
    t0 = time.perf_counter()
    page.get_pixmap(matrix=pymupdf.Matrix(ZOOM_SCALE, ZOOM_SCALE), alpha=False)
    r["rezoom_uncached_ms"] = ms(t0)

    # one 1024x1024 tile from the middle of page 1 at 400%
    rect = page.rect
    cx, cy = rect.width / 2, rect.height / 2
    half = TILE_PX / TILE_SCALE / 2
    clip = pymupdf.Rect(cx - half, cy - half, cx + half, cy + half)
    t0 = time.perf_counter()
    pix = page.get_pixmap(matrix=pymupdf.Matrix(TILE_SCALE, TILE_SCALE), clip=clip, alpha=False)
    r["tile_ms"] = ms(t0)
    pix.save(os.path.join(out_dir, "mupdf_tile.png"))
    return r


def bench_pdfium(path, max_pages, out_dir):
    import pypdfium2 as pdfium

    r = {"engine": "pdfium " + str(pdfium.PDFIUM_INFO.build)}
    t0 = time.perf_counter()
    doc = pdfium.PdfDocument(path)
    n = len(doc)
    sizes = [doc.get_page_size(i) for i in range(n)]
    r["open_ms"] = ms(t0)
    r["pages"] = n
    count = min(n, max_pages)

    per = []
    t_all = time.perf_counter()
    for i in range(count):
        t0 = time.perf_counter()
        page = doc[i]
        bmp = page.render(scale=DPI_SCALE)
        per.append(ms(t0))
        if i == 0:
            bmp.to_pil().save(os.path.join(out_dir, "pdfium_p1.png"))
        page.close()
    r["render_total_ms"] = ms(t_all)
    r["first_page_ms"] = per[0]
    r["render_pages"] = count

    # zoom change on an already-loaded page (PDFium keeps the parsed page objects)
    page = doc[0]
    page.render(scale=DPI_SCALE)
    t0 = time.perf_counter()
    page.render(scale=ZOOM_SCALE)
    r["rezoom_cached_ms"] = ms(t0)
    page.close()
    t0 = time.perf_counter()
    page = doc[0]
    page.render(scale=ZOOM_SCALE)
    r["rezoom_uncached_ms"] = ms(t0)

    # one 1024x1024 tile from the middle of page 1 at 400%; crop is in PDF points
    w, h = page.get_size()
    side = TILE_PX / TILE_SCALE
    left = (w - side) / 2
    bottom = (h - side) / 2
    t0 = time.perf_counter()
    bmp = page.render(scale=TILE_SCALE, crop=(left, bottom, w - left - side, h - bottom - side))
    r["tile_ms"] = ms(t0)
    bmp.to_pil().save(os.path.join(out_dir, "pdfium_tile.png"))
    return r


def main():
    engine, path, max_pages, out_dir = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
    os.makedirs(out_dir, exist_ok=True)
    fn = bench_mupdf if engine == "mupdf" else bench_pdfium
    r = fn(path, max_pages, out_dir)
    r["file"] = os.path.basename(path)
    print(json.dumps(r))


if __name__ == "__main__":
    main()
