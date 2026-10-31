# Flores PDF: engine choice and architecture

The reader was called pdfreader until it was named Flores PDF; the measurements below were taken under the old name.

Status, 2026-10-31: a working viewer is built (§9). It opens PDFs (with passwords too), EPUB, DjVu (with its text layer), CBZ, CBR and CB7 comics and JPEG, PNG, GIF and WebP pictures, fills in PDF forms and annotates PDFs (highlights, underlines, strikeouts, notes, drawing, text, shapes), saving both into the file, scrolls and zooms, searches, selects and copies text, follows links with back and forward, shows the outline and bookmarks in a sidebar, turns pages and lays them out two at a time or as a book, presents them full screen, prints, highlights text, reloads files that change, opens files in tabs of one window, remembers recent files (with thumbnails on the start page) and where you were in each, scrolls smoothly, copies or saves pages as images, shows document properties, and has a toolbar, a main menu, right-click menus, a settings panel, light, dark and black (OLED) themes, and dark, sepia or black page colors. Display lists and a glyph cache make text pages render about 40% faster (§9).

## 1. What we measured

MuPDF 1.28.5 (via PyMuPDF), PDFium 156 (via pypdfium2) and hayro 0.8.0 (pure Rust) were run on the same machine (16 logical cores). Each number is the better of 2 runs, in milliseconds, single-threaded unless noted. The harness and raw data are in [`../bench`](../bench).

**Render pages at 150 DPI** (total for the first 40 pages, or all pages if fewer)

| file | what it stresses | MuPDF | PDFium | hayro | hayro, 16 threads |
|---|---|---:|---:|---:|---:|
| paper (14 p) | text + vector figures | **197** | 214 | 254 | 46 |
| survey (40 of 144 p) | dense text | **480** | 598 | 780 | 167 |
| tubemap (2 p) | heavy vector art | 362 | 240 | **141** | 61 |
| naca (40 of 61 p) | scanned (CCITT) | 807 | 669 | **476** | 66 |
| **all four** | | 1846 | 1720 | **1651** | **340** |

**Re-render page 1 at 300 DPI** (page already loaded once)

| file | MuPDF | PDFium | hayro |
|---|---:|---:|---:|
| paper | **9** | 17 | 24 |
| survey | **9** | 20 | 26 |
| tubemap | 407 | 214 | **99** |
| naca | **21** | 118 | 71 |

**One 1024×1024 tile at 400% zoom**

| file | MuPDF | PDFium | hayro |
|---|---:|---:|---:|
| paper | 7.2 | **4.1** | 4.6 |
| survey | 5.9 | 4.3 | 4.3 |
| tubemap | 54 | **17.7** | 19.6 |
| naca | **5.0** | 8.2 | 36 |

**Open and read all page sizes**: under 8 ms for every engine on every file. The one exception is MuPDF on tubemap at 21 ms.

**Correctness**: all three engines produce visually identical pages, and a contact sheet was checked by eye. Tile diffs against MuPDF stay under 10% of pixels and come from anti-aliasing differences.

Caveats: one machine, 2 runs, no 700+ page document (the download of the PDF spec failed). The Python bindings add microseconds per call, which is negligible next to these times.

### What the numbers say

1. **No engine wins everywhere.** MuPDF is fastest on text, because it caches glyphs. hayro is fastest on vector art and scans. PDFium sits in between. Across all four files, single-threaded totals are within 12% of each other.
2. **A normal page takes 10–20 ms on any engine.** That's already about one 60 Hz frame. Perceived speed comes from scheduling (Sumatra's tricks), not from raw raster speed. Heavy pages (60–400 ms) are where tiling, cancellation and placeholder images matter.
3. **hayro's weak spots are exactly the ones its maintainer lists:** no glyph cache (dense text is 1.6× slower than MuPDF), and it re-interprets the whole page on every render, so there's no display list. Re-zoom is slow, and a tile of a scanned page costs 36 ms against MuPDF's 5 ms. It probably re-decodes the whole scan for each tile. All of these are fixable caching problems, not design limits.
4. **hayro's big advantage is thread safety.** Pages of one document render in parallel: 4.7–7× faster on the multi-page files (tubemap has only 2 pages), and 5× faster than single-threaded MuPDF overall. PDFium can't do this in one process because it isn't thread-safe. Sumatra doesn't do it either: MuPDF can, but Sumatra serializes each document behind a `renderLock`.

## 2. Should we write our own engine?

**Not from scratch.** A full PDF engine needs:
- a parser that repairs broken files
- encryption support
- 8 compression filters, including JBIG2, JPEG 2000 and CCITT
- 5 font formats, plus CID/CMap and CJK fallbacks
- about 10 color space types
- 7 shading types
- transparency groups, soft masks and blend modes
- annotation appearances, forms, and optional content

hayro has 25+ contributors over more than a year and still lacks knockout groups and CMYK blending. A from-scratch engine would be built from the same Rust parts hayro uses: `skrifa`/`read-fonts` for fonts, `vello_cpu` for rasterization, and the hayro image decoders. So it would at best match hayro, after a long time.

**What is worth owning is the layer above the interpreter.** That layer is where Sumatra's speed comes from, and where hayro is weakest:

- **Display list**: hayro exposes a public `Device` trait. We implement a recording device that captures a page once, then replay it at any zoom or tile with bounding-box culling. That fixes re-zoom and tile cost, and gives us cancellation for free, since our replay loop can check a flag between commands. hayro itself has no cancellation. *Built; it helped less than expected. Interpreting a page turned out to be cheap: the time goes into vello_cpu turning glyph outlines into pixels, which a display list doesn't avoid. See §9.*
- **Glyph and image caches**: cache rasterized glyphs per (font, glyph, size), and decoded images per page. *Both built. The image cache is the big win on scans, the glyph cache on text (§9).*
- **Tiles, scheduler, compositor**: see §3.

That's "our own engine" in the sense that matters for speed. The measurable goal is to beat MuPDF's re-zoom and tile numbers above while keeping hayro's parallel speedup.

### Engine choice

| | hayro | PDFium | MuPDF |
|---|---|---|---|
| License | MIT/Apache | BSD-3 | **AGPL-3.0**: the whole app must be AGPL, or you buy a license from Artifex |
| Language | pure Rust, `#![forbid(unsafe_code)]` | C++ DLL (~6 MB) | C, needs libclang to build |
| Threads | page-parallel | everything serialized behind one mutex | per-thread contexts |
| Cancel mid-render | no (we add it in our replay) | progressive render API | yes (`fz_cookie`) |
| Editing, forms, annotations, save | **no, read-only** | yes | yes |
| Known risks | some pathological pages use huge memory (one public report: 14.8 GB) | slower on vector art and scans | license |

**Recommendation:** put the engine behind an `Engine` trait.
- hayro is the primary renderer, wrapped in our display list and caches.
- Add PDFium later for:
  - form filling, annotation editing and saving, which hayro can't do
  - a fallback for pages hayro fails on or that blow a memory or time budget
