//! Text extraction: hayro's interpreter run through a device that keeps where each character goes
//! instead of drawing it. Invisible text counts too, so OCR'd scans are searchable.
//!
//! Spaces a PDF only implies, by leaving room between glyphs, are found from the gap between where
//! one glyph's advance ends and the next begins. A Type 3 font's glyphs are drawing procedures whose
//! widths hayro keeps to itself, so for them the gap is measured between the ink each one draws.

use crate::{PageText, TextChar};
use hayro_cmap::BfString;
use hayro_interpret::font::{Glyph, GlyphRun};
use hayro_interpret::hayro_syntax::page::Page;
use hayro_interpret::util::TransformExt;
use hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use kurbo::{Affine, BezPath, Point, Rect, Shape, Vec2};
use std::collections::HashMap;

/// Glyph space is 1000 units per em. A glyph's box runs from the descender to the ascender.
const EM: f64 = 1000.0;
const DESCENT: f64 = -200.0;
const ASCENT: f64 = 800.0;
/// A gap wider than this share of the font size between two glyphs is a space. Justified lines
/// squeeze word spaces to about 0.15 em, while letters sit closer than 0.05.
const SPACE_GAP: f64 = 0.1;
/// The same between the ink of Type 3 glyphs, which leaves out side bearings: in a study guide
/// printed from a browser, letters sat up to 0.175 em apart (H and D, I and P), and the spaces it
/// only implied 0.36 em or more.
const INK_SPACE_GAP: f64 = 0.2;
/// A baseline that moves more than this share of the font size starts a new line.
const LINE_SHIFT: f64 = 0.5;

pub(crate) fn extract(page: &Page<'_>, settings: &InterpreterSettings) -> PageText {
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut state = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, width as f64, height as f64),
        &cache,
        page.xref(),
        settings.clone(),
    );
    let mut device = TextDevice::default();
    interpret_page(page, &mut state, &mut device);
    PageText { chars: device.chars }
}

/// Where the last glyph ended, to tell spaces and line breaks.
#[derive(Clone, Copy)]
struct Pen {
    /// End of the glyph's advance, on its baseline.
    end: Point,
    /// Unit vector along the baseline, and unit vector up.
    along: Vec2,
    up: Vec2,
    /// Font size in page units.
    size: f64,
    /// The share of the size a gap must pass to be a space: wider for gaps measured between ink.
    space: f64,
}

/// The letters of a typographic ligature, so "ﬁnd" searches and copies as "find".
fn unligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => return None,
    })
}

#[derive(Default)]
struct TextDevice {
    chars: Vec<TextChar>,
    pen: Option<Pen>,
    /// First glyph's position and count of the last filled run: text that's filled and stroked
    /// arrives twice, and the stroke is dropped.
    last_fill: Option<(Point, usize)>,
    /// The ink of Type 3 glyphs drawn so far, in their own space, by glyph; None for one that draws
    /// nothing, such as a space.
    ink: HashMap<u128, Option<Rect>>,
}

impl TextDevice {
    fn push(&mut self, ch: char, rect: Rect) {
        self.chars.push(TextChar { ch, rect: [rect.x0 as f32, rect.y0 as f32, rect.x1 as f32, rect.y1 as f32] });
    }

    fn ends_in_space(&self) -> bool {
        self.chars.last().is_none_or(|c| c.ch.is_whitespace())
    }

    /// Adds a space or a line break if the glyph at `origin`, whose ink starts at `start`, doesn't
    /// follow on from the last one.
    fn separate(&mut self, origin: Point, start: Point, glyph_box: Rect) {
        let Some(pen) = self.pen else { return };
        let delta = origin - pen.end;
        let along = delta.dot(pen.along);
        let across = delta.dot(pen.up);
        let new_line = across.abs() > LINE_SHIFT * pen.size || along < -pen.size;
        if new_line {
            if self.chars.last().is_some_and(|c| c.ch != '\n') {
                let end = Rect::from_points(pen.end, pen.end);
                self.push('\n', end);
            }
        } else if (start - pen.end).dot(pen.along) > pen.space * pen.size && !self.ends_in_space() {
            let gap = Rect::new(
                pen.end.x.min(glyph_box.x0),
                glyph_box.y0,
                pen.end.x.max(glyph_box.x0),
                glyph_box.y1,
            );
            self.push(' ', gap);
        }
    }
}

