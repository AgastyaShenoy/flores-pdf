//! Text extraction: hayro's interpreter run through a device that keeps where each character goes
//! instead of drawing it. Invisible text counts too, so OCR'd scans are searchable.

use crate::{PageText, TextChar};
use hayro_cmap::BfString;
use hayro_interpret::font::{Glyph, GlyphRun};
use hayro_interpret::hayro_syntax::page::Page;
use hayro_interpret::util::TransformExt;
use hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, interpret_page,
};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};

/// Glyph space is 1000 units per em. A glyph's box runs from the descender to the ascender.
const EM: f64 = 1000.0;
const DESCENT: f64 = -200.0;
const ASCENT: f64 = 800.0;
/// A gap wider than this share of the font size between two glyphs is a space. Justified lines
/// squeeze word spaces to about 0.15 em, while letters sit closer than 0.05.
const SPACE_GAP: f64 = 0.1;
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
}

impl TextDevice {
    fn push(&mut self, ch: char, rect: Rect) {
        self.chars.push(TextChar { ch, rect: [rect.x0 as f32, rect.y0 as f32, rect.x1 as f32, rect.y1 as f32] });
    }

    fn ends_in_space(&self) -> bool {
        self.chars.last().is_none_or(|c| c.ch.is_whitespace())
    }

    /// Adds a space or a line break if the glyph at `origin` doesn't follow on from the last one.
    fn separate(&mut self, origin: Point, glyph_box: Rect) {
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
        } else if along > SPACE_GAP * pen.size && !self.ends_in_space() {
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
            let advance = match &**glyph {
                Glyph::Outline(o) => o.advance_width().map_or(EM / 2.0, f64::from),
                Glyph::Type3(_) => EM / 2.0,
            };
            let origin = t * Point::ORIGIN;
            let end = t * Point::new(advance, 0.0);
            let up = t * Point::new(0.0, EM) - origin;
            let size = up.hypot();
            if size <= 0.0 {
                continue;
            }
            let along = if (end - origin).hypot() > 0.0 { (end - origin).normalize() } else { Vec2::new(1.0, 0.0) };
            let glyph_box = t.transform_rect_bbox(Rect::new(0.0, DESCENT, advance.max(1.0), ASCENT));
            self.separate(origin, glyph_box);
            self.pen = Some(Pen { end, along, up: up / size, size });

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
