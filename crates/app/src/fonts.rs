//! UI text: Windows' own fonts (Segoe UI, and Segoe Fluent Icons for toolbar icons), outlined by
//! skrifa with TrueType hinting so small text stays crisp, and filled with vello_cpu.
//!
//! Font files are memory-mapped, so their pages are shared with every other program using them
//! instead of counting as our memory. Simple shaping: one glyph per character, no kerning, which is
//! fine for short UI labels.

use kurbo::{Affine, BezPath};
use memmap2::Mmap;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, HintingOptions, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::HashMap;
use std::fs::File;
use vello_cpu::RenderContext;

pub struct Font {
    data: Mmap,
    /// Outlines and advances per pixel size (in quarter pixels).
    sizes: HashMap<u32, Sized>,
}

struct Sized {
    hinting: Option<HintingInstance>,
    glyphs: HashMap<char, (BezPath, f32)>,
}

impl Font {
    /// The first of `names` (in the Windows fonts folder) that exists and parses.
    pub fn load(names: &[&str]) -> Option<Font> {
        let dir = std::env::var_os("WINDIR").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from).join("Fonts");
        names.iter().find_map(|name| {
            let file = File::open(dir.join(name)).ok()?;
            // SAFETY: font files in the Windows folder aren't modified while in use.
            let data = unsafe { Mmap::map(&file) }.ok()?;
            FontRef::new(&data).ok()?;
            Some(Font { data, sizes: HashMap::new() })
        })
    }

    fn font(&self) -> FontRef<'_> {
        FontRef::new(&self.data).expect("checked when loaded")
    }

    fn glyph(&mut self, c: char, px: f32) -> &(BezPath, f32) {
        let key = (px * 4.0).round() as u32;
        let font = FontRef::new(&self.data).expect("checked when loaded");
        let sized = self.sizes.entry(key).or_insert_with(|| Sized {
            hinting: HintingInstance::new(&font.outline_glyphs(), Size::new(px), LocationRef::default(), HintingOptions::default())
                .ok(),
            glyphs: HashMap::new(),
        });
        sized.glyphs.entry(c).or_insert_with(|| {
            let id = font.charmap().map(c).unwrap_or(GlyphId::NOTDEF);
            let mut pen = Pen(BezPath::new());
            if let Some(outline) = font.outline_glyphs().get(id) {
                let settings = match &sized.hinting {
                    Some(h) => DrawSettings::hinted(h, false),
                    None => DrawSettings::unhinted(Size::new(px), LocationRef::default()),
                };
                let _ = outline.draw(settings, &mut pen);
            }
            let advance = font.glyph_metrics(Size::new(px), LocationRef::default()).advance_width(id).unwrap_or(0.0);
            (pen.0, advance)
        })
    }

    /// `text` as one path at `size` (in any unit: points, pixels), unhinted, y down, starting at x 0
    /// with its baseline at y 0; and its width. For text drawn at any scale, and written into PDFs.
    pub fn outlines(&self, text: &str, size: f32) -> (BezPath, f32) {
        let font = self.font();
        let (glyphs, metrics) = (font.outline_glyphs(), font.glyph_metrics(Size::new(size), LocationRef::default()));
        let mut out = BezPath::new();
        let mut x = 0.0;
        for c in text.chars() {
            let id = font.charmap().map(c).unwrap_or(GlyphId::NOTDEF);
            let mut pen = Pen(BezPath::new());
            if let Some(outline) = glyphs.get(id) {
                let _ = outline.draw(DrawSettings::unhinted(Size::new(size), LocationRef::default()), &mut pen);
            }
            out.extend(Affine::translate((f64::from(x), 0.0)) * Affine::scale_non_uniform(1.0, -1.0) * pen.0);
            x += metrics.advance_width(id).unwrap_or(0.0);
        }
        (out, x)
    }

    /// Width of `text` at `px` pixels, as `draw` lays it out.
    pub fn measure(&mut self, text: &str, px: f32) -> f32 {
        text.chars().map(|c| self.glyph(c, px).1.round()).sum()
    }

    /// Distance from the top of a line to the baseline, and the line's height.
    pub fn line(&self, px: f32) -> (f32, f32) {
        let m = self.font().metrics(Size::new(px), LocationRef::default());
        (m.ascent, m.ascent - m.descent)
    }

    /// Fills `text` in the context's current paint, starting at `x` with the baseline at `y`.
    /// Glyphs sit on whole pixels so hinted stems stay sharp.
    pub fn draw(&mut self, ctx: &mut RenderContext, text: &str, px: f32, x: f64, y: f64) {
        let (mut x, y) = (x.round(), y.round());
        for c in text.chars() {
            let (path, advance) = self.glyph(c, px);
            ctx.set_transform(Affine::translate((x, y)) * Affine::scale_non_uniform(1.0, -1.0));
            ctx.fill_path(path);
            x += f64::from(advance.round());
        }
        ctx.set_transform(Affine::IDENTITY);
    }

    /// Draws `text` centered in the box at (x, y) with size w × h.
    pub fn draw_centered(&mut self, ctx: &mut RenderContext, text: &str, px: f32, (x, y, w, h): (f64, f64, f64, f64)) {
        let width = f64::from(self.measure(text, px));
        let (ascent, height) = self.line(px);
        let top = y + (h - f64::from(height)) / 2.0;
        self.draw(ctx, text, px, x + (w - width) / 2.0, top + f64::from(ascent));
    }
}

/// Collects a glyph outline. skrifa draws y-up; `draw` flips it.
struct Pen(BezPath);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((f64::from(x), f64::from(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((f64::from(x), f64::from(y)));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((f64::from(cx0), f64::from(cy0)), (f64::from(x), f64::from(y)));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((f64::from(cx0), f64::from(cy0)), (f64::from(cx1), f64::from(cy1)), (f64::from(x), f64::from(y)));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}
