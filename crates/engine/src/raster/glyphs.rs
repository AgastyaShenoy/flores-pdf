//! Not from hayro: a glyph cache. Each glyph is rasterized once per size (and quarter-pixel
//! position) into a coverage mask, and then stamped wherever it appears, instead of having vello_cpu
//! turn its outline into pixels every time. That conversion is most of the cost of a text page.
//!
//! Stamping writes straight into the rendered tile, so it happens after vello_cpu has drawn
//! everything else. `record.rs` decides which text can wait until then.

use kurbo::{Affine, BezPath, Rect};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use vello_cpu::color::palette::css::{TRANSPARENT, WHITE};
use vello_cpu::peniko::Fill;
use vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};

/// Glyphs drawn bigger than this many pixels are left to vello_cpu: few of them fit on a screen,
/// and their masks would be large.
pub(crate) const MAX_GLYPH_PX: f64 = 192.0;
/// Horizontal positions are rounded to quarter pixels, vertical ones to whole pixels (baselines
/// snap, as in most text renderers), so a glyph has at most 4 masks per size.
const SUBPIXELS: f64 = 4.0;
/// Over this many bytes of masks, the shared cache starts over.
const BUDGET: usize = 8 << 20;
/// Over this many entries, a thread's own cache starts over.
const LOCAL_ENTRIES: usize = 20_000;

/// Where a glyph's coverage is, relative to the whole-pixel position it's drawn at.
struct Mask {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    glyph: u128,
    /// The transform's scale and skew, to 1e-6.
    matrix: [i64; 4],
    /// Quarter-pixel offset in x.
    offset: u8,
}

/// A rendered tile's pixels: premultiplied RGBA8, `stride` pixels per row.
pub(crate) struct Target<'a> {
    pub(crate) data: &'a mut [u8],
    pub(crate) stride: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

/// `None` for glyphs with nothing to draw, such as spaces.
type Masks = HashMap<Key, Option<Arc<Mask>>>;

/// Masks shared by a document's render threads, so each glyph is rasterized once, not once per
/// thread.
#[derive(Default)]
pub(crate) struct SharedGlyphs(Mutex<(Masks, usize)>);

impl SharedGlyphs {
    pub(crate) fn clear(&self) {
        *self.0.lock().unwrap() = Default::default();
    }

    fn get(&self, key: &Key) -> Option<Option<Arc<Mask>>> {
        self.0.lock().unwrap().0.get(key).cloned()
    }

    fn insert(&self, key: Key, mask: Option<Arc<Mask>>) {
        let mut shared = self.0.lock().unwrap();
        shared.1 += mask.as_ref().map_or(0, |m| m.alpha.len()) + 64;
        if shared.1 > BUDGET {
            *shared = Default::default();
        }
        shared.0.insert(key, mask);
    }
}

/// A render thread's view of the glyph cache: its own map, filled from the shared one, so the
/// common case needs no lock.
pub(crate) struct GlyphCache {
    shared: Arc<SharedGlyphs>,
    local: Masks,
    ctx: RenderContext,
    pixmap: Pixmap,
}

/// Splits a coordinate into whole pixels and a quarter-pixel step (0 to 3).
fn split(v: f64) -> (i32, u8) {
    let quarters = (v * SUBPIXELS).round();
    let whole = (quarters / SUBPIXELS).floor();
    (whole as i32, (quarters - whole * SUBPIXELS) as u8)
}

