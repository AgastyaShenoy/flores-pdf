use crate::cache::{Tile, TileCache, TileKey};
use crate::layout::{Layout, TILE, Viewport};
use crate::look::Look;

/// Where the engine failed on a tile.
const FAILED: u32 = 0x00F4_CCCC;

/// How pages are drawn.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Around the pages.
    pub background: u32,
    /// A one-pixel line around each page.
    pub border: Option<u32>,
    pub look: Look,
}

/// A rectangle of 0x00RRGGBB pixels inside the window buffer: `width` × `height`, with rows
/// `stride` pixels apart.
pub struct Frame<'a> {
    pub pixels: &'a mut [u32],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
}

/// A rectangle on screen, from (x0, y0) up to but not including (x1, y1).
#[derive(Clone, Copy)]
struct Span {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Span {
    fn new(x: i64, y: i64, width: u32, height: u32) -> Self {
        Self { x0: x, y0: y, x1: x + i64::from(width), y1: y + i64::from(height) }
    }

    fn intersect(self, o: Span) -> Option<Span> {
        let s = Span { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        (s.x0 < s.x1 && s.y0 < s.y1).then_some(s)
    }
}

/// Draws the pages in `view` into `frame` from cached tiles. Where a sharp tile is missing, tiles of the
/// same page cached at other zoom levels are stretched over the gap, as Sumatra does, so zooming never
/// shows a blank page.
pub fn compose(frame: &mut Frame, layout: &Layout, view: &Viewport, cache: &TileCache, style: &Style) {
    fill(frame, Span::new(0, 0, frame.width, frame.height), style.background);
    let screen = Span::new(0, 0, frame.width, frame.height);
    let scale = layout.scale.to_bits();
    let (look, paper) = (style.look as u8, style.look.paper());
    // one row more on each side, for the border
    for page in layout.pages_between(view.y - 1, view.y + i64::from(view.height) + 1) {
        let pos = layout.pages[page];
        let (ox, oy) = (pos.x - view.x, pos.y - view.y);
        if let Some(border) = style.border {
            let outer = Span::new(ox - 1, oy - 1, pos.width + 2, pos.height + 2);
            if let Some(s) = outer.intersect(screen) {
                fill(frame, s, border);
            }
        }
        let mut stale: Option<Vec<(&TileKey, &Tile)>> = None;
        for (col, row, area) in layout.tiles(page, view) {
            let (tx, ty) = (ox + i64::from(area.x), oy + i64::from(area.y));
            let Some(dest) = Span::new(tx, ty, area.width, area.height).intersect(screen) else { continue };
            match cache.get(&TileKey { page, scale, col, row, look, rotation: layout.rotation }) {
                Some(tile) if !tile.pixels.is_empty() => blit(frame, dest, tx, ty, tile),
                Some(_) => fill(frame, dest, FAILED),
                None => {
                    fill(frame, dest, paper);
                    let stale = stale.get_or_insert_with(|| {
                        // farthest zoom first, so the closest one ends up on top
                        let mut v: Vec<_> = cache.other_scales(page, scale, look, layout.rotation).collect();
                        let off = |k: &TileKey| (f32::from_bits(k.scale) / layout.scale).ln().abs();
                        v.sort_by(|a, b| off(b.0).total_cmp(&off(a.0)));
                        v
                    });
                    for &(key, tile) in stale.iter() {
                        stretch(frame, dest, (ox, oy), layout.scale, key, tile, cache);
                    }
                }
            }
        }
    }
}

pub fn fill_rect(frame: &mut Frame, x: i64, y: i64, width: u32, height: u32, color: u32) {
    if let Some(s) = Span::new(x, y, width, height).intersect(Span::new(0, 0, frame.width, frame.height)) {
        fill(frame, s, color);
    }
}

/// Blends `color` over a rectangle, `alpha` of 255 being opaque. For highlights, so the text
/// underneath stays readable.
pub fn tint_rect(frame: &mut Frame, x: i64, y: i64, width: u32, height: u32, color: u32, alpha: u32) {
    let Some(s) = Span::new(x, y, width, height).intersect(Span::new(0, 0, frame.width, frame.height)) else { return };
    let stride = frame.stride;
    let keep = 255 - alpha;
    let [_, r, g, b] = color.to_be_bytes().map(u32::from);
    for y in s.y0..s.y1 {
        for p in &mut frame.pixels[y as usize * stride..][s.x0 as usize..s.x1 as usize] {
            let [_, pr, pg, pb] = p.to_be_bytes().map(u32::from);
            let mix = |c: u32, d: u32| ((c * alpha + d * keep + 127) / 255) as u8;
            *p = u32::from_be_bytes([0, mix(r, pr), mix(g, pg), mix(b, pb)]);
        }
    }
}

fn fill(frame: &mut Frame, s: Span, color: u32) {
    let stride = frame.stride;
    for y in s.y0..s.y1 {
        frame.pixels[y as usize * stride..][s.x0 as usize..s.x1 as usize].fill(color);
    }
}

/// Copies the part of `tile` (whose top-left is at `tx, ty` on screen) that falls in `dest`.
fn blit(frame: &mut Frame, dest: Span, tx: i64, ty: i64, tile: &Tile) {
    let stride = frame.stride;
    let (n, sx) = ((dest.x1 - dest.x0) as usize, (dest.x0 - tx) as u32);
    for y in dest.y0..dest.y1 {
        let out = &mut frame.pixels[y as usize * stride + dest.x0 as usize..][..n];
        tile.pixels.read_row(tile.width, (y - ty) as u32, sx, out);
    }
}

/// Draws a tile cached at another zoom level, scaled to the current one with nearest-neighbour
/// sampling, into the part of `dest` it covers. `origin` is the page's top-left on screen.
fn stretch(frame: &mut Frame, dest: Span, origin: (i64, i64), scale: f32, key: &TileKey, tile: &Tile, cache: &TileCache) {
    let r = f64::from(scale) / f64::from(f32::from_bits(key.scale));
    let (tx, ty) = (f64::from(key.col * TILE), f64::from(key.row * TILE));
    let covers = Span {
        x0: origin.0 + (tx * r).floor() as i64,
        y0: origin.1 + (ty * r).floor() as i64,
        x1: origin.0 + ((tx + f64::from(tile.width)) * r).ceil() as i64,
        y1: origin.1 + ((ty + f64::from(tile.height)) * r).ceil() as i64,
    };
    let Some(d) = dest.intersect(covers) else { return };
    cache.mark_used(tile);
    // screen pixel -> pixel of the stale tile
    let src = |screen: i64, origin: i64, t: f64, len: u32| {
        (((screen - origin) as f64 + 0.5) / r - t).clamp(0.0, f64::from(len - 1)) as usize
    };
    let cols: Vec<usize> = (d.x0..d.x1).map(|x| src(x, origin.0, tx, tile.width)).collect();
    let stride = frame.stride;
    let (mut row, mut decoded) = (vec![0; tile.width as usize], usize::MAX);
    for y in d.y0..d.y1 {
        // when zooming in, several screen rows come from the same tile row
        let sy = src(y, origin.1, ty, tile.height);
        if sy != decoded {
            tile.pixels.read_row(tile.width, sy as u32, 0, &mut row);
            decoded = sy;
        }
        let out = &mut frame.pixels[y as usize * stride + d.x0 as usize..][..cols.len()];
        for (px, &sx) in out.iter_mut().zip(&cols) {
            *px = row[sx];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::Pixels;
    use engine::PageSize;

    const BLACK: u32 = 0;
    const RED: u32 = 0x00FF_0000;
    const PAGE: u32 = 0x00FF_FFFF;

    fn style(background: u32) -> Style {
        Style { background, border: None, look: Look::Normal }
    }

    /// One 100×100 pt page at 1 px per point, filling a 100×100 window.
    fn setup() -> (Layout, Viewport, Vec<u32>) {
        let layout = Layout::new(&[PageSize { width: 100.0, height: 100.0 }], 1.0, 0, 100);
        (layout, Viewport { x: 0, y: 0, width: 100, height: 100 }, vec![0x0012_3456; 100 * 100])
    }

    /// A 50×50 tile from zoom 0.5: black on the left half, white on the right.
    fn half_scale_tile(cache: &mut TileCache) {
        let px = (0..50 * 50).map(|i| if i % 50 < 25 { BLACK } else { PAGE }).collect();
        let key = TileKey { page: 0, scale: 0.5f32.to_bits(), col: 0, row: 0, look: 0, rotation: 0 };
        cache.insert(key, 50, 50, Pixels::encode(px, 50));
    }

    #[test]
    fn stretches_a_tile_from_another_zoom() {
        let (layout, view, mut buf) = setup();
        let mut cache = TileCache::default();
        half_scale_tile(&mut cache);
        compose(&mut Frame { pixels: &mut buf, width: 100, height: 100, stride: 100 }, &layout, &view, &cache, &style(0));
        for y in [0, 37, 99] {
            assert_eq!(buf[y * 100 + 49], BLACK);
            assert_eq!(buf[y * 100 + 50], PAGE);
        }
    }

    #[test]
    fn sharp_tile_wins() {
        let (layout, view, mut buf) = setup();
        let mut cache = TileCache::default();
        half_scale_tile(&mut cache);
        let key = TileKey { page: 0, scale: 1.0f32.to_bits(), col: 0, row: 0, look: 0, rotation: 0 };
        cache.insert(key, 100, 100, Pixels::encode(vec![RED; 100 * 100], 100));
        compose(&mut Frame { pixels: &mut buf, width: 100, height: 100, stride: 100 }, &layout, &view, &cache, &style(0));
        assert!(buf.iter().all(|&p| p == RED));
    }

    #[test]
    fn missing_tile_is_white_page_on_background() {
        let layout = Layout::new(&[PageSize { width: 50.0, height: 50.0 }], 1.0, 10, 100);
        let view = Viewport { x: 0, y: 0, width: 100, height: 100 };
        let mut buf = vec![0; 100 * 100];
        compose(&mut Frame { pixels: &mut buf, width: 100, height: 100, stride: 100 }, &layout, &view, &TileCache::default(), &style(7));
        // the page is centered: x 25..75, y 10..60
        assert_eq!(buf[5 * 100 + 50], 7);
        assert_eq!(buf[10 * 100 + 25], PAGE);
        assert_eq!(buf[59 * 100 + 74], PAGE);
        assert_eq!(buf[30 * 100 + 24], 7);
        assert_eq!(buf[60 * 100 + 50], 7);
    }
}
