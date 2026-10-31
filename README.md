# Flores PDF

A PDF and comic reader in Rust that aims to stay as light as Sumatra, named after the island east of Sumatra. Pages render on the CPU with [hayro](https://github.com/LaurenzV/hayro)'s interpreter and are blitted with GDI, so no graphics driver is loaded. Design notes and measurements are in [docs/architecture.md](docs/architecture.md).

It opens PDFs (password-protected ones too), EPUB e-books, DjVu documents, CBZ, CBR and CB7 comics, and JPEG, PNG, GIF and WebP pictures. PDFs can be annotated (highlights, underlines, strikeouts, notes, freehand drawing, text, rectangles, ellipses, lines and arrows) and their forms filled in, and Ctrl + S saves both into the PDF itself.

`FloresPDF.exe` is the whole program, one file. Filling in forms and saving annotations use [PDFium](third_party/pdfium/README.md), which is built into the exe and written to `%LOCALAPPDATA%\FloresPDF` (with its license texts) the first time you fill in a form or save; reading never loads it.

## Install

Run `FloresPDF-Setup.exe`. It installs Flores PDF for your Windows account, with no administrator rights, in `%LOCALAPPDATA%\Programs\Flores PDF`:

- a Start menu entry;
- Flores PDF offered for PDF, EPUB, DjVu, CBZ, CBR and CB7 files ("Open with", and Windows' Default apps);
- an entry in Settings › Apps › Installed apps, whose Uninstall removes all of it (and, if you say so, your settings and recent files).

At the end it offers to make Flores PDF your default PDF reader. Windows doesn't let any program do that itself, so it opens Windows' Default apps page on Flores PDF, where you click "Set default". Running a newer setup over an installed copy updates it, even while it's open. `FloresPDF-Setup.exe /S` installs without asking anything.

`FloresPDF.exe` also runs on its own from any folder, without installing (put an empty `FloresPDF-settings.toml` beside it to keep its settings there).

Download `FloresPDF-Setup.exe` (or the portable `FloresPDF.exe`) from the [latest release](https://github.com/AgastyaShenoy/flores-pdf/releases/latest).

## Updates

A few seconds after it starts, Flores PDF asks GitHub whether there's a newer release; if there is, an "Update to …" button shows in the toolbar. In an installed copy it downloads the new setup, closes, installs and opens again (after asking, and after any changes not saved). A portable copy opens the release's page. Turn the check off under Settings › System › Check for updates; nothing is sent but the request.

To publish a release: set `version` in `Cargo.toml`, build both exes (below), and attach `FloresPDF-Setup.exe` and `FloresPDF.exe`, named exactly so, to a GitHub release tagged `v` and the version (`v1.2.0`). Copies older than the tag offer it.

## Build and run

```bash
cargo build --release -p flores-pdf
```

```bash
cargo build --release -p flores-setup
```

The setup packs the `FloresPDF.exe` it finds in the same target folder, so build the app first.

```bash
target/release/FloresPDF.exe file.pdf
```

Rust 1.99 is pinned in `rust-toolchain.toml`. A release build takes about three minutes, since it's optimized as one unit. You can also drop files onto the window, open them with the toolbar's folder button, or pick one from the start page, which shows your recent files with a thumbnail of each one's first page. Each file opens in a tab, where you left it last time. Opening a file while Flores PDF is running adds a tab to its window instead of starting a second copy.

Command-line flags follow Sumatra's: `-page N` and `-zoom` with `fit-page`, `fit-width` or a percentage, for example `-zoom 150`. `-print-to "printer name"` or `-print-to-default` prints the files given and quits, without a window.

To make a copy that wasn't installed your default PDF reader, open the settings and use "Make Flores PDF the default…": it registers that copy the way setup does (no admin needed) and opens Windows' Default apps page, where you pick it.

## Controls

| Action | Keys |
|---|---|
| Open a file in a new tab | Ctrl + O, or the + after the tabs |
| Home: the start page with recent files | Alt + Home, or the toolbar's house. Open tabs stay; click one, or its file's card, to go back |
| Next / previous tab | Ctrl + Tab / Ctrl + Shift + Tab, or Ctrl + Page Down / Page Up |
| Close the tab | Ctrl + W or Ctrl + F4, its ×, or a middle click on it |
| Reopen a closed tab | Ctrl + Shift + T, or right-click a tab |
| Move a tab | drag it |
| Right-click menu on the page | right-click without turning the wheel. With text selected: copy, highlight, underline, strike out, find it in the document, search the web for it. Over a web link: copy its address. On an annotation: edit a note's or text box's text, or delete it. Always: zoom in and out around the spot clicked, fit page, fit width, actual size; bookmark the page; copy the page as an image or save it as PNG; print; save, save a copy; show the file in its folder, copy its path, document properties |
| Right-click menu on a tab | close it, close the other tabs or those to its right, reopen a closed tab, show in folder, copy file path |
| Page layout, rotation, presentation | the toolbar's view button (two pages, right of the zoom buttons), or the keys below |
| One page / two pages / book view | Ctrl + 6 / Ctrl + 7 / Ctrl + 8 |
| Rotate left / right | Ctrl + Shift + − / Ctrl + Shift + + |
| Presentation | F5: one page at a time on a black screen. Click, →, Space or the wheel go on; right-click, ←, Backspace go back; Esc or F5 stop |
| Print | Ctrl + P; Esc stops a print job in progress |
| Bookmark the page, or remove its bookmark | Ctrl + D, then name it (Enter). Bookmarks are in the sidebar's Bookmarks tab; right-click one to rename or remove it |
| Highlight / underline the selected text | Ctrl + H / Ctrl + U, or right-click it (strike out too) |
| Annotate | the toolbar's pen (PDFs). Its bar has the tools: highlight, underline and strike out (drag across text), note (click, type, OK), pen (drag), text (click, type, OK), rectangle, ellipse, line and arrow (drag), and the eraser (click an annotation, including ones already in the file); then six colors, three line widths (also the text size), undo (Ctrl + Z) and Save. Esc puts the tool down. Hovering over a note shows its text |
| Fill in a form | the toolbar's pencil (shown for PDFs with a form), or "Fill in form" in the page menu. Click a field and type (Tab goes to the next; Enter or Esc stops), Ctrl + V pastes; click check boxes and radio buttons, click a drop-down or list box to pick. Closing a tab or the window with changes not saved asks whether to save them first |
| Save form entries and annotations into the PDF | Ctrl + S, the annotation bar's Save, or the page menu (PDFs only) |
| Save a copy | Ctrl + Shift + S: the PDF with its entries and annotations, as another file; the open one is left as it is |
| Main menu | F10, or the ⋯ button left of the gear: open, home, save, save a copy, annotate, print, properties, close, page navigation, find, toolbar, sidebar, full screen, presentation, settings, and About |
| Move far, fast | drag a scroll bar's thumb, or press anywhere on the bar to jump there; the wheel over the side bar moves a page per notch, over the bottom one sideways |
| Scroll | mouse wheel, arrow keys, drag with the middle button, or with the left button away from text. The wheel and keys glide smoothly; a touchpad moves the pages as your fingers do |
| Select text | drag over it (past the window's edge, the pages scroll); double-click selects a word, Ctrl + A the page's text |
| Copy the selection | Ctrl + C |
| Find | Ctrl + F (or click the search box), type; Enter / Shift + Enter or F3 / Shift + F3 for next and previous |
| Clear the selection, then the search | Esc |
| Scroll sideways | Shift + wheel |
| Screen down / up | Page Down / Page Up, Space / Shift + Space |
| Next / previous page | N / P, or → / ← when the page fits the window's width; the next pair of pages in a two-page layout |
| Go to a page | Ctrl + G, or click the page number, type, Enter |
| Follow a link | click it; hovering shows where it goes. Web and mail links open in your browser or mail program; links to files or programs are ignored |
| Back / forward after a jump | Alt + ← / Alt + →, or the mouse's Back / Forward buttons |
| Show or hide the sidebar (outline, bookmarks) | F12, or the toolbar's leftmost button; drag its edge to resize it |
| First / last page | Home / End |
| Zoom in / out | hold the right button and turn the wheel, or Ctrl + wheel (both zoom around the mouse); + / − |
| Fit page / actual size / fit width | Ctrl + 0 / Ctrl + 1 / Ctrl + 2 |
| Settings | Ctrl + , (comma), or the gear button |
| Show or hide the toolbar | F8 |
| Full screen | F11; Esc leaves it once nothing else is open |
| Close the window | Q, or Ctrl + W with no tab open |

EPUB books are laid out as 6 × 9 inch pages in Georgia: headings, paragraphs, lists, quotes, preformatted text, pictures and links, each chapter on a new page, with the table of contents in the sidebar. The book's own styles (CSS) aren't applied. Search and selection work in EPUB and in DjVu files that have a text layer.

A password-protected PDF asks for its password when it opens. The password is kept in memory for that tab only, never written anywhere.

## Settings

The settings window (the gear, or Ctrl + ,) has its categories down the left (Appearance, Reading, Files, Memory and speed, System) and each setting as a card with a short explanation and its switch, number or drop-down. Changes apply as you make them; Esc or a click outside closes it. The settings are: theme (system, light, dark, black), page colors (normal, dark, sepia, black), toolbar, sidebar and its width, space between pages, zoom for new documents, wheel scroll distance, window position, smooth scrolling, the web search for selected text (Google, Bing or DuckDuckGo), highlight color, opening files as tabs of one window, remembering recent files, reopening last session's tabs, reloading files when they change, the resolution of page images, page cache size and render threads. System shows whether Flores PDF is your default PDF reader, opens the settings file, resets the settings and opens About. Black theme and black pages together make everything pure black except text and pictures, title bar included (on Windows 11), which OLED screens show by turning those pixels off.

They're saved to `%APPDATA%\FloresPDF\settings.toml`, a plain text file that describes each setting. If a `FloresPDF-settings.toml` sits next to `FloresPDF.exe`, that one is used instead, which keeps a portable copy self-contained. Settings from before the rename, in `%APPDATA%\pdfreader`, are copied over the first time.

Beside the settings file are plain text files Flores PDF keeps for you (with a `FloresPDF-` prefix in a portable copy):

| File | What's in it |
|---|---|
| `history.txt` | Recent files, where you were in each (page, zoom, rotation, layout) and which tabs were open. Turning off "Remember opened files" deletes it |
| `favorites.txt` | Bookmarks: page and name |
| `highlights.txt` | Highlights of e-books, DjVu documents and comics, which can't hold them. (PDF highlights made before they went into the PDF are kept here until the PDF is next saved.) |

## Layout

| Crate | What it does |
|---|---|
| `crates/engine` | The `Engine` trait, the PDF adapter (links, outline, properties, passwords, forms), EPUB, DjVu, comics and pictures, display lists, the glyph and image caches, and hayro's renderer layer (vendored in `src/raster`, Apache-2.0 or MIT) |
| `crates/render` | Page layout (spreads, rotation), tile cache, render threads, compositor, page colors, the text thread for search and selection |
| `crates/app` | The window, input, tabs, toolbar, sidebar, menus, start page and its thumbnails, printing, recent files, bookmarks, annotations, form filling and saving through PDFium, file watching, settings panel and settings file (`FloresPDF.exe`, its icon in `flores.ico`) |
| `crates/setup` | `FloresPDF-Setup.exe`: holds the app, compressed, and installs it for the current user |
| `third_party/pdfium` | PDFium build 7881 for Windows x64 (`pdfium.dll`, without its JavaScript engine), with its licenses; the app's build packs both into the exe |
| `bench/` | Engine, memory and CPU benchmarks, separate from the app |

`cargo run --release -p engine --example bench -- file.pdf` times the engine with display lists on and off and compares their pixels. `cargo run --release -p engine --example text -- file.pdf 3` prints page 3's extracted text. `cargo run --release -p engine --example nav -- file.pdf 3` prints the outline and page 3's links. `cargo run --release -p flores-pdf --example icon` draws the icon again. `cargo run --release -p engine --example samples -- page.png folder` writes a DjVu (with a text layer for its title line), WebP and GIF of a page, for testing.

## License

Flores PDF is under the [MIT license](LICENSE). What it includes keeps its own license: hayro and its renderer (vendored in `crates/engine/src/raster`), Apache-2.0 or MIT; PDFium, BSD-3-Clause, with the licenses of its parts in `third_party/pdfium`; UnRAR, under the UnRAR license, which allows reading RAR files but not making them; and the other Rust crates under theirs, listed in About.
