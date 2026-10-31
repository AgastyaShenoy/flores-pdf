//! Annotations made in the reader: highlights, underlines, strikeouts, notes, freehand lines, text
//! boxes, rectangles, ellipses, lines and arrows. They're drawn over the pages until Ctrl+S writes
//! them into the PDF through PDFium; from then on they're part of the file, drawn by hayro like any
//! other annotation. Coordinates are points from the top-left of the page as it is, like the text's.

use crate::fonts::Font;
use crate::pdfium;
use kurbo::{Affine, BezPath, Ellipse, ParamCurveNearest, PathEl, Point, Rect, Shape as _};
use std::fmt::Write as _;

/// A note's icon, square, in points, and its color: PDFium's, which draws the icon in the file.
pub const NOTE_SIZE: f32 = 18.0;
pub const NOTE_COLOR: u32 = 0xFFFF00;
/// Lines of a text box, as a multiple of its text size.
const LINE: f32 = 1.25;
/// Room around a text box's text, in points.
const TEXT_PAD: f32 = 2.0;
/// How close to a line a click has to be to count as on it, in points.
const NEAR: f64 = 4.0;

/// What a press on the page does while annotating.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Highlight,
    Underline,
    Strikeout,
    Note,
    Pen,
    Text,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 11] = [
        Tool::Highlight,
        Tool::Underline,
        Tool::Strikeout,
        Tool::Note,
        Tool::Pen,
        Tool::Text,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Eraser,
    ];

    /// It marks text: dragging selects text, and letting go marks it.
    pub fn marks_text(self) -> bool {
        matches!(self, Tool::Highlight | Tool::Underline | Tool::Strikeout)
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Highlight => "Highlight text",
            Tool::Underline => "Underline text",
            Tool::Strikeout => "Strike out text",
            Tool::Note => "Add a note",
            Tool::Pen => "Draw",
            Tool::Text => "Add text",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Eraser => "Erase annotations",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Markup {
    Highlight,
    Underline,
    Strikeout,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Shape {
    /// Lines of text, as rectangles (x0, y0, x1, y1).
    Markup(Markup, Vec<[f32; 4]>),
    /// A note's icon, by its top-left corner, and its text.
    Note([f32; 2], String),
    /// Freehand lines and their width.
    Ink(Vec<Vec<[f32; 2]>>, f32),
    /// Corners and line width.
    Rect([f32; 4], f32),
    Ellipse([f32; 4], f32),
    /// From, to, width, and whether it ends in an arrowhead.
    Line([f32; 2], [f32; 2], f32, bool),
    /// Top-left corner, text (lines split at \n) and text size.
    Text([f32; 2], String, f32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Mark {
    pub page: usize,
    /// 0x00RRGGBB.
    pub color: u32,
    pub shape: Shape,
    /// A highlight from highlights.txt, where highlights were kept before they went into the PDF.
    pub legacy: bool,
}

/// One piece of a mark's look: a path in page points, filled, or stroked this wide.
pub struct Piece {
    pub path: BezPath,
    pub color: u32,
    /// Out of 255.
    pub alpha: u8,
    /// 0 to fill.
    pub stroke: f64,
}

impl Mark {
    /// Where it is, (x0, y0, x1, y1), with its line width.
    pub fn bounds(&self, font: &mut Font) -> [f32; 4] {
        let around = |pts: &mut dyn Iterator<Item = [f32; 2]>, pad: f32| {
            let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for p in pts {
                b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
            }
            [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
        };
        match &self.shape {
            Shape::Markup(_, rects) => around(&mut rects.iter().flat_map(|r| [[r[0], r[1]], [r[2], r[3]]]), 0.0),
            Shape::Note(at, _) => [at[0], at[1], at[0] + NOTE_SIZE, at[1] + NOTE_SIZE],
            Shape::Ink(strokes, w) => around(&mut strokes.iter().flatten().copied(), w / 2.0),
            Shape::Rect(r, w) | Shape::Ellipse(r, w) => around(&mut [[r[0], r[1]], [r[2], r[3]]].into_iter(), w / 2.0),
            Shape::Line(a, b, w, arrow) => around(&mut [*a, *b].into_iter(), if *arrow { w * 4.0 } else { w / 2.0 }),
            Shape::Text(at, text, size) => {
                let (w, h) = text_size(font, text, *size);
                [at[0], at[1], at[0] + w + 2.0 * TEXT_PAD, at[1] + h + 2.0 * TEXT_PAD]
            }
        }
    }

    /// Whether (x, y) is on it: in a marked line or a note, on a line drawn, inside a text box.
    pub fn contains(&self, font: &mut Font, x: f32, y: f32) -> bool {
        let p = Point::new(f64::from(x), f64::from(y));
        let near = |path: &BezPath, w: f32| path.segments().any(|s| s.nearest(p, 0.1).distance_sq.sqrt() <= f64::from(w) / 2.0 + NEAR);
        match &self.shape {
            Shape::Markup(_, rects) => rects.iter().any(|r| r[0] <= x && x <= r[2] && r[1] <= y && y <= r[3]),
            Shape::Note(..) | Shape::Text(..) => {
                let b = self.bounds(font);
                b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3]
            }
            Shape::Ink(strokes, w) => strokes.iter().any(|s| near(&polyline(s), *w)),
            Shape::Rect(r, w) => near(&rect(*r).to_path(0.1), *w),
            Shape::Ellipse(r, w) => near(&Ellipse::from_rect(rect(*r)).to_path(0.1), *w),
            Shape::Line(.., w, _) => self.pieces(font).iter().any(|piece| near(&piece.path, *w)),
        }
    }

    /// How it looks, in page points.
    pub fn pieces(&self, font: &mut Font) -> Vec<Piece> {
        let piece = |path: BezPath, alpha: u8, stroke: f32| Piece { path, color: self.color, alpha, stroke: f64::from(stroke) };
        match &self.shape {
            Shape::Markup(kind, rects) => rects
                .iter()
                .map(|r| {
                    let h = r[3] - r[1];
                    // a line under the text, or through its middle, a fourteenth of its height thick
                    let t = (h / 14.0).max(0.6);
                    let r = match kind {
                        Markup::Highlight => *r,
                        Markup::Underline => [r[0], r[3] - t * 1.5, r[2], r[3] - t * 0.5],
                        Markup::Strikeout => [r[0], r[1] + h * 0.55 - t / 2.0, r[2], r[1] + h * 0.55 + t / 2.0],
                    };
                    piece(rect(r).to_path(0.1), if *kind == Markup::Highlight { 110 } else { 255 }, 0.0)
                })
                .collect(),
            Shape::Note(at, _) => {
                let r = rect([at[0], at[1], at[0] + NOTE_SIZE, at[1] + NOTE_SIZE]);
                let mut lines = BezPath::new();
                for i in 0..3 {
                    let y = r.y0 + 5.0 + 3.5 * f64::from(i);
                    lines.move_to((r.x0 + 4.0, y));
                    lines.line_to((r.x1 - if i == 2 { 8.0 } else { 4.0 }, y));
                }
                vec![
                    piece(kurbo::RoundedRect::from_rect(r, 2.5).to_path(0.1), 255, 0.0),
                    Piece { path: lines, color: 0x1B1B1B, alpha: 255, stroke: 1.2 },
                    Piece { path: kurbo::RoundedRect::from_rect(r, 2.5).to_path(0.1), color: 0x1B1B1B, alpha: 160, stroke: 0.8 },
                ]
            }
            Shape::Ink(strokes, w) => strokes.iter().map(|s| piece(polyline(s), 255, *w)).collect(),
            Shape::Rect(r, w) => vec![piece(rect(*r).to_path(0.1), 255, *w)],
            Shape::Ellipse(r, w) => vec![piece(Ellipse::from_rect(rect(*r)).to_path(0.1), 255, *w)],
            Shape::Line(a, b, w, arrow) => {
                let mut v = vec![piece(polyline(&[*a, *b]), 255, *w)];
                if *arrow && let Some(head) = arrowhead(*a, *b, *w) {
                    v.push(piece(polyline(&head), 255, *w));
                }
                v
            }
            Shape::Text(at, text, size) => vec![piece(text_path(font, *at, text, *size), 255, 0.0)],
        }
    }

    /// As PDFium adds it, in the page's own coordinates through the page's transform `t` (as in
    /// `Engine::page_transform`).
    pub fn to_pdf(&self, t: [f64; 6], font: &mut Font) -> pdfium::NewAnnot {
        let to = inverse(t);
        let point = |p: [f32; 2]| {
            let q = to * Point::new(f64::from(p[0]), f64::from(p[1]));
            [q.x as f32, q.y as f32]
        };
        let rect_of = |r: [f32; 4]| {
            let (a, b) = (point([r[0], r[1]]), point([r[2], r[3]]));
            [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
        };
        let shape = match &self.shape {
            Shape::Markup(kind, rects) => {
                let kind = match kind {
                    Markup::Highlight => pdfium::ANNOT_HIGHLIGHT,
                    Markup::Underline => pdfium::ANNOT_UNDERLINE,
                    Markup::Strikeout => pdfium::ANNOT_STRIKEOUT,
                };
                pdfium::Shape::Markup(kind, rects.iter().map(|r| rect_of(*r)).collect())
            }
            Shape::Note(at, text) => pdfium::Shape::Note(rect_of([at[0], at[1], at[0] + NOTE_SIZE, at[1] + NOTE_SIZE]), text.clone()),
            Shape::Ink(strokes, w) => pdfium::Shape::Ink(strokes.iter().map(|s| s.iter().map(|p| point(*p)).collect()).collect(), *w),
            // PDF draws a rectangle's or ellipse's line inside its box: the box goes out by half the line
            Shape::Rect(r, w) | Shape::Ellipse(r, w) => {
                let h = w / 2.0;
                let ellipse = matches!(self.shape, Shape::Ellipse(..));
                pdfium::Shape::Square(rect_of([r[0].min(r[2]) - h, r[1].min(r[3]) - h, r[0].max(r[2]) + h, r[1].max(r[3]) + h]), *w, ellipse)
            }
            Shape::Line(a, b, w, arrow) => {
                let mut strokes = vec![vec![point(*a), point(*b)]];
                if *arrow && let Some(head) = arrowhead(*a, *b, *w) {
                    strokes.push(head.iter().map(|p| point(*p)).collect());
                }
                pdfium::Shape::Ink(strokes, *w)
            }
            Shape::Text(at, text, size) => {
                let bounds = self.bounds(font);
                let path = to * text_path(font, *at, text, *size);
                let (r, g, b) = ((self.color >> 16 & 0xFF) as f32, (self.color >> 8 & 0xFF) as f32, (self.color & 0xFF) as f32);
                let mut stream = format!("{:.3} {:.3} {:.3} rg\n", r / 255.0, g / 255.0, b / 255.0);
                write_path(&mut stream, &path);
                stream.push_str("f\n");
                pdfium::Shape::FreeText(rect_of(bounds), text.clone(), stream)
            }
        };
        pdfium::NewAnnot { page: self.page, color: self.color, shape }
    }
}

/// The mark a drag makes with `tool`, from `a` to `b` (and through `points`, for the pen).
pub fn dragged(tool: Tool, page: usize, color: u32, width: f32, a: [f32; 2], b: [f32; 2], points: &[[f32; 2]]) -> Option<Mark> {
    let r = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
    let shape = match tool {
        Tool::Pen => Shape::Ink(vec![points.to_vec()], width),
        Tool::Rect => Shape::Rect(r, width),
        Tool::Ellipse => Shape::Ellipse(r, width),
        Tool::Line => Shape::Line(a, b, width, false),
        Tool::Arrow => Shape::Line(a, b, width, true),
        _ => return None,
    };
    Some(Mark { page, color, shape, legacy: false })
}

/// Whether a mark is big enough to keep: a click without a drag makes nothing.
pub fn worth_keeping(mark: &Mark) -> bool {
    let far = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) >= 2.0;
    match &mark.shape {
        Shape::Ink(strokes, _) => strokes.iter().any(|s| s.len() > 1 && far(s[0], *s.last().unwrap()) || s.len() > 4),
        Shape::Rect(r, _) | Shape::Ellipse(r, _) => r[2] - r[0] >= 2.0 && r[3] - r[1] >= 2.0,
        Shape::Line(a, b, ..) => far(*a, *b),
        _ => true,
    }
}

/// The affine map from page points to wherever (x, y) of the page goes, given where its origin and
/// unit points go: how page points are turned and scaled onto the screen.
pub fn affine(origin: (f64, f64), x_unit: (f64, f64), y_unit: (f64, f64)) -> Affine {
    Affine::new([x_unit.0 - origin.0, x_unit.1 - origin.1, y_unit.0 - origin.0, y_unit.1 - origin.1, origin.0, origin.1])
}

fn rect(r: [f32; 4]) -> Rect {
    Rect::new(f64::from(r[0]), f64::from(r[1]), f64::from(r[2]), f64::from(r[3])).abs()
}

fn polyline(points: &[[f32; 2]]) -> BezPath {
    let mut path = BezPath::new();
    for (i, p) in points.iter().enumerate() {
        let p = (f64::from(p[0]), f64::from(p[1]));
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    if points.len() == 1 {
        // a dot
        path.line_to((f64::from(points[0][0]) + 0.01, f64::from(points[0][1])));
    }
    path
}

/// The two sides of an arrowhead at `b`, pointing away from `a`, as one line through its tip.
fn arrowhead(a: [f32; 2], b: [f32; 2], width: f32) -> Option<[[f32; 2]; 3]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if len < 0.5 {
        return None;
    }
    let size = (width * 4.0).max(8.0).min(len);
    let (ux, uy) = (dx / len, dy / len);
    // 30 degrees either side of the line, back from the tip
    let (c, s) = (0.866, 0.5);
    let side = |sign: f32| [b[0] - size * (ux * c - sign * uy * s), b[1] - size * (uy * c + sign * ux * s)];
    Some([side(1.0), b, side(-1.0)])
}

fn inverse(t: [f64; 6]) -> Affine {
    Affine::new(t).inverse()
}

/// Width and height of a text box's text.
fn text_size(font: &mut Font, text: &str, size: f32) -> (f32, f32) {
    let lines: Vec<&str> = text.split('\n').collect();
    let width = lines.iter().map(|l| font.outlines(l, size).1).fold(0.0, f32::max);
    (width.max(size), lines.len() as f32 * size * LINE)
}

/// A text box's text as one path, its first line's top at `at` plus the padding.
fn text_path(font: &mut Font, at: [f32; 2], text: &str, size: f32) -> BezPath {
    let (ascent, _) = font.line(size);
    let mut out = BezPath::new();
    for (i, line) in text.split('\n').enumerate() {
        let (path, _) = font.outlines(line, size);
        let base = f64::from(at[1] + TEXT_PAD + ascent + i as f32 * size * LINE);
        out.extend(Affine::translate((f64::from(at[0] + TEXT_PAD), base)) * path);
    }
    out
}

/// A path as content stream operators.
fn write_path(out: &mut String, path: &BezPath) {
    let mut last = Point::ZERO;
    for el in path.elements() {
        let _ = match *el {
            PathEl::MoveTo(p) => writeln!(out, "{:.2} {:.2} m", p.x, p.y),
            PathEl::LineTo(p) => writeln!(out, "{:.2} {:.2} l", p.x, p.y),
            PathEl::QuadTo(c, p) => {
                // as the cubic curve it is
                let (c1, c2) = (last + (c - last) * (2.0 / 3.0), p + (c - p) * (2.0 / 3.0));
                writeln!(out, "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c", c1.x, c1.y, c2.x, c2.y, p.x, p.y)
            }
            PathEl::CurveTo(a, b, p) => writeln!(out, "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c", a.x, a.y, b.x, b.y, p.x, p.y),
            PathEl::ClosePath => writeln!(out, "h"),
        };
        last = match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => p,
            PathEl::ClosePath => last,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrowhead_points_back_from_the_tip() {
        let [l, tip, r] = arrowhead([0.0, 0.0], [100.0, 0.0], 2.0).unwrap();
        assert_eq!(tip, [100.0, 0.0]);
        assert!(l[0] < 100.0 && r[0] < 100.0);
        assert!((l[1] + r[1]).abs() < 1e-4 && l[1] != 0.0);
    }

    #[test]
    fn pdf_coordinates_flip_y() {
        // a 600 pt tall page, unrotated: page points y down, PDF y up
        let t = [1.0, 0.0, 0.0, -1.0, 0.0, 600.0];
        let mark = Mark { page: 0, color: 0xFF0000, shape: Shape::Rect([10.0, 20.0, 110.0, 70.0], 2.0), legacy: false };
        let Some(mut font) = Font::load(&["segoeui.ttf", "arial.ttf"]) else { return };
        let annot = mark.to_pdf(t, &mut font);
        let pdfium::Shape::Square(r, w, ellipse) = annot.shape else { panic!("a square") };
        assert_eq!((w, ellipse), (2.0, false));
        assert_eq!(r, [9.0, 529.0, 111.0, 581.0]);
    }

    #[test]
    fn a_click_without_a_drag_makes_nothing() {
        let still = dragged(Tool::Rect, 0, 0, 1.0, [5.0, 5.0], [5.5, 5.0], &[]).unwrap();
        assert!(!worth_keeping(&still));
        let line = dragged(Tool::Arrow, 0, 0, 1.0, [5.0, 5.0], [50.0, 5.0], &[]).unwrap();
        assert!(worth_keeping(&line));
    }
}