impl<'a> Device<'a> for TextDevice {
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, draw_mode: &DrawMode) {
        let glyphs = run.glyphs();
        let Some(first) = glyphs.first() else { return };
        let start = (props.transform * first.transform()) * Point::ORIGIN;
        match draw_mode {
            DrawMode::Stroke(_) if self.last_fill == Some((start, glyphs.len())) => return,
            DrawMode::Stroke(_) => {}
            _ => self.last_fill = Some((start, glyphs.len())),
        }
        for glyph in glyphs {
            let t: Affine = props.transform * glyph.transform();
            let origin = t * Point::ORIGIN;
            let up = t * Point::new(0.0, EM) - origin;
            let size = up.hypot();
            if size <= 0.0 {
                continue;
            }
            let (glyph_box, along, start, end, space) = match &**glyph {
                Glyph::Outline(o) => {
                    let advance = o.advance_width().map_or(EM / 2.0, f64::from);
                    let end = t * Point::new(advance, 0.0);
                    let along = if (end - origin).hypot() > 0.0 { (end - origin).normalize() } else { Vec2::new(1.0, 0.0) };
                    let glyph_box = t.transform_rect_bbox(Rect::new(0.0, DESCENT, advance.max(1.0), ASCENT));
                    (glyph_box, along, origin, end, SPACE_GAP)
                }
                Glyph::Type3(g) => {
                    // the glyph's ink, drawn once in its own space and kept
                    let ink = *self.ink.entry(g.cache_key()).or_insert_with(|| {
                        let mut d = InkDevice::default();
                        g.interpret(&mut d, Affine::IDENTITY, Affine::IDENTITY, &props.paint);
                        d.0
                    });
                    let x = t * Point::new(EM, 0.0) - origin;
                    let along = if x.hypot() > 0.0 { x.normalize() } else { Vec2::new(1.0, 0.0) };
                    let em_box = t.transform_rect_bbox(Rect::new(0.0, DESCENT, EM / 2.0, ASCENT));
                    match ink.map(|r| t.transform_rect_bbox(r)) {
                        Some(r) => {
                            // the ink's extent along the baseline, from its corners
                            let corners = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x0, r.y1), Point::new(r.x1, r.y1)];
                            let (lo, hi) = corners.iter().map(|c| (*c - origin).dot(along)).fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
                            let glyph_box = Rect::new(r.x0, em_box.y0, r.x1, em_box.y1);
                            (glyph_box, along, origin + along * lo, origin + along * hi, INK_SPACE_GAP)
                        }
                        // nothing drawn, a space say: an empty box where it is
                        None => (Rect::new(origin.x, em_box.y0, origin.x, em_box.y1), along, origin, origin, INK_SPACE_GAP),
                    }
                }
            };
            self.separate(origin, start, glyph_box);
            self.pen = Some(Pen { end, along, up: up / size, size, space });

            let text = match glyph.as_unicode() {
                Some(BfString::Char(c)) => unligature(c).map_or_else(|| c.to_string(), str::to_owned),
                Some(BfString::String(s)) => s,
                None => continue,
            };
            // a ligature such as "fi" shares its box between its letters
            let n = text.chars().count().max(1) as f64;
            for (i, c) in text.chars().enumerate() {
                if c.is_control() {
                    continue;
                }
                let x0 = glyph_box.x0 + glyph_box.width() * i as f64 / n;
                let x1 = glyph_box.x0 + glyph_box.width() * (i + 1) as f64 / n;
                self.push(c, Rect::new(x0, glyph_box.y0, x1, glyph_box.y1));
            }
        }
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Collects the box around what a Type 3 glyph draws: its paths and images.
#[derive(Default)]
struct InkDevice(Option<Rect>);

impl InkDevice {
    fn add(&mut self, r: Rect) {
        self.0 = Some(self.0.map_or(r, |o| o.union(r)));
    }
}

impl<'a> Device<'a> for InkDevice {
    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {}
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, _: &DrawMode) {
        self.add(props.transform.transform_rect_bbox(path.bounding_box()));
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, props: ImageDrawProps<'a>) {
        self.add(props.transform.transform_rect_bbox(Rect::new(0.0, 0.0, 1.0, 1.0)));
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}
