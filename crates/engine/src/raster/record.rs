//! Not from hayro: display lists. A page is interpreted once into a list of drawing commands at
//! 1 pixel per point, then replayed for each tile and zoom level. Replay skips commands whose bounds
//! miss the tile, and whole clipped sections when the clip misses it.
//!
//! Only what can be moved to another scale by changing its transform is recorded. Shadings, tiling
//! patterns and soft masks carry matrices from interpretation time, and Type 3 glyphs re-enter the
//! interpreter, so pages using them return `None` and render directly instead. What's left is plain
//! data (colors, paths, decoded images), so one list is shared by all render threads.
//!
//! Text that nothing drawn after it overlaps is "deferred": vello_cpu draws everything else, then
//! the text is stamped on top from the glyph cache (`glyphs.rs`), which is much cheaper than
//! rasterizing every glyph outline. On a typical page that's all of the text.

use super::glyphs::{GlyphCache, MAX_GLYPH_PX, Target};
use super::shared::{Decoded, ImageCache};
use super::{RenderCache, Renderer};
use hayro_interpret::color::{AlphaColor, Color};
use hayro_interpret::font::{Glyph, GlyphRun};
use hayro_interpret::hayro_syntax::page::Page;
use hayro_interpret::util::{RectExt, TransformExt};
use hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, DrawMode, DrawProps, FillRule, Image, ImageDrawProps,
    InterpreterSettings, Paint, SoftMask, interpret_page,
};
use kurbo::{Affine, BezPath, Join, Rect, Shape};
use std::collections::HashMap;
use std::sync::Arc;

/// A page whose images decode to more than this renders directly. Recording has to decode images
/// at full resolution, where direct rendering asks for the size it needs, which for scans is much
/// cheaper. Direct rendering still shares scaled images between tiles (`shared.rs`).
const MAX_IMAGE_BYTES: usize = 4 << 20;

/// How a draw is painted: always a plain color, since anything else isn't recorded.
#[derive(Clone, Copy)]
struct Paint8 {
    transform: Affine,
    /// RGBA, not premultiplied, as hayro converts the PDF's color.
    color: [u8; 4],
    blend: BlendMode,
}

impl Paint8 {
    /// hayro's drawing properties for this paint, moved by `transform`.
    fn props(&self, transform: Affine) -> DrawProps<'static> {
        let [r, g, b, a] = self.color;
        DrawProps {
            transform: transform * self.transform,
            paint: Paint::Color(Color::from_rgba(AlphaColor::from_rgba8(r, g, b, a))),
            soft_mask: None,
            blend_mode: self.blend,
        }
    }
}

enum Cmd {
    Path(BezPath, Paint8, DrawMode),
    Rect(Rect, Paint8, DrawMode),
    /// Glyph outlines, their transforms within the text, and their cache keys.
    Glyphs(Vec<(Arc<BezPath>, Affine, u128)>, Paint8, DrawMode),
    /// `size` is the image's size as declared in the PDF; `paint.color` is a stencil's color.
    Image { decoded: Arc<Decoded>, key: u128, size: (u32, u32), paint: Paint8 },
    PushClip(BezPath, FillRule),
    PopClip,
    PushGroup(f32, BlendMode),
    PopGroup,
}

struct Entry {
    cmd: Cmd,
    /// Bounds in recording space, for draws and clips.
    bbox: Rect,
    /// For `PushClip`: the index of its `PopClip`.
    end: usize,
    /// For `Glyphs`: stamped after everything else (see the module docs).
    deferred: bool,
}

pub(crate) struct DisplayList {
    entries: Vec<Entry>,
    /// The biggest glyph's width or height, in recording space.
    max_glyph: f64,
}

/// Records `page` at 1 pixel per point (y down, origin at the top left), or returns `None` if it
/// can't be replayed at other scales.
pub(crate) fn record<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    images: &ImageCache,
) -> Option<DisplayList> {
    let (width, height) = page.render_dimensions();
    let initial = page.initial_transform(true).to_kurbo();
    let mut state = Context::new(
        initial,
        Rect::new(0.0, 0.0, width as f64, height as f64),
        &cache.interpreter_cache,
        page.xref(),
        settings.clone(),
    );
    let mut rec = Recorder {
        entries: Vec::new(),
        open: Vec::new(),
        ok: true,
        image_bytes: 0,
        max_glyph: 0.0,
        outlines: HashMap::new(),
        cache,
        images,
    };
    let mut crop = page.intersected_crop_box().to_kurbo().to_path(0.1);
    crop.apply_affine(initial);
    rec.push_clip_path(&ClipPath { path: crop, fill: FillRule::NonZero });
    interpret_page(page, &mut state, &mut rec);
    rec.pop_clip();
    if !rec.ok {
        return None;
    }
    // a clip left open covers the rest of the page
    let len = rec.entries.len();
    for i in rec.open.drain(..) {
        rec.entries[i].end = len;
    }
    defer_text(&mut rec.entries);
    Some(DisplayList { entries: rec.entries, max_glyph: rec.max_glyph })
}