- MuPDF is only an option if you're happy for the app to be AGPL open source.

## 3. Sumatra's techniques in Rust

| Sumatra does | We do | Better because |
|---|---|---|
| Renders visible pages, plus a chain of up to 4 predicted pages | Same, computed from the viewport each frame | — |
| LIFO queue with 8 slots, aborts via `fz_cookie` | Priority queue (visible, then nearby, then predicted, nearest to the viewport center first), a generation counter, and an `Arc<AtomicBool>` cancel token checked in our replay loop | Explicit priorities instead of queue-order tricks |
| Stretches an old-zoom bitmap with GDI while re-rendering | Same: scale the cached tiles on the CPU while sharp ones render. A tiny thumbnail of each page is the last-resort placeholder | Never a blank page |
| Power-of-two tiles about the size of the screen | Fixed 512 px tiles per zoom level, like a map viewer | Finer cache granularity, cheap partial redraws |
| One MuPDF display list per page, and MuPDF's glyph cache | Our own display list, recorded from hayro's `Device` into plain data shared by all render threads, dropped after 3 s idle. Text that nothing covers is stamped from a shared glyph cache after vello_cpu draws the rest | Text pages render about 40% faster (§9) |
| Cache capped at 256 bitmaps by count (Sumatra's own TODO says it should be by bytes) | LRU with a byte budget (a setting, in windowfuls); tiles on screen or about to be are never evicted | Memory follows actual size |
| Page sizes read from page dictionaries, without parsing contents | hayro already does this: 144 pages open in 4.8 ms | — |
| Files under 32 MB read into memory (so LaTeX tools can overwrite them), larger ones read on demand | Same rule: `Vec<u8>` for small files, `memmap2` for big ones | — |
| Render threads up to the core count, but one document is serialized | Up to 4 render threads; tiles of every page render in parallel. Idle threads drop their caches | 4.7–7× measured on multi-page files |
| Thread-local temp arena reset every message-loop iteration | The Windows segment heap, switched on in the exe's manifest. It hands freed memory back to Windows | Cut 2.5 MB at startup (§9). `mimalloc` and arenas not needed so far |
| Bitmaps stored raw | Tiles stored run-length encoded when that saves a quarter or more; decoded straight into the window buffer | Text pages take a fraction of the memory (§9) |
| `/O1` + LTO everywhere, `/O2` for codecs | Planned: `opt-level = "s"` workspace-wide, `opt-level = 3` for hayro, vello_cpu and decoders, fat LTO, one codegen unit. Today it's plain `opt-level = 3` | Same idea, Cargo supports it natively |
| GDI blits | Same, through `softbuffer` (a GDI blit on Windows). A GPU compositor only as an opt-in setting | §5: a GPU backend costs 40–100 MB more |
| A text file for settings | One table of settings (label, help text, kind, range) that generates both the settings file and the settings panel. Changes apply live. No serde: a flat TOML subset, written with its own comments | What you asked for |

New things Sumatra doesn't have:
- a per-page memory and time budget, with an error tile or PDFium fallback instead of a crash
- text extraction and a search index built in the background
- page-parallel thumbnails

## 4. Proposed shape

```
UI thread: winit event loop, UI chrome, CPU compositor (copies cached tiles into the window buffer, GDI blit)
   │ viewport / zoom / scroll changes
   ▼
Scheduler: works out which tiles are needed, priorities, cancels stale jobs
   │ jobs
   ▼
Render threads: Engine trait (hayro now, PDFium later)
   │ record once            │ replay per tile, culled and cancellable
   ▼                        ▼
Display-list cache  ──►  tile bitmaps (byte budget)  ──►  window buffer
```

Crates: `engine` (trait, hayro adapter, later the display list), `render` (layout, scheduler, tile cache, compositor), `app` (window, input, later the UI and settings). The display list isn't built yet.

## 5. Memory: what the UI stack costs

Measured with [`../bench/ui-mem`](../bench/ui-mem). Each app opened a 1250×938 px window (1000×750 at 125% scaling) and was read 6 s after launch, median of 3 runs. "+ pdf" means page 1 of `paper.pdf` is shown, fitted to the window height. Our apps render it with hayro and keep the document open. Sumatra 3.5.2 ran as a portable copy with fresh settings.

- **Task Manager** is the private working set, the number users see. Windows can trim it, so it can look smaller than it is.
- **Commit** is private memory the process has reserved, the steadier number.
- **VRAM** is dedicated GPU memory.

All numbers in MB.

| app | Task Manager | commit | VRAM | GPU driver loaded |
|---|---:|---:|---:|---|
| Sumatra | 3.4 | 11.1 | 0 | no |
| **Sumatra + pdf** | **8.3** | **16.4** | 0 | no |
| winit + softbuffer (GDI blit) | 6.4 | 7.0 | 0 | no |
| **winit + softbuffer + pdf** | **10.6** | **11.7** | 0 | no |
| egui on the CPU (`egui_software_backend`) | 16.3 | 16.9 | 0 | no |
| **egui on the CPU + pdf** | **22.5** | **23.4** | 0 | no |
| egui on OpenGL (eframe glow) + pdf | 30.4 | 65.3 | 26 | NVIDIA OpenGL |
| wgpu clearing a window, DX12 | 107 | 443 | 321 | NVIDIA D3D12 |
| wgpu clearing a window, DX12, `MemoryHints::MemoryUsage` | 47 | 136 | 73 | NVIDIA D3D12 |
| wgpu clearing a window, Vulkan | 196 | 271 | 64 | NVIDIA Vulkan |
| egui on wgpu (eframe default) + pdf | 115 | 452 | 321 | NVIDIA D3D12 |
| egui on wgpu, `MemoryUsage` + pdf | 55 | 142 | 73 | NVIDIA D3D12 |

What this says:
1. **The GPU backend is the cost, not egui.** egui itself adds about 10 MB (CPU row vs GDI row). wgpu loads the GPU driver into the process: about 100 MB more in Task Manager by default, about 40 MB more with memory-saving hints. That puts egui on wgpu at 7–14× Sumatra's Task Manager number before any page is cached.
2. **A Rust window drawn on the CPU and blitted with GDI already undercuts Sumatra:** 11.7 MB committed with a page shown, against Sumatra's 16.4.
3. **egui on the CPU lands at 1.4× Sumatra's commit** (23 vs 16 MB). In Task Manager the gap looks bigger (22.5 vs 8.3 MB), because Sumatra keeps half its commit out of its working set. It keeps egui's productivity for menus, settings and dialogs. The catch: `egui_software_backend` is a 0.0.x crate with one maintainer, so expect to fork or patch it.

**Decision:** no GPU by default. Pages are composited on the CPU and blitted with GDI, as Sumatra does. Chrome and settings use our own widgets drawn with vello_cpu (already a dependency through hayro), not egui, to keep the ~12 MB footprint of the GDI row. A GPU compositor can come later as an opt-in setting for very smooth zoom on 4K/high-refresh screens.

Not measured yet: Slint's software renderer, Direct2D/DirectComposition, other GPU vendors' drivers, large documents, and whether CPU compositing keeps up at 4K and 120 Hz (Sumatra manages fine at 60 Hz).

## 6. Open decisions

- **License.** Closed source or permissive rules out MuPDF.
- **Which "more features" come first** (annotations, forms, tabs, dark mode, etc.). Editing features pull PDFium in earlier.

## 7. Other file types

Sumatra's current source (`src/shared/EngineCreate.cpp`) opens these. Each line is one engine behind Sumatra's `EngineBase` interface, which maps directly onto our `Engine` trait.

| Format | Sumatra uses | Rust option | Notes |
|---|---|---|---|
| PDF | MuPDF | hayro, PDFium later | §2 |
| XPS, OXPS | MuPDF | nothing mature | XPS is XML pages with fonts and images. It could be drawn with the same skrifa + vello_cpu stack. Low priority, the format is rare. |
| EPUB, FB2, MOBI, PalmDoc, HTML, TXT, Markdown | MuPDF's HTML layout by default, its own simpler layout engine as fallback | parse with `rbook` (EPUB, Apache-2.0), `mobi` (MIT), `quick-xml` (FB2) and `pulldown-cmark` (Markdown); lay out with our own reflow engine on `parley` (Linebender's text layout) and draw with vello_cpu | The biggest piece of new work: reflowable text needs a layout engine, not a renderer. Start with Sumatra's approach: a small HTML subset, not full CSS. |
| DOCX, XLSX, PPTX | MuPDF (newest source only) | skip | Better left to Office. |
| CBZ, CBR, CB7, CBT, image folders | its own archive engine | `zip`, `unrar` (bundles the UnRAR source, which may only be used to extract), `sevenz-rust2`, `tar` | Pages are just images, so this is cheap. |
| PNG, JPEG, GIF, TIFF, BMP, ICO, TGA, WebP | GDI+ and bundled decoders | `image` crate (pure Rust decoders) | |
| JPEG 2000, JPEG XL | bundled decoders | `hayro-jpeg2000` (already in hayro), `jxl-oxide` | |
| HEIC, AVIF, JPEG XR | bundled decoders | Windows' own WIC codecs through the `windows` crate | No extra code in our binary. |
| DjVu | djvulibre (GPL) | `djvu-rs` 0.43 (pure Rust, MIT) | Young crate: test its output and speed before relying on it. djvulibre's GPL would force the whole app to be GPL. |
| CHM | chmlib | `libchm` 0.3 (pure Rust, MIT) plus our HTML layout | |
| PostScript, EPS | Ghostscript, only if the user installed it | same: run an installed Ghostscript | Ghostscript is AGPL, so call it as a separate program and never bundle it. |
| SVG | (as an image) | `resvg` | |
| DVI, LIT | newest source only | skip | Niche. |

