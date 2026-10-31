//! Printing, on a thread of its own. Each page is drawn at the printer's resolution in bands, so
//! a page at 600 dpi never needs a hundred megabytes at once. Pages are shrunk to fit the printable
//! area if they're too big, never enlarged, and turned when they're landscape on portrait paper or
//! the other way round. Colors are the page's own, whatever the reading colors are.

use crate::win::Printer;
use engine::{Engine, PixelRect};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Rows drawn and sent at a time.
const BAND: u32 = 256;
/// The renderer's limit on a side, in pixels.
const MAX_SIDE: f32 = 30_000.0;

/// Prints the printer's chosen pages of `engine`'s document, calling `progress` with the number
/// done and the total before each page. Returns how many pages were printed.
pub fn print(printer: &Printer, engine: Arc<dyn Engine>, name: &str, cancel: &AtomicBool, progress: impl Fn(usize, usize)) -> Result<usize, String> {
    let (area_w, area_h, dpi_x, dpi_y) = printer.area();
    let pages: Vec<usize> = printer.pages.clone().filter(|&p| p < engine.page_count()).collect();
    if !printer.start(name) {
        return Err("The printer didn't start the print job".to_owned());
    }
    let mut renderer = engine.renderer();
    for (done, &page) in pages.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            printer.abort();
            return Err("Printing stopped".to_owned());
        }
        progress(done, pages.len());
        let size = engine.page_size(page);
        // a landscape page on portrait paper is turned, and the other way round
        let turn = (size.width > size.height) != (area_w > area_h);
        let (w, h) = if turn { (size.height, size.width) } else { (size.width, size.height) };
        // pixels per point across, at the printer's resolution, shrunk to fit
        let full = dpi_x as f32 / 72.0;
        let fit = (area_w as f32 / w).min(area_h as f32 * dpi_x as f32 / dpi_y as f32 / h);
        let scale = full.min(fit).min(MAX_SIDE / w.max(h));
        let (width, height) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
        // down the page, printer dots aren't always the same size as across
        let stretch = dpi_y as f32 / dpi_x as f32;
        let dest_h = |rows: u32| (rows as f32 * stretch).round() as i32;
        let x = (area_w.saturating_sub(width) / 2) as i32;
        let y = ((area_h as i32 - dest_h(height)) / 2).max(0);
        if !printer.start_page() {
            printer.abort();
            return Err("The printer didn't take a page".to_owned());
        }
        for top in (0..height).step_by(BAND as usize) {
            let rows = BAND.min(height - top);
            let area = PixelRect { x: 0, y: top, width, height: rows };
            let pixels = render::render_turned(&mut *renderer, &*engine, page, scale, u8::from(turn), area);
            if !printer.draw(x, y + dest_h(top), width as i32, dest_h(rows), width, &pixels) {
                printer.abort();
                return Err("The printer didn't take a page".to_owned());
            }
        }
        if !printer.end_page() {
            printer.abort();
            return Err("The printer didn't finish a page".to_owned());
        }
    }
    drop(renderer);
    // what printing drew stays in the engine's shared caches otherwise
    engine.trim();
    if !printer.end() {
        return Err("The printer didn't finish the print job".to_owned());
    }
    Ok(pages.len())
}