/// The rectangle `path` is, if it's an axis-aligned one.
fn axis_rect(path: &BezPath) -> Option<Rect> {
    use kurbo::PathEl::{ClosePath, LineTo, MoveTo};
    let mut points = Vec::with_capacity(5);
    for el in path.elements() {
        match el {
            MoveTo(p) if points.is_empty() => points.push(*p),
            LineTo(p) => points.push(*p),
            ClosePath => {}
            _ => return None,
        }
    }
    if !(4..=5).contains(&points.len()) {
        return None;
    }
    let n = points.len();
    let straight = (0..n).all(|k| {
        let (a, b) = (points[k], points[(k + 1) % n]);
        (a.x - b.x).abs() < 1e-6 || (a.y - b.y).abs() < 1e-6
    });
    straight.then(|| path.control_box())
}

/// Marks text that can be stamped after everything else: filled with normal blending, outside
/// transparency groups, only under rectangular clips, and not overlapped by anything drawn later
/// that isn't deferred too.
fn defer_text(entries: &mut [Entry]) {
    let mut candidate = vec![false; entries.len()];
    let mut clips: Vec<bool> = Vec::new();
    let mut groups = 0usize;
    for (i, e) in entries.iter().enumerate() {
        match &e.cmd {
            Cmd::PushClip(path, _) => clips.push(axis_rect(path).is_some()),
            Cmd::PopClip => {
                clips.pop();
            }
            Cmd::PushGroup(..) => groups += 1,
            Cmd::PopGroup => groups = groups.saturating_sub(1),
            Cmd::Glyphs(_, paint, mode) => {
                candidate[i] = groups == 0
                    && clips.iter().all(|&rect| rect)
                    && matches!(mode, DrawMode::Fill(_))
                    && paint.blend == BlendMode::Normal;
            }
            _ => {}
        }
    }
    // backwards, collecting what vello_cpu will draw later on
    let mut later: Vec<Rect> = Vec::new();
    for i in (0..entries.len()).rev() {
        let e = &mut entries[i];
        if !matches!(e.cmd, Cmd::Path(..) | Cmd::Rect(..) | Cmd::Glyphs(..) | Cmd::Image { .. }) {
            continue;
        }
        if candidate[i] && !later.iter().any(|r| overlaps(*r, e.bbox)) {
            e.deferred = true;
        } else {
            later.push(e.bbox);
            if later.len() > 256 {
                // keep the check cheap on busy pages; this only defers less
                let all = later.iter().fold(e.bbox, |a, r| a.union(*r));
                later = vec![all];
            }
        }
    }
}

impl DisplayList {
    /// Whether text is stamped at `scale`: not when glyphs get too big for masks.
    pub(crate) fn defers_at(&self, scale: f64) -> bool {
        self.max_glyph * scale <= MAX_GLYPH_PX
    }

    /// Draws the commands that touch `view` (recording space), moved by `transform`, leaving out
    /// deferred text when `defer` is set.
    pub(super) fn replay(&self, r: &mut Renderer<'_>, transform: Affine, view: Rect, defer: bool) {
        let mut i = 0;
        while i < self.entries.len() {
            let e = &self.entries[i];
            let visible = overlaps(e.bbox, view);
            match &e.cmd {
                Cmd::PushClip(path, fill) => {
                    if !visible && e.end > i {
                        // nothing inside can show
                        i = e.end + 1;
                        continue;
                    }
                    let mut path = path.clone();
                    path.apply_affine(transform);
                    r.push_clip_path_inner(&path, *fill);
                }
                Cmd::PopClip => r.pop_clip(),
                Cmd::PushGroup(opacity, blend) => r.push_transparency_group(*opacity, None, *blend),
                Cmd::PopGroup => r.pop_transparency_group(),
                _ if !visible => {}
                Cmd::Glyphs(..) if defer && e.deferred => {}
                Cmd::Path(path, paint, mode) => r.draw_path(path, paint.props(transform), mode),
                Cmd::Rect(rect, paint, mode) => r.draw_rect(rect, paint.props(transform), mode),
                Cmd::Glyphs(glyphs, paint, mode) => r.draw_outline_glyphs(glyphs, &paint.props(transform), mode),
                Cmd::Image { decoded, key, size, paint } => {
                    let props = ImageDrawProps {
                        transform: transform * paint.transform,
                        soft_mask: None,
                        blend_mode: paint.blend,
                    };
                    r.draw_decoded(decoded, *key, *size, paint.color, &props);
                }
            }
            i += 1;
        }
    }

