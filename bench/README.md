# Engine benchmark

Runs the same workload on MuPDF, PDFium and hayro so we can compare them, and later our own render pipeline, on equal terms.

Workload per file:
- **open**: open the document and read every page size
- **render**: first 40 pages at 150 DPI, single-threaded
- **re-render**: page 1 again at 300 DPI (the page has already been loaded)
- **tile**: one 1024×1024 tile from the middle of page 1 at 400% zoom
- **parallel** (hayro only): the same 40 pages on all cores

## Setup

```bash
python -m venv .venv
.venv/Scripts/python -m pip install pymupdf pypdfium2 pillow
```

```bash
cd hayro-bench && cargo build --release
```

`hayro-bench` pins Rust 1.99.0 through `rust-toolchain.toml`, because hayro needs 1.92 or newer.

## Test files (put them in `pdfs/`)

| file | source | stresses |
|---|---|---|
| paper.pdf | https://raw.githubusercontent.com/mozilla/pdf.js/master/web/compressed.tracemonkey-pldi-09.pdf | text + vector figures |
| survey.pdf | https://arxiv.org/pdf/2303.18223v16 | 144 pages of dense text, some JPEGs |
| tubemap.pdf | https://content.tfl.gov.uk/standard-tube-map.pdf | heavy vector art |
| naca.pdf | https://ntrs.nasa.gov/api/citations/19930091108/downloads/19930091108.pdf | scanned pages (CCITT fax images) |

## Run

```bash
for f in paper survey tubemap naca; do
  .venv/Scripts/python -I bench_c.py mupdf  pdfs/$f.pdf 40 out/$f
  .venv/Scripts/python -I bench_c.py pdfium pdfs/$f.pdf 40 out/$f
  hayro-bench/target/release/hayro-bench pdfs/$f.pdf 40 out/$f
done
```

```bash
.venv/Scripts/python -I compare.py out out/sheet.png
```

`compare.py` diffs every engine's page 1 and tile against MuPDF and writes a side-by-side contact sheet, so a fast-but-wrong renderer gets caught.

Each line of output is one JSON record. The results from 2026-10-30 are in `results-2026-10-30.jsonl`.

## UI memory (`ui-mem/`)

Measures what each windowing and UI stack costs in memory, against Sumatra. Each binary opens a 1000×750 window and, given a PDF, shows page 1 rendered by hayro:

| binary | stack |
|---|---|
| `gdi` | winit + softbuffer: drawn on the CPU, blitted with GDI like Sumatra |
| `egui-cpu` | egui drawn on the CPU by `egui_software_backend` |
| `egui-glow` | egui through eframe's OpenGL renderer |
| `egui-wgpu` | egui through eframe's wgpu renderer (DX12); `MEM=low` sets `MemoryHints::MemoryUsage` |
| `wgpu-clear` | wgpu clearing the window, nothing else; `WGPU_BACKEND` and `MEM=low` |

`measure.ps1` also runs the reader itself (`pdfreader`, built from the repo root with `cargo build --release`). The test binaries above aren't kept built; entries whose exe is missing are skipped with a warning.

```bash
cd ui-mem
for p in gdi egui-cpu egui-glow egui-wgpu@0.1.0 wgpu-clear; do cargo build --release -p $p; done
```

Build each package on its own so features don't unify across stacks. `egui-wgpu@0.1.0` avoids a clash with the `egui-wgpu` crate eframe depends on.

`measure.ps1` needs a portable copy of Sumatra: copy `SumatraPDF.exe` and `libmupdf.dll` into an empty folder with a `SumatraPDF-settings.txt` (see the script header), so it doesn't restore your own session.

```powershell
powershell -File ui-mem\measure.ps1 -Sumatra <copy>\SumatraPDF.exe -Pdf pdfs\paper.pdf
```

Add `-Only '*+ pdf'` to run only the entries with a document open, and `-Scroll 150` to send 150 wheel notches before measuring, which shows what the page caches grow to.

The UI stack results from 2026-10-30 are in `ui-mem/results-2026-10-30.jsonl` and summarized in `docs/architecture.md` §5. The reader against Sumatra is in `ui-mem/results-2026-10-30-reader.jsonl`, summarized in §9.

## CPU (`ui-mem/cpu.ps1`)

Runs Sumatra and the reader through the same script and reads each process's total CPU time: opening a file, 10 s idle, 20 × Page Down, then zooming in 3 steps and out 3. Keys go to the window as messages, so the window doesn't need focus. Median of 3 runs; `-Apps pdfreader` runs one app only.

This machine's speed drifts by up to 2× over minutes, so compare apps or builds by alternating single runs (`-Runs 1` in a loop), not by measurements taken at different times.

```powershell
powershell -File ui-mem\cpu.ps1 -Sumatra <copy>\SumatraPDF.exe -Pdf pdfs\survey.pdf
```

Results: `ui-mem/results-2026-10-30-cpu-before.jsonl` (before display lists), `ui-mem/results-2026-10-30-cpu-after.jsonl`.

## Our engine (`crates/engine/examples/bench.rs`)

The workload above through our `Engine` trait, once with display lists and the glyph cache and once without, plus each page cut into the viewer's 512 px tiles. It also diffs the two outputs; with `BENCH_CROPS=<dir>` it saves a crop of page 1 from each, to compare by eye.

```bash
cargo run --release -p engine --example bench -- pdfs/paper.pdf pdfs/survey.pdf
```

Results from 2026-10-30: `results-2026-10-30-engine.jsonl`. Timings on this machine vary up to 2× between runs, so compare within a run.
