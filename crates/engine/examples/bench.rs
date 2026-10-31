//! The engine on the workload of bench/README.md, with display lists on and off, so it lines up
//! with the MuPDF, PDFium and hayro numbers in docs/architecture.md §1. Single-threaded.
//! Also checks that both ways draw the same pixels.
//!
//! usage: cargo run --release -p engine --example bench -- <pdf>...

use engine::{Options, PageRenderer, PixelRect};
use std::path::Path;
use std::time::Instant;

const DPI_150: f32 = 150.0 / 72.0;
const DPI_300: f32 = 300.0 / 72.0;
const TILE_SCALE: f32 = 4.0;
const TILE: u32 = 1024;
const PAGES: usize = 40;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn whole_page(size: engine::PageSize, scale: f32) -> PixelRect {
    PixelRect { x: 0, y: 0, width: (size.width * scale).ceil() as u32, height: (size.height * scale).ceil() as u32 }
}

/// The 1024 px square in the middle of the page at 400%, moved right by `step` tiles.
fn middle_tile(size: engine::PageSize, step: u32) -> PixelRect {
    let cx = (size.width * TILE_SCALE / 2.0) as u32;
    let cy = (size.height * TILE_SCALE / 2.0) as u32;
    PixelRect { x: cx.saturating_sub(TILE / 2) + step * TILE, y: cy.saturating_sub(TILE / 2), width: TILE, height: TILE }
}

struct Run {
    open_ms: f64,
    first_page_ms: f64,
    pages_ms: f64,
    rezoom_ms: f64,
    tile_ms: f64,
    next_tile_ms: f64,
    /// Page 1 cut into the viewer's 512 px tiles, at 100% and then 200%.
    tiled_ms: f64,
    tiled_200_ms: f64,
    page: Vec<u32>,
    tile: Vec<u32>,
}

/// Renders a whole page as the viewer does, in 512 px tiles. Returns the time taken.
fn tiled(r: &mut Box<dyn PageRenderer + '_>, size: engine::PageSize, scale: f32) -> f64 {
    let all = whole_page(size, scale);
    let t = Instant::now();
    for y in (0..all.height).step_by(512) {
        for x in (0..all.width).step_by(512) {
            let area = PixelRect { x, y, width: 512.min(all.width - x), height: 512.min(all.height - y) };
            r.render(0, scale, area);
        }
    }
    ms(t)
}

fn run(path: &Path, display_lists: bool) -> Run {
    let t = Instant::now();
    let doc = engine::open_with(path, Options { display_lists }).expect("opening");
    let open_ms = ms(t);
    let mut r = doc.renderer();
    let size = doc.page_size(0);

    let t = Instant::now();
    let page = r.render(0, DPI_150, whole_page(size, DPI_150));
    let first_page_ms = ms(t);
    let t = Instant::now();
    for i in 1..doc.page_count().min(PAGES) {
        r.render(i, DPI_150, whole_page(doc.page_size(i), DPI_150));
    }
    let pages_ms = first_page_ms + ms(t);

    // page 1 again at another zoom; with display lists, page 1 is still recorded
    let t = Instant::now();
    r.render(0, DPI_300, whole_page(size, DPI_300));
    let rezoom_ms = ms(t);

    let t = Instant::now();
    let tile = r.render(0, TILE_SCALE, middle_tile(size, 0));
    let tile_ms = ms(t);
    let t = Instant::now();
    r.render(0, TILE_SCALE, middle_tile(size, 1));
    let next_tile_ms = ms(t);

    let tiled_ms = tiled(&mut r, size, 96.0 / 72.0);
    let tiled_200_ms = tiled(&mut r, size, 2.0 * 96.0 / 72.0);

    Run { open_ms, first_page_ms, pages_ms, rezoom_ms, tile_ms, next_tile_ms, tiled_ms, tiled_200_ms, page, tile }
}

/// Share of pixels where any channel differs by more than 8 levels, and the largest difference.
fn diff(a: &[u32], b: &[u32]) -> (f64, u32) {
    let mut off = 0;
    let mut max = 0;
    for (&x, &y) in a.iter().zip(b) {
        let d = (0..3).map(|i| ((x >> (8 * i)) & 0xFF).abs_diff((y >> (8 * i)) & 0xFF)).max().unwrap();
        max = max.max(d);
        off += usize::from(d > 8);
    }
    (100.0 * off as f64 / a.len().max(1) as f64, max)
}

/// Saves the 480 × 280 px block at (x, y) of a page rendered at 150 DPI, `width` pixels wide.
fn save_crop(page: &[u32], width: usize, (x, y): (usize, usize), path: &Path) {
    let (w, h) = (480, 280);
    let mut pixmap = vello_cpu::Pixmap::new(w as u16, h as u16);
    let data = pixmap.data_as_u8_slice_mut();
    for row in 0..h {
        for col in 0..w {
            let [_, r, g, b] = page[(y + row) * width + x + col].to_be_bytes();
            data[(row * w + col) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    std::fs::write(path, pixmap.into_png().expect("encoding png")).expect("writing png");
}

fn main() {
    // BENCH_CROPS=<dir> also saves a block of page 1 from both ways, to compare by eye
    let crops = std::env::var_os("BENCH_CROPS").map(std::path::PathBuf::from);
    for arg in std::env::args().skip(1) {
        let path = Path::new(&arg);
        let name = path.file_name().unwrap().to_string_lossy();
        let direct = run(path, false);
        let listed = run(path, true);
        if let Some(dir) = &crops {
            let doc = engine::open(path).expect("opening");
            let width = whole_page(doc.page_size(0), DPI_150).width as usize;
            let at = (width / 2 - 240, 900);
            save_crop(&direct.page, width, at, &dir.join(format!("{name}-direct.png")));
            save_crop(&listed.page, width, at, &dir.join(format!("{name}-stamped.png")));
        }
        let (page_off, page_max) = diff(&direct.page, &listed.page);
        let (tile_off, tile_max) = diff(&direct.tile, &listed.tile);
        for (mode, r) in [("direct", &direct), ("display list", &listed)] {
            println!(
                "{{\"file\":{name:?},\"mode\":{mode:?},\"open_ms\":{:.1},\"first_page_ms\":{:.1},\"pages_ms\":{:.1},\
\"rezoom_300dpi_ms\":{:.1},\"tile_400_ms\":{:.1},\"next_tile_400_ms\":{:.1},\"tiled_100_ms\":{:.1},\"tiled_200_ms\":{:.1}}}",
                r.open_ms, r.first_page_ms, r.pages_ms, r.rezoom_ms, r.tile_ms, r.next_tile_ms, r.tiled_ms, r.tiled_200_ms
            );
        }
        println!(
            "{{\"file\":{name:?},\"pixels_off_page_pct\":{page_off:.3},\"max_diff_page\":{page_max},\
\"pixels_off_tile_pct\":{tile_off:.3},\"max_diff_tile\":{tile_max}}}"
        );
    }
}
