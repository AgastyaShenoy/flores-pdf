//! Opens a PDF with hayro and renders page 1 fitted to a height, as RGBA8.
//! The document stays open, as it would in a real viewer. The render cache borrows from it, so it is dropped here.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

pub struct Page {
    pub doc: Pdf,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn render_first_page(path: &str, height_px: u32) -> Page {
    let doc = Pdf::new(std::fs::read(path).unwrap()).unwrap();
    let pages = doc.pages();
    let page = &pages[0];
    let scale = height_px as f32 / page.render_dimensions().1;
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let pm = render(page, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let (width, height) = (pm.width() as u32, pm.height() as u32);
    let rgba = pm.data_as_u8_slice().to_vec();
    Page { doc, width, height, rgba }
}