    /// Stamps the deferred text that touches `view` into the rendered tile, from the glyph cache.
    pub(super) fn stamp_text(&self, target: &mut Target, transform: Affine, view: Rect, glyphs: &mut GlyphCache) {
        let tile = Rect::new(0.0, 0.0, target.width as f64, target.height as f64);
        // the rectangle each open clip is, on the tile (only rectangles hold deferred text)
        let mut clips: Vec<Option<Rect>> = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            let e = &self.entries[i];
            let visible = overlaps(e.bbox, view);
            match &e.cmd {
                Cmd::PushClip(path, _) => {
                    if !visible && e.end > i {
                        i = e.end + 1;
                        continue;
                    }
                    clips.push(axis_rect(path).map(|r| transform.transform_rect_bbox(r)));
                }
                Cmd::PopClip => {
                    clips.pop();
                }
                Cmd::Glyphs(run, paint, _) if e.deferred && visible => {
                    let clip = clips.iter().flatten().fold(tile, |a, r| a.intersect(*r));
                    let placed = transform * paint.transform;
                    for (outline, at, key) in run {
                        glyphs.draw(target, outline, *key, placed * *at, paint.color, clip);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

struct Recorder<'a, 'c> {
    entries: Vec<Entry>,
    /// Indices of `PushClip`s not yet popped.
    open: Vec<usize>,
    /// Cleared on the first thing that can't be replayed; everything after is ignored.
    ok: bool,
    image_bytes: usize,
    max_glyph: f64,
    /// Outlines already shared in this list, so a glyph used many times is stored once.
    outlines: HashMap<u128, Arc<BezPath>>,
    cache: &'c RenderCache<'a>,
    images: &'c ImageCache,
}

impl<'a> Recorder<'a, '_> {
    /// The paint of a draw, or `None` (and the recording given up) if it isn't a plain color.
    fn paint(&mut self, props: &DrawProps<'a>) -> Option<Paint8> {
        match (&props.paint, &props.soft_mask) {
            (Paint::Color(c), None) if self.ok => {
                Some(Paint8 { transform: props.transform, color: c.to_rgba().to_rgba8(), blend: props.blend_mode })
            }
            _ => {
                self.ok = false;
                None
            }
        }
    }

    fn push(&mut self, cmd: Cmd, bbox: Rect) {
        self.entries.push(Entry { cmd, bbox, end: 0, deferred: false });
    }

    /// The glyph's outline and its cache key.
    fn outline(&mut self, glyph: &hayro_interpret::font::OutlineGlyph) -> (Arc<BezPath>, u128) {
        let id = glyph.identifier().cache_key();
        if let Some(path) = self.outlines.get(&id) {
            return (path.clone(), id);
        }
        let cached = self.cache.outline_cache.borrow().get(&id).cloned();
        let path = cached.unwrap_or_else(|| {
            let path = std::rc::Rc::new(glyph.outline());
            self.cache.outline_cache.borrow_mut().insert(id, path.clone());
            path
        });
        let shared = Arc::new(BezPath::clone(&path));
        self.outlines.insert(id, shared.clone());
        (shared, id)
    }

    fn add_image(&mut self, decoded: Arc<Decoded>, key: u128, size: (u32, u32), paint: Paint8) {
        self.image_bytes += decoded.bytes();
        if self.image_bytes > MAX_IMAGE_BYTES {
            self.ok = false;
            return;
        }
        let bbox = paint.transform.transform_rect_bbox(Rect::new(0.0, 0.0, size.0 as f64, size.1 as f64));
        self.push(Cmd::Image { decoded, key, size, paint }, bbox);
    }
}

/// Bounds of a draw in recording space, widened for strokes. Generous on purpose: a draw that's
/// replayed but misses the tile costs a little time, one that's skipped wrongly leaves a gap.
fn draw_bbox(local: Rect, transform: &Affine, mode: &DrawMode) -> Rect {
    let local = match mode {
        DrawMode::Stroke(s) | DrawMode::FillAndStroke(_, s) => {
            let miter = if s.line_join == Join::Miter { s.miter_limit.max(1.0) } else { 1.0 };
            let grow = (s.line_width.max(0.0) * miter) as f64;
            local.inflate(grow, grow)
        }
        _ => local,
    };
    transform.transform_rect_bbox(local)
}

impl<'a> Device<'a> for Recorder<'a, '_> {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, draw_mode: &DrawMode) {
        if matches!(draw_mode, DrawMode::Invisible) {
            return;
        }
        let Some(paint) = self.paint(&props) else { return };
        let bbox = draw_bbox(path.control_box(), &props.transform, draw_mode);
        self.push(Cmd::Path(path.clone(), paint, draw_mode.clone()), bbox);
    }

    fn draw_rect(&mut self, rect: &Rect, props: DrawProps<'a>, draw_mode: &DrawMode) {
        if matches!(draw_mode, DrawMode::Invisible) {
            return;
        }
        let Some(paint) = self.paint(&props) else { return };
        let bbox = draw_bbox(*rect, &props.transform, draw_mode);
        self.push(Cmd::Rect(*rect, paint, draw_mode.clone()), bbox);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        if !self.ok {
            return;
        }
        self.open.push(self.entries.len());
        // clip paths arrive already in recording space
        let bbox = clip_path.path.control_box();
        self.push(Cmd::PushClip(clip_path.path.clone(), clip_path.fill), bbox);
    }

    fn pop_clip(&mut self) {
        if !self.ok {
            return;
        }
        if let Some(i) = self.open.pop() {
            self.entries[i].end = self.entries.len();
        }
        self.push(Cmd::PopClip, Rect::ZERO);
    }

    fn push_transparency_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, blend_mode: BlendMode) {
        if mask.is_some() {
            self.ok = false;
        }
        if self.ok {
            self.push(Cmd::PushGroup(opacity, blend_mode), Rect::ZERO);
        }
    }

    fn pop_transparency_group(&mut self) {
        if self.ok {
            self.push(Cmd::PopGroup, Rect::ZERO);
        }
    }

    fn draw_glyph_run(&mut self, glyph_run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, draw_mode: &DrawMode) {
        if matches!(draw_mode, DrawMode::Invisible) {
            return;
        }
        let Some(paint) = self.paint(&props) else { return };
        let mut glyphs = Vec::with_capacity(glyph_run.glyphs().len());
        let mut bbox: Option<Rect> = None;
        for glyph in glyph_run.glyphs() {
            let Glyph::Outline(outline) = &**glyph else {
                self.ok = false;
                return;
            };
            let (outline, key) = self.outline(outline);
            let b = draw_bbox(outline.control_box(), &(props.transform * glyph.transform()), draw_mode);
            self.max_glyph = self.max_glyph.max(b.width()).max(b.height());
            bbox = Some(bbox.map_or(b, |a| a.union(b)));
            glyphs.push((outline, glyph.transform(), key));
        }
        if let Some(bbox) = bbox {
            self.push(Cmd::Glyphs(glyphs, paint, draw_mode.clone()), bbox);
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        if props.soft_mask.is_some() {
            self.ok = false;
        }
        if !self.ok {
            return;
        }
        let key = image.cache_key();
        let size = (image.width(), image.height());
        // at least a byte per pixel once decoded: give up before decoding a big one
        if self.image_bytes + size.0 as usize * size.1 as usize > MAX_IMAGE_BYTES {
            self.ok = false;
            return;
        }
        let paint = |color| Paint8 { transform: props.transform, color, blend: props.blend_mode };
        match image {
            Image::Raster(r) => {
                let decoded = self.images.decoded(key).or_else(|| {
                    let mut out = None;
                    r.with_rgba(|data, alpha| out = Some(Arc::new(Decoded::Raster { data, alpha })), None);
                    let d = out?;
                    self.images.put_decoded(key, d.clone());
                    Some(d)
                });
                // an image that doesn't decode draws nothing, as in the direct path
                if let Some(d) = decoded {
                    self.add_image(d, key, size, paint([0; 4]));
                }
            }
            Image::Stencil(s) => {
                let mut out = None;
                s.with_stencil(
                    |stencil, paint| {
                        out = Some(match paint {
                            Paint::Color(c) => Some((stencil, c.to_rgba().to_rgba8())),
                            Paint::Pattern(_) => None,
                        })
                    },
                    None,
                );
                match out {
                    Some(Some((stencil, color))) => {
                        self.add_image(Arc::new(Decoded::Stencil(stencil)), key, size, paint(color));
                    }
                    Some(None) => self.ok = false,
                    None => {}
                }
            }
        }
    }
}