Sumatra itself is GPLv3, so its code can't be copied into a closed-source or MIT app. Reading it for ideas is fine.

## 8. Build order

1. **Done:** workspace, `Engine` trait, hayro adapter, and an engine benchmark on the trait (`crates/engine/examples/bench.rs`).
2. **Done, partly met:** recording `Device` + display list, plus a shared image cache. Targets from §1 were re-zoom on survey ≤ 10 ms and a tile on naca ≤ 10 ms. The naca tile is met from the second tile at a zoom level on (about 7 ms). Re-zoom isn't: about 35 ms for a whole page at 300 DPI, because the time is in rasterizing glyphs (§9).
3. **Done:** scheduler, tile cache, CPU compositor in a window (softbuffer), with scroll and zoom. Measured against Sumatra in §9.
4. **Started:** toolbar, settings panel and settings file, drawn with our own widgets on vello_cpu; search, text selection and copying; links, back/forward history and the outline sidebar; tabs, recent files, a start page and right-click menus; rotation, two-page and book layouts, presentation, printing, passwords, reloading, favorites and highlights. Still to come: a main menu.
5. **Done:** a glyph cache, so each glyph is rasterized once per size and position step instead of on every tile (§9).
6. **Done, partly:** PDFium for forms, and for writing form entries and annotations into the PDF (§9). Not done: PDFium as a fallback renderer for pages hayro gets wrong. Annotations already in a PDF are drawn by hayro when they carry their appearance, as nearly all do.

## 9. What's built

`FloresPDF.exe` opens a PDF given on the command line, dropped on the window or picked in the open dialog. It shows the pages in a continuous column, scrolls (wheel, keys, dragging) and zooms (Ctrl+wheel around the mouse, +/−, fit page, fit width). The controls are in the [README](../README.md).

- **`engine`**: the `Engine` trait gives page count, page sizes and a per-thread `PageRenderer`. The PDF adapter renders any pixel rectangle of a page at any scale. hayro's renderer layer is vendored in `src/raster` so we can change it (Apache-2.0 or MIT, licenses kept). Files over 32 MB are memory-mapped.
  - **Display lists** (`raster/record.rs`): a page is recorded once at 1 px per point into plain data (colors, paths, decoded images), then replayed per tile with its transform changed. One list per page is shared by all render threads: the first thread that needs a page records it, and others wanting it meanwhile wait instead of recording it again. The 12 most recent pages are kept. Commands whose bounds miss the tile are skipped, and so are whole clipped sections. Pages using shadings, tiling patterns, soft masks, Type 3 fonts or over 4 MB of images render directly, since those carry matrices from interpretation time or decode better at the size needed. A document whose pages mostly can't be recorded (a scan) stops trying after three, so it doesn't pay for interpretations it throws away.
  - **Glyph cache** (`raster/glyphs.rs`): vello_cpu has no way to stamp a ready-made bitmap, so text is split off. While recording, every run of text that's filled in a plain color, outside transparency groups and under rectangular clips only, and that nothing drawn later overlaps, is marked. vello_cpu draws everything else; then the marked text is stamped on top from cached coverage masks, one per glyph, size and quarter-pixel horizontal position (baselines snap to whole pixels, as in most text renderers). Masks are shared by all render threads (capped at 8 MB); each thread also keeps its own map of the ones it has used, so the common case takes no lock. Everything is dropped after 3 s idle. Glyphs drawn bigger than 192 px go through vello_cpu as before. Output differs from vello_cpu's only in antialiasing at letter edges, which can't be seen at normal size.
  - **Shared images** (`raster/shared.rs`): images decoded and scaled for one zoom level are shared by all tiles and threads, under a 48 MB budget.