impl GlyphCache {
    pub(crate) fn new(shared: Arc<SharedGlyphs>) -> Self {
        Self {
            shared,
            local: HashMap::new(),
            ctx: RenderContext::new_with(1, 1, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() }),
            pixmap: Pixmap::new(1, 1),
        }
    }

    /// Stamps `outline` (in glyph space) placed by `transform`, in `color` (RGBA, not premultiplied),
    /// inside `clip` (pixels).
    pub(crate) fn draw(&mut self, target: &mut Target, outline: &BezPath, glyph: u128, transform: Affine, color: [u8; 4], clip: Rect) {
        let [a, b, c, d, e, f] = transform.as_coeffs();
        let (x, offset) = split(e);
        let y = f.round() as i32;
        let key = Key { glyph, matrix: [a, b, c, d].map(|v| (v * 1e6).round() as i64), offset };
        if !self.local.contains_key(&key) {
            let mask = self.shared.get(&key).unwrap_or_else(|| {
                let shape = Affine::new([a, b, c, d, f64::from(offset) / SUBPIXELS, 0.0]);
                let mask = self.rasterize(outline, shape).map(Arc::new);
                self.shared.insert(key, mask.clone());
                mask
            });
            if self.local.len() >= LOCAL_ENTRIES {
                self.local.clear();
            }
            self.local.insert(key, mask);
        }
        if let Some(mask) = &self.local[&key] {
            stamp(target, mask, x + mask.x, y + mask.y, color, clip);
        }
    }

    /// Coverage of `outline` drawn with `transform`, which places it within a pixel of the origin.
    fn rasterize(&mut self, outline: &BezPath, transform: Affine) -> Option<Mask> {
        let bounds = transform.transform_rect_bbox(outline.control_box());
        let (x0, y0) = (bounds.x0.floor(), bounds.y0.floor());
        let (width, height) = ((bounds.x1.ceil() - x0) as usize, (bounds.y1.ceil() - y0) as usize);
        if width == 0 || height == 0 || width > u16::MAX as usize || height > u16::MAX as usize {
            return None;
        }
        let (w, h) = (width as u16, height as u16);
        self.ctx.reset_and_resize(w, h);
        self.ctx.set_transform(Affine::translate((-x0, -y0)) * transform);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_paint(WHITE);
        self.ctx.fill_path(outline);
        self.ctx.flush();
        if self.pixmap.width() < w || self.pixmap.height() < h {
            self.pixmap = Pixmap::new(w.max(self.pixmap.width()), h.max(self.pixmap.height()));
        }
        self.ctx.render_with(
            &mut self.pixmap,
            &mut Resources::default(),
            RasterizerSettings { target_init: TargetInit::Clear(TRANSPARENT), ..Default::default() },
        );
        let stride = usize::from(self.pixmap.width()) * 4;
        let mut alpha = Vec::with_capacity(width * height);
        for row in self.pixmap.data_as_u8_slice().chunks_exact(stride).take(height) {
            alpha.extend(row[..width * 4].chunks_exact(4).map(|p| p[3]));
        }
        Some(Mask { x: x0 as i32, y: y0 as i32, width, height, alpha })
    }
}

/// Blends `color` through `mask`, whose top left is at (x, y), into the target within `clip`.
fn stamp(target: &mut Target, mask: &Mask, x: i32, y: i32, color: [u8; 4], clip: Rect) {
    let cx0 = clip.x0.round().max(0.0) as i32;
    let cy0 = clip.y0.round().max(0.0) as i32;
    let cx1 = (clip.x1.round() as i32).min(target.width as i32);
    let cy1 = (clip.y1.round() as i32).min(target.height as i32);
    let (x0, x1) = (x.max(cx0), (x + mask.width as i32).min(cx1));
    let (y0, y1) = (y.max(cy0), (y + mask.height as i32).min(cy1));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let [r, g, b, a] = color.map(u32::from);
    for py in y0..y1 {
        let coverage = &mask.alpha[(py - y) as usize * mask.width..][(x0 - x) as usize..(x1 - x) as usize];
        let row = &mut target.data[(py as usize * target.stride + x0 as usize) * 4..][..(x1 - x0) as usize * 4];
        for (px, &cov) in row.chunks_exact_mut(4).zip(coverage) {
            if cov == 0 {
                continue;
            }
            let alpha = (u32::from(cov) * a + 127) / 255;
            let keep = 255 - alpha;
            let mix = |src: u32, dst: u8| ((src * alpha + u32::from(dst) * keep + 127) / 255) as u8;
            px[0] = mix(r, px[0]);
            px[1] = mix(g, px[1]);
            px[2] = mix(b, px[2]);
            px[3] = (alpha + (u32::from(px[3]) * keep + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn split_rounds_to_quarters() {
        assert_eq!(split(10.0), (10, 0));
        assert_eq!(split(10.26), (10, 1));
        assert_eq!(split(10.9), (11, 0));
        assert_eq!(split(-0.3), (-1, 3));
    }

    #[test]
    fn stamps_a_square_in_color_and_clips() {
        let mut cache = GlyphCache::new(Arc::default());
        let square = Rect::new(0.0, 0.0, 4.0, 4.0).to_path(0.1);
        let mut data = vec![255u8; 8 * 8 * 4];
        let mut target = Target { data: &mut data, stride: 8, width: 8, height: 8 };
        let clip = Rect::new(0.0, 0.0, 5.0, 8.0);
        cache.draw(&mut target, &square, 1, Affine::translate((2.0, 2.0)), [255, 0, 0, 255], clip);
        let px = |x: usize, y: usize| &data[(y * 8 + x) * 4..][..4];
        assert_eq!(px(2, 2), [255, 0, 0, 255]);
        assert_eq!(px(4, 5), [255, 0, 0, 255]);
        // outside the square, and inside it but outside the clip
        assert_eq!(px(1, 2), [255, 255, 255, 255]);
        assert_eq!(px(5, 3), [255, 255, 255, 255]);
    }
}