- **`render`**: 512 px tiles per zoom level. Each frame, the UI thread lists the visible tiles (nearest the center first), then the next screen in the scroll direction. Four render threads take them from a queue that is replaced every frame, so requests the user has scrolled past disappear. Threads prefer tiles of the page they last rendered, and drop their recordings and the shared caches after 3 s with nothing to do. Tiles are recolored for dark, black or sepia pages (black maps white to pure black and black to light gray, keeping hues, like dark) and run-length encoded on the render thread. A missing tile is drawn from tiles of the same page at other zoom levels, stretched.
- **`app`**: winit and softbuffer, no GPU. The exe's manifest turns on the Windows segment heap.
  - **Toolbar and settings panel** (`chrome.rs`), drawn with vello_cpu. Each part is redrawn only when what it shows changes, so scrolling costs one copy per part. Text is Windows' Segoe UI with TrueType hinting through skrifa, icons are Segoe Fluent Icons, and both are memory-mapped, so they don't count as our memory.
  - **Settings** (`settings.rs`): one table describes each setting, and the file and the settings window are both generated from it. Changes apply live and are saved immediately. The window (`draw_panel` in `chrome.rs`) is centered over the dimmed reader, up to 860 × 640 logical pixels: the table's sections as categories down the left, plus System (default PDF reader, settings file, reset, about); each setting a card with its label, help and control, a switch, a number with − and +, or a drop-down that opens a menu of its choices. It takes the mouse and wheel while it's open; a click outside closes it.
  - **Home** (the toolbar's house, Alt+Home): puts the shown document back in its tab, as switching tabs does, so the start page shows with the tabs kept; no tab is marked current there, and Ctrl+W does nothing. Light, dark and system themes, with the title bar following.
- **Text, search and selection**:
  - `engine/src/text.rs` runs hayro's interpreter through a device that records each character and its box instead of drawing; about 3 ms for a dense page. Spaces and line breaks the PDF only implies by position are added (a gap over 0.1 em is a space; a baseline shift over half the font size is a new line), ligatures such as "ﬁ" are split into letters, and invisible text counts, so OCR'd scans are searchable.
  - `render/src/text.rs` is a background thread per document: it extracts the text of the pages on screen (keeping 6) and runs searches. A search goes through every page once, starting at the current one, and keeps only where the matches are. It ignores case and runs of whitespace, and finds words hyphenated across a line break. 144 pages take under 3 s.
  - The app highlights matches (the current one in orange), jumps to the first match as it's found, and shows "3 of 17". Dragging over text selects it in the order the PDF draws it, which for most documents is reading order; a double-click selects a word. Ctrl+C puts the selection on the clipboard with Windows line breaks.
  - Memory: about 0.1 MB for the visible pages' text. A search with thousands of matches ("the" in survey: 6,730) adds about 4 MB while it's active.
- **Links and outline**:
  - `engine/src/nav.rs` reads a page's link annotations and the document outline with hayro-syntax. A link goes to a page and, for the usual destination kinds (XYZ, FitH, FitBH, FitR), a height on it; named destinations are looked up in the catalog's `/Dests` and the `/Names` tree. Actions followed: GoTo, URI, and the named actions next, previous, first and last page. Others (launching programs, JavaScript, other files, forms) are ignored. The outline walk is guarded against loops and capped at 20,000 entries and 32 levels.
  - In the app, hovering a link shows a hand and, after 0.6 s, where it goes. A link is followed when the button is released over the one it was pressed on, so moving off it before letting go cancels. Internal jumps put the target 12 points below the top of the window and are remembered: Alt+←/→ and the mouse's Back and Forward buttons move through the last 100.
  - Web links open only for http, https and mailto, or a bare address like `example.org`; `file:`, `javascript:` and paths never reach the shell (unit-tested in `win.rs`).
  - The outline sidebar (F12, the toolbar's leftmost button, or the "Outline sidebar" setting) opens entries the PDF marks open, highlights the section you're reading, and scrolls with the wheel separately from the pages. It's 280 px wide at 100% scaling and at most half the window. It appears only for documents that have an outline.
  - Memory: the sidebar's pixels are kept like the toolbar's, about 1 MB on survey (commit 21.1 MB idle, against 20.0 before). Links are read per page when the mouse first moves over it, and kept for the open document.
- **Tabs, recent files and one window** (`main.rs`, `history.rs`, `win.rs`):
  - Every file opens in a tab; opening one that's already open shows its tab. The tab strip appears once there are two tabs, so a single document keeps the whole window. The shown tab's document is the only one with render threads and tiles: switching away drops its tiles and render threads and empties the engine's caches, and a background tab keeps only the parsed file, its text cache and its search. Switching back renders the screen again.
  - `history.txt` keeps the last 50 files, most recently looked at first, with page, offset and zoom, and marks which were open as tabs. A file reopens where it was left. With "Reopen last session's tabs" on, the tabs come back at start; only the shown one is read, and each other one is read when it's first shown, so a big session costs nothing until it's looked at. A file that's gone shows an error and its tab closes.
  - The start page lists recent files when nothing is open. Its pixels aren't kept (it would be a whole window of them), only where its rows are.
  - One window: the window has the class name `FloresPDF`. A second launch looks for it, hands over its command line (paths made absolute) with `WM_COPYDATA`, brings it to the front and exits. The window procedure is wrapped to take these messages, since winit doesn't pass them on. A named mutex makes copies that start at once (several files opened together from Explorer) wait until the first has its window up, so they don't all open their own. The setting "Open files as tabs in one window" turns this off.
- **Right button**: holding it and turning the wheel zooms around the mouse, a step per notch, as Ctrl+wheel does. A right-click without the wheel opens a menu when the button comes up, drawn like the rest of the UI, below and right of the click, or above or left of it if there's no room. Any key, wheel turn or click elsewhere closes it.
  - On the page: Copy, Find in document and Search the web when there's a selection (the selection on one line, at most 200 characters; the web search goes through `open_link`, so only to the https address of the engine the settings name); Copy link address over a web link; zoom in and out around the spot clicked, fit page, fit width, actual size; Copy page as image and Save page as PNG for the page clicked; Show in folder (Explorer with the file selected), Copy file path, Document properties.
  - On a tab: Close tab, Close other tabs, Close tabs to the right, Reopen closed tab (also Ctrl+Shift+T, the last 20), Show in folder, Copy file path. Closing other tabs shows the kept one first, so no tab being closed is read from disk just to be closed.
  - **Page images** are drawn whole on the UI thread, in the page's own colors, at the "Page image resolution" setting (150 dpi by default; survey's Letter pages come out 1275 × 1650). Pages that would pass 24 million pixels get a lower resolution, so a poster can't take gigabytes. The clipboard gets a 24-bit device-independent bitmap, which every Windows program that takes pasted images reads; the PNG is written with the `png` crate (already in the build through vello_cpu) and records its resolution. The engine's shared caches are emptied afterwards, since the render threads wouldn't drop what the big render left in them until they next went idle.
  - **Document properties** come from the PDF's information dictionary through hayro-syntax (title, author, subject, keywords, dates, creating and producing programs), plus the file's folder and size, the PDF version, the page count and the current page's size in millimetres and inches. The box closes with Esc, its ×, or a click outside it.
- **Smooth scrolling**: a wheel notch or an arrow, Page Up/Down, Space, Home or End key glides the pages to where they're going over 160 ms, quickly at first and easing in. Notches that come during a glide add to where it's going, so spinning the wheel keeps one glide moving rather than stuttering. Each step is drawn after `DwmFlush`, which waits for the desktop's next frame, so steps land one per screen refresh without a timer. A touchpad's pixel scrolling is followed exactly, since it is smooth already; dragging, jumps, zooming and switching tabs stop a glide. The "Smooth scrolling" setting turns it off. Measured: one notch on survey moved the pages 125 px across several frames, and the app used 16 ms of CPU in the 2 s after the glide ended.
- **Scroll bar**: the 18-pixel strip at the right edge of the pages takes the mouse. Over it, the thumb widens on a shaded track; dragging the thumb moves through the document, pressing elsewhere on the strip brings the thumb there, and a wheel notch moves a whole page instead of the usual distance.
- **Memory with tabs**, commit 7 s after opening, survey.pdf with its outline: 21.2 MB alone (21.1 before tabs), 21.5 MB with paper.pdf shown and survey in a background tab. No document: 7.9 MB, unchanged.
- **Name and icon**: `FloresPDF.exe`, after Sumatra's `SumatraPDF.exe`. The icon, a white flower on a sea-teal tile (Flores is Portuguese for flowers), is drawn by `crates/app/examples/icon.rs` into `flores.ico`: PNG at 256 px, bitmaps below, since older Windows code only reads PNG at 256. The build script writes it into a compiled resource file itself, so no resource compiler from the Windows SDK is needed, and the linker takes that file as it is. Settings in `%APPDATA%\pdfreader` are copied to `%APPDATA%\FloresPDF` the first time.
- **One file, and an installer**:
  - PDFium is built into `FloresPDF.exe`: the build script deflates `pdfium.dll` (7.2 MB to 3.6 MB) and its license texts, and the exe carries them as data. The first time PDFium is needed it's inflated and written to `%LOCALAPPDATA%\FloresPDF\pdfium-7881.dll`, under a temporary name first and then renamed, with `pdfium-licenses.txt` beside it; later runs load that file once its bytes match the copy inside. A `pdfium.dll` beside the exe still takes precedence. The exe grew from 9.3 to 13.0 MB; memory didn't change (survey.pdf 21.4 MB, no document 8.0 MB), since the packed copy is part of the mapped image and is only read when it's written out.
  - `FloresPDF-Setup.exe` (`crates/setup`, 8.1 MB) carries the app deflated. Its build script packs `target/<profile>/FloresPDF.exe`, so the app is built first. Setup asks, writes the exe to `%LOCALAPPDATA%\Programs\Flores PDF` (a copy that's open is renamed aside, since Windows allows renaming a running exe but not overwriting it), and runs it with `--register`. Then it offers Windows' Default apps page and starts the reader. Its manifest asks for no elevation: the linker's asInvoker entry keeps Windows' installer detection, which elevates programs with "setup" in their name, from asking for admin rights.
  - `--register` (`app/src/install.rs`) writes, under HKEY_CURRENT_USER, a program ID for each type (PDF, EPUB, DjVu, CBZ, CBR, CB7) with its icon, command and an `Application` key naming Flores PDF; `OpenWithProgids`; `Applications\FloresPDF.exe`; Capabilities with every type and `RegisteredApplications`, which is what puts it in Default apps; and an Uninstall entry (name, icon, version, publisher, size, uninstall commands). It makes the Start menu shortcut through the shell's ShellLink COM object, called by vtable position since windows-sys has no COM wrappers.
  - `--uninstall` (run by Settings' Uninstall) asks, waits for open windows to close, deletes those keys and values, the shortcut and the PDFium folder, offers to delete the settings folder, and leaves a hidden `cmd` that deletes the program's own files and then the folder once the process has ended. It deletes only the files setup and the reader put there, so a folder holding anything else is left.
  - Both exes carry version information (Flores PDF, aga2522, 0.1.0), which Windows' app lists and Explorer show.
  - Tested with `FLORES_INSTALL_TEST` pointing at a scratch folder, which moves the program folder, the shortcut and every registry value aside (under `Software\FloresPDF-Test`), so this machine's real registration wasn't touched. Covered: a silent install, all the values and the shortcut's target and icon, PDFium written on first use of a form from the installed copy, installing again over a running copy, and a quiet uninstall that removed the values, the shortcut and the program's files. The real Default apps page, the dialogs after the first question, and Settings' own Uninstall button weren't tried.
- **Default PDF reader**: Windows doesn't let a program make itself the default. "Make Flores PDF the default…" in the settings writes the per-user registry values that offer it for PDFs (a program ID, Capabilities, RegisteredApplications; no admin) and opens Windows' Default apps page on it. The panel says whether it's the default, by asking Windows which program opens `.pdf`. Not clicked in testing, to leave this machine's registry alone; a unit test checks the values.
- **Rotation and spreads** (`render/src/layout.rs`, `pool.rs`): the layout takes a rotation in quarter turns and a spread (one page per row, two, or a book with the cover alone on the right). Pages of a row are top-aligned with a gap between them, meeting in the middle. Rotation lives in the layout and the tile key, so the scheduler, cache and compositor follow it without being told. The engine always draws upright: a render thread draws the matching upright area and turns the pixels, so display lists and the glyph cache work the same at any rotation. The app maps page points to the screen and back through the rotation for text, links, highlights and search. N/P and the arrows step by row. Each file's rotation and spread are kept in the history. Ctrl+Shift+−/+, Ctrl+6/7/8 and the toolbar's view menu set them.
- **Presentation** (F5): full screen, no toolbar, tabs, sidebar or scroll bars, black around the page, fit page. The layout's gap becomes the screen's height, so exactly one page or spread shows; it's re-centered on every frame, which also covers the switch to full screen. Clicks, keys and the wheel step pages; Esc or F5 restores the zoom and window.
- **Printing** (`print.rs`): Ctrl+P shows Windows' print dialog (on Windows 11 the modern one, which runs in its own process). The job runs on its own thread: each page is drawn at the printer's resolution in bands of 256 rows, so a 600 dpi page never needs more than a few megabytes at once, shrunk to the printable area if too big (never enlarged), turned when its orientation differs from the paper's, and sent with `StretchDIBits`. Pages print in their own colors. The window shows progress; Esc stops the job. `-print-to "printer"` and `-print-to-default` print from the command line without a window, as Sumatra's flags do. Tested by printing paper.pdf to Microsoft Print to PDF from the command line: 14 pages in about 16 s, and the result opened with every page right. The dialog was seen to open but not driven, since it's in another process.
- **Passwords**: the engine tells a missing or wrong password apart from encryption it can't read. The app asks in a box of its own, says when a password is wrong, and keeps the password in memory for that tab only, to read the file again if it changes. Tested with a PDF encrypted for the test (40-bit RC4, the classic handler): a wrong password was refused, the right one opened it.
- **Reloading** (`watch.rs`): one thread checks the open tabs' files once a second (size and modification time). A change counts once the file has held still for a check, so one still being written isn't read half-done. The shown tab reads it again and keeps its page, zoom, rotation, history and search; a tab in the background reads it when shown. Tested by copying a 144-page PDF over an open 14-page one: the window showed 144 pages within two seconds.
- **Bookmarks** (`favorites.rs`, once called favorites): Ctrl+D or the page menu bookmarks a page, named after its outline section, and opens a box to rename it (the name selected, so typing replaces it). They're kept in `favorites.txt`. The sidebar gets Outline and Bookmarks tabs, and shows for a document with bookmarks even without an outline; right-clicking a bookmark renames or removes it.
- **Highlights of other documents** (`highlights.rs`): e-books, DjVu and comics can't hold annotations, so their highlights (Ctrl+H) are kept in `highlights.txt` as before. PDF highlights made before annotations went into PDFs are shown from there and written into the PDF the next time it's saved, then dropped from the file.
- **Annotations** (`annots.rs`, `pdfium.rs`, `chrome.rs`):
  - The toolbar's pen (PDFs only) shows a second bar: highlight, underline, strikeout, note, pen, text, rectangle, ellipse, line, arrow, eraser; six colors (the four highlight colors, red, black: highlights keep their own, the rest share one); three widths (1, 2.5 and 5 pt lines; 10, 14 and 20 pt text); undo; Save, lit while there are changes. The text tools mark what a drag selects; the shapes and pen follow the drag on the page it started on; note and text open a text box (Enter makes a new line, Ctrl+Enter or OK finishes; empty makes nothing).
  - Until saved, annotations are `Mark`s in the document, in points of the page as it is, drawn over the pages each frame: highlights tinted in with the search matches, the rest as vector pieces (kurbo paths through the page's turn and scale) drawn with vello_cpu over the frame, each in a box just its size. A text box's text is Segoe UI's outlines, unhinted, so it looks the same on screen and in the file.
  - Ctrl+S (`save`) writes them through PDFium: removed annotations first, then new ones (Highlight, Underline, StrikeOut, Text for notes, Ink for the pen, lines and arrows, Square, Circle, FreeText), each printable, then the page is drawn once so PDFium makes their appearances, and the document is saved whole. PDFium makes no appearance for FreeText, so the text box's own is given: its glyph outlines as a content stream in page coordinates, which `FPDFAnnot_SetAP` sets with the annotation's rectangle as the bounding box. The new file is shown from memory, written beside the old one and renamed over it (tried for up to three seconds, while a render thread may still map the old file), and then read from disk again. Saved annotations are drawn by hayro from the file like anyone's.
  - Annotations already in the file are listed by the engine (`page_annotations`: kind, rectangle, text; not links, form fields or pop-ups). The eraser or "Delete annotation" removes one by writing the PDF again in memory (with the marks not saved yet put in too) and reading it from there (`engine::open_pdf_bytes`) until Ctrl+S. Before removing, PDFium checks the annotation at that place is still at that rectangle, or finds the one that is. A note's text, or an annotation's comment, shows as a tooltip.
  - Ctrl+Shift+S saves the same into another file and leaves the open one as it was. Closing with changes not saved asks Yes (save into the file), No or Cancel; a file changed on disk isn't reloaded over unsaved changes.
  - Tested by sending window messages to a copy of paper.pdf: every tool, a note and a text box typed in, the note's tooltip, Save, and the file reopened with all of them drawn by hayro from the file; the eraser on an annotation in the file; the right-click menu's Delete annotation; the unsaved-changes question on closing. PDFium draws a note's icon yellow whatever its color, so notes are made yellow here too. Not tried: Ctrl shortcuts (the test can't hold Ctrl), a rotated page, password-protected PDFs and very large files being saved.
- **Start page** (`thumbs.rs`, `chrome.rs`): recent files as cards: the first page, the name, the page you were on and the folder, as many across as fit (up to six). Thumbnails are made one file at a time on a thread of their own, fitted to 132 × 172 logical pixels, and kept in `%LOCALAPPDATA%\FloresPDF\thumbnails` with the file's modification time, so they show at once next time and are made again when the file changes; they're dropped from memory once a document is shown, and colored like the pages (dark, black, sepia) when drawn. A file that can't be read, or needs a password, gets a plain page. The start page with seven files took 9.3 MB.
- **Comics and pictures** (`engine/src/images.rs`): a CBZ (a zip of page images, read from its central directory by our own small reader, stored or deflated, no zip64) or a single JPEG or PNG. Pages are in natural order of their names (page2 before page10), sized as if at 96 dpi, decoded with zune-jpeg and png (both already in the build through hayro and vello_cpu) when first drawn, and kept three at a time until the render threads go idle. Shrinking averages the pixels each output pixel covers, so scans stay legible; enlarging blends neighbors. GIF, WebP, CBR and DjVu need libraries that aren't in the build.
- **Polish**: tabs can be dragged into a new order; the sidebar's edge drags to resize it (the width is a setting); a selection dragged above or below the pages scrolls them, faster the farther the mouse is, by time rather than frames; a scroll bar along the bottom appears when the pages are wider than the window. Release builds are optimized as one unit (`lto = "fat"`, one codegen unit): the exe went from 8.19 to 7.55 MB, and is 7.34 MB with everything above, at the price of a three-minute build.
- **Memory after all of this**, commit 7 s after opening, median of three: survey.pdf 21.4 MB (21.2 before), paper.pdf 11.1 MB (10.9), no document 7.9 MB (7.9).
- **More formats** (`engine/src/images.rs`, `djvu.rs`): GIF (the first frame) and WebP with the `gif` and `image-webp` crates; CBR with `unrar`, which builds RarLab's UnRAR source (its license allows reading only, which is all this does). UnRAR reads archives from their file, from the start, so a page is read by skipping to its entry; opening reads each image once for its size. DjVu with `djvu-rs` (pure Rust, MIT): any area of a page at any size, bookmarks as the outline, sized from each page's resolution. DjVu's text layer gives search and selection: each word's characters are spread evenly across the word's box, with spaces between words and line breaks between lines. CB7 with `sevenz-rust2` (pure Rust, Apache-2.0, without its compression and AES features): a page is read by opening the archive from its file and decoding to its entry, as for CBR.
- **PDFium** (`app/src/pdfium.rs`, `third_party/pdfium`): build 7881 without V8, so no JavaScript, from the pdfium-binaries project, with its version, source and checksum recorded. It's loaded from beside the exe the first time it's needed and never otherwise, so reading costs nothing more. The binding is ours, about thirty functions looked up by name, rather than a wrapper crate: the wrapper we looked at sets field values without making their appearance and assumes check boxes turn "on" with the name Yes, which would leave fields blank in other readers or checked wrongly.
  - Forms: a PDF with a form (the engine checks for `/AcroForm /Fields` without PDFium) gets a pencil on the toolbar. Filling in reads the fields with PDFium (text, multi-line text, check boxes, radio buttons, drop-downs) and draws them over the page as pale boxes, in the page's own coordinates mapped through the engine's page transform. Typing, ticking and picking change our copy of the values.
  - Saving (Ctrl+S, see Annotations) opens the file in PDFium and gives each changed field its value the way PDFium's viewer does: focus the field and replace its text, click a check box or radio button in its middle, select a drop-down's choice. PDFium then makes the fields' appearances itself. Annotations are added as described under Annotations.
  - Tested with a form made for the test (a text field, a check box whose "on" is named On, a drop-down): the saved copy holds the name typed, the check box's own On state and the choice, and reopened in Flores PDF (hayro, no PDFium) shows all three. A highlight saved into a copy of survey.pdf shows when the copy is reopened.
  - Memory: none while reading. Filling in or saving loads PDFium (the DLL is mapped from disk) and the document into it, about 15 MB more for survey.pdf while the copy is written; PDFium stays loaded until the reader closes.
- **Memory with the new formats**, commit 7 s after opening, median of three: survey.pdf 21.5 MB, paper.pdf 11.2 MB, a one-page DjVu (1275 × 1650 at 150 dpi) 17.5 MB, no document 7.9 MB. The exe is 8.39 MB with the DjVu, GIF, WebP and RAR decoders; `pdfium.dll` is 6.88 MB beside it.
- **EPUB** (`engine/src/epub.rs`, about 900 lines, ours): the book is a zip whose first entry says `application/epub+zip`. `META-INF/container.xml` names the package file; its manifest and spine give the chapters in order, its metadata the title and author, and its nav document (EPUB 3) or NCX (EPUB 2) the table of contents. A small XML reader of our own handles tags, attributes, entities, comments, CDATA and doctypes.
  - Layout happens once, when the book opens: 6 × 9 inch pages (432 × 648 pt) with 54 pt margins, 11.5 pt body text, justified except a paragraph's last line. Headings (21, 17, 14.5, 12.5 pt, bold), bold and italic, code and `pre` in Consolas, bulleted and numbered lists, block quotes indented, rules, line breaks, pictures scaled to fit the page and centered, and every chapter on a new page. The book's CSS isn't applied, and there is no hyphenation or shaping beyond one glyph per character, which suits Latin, Greek and Cyrillic.
  - Fonts: Windows' Georgia in four faces, Consolas, and Segoe UI for characters they lack, memory-mapped from the Fonts folder (so they cost no private memory). Glyph outlines come from skrifa (already in the build for the interface), are cached per render thread at 1 pt, and are filled with vello_cpu. Pictures are decoded and scaled as comic pages are.
  - Each page keeps only its lines (runs of text with their face and position, and the extra width per space that justifies the line), pictures, rules and link areas. Links inside the book go to the anchor's page and height, ids being recorded as layout passes them; web links open in the browser. Text for search and selection is measured again from the runs.
  - Tested with a two-chapter book made here (headings, a nested table of contents, lists, a picture, a quote, preformatted text, an internal and a web link, Greek and Cyrillic): six pages, outline entries landing on their chapters and the section, the internal link going to its section's height, and the text read back with its line breaks. Memory: 11.1 MB commit with the book open.
- **Forms, finished**: Ctrl+V pastes into the field being typed in (one line for single-line fields) and into the password box. List boxes are picked from like drop-downs, from a menu of their choices. Closing a tab or the window while a form has entries not saved asks whether to save them: Yes saves into the file and closes only if that worked, No closes, Cancel keeps it open. A file reloaded after changing on disk keeps its unsaved entries rather than reloading.
- **Main menu**: the ⋯ button left of the gear, or F10, opens a menu of everything the reader does: open, save, save a copy, annotate, undo, print, properties, show in folder, reopen a closed tab, close, first and last page, go to page, back and forward, find, the toolbar and sidebar, full screen, presentation, settings and an About box that names the author, the project's Discord and the libraries inside with their licenses. Saving is offered only for PDFs, here and in the page menu.
- **Memory after this round**, commit after opening, settled: survey.pdf 21.5 MB, paper.pdf 11.2 MB, the test EPUB 11.1 MB, the one-page DjVu 17.5 MB, a three-page CB7 9.7 MB, no document 8.1 MB. The exe is 9.29 MB with the 7-Zip reader and the EPUB engine (8.39 MB before).

### Memory against Sumatra

Sumatra 3.5.2 ran as a portable copy with fresh settings. Both apps had a 1250×938 px window with their toolbar, showed pages at fit page, and were read 6 s after launch, median of 3 runs. "Scrolled" means after 150 wheel notches. Both apps read files under 32 MB into memory: survey.pdf is 8.2 MB and naca.pdf 5.8 MB. Raw data: [`../bench/ui-mem/results-2026-10-30-reader.jsonl`](../bench/ui-mem/results-2026-10-30-reader.jsonl) (before the toolbar) and [`results-2026-10-30-reader-ui.jsonl`](../bench/ui-mem/results-2026-10-30-reader-ui.jsonl) (with it).

All numbers in MB. pdfreader is shown with the toolbar and settings, and in brackets without them.

| | pdfreader, Task Manager | pdfreader, commit | Sumatra, Task Manager | Sumatra, commit |
|---|---:|---:|---:|---:|
| no document | 7.3 (6.5) | **7.9** (7.1) | 3.5 | 11.3 |
| paper.pdf (14 p, text) | 10.3 (9.9) | **11.1** (10.7) | 8.6 | 16.7 |
| paper.pdf, scrolled | **13.7** (12.8) | **14.5** (13.6) | 19.1 | 27.4 |
| survey.pdf (144 p, dense text) | 19.1 (18.6) | **20.0** (19.4) | 17.1 | 25.3 |
| survey.pdf, scrolled | **23.2** (22.2) | **24.1** (23.0) | 25.7 | 35.5 |
| naca.pdf (61 p, scans) | 18.2 (17.9) | **19.1** (18.7) | 16.0 | 23.9 |
| naca.pdf, scrolled | **18.5** (17.9) | **19.3** (18.6) | 30.3 | 38.3 |

What this says:
1. **Commit is below Sumatra's in every case**: 20–34% lower right after opening, 32–50% lower after scrolling. Commit is the memory the process actually holds.
2. **In Task Manager we're 1.7–2.2 MB above Sumatra right after opening, and 2.5–12 MB below after scrolling.** Sumatra keeps every page it renders, up to 256 bitmaps. We keep about one windowful of compressed tiles, plus whatever is on screen or about to be.
3. **The toolbar and settings cost under 1 MB.**
4. **What got us here**, measured on paper.pdf right after opening: the segment heap took commit from 17.5 to 15.0 MB, and compressed tiles took it from 15.0 to 10.5 MB. The price of the small cache is that scrolling back more than a few screens renders the pages again.
5. **While busy, memory goes up** (recordings, scaled images, the renderers' scratch space): a commit of 25–35 MB in the seconds after opening or zooming, back down 3 s after the last render.

### CPU against Sumatra

CPU seconds the whole process used (all threads), from [`../bench/ui-mem/cpu.ps1`](../bench/ui-mem/cpu.ps1). This machine's speed drifts by up to 2× over minutes, so the two apps ran alternately, one run each in turn, and the table shows the median of 3. Each app pages and zooms from its own fit-page zoom, so the zoom steps differ a little between them. Raw data: `bench/ui-mem/results-2026-10-30-cpu-final.jsonl`.

| | Sumatra | pdfreader |
|---|---:|---:|
| paper: open | 0.23 | **0.14** |
| paper: 20 × Page Down | **0.52** | 0.69 |
| paper: zoom in 3 steps, out 3 | 0.30 | **0.16** |
| survey: open | 0.19 | 0.17 |
| survey: 20 × Page Down | **1.00** | 1.12 |
| survey: zoom | 0.53 | **0.36** |
| naca (scans): open | 0.25 | 0.27 |
| naca: 20 × Page Down | 0.86 | 0.92 |
| naca: zoom | **0.33** | 0.45 |
| tubemap: open | 0.55 | **0.25** |
| tubemap: 20 × Page Down | **0.05** | 0.20 |
| tubemap: zoom | 0.95 | **0.62** |
| 10 s idle, any file | 0 | 0 |

1. **Idle costs nothing, and opening costs the same or less than Sumatra.**
2. **Zooming costs less than Sumatra** on text and vector art (30–47% less), more on scans (+36%).
3. **Paging costs 7–33% more than Sumatra** on real documents. On tubemap it's 4× but tiny in absolute terms.
4. **What the display list and glyph cache bought**, measured the same alternating way against the same build with them off ([`results-2026-10-30-cpu-ab.txt`](../bench/ui-mem/results-2026-10-30-cpu-ab.txt)): paging survey about 30% less CPU, zooming it about 45% less, paging paper about 15% less.
5. Sumatra renders a document on one thread; we spread the work over four, so pages turn sharp sooner.

An earlier version of this table compared runs taken at different times and was off by up to 2× because of the drift. That's why the alternating method.

### Display list and glyph cache: what they bought

Single-threaded, our engine through the `Engine` trait, in ms ([`../bench/results-2026-10-30-engine.jsonl`](../bench/results-2026-10-30-engine.jsonl)). "Direct" renders every tile from scratch through hayro's renderer; the other column records each page once, replays it per tile, and stamps text from the glyph cache.

| | direct | display list + glyph cache |
|---|---:|---:|
| paper, first 14 pages at 150 DPI (includes recording) | 240 | **145** |
| survey, first 40 pages at 150 DPI | 736 | **450** |
| paper, page 1 as 512 px tiles at 100% | 23 | **13** |
| paper, the same at 200% | 68 | **38** |
| survey, page 1 as tiles at 100% | 26 | **14** |
| survey, whole page 1 at 300 DPI after a render at 150 DPI | 32 | **26** |
| naca (scans), a 1024 px tile at 400%, first / next | 44 / 4.8 | same: scans render directly |

1. **Text pages render about 40% faster the first time** (recording included) and about 45% faster as tiles; a whole-page re-zoom 20% faster. Recording costs less than it saves, because text is stamped instead of rasterized.
2. **The display list alone bought only 20–35%,** and nothing on a whole-page re-zoom. Timing the stages showed why: interpreting a page is cheap; about 80% of the time was vello_cpu turning glyph outlines into pixels, which only the glyph cache avoids.
3. **The shared image cache is the big win on scans:** the second tile at a zoom level costs 7 ms instead of 70, because the scan is decoded and scaled once.
4. **Pixels:** the display list alone draws identical pixels. With the glyph cache, 5–10% of pixels on a text page differ, by up to 140 of 255 levels, all on letter edges: glyphs sit on quarter pixels horizontally and whole pixels vertically. Crops of both are indistinguishable at normal size.
5. **Sharing matters.** With four render threads each recording pages and rasterizing glyphs for themselves, the app used *more* CPU paging than without display lists. Recordings are now plain data shared by all threads (the first thread to need a page records it, others wait for it), and glyph masks are shared too.

### Not done or not verified

- Forms can't be signed and have no JavaScript calculations. A list box takes one choice, not several. EPUB ignores the book's CSS, fixed-layout books, audio, video and right-to-left or complex scripts. Tabs can't be dragged into their own window. No code signing, so Windows' SmartScreen warns on first run.
- Web links were checked by unit test and tooltip only; none was clicked, since that would open the browser. Outline titles too long for the sidebar are cut with "…".
- Selection follows the PDF's drawing order, not a reading-order analysis, so tables, footnotes and unusual layouts can select in surprising order. Text in Type 3 fonts gets approximate boxes.
- The test files for EPUB, DjVu, WebP, GIF, CBR and CB7 were made here (`examples/samples.rs`, and a stored-RAR writer); real-world DjVu books, EPUBs from publishers and compressed CBRs weren't tried, since none were at hand.
- Not tried in the app: the print dialog's own path (its window is in another process; the printing behind it was tested from the command line), Esc stopping a print, "Make Flores PDF the default…" (it changes the registry), a protected tab reloading after a change, a password-protected file restored from the last session, and favorites in a document without an outline.
- Ctrl+Tab, Ctrl+W and the scroll bar's look on hover were checked by hand. Copying (Ctrl+C), Ctrl+F/Ctrl+A and Alt+←/→ weren't exercised by the tests: held Ctrl can't be faked with window messages. Search was tested by clicking the search box and typing; selection by dragging and double-clicking with posted mouse messages.
- Rendering speed inside the app (time until a page is sharp, frame time while scrolling) isn't measured.
- The stretched placeholder is covered by unit tests but wasn't caught on screen: tiles finished before the screenshot.
- Tested by sending window messages: wheel scrolling, Page Down, End, N, +, −, F8, clicks on the toolbar and settings panel, typing into the page box, the `-page`/`-zoom` flags, F12, clicking outline entries and their arrows, clicking a citation link, the mouse's Back button and a link's tooltip; a second launch handing its file to the open window, switching tabs, closing one with a middle click, reopening last session's tabs at their pages, a missing file in the session, the start page, the right-click menu (with a selection and over a web link) and its Zoom in, zooming with the wheel while the right button is held (and no menu after), and the scroll bar's wheel, press and drag; the full page menu with a selection, Find in document, Document properties, the tab menu's Close tab, Reopen closed tab and Close other tabs, Save page as PNG through the Save dialog (the file came out 1275 × 1650 at 150 dpi), and frames caught mid-glide while scrolling. The scroll bar's wide look was seen mid-drag only: the test's mouse isn't really over the window, so Windows reports it gone right after each move. "Copy link address" wasn't clicked, to leave the clipboard alone, and turning off "Open files as tabs in one window" wasn't tried. For the same reasons Copy page as image, Copy file path, Search the web and Show in folder weren't clicked; unit tests cover the clipboard image's layout and the search address's encoding. Ctrl and Alt shortcuts, toolbar tooltips, F11, the open dialog and dropping a file haven't been tried.
- Only this machine (NVIDIA, 125% scaling, dark mode) and the four public test files. A 756-page test file turned out to be a truncated download (MuPDF shows its pages blank too), so a very long real document is still untested.
- Release builds are optimized as one unit, but `opt-level` stays 3: the size-tuned profile in §3 isn't set up.
