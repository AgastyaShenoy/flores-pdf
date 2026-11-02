//! Annotations made on Android: highlights, underlines and strikeouts of text, freehand lines, notes
//! and rectangles. They're drawn over the pages until saved, when PDFium writes them into the PDF, as
//! the Windows app's are (crates/app/src/annots.rs, which this follows). Coordinates are points from
//! the top-left of the page as it is, like the text's.

use pdfium_write as pdfium;

/// A note's icon, square, in points.
pub const NOTE_SIZE: f32 = 18.0;
/// How close to a line a touch has to be to count as on it, in points.
const NEAR: f32 = 6.0;

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
    /// A freehand line and its width.
    Ink(Vec<[f32; 2]>, f32),
    /// A note's icon, by its top-left corner, and its text.
    Note([f32; 2], String),
    /// Corners and line width.
    Rect([f32; 4], f32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Mark {
    pub page: usize,
    /// 0x00RRGGBB.
    pub color: u32,
    pub shape: Shape,
}

/// A piece of a mark's look, in page points: a filled rectangle, or a line this wide.
pub enum Piece {
    Fill([f32; 4], u32, u32),
    Line([f32; 2], [f32; 2], f32, u32),
}

impl Mark {
    /// Where it is, (x0, y0, x1, y1), with its line width.
    pub fn bounds(&self) -> [f32; 4] {
        let around = |pts: &mut dyn Iterator<Item = [f32; 2]>, pad: f32| {
            let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for p in pts {
                b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
            }
            [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
        };
        match &self.shape {
            Shape::Markup(_, rects) => around(&mut rects.iter().flat_map(|r| [[r[0], r[1]], [r[2], r[3]]]), 0.0),
            Shape::Ink(points, w) => around(&mut points.iter().copied(), w / 2.0),
            Shape::Note(at, _) => [at[0], at[1], at[0] + NOTE_SIZE, at[1] + NOTE_SIZE],
            Shape::Rect(r, w) => around(&mut [[r[0], r[1]], [r[2], r[3]]].into_iter(), w / 2.0),
        }
    }

    /// Whether (x, y) is on it: in a marked line or a note, or on a line drawn.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let inside = |r: &[f32; 4]| r[0] <= x && x <= r[2] && r[1] <= y && y <= r[3];
        match &self.shape {
            Shape::Markup(_, rects) => rects.iter().any(inside),
            Shape::Note(..) => inside(&self.bounds()),
            Shape::Ink(points, w) => {
                points.windows(2).any(|s| distance(s[0], s[1], [x, y]) <= w / 2.0 + NEAR)
                    || points.len() == 1 && distance(points[0], points[0], [x, y]) <= w / 2.0 + NEAR
            }
            Shape::Rect(r, w) => {
                let c = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]], [r[0], r[1]]];
                c.windows(2).any(|s| distance(s[0], s[1], [x, y]) <= w / 2.0 + NEAR)
            }
        }
    }

    /// How it looks, in page points.
    pub fn pieces(&self) -> Vec<Piece> {
        let c = self.color;
        match &self.shape {
            Shape::Markup(kind, rects) => rects
                .iter()
                .map(|r| {
                    let h = r[3] - r[1];
                    // a line under the text, or through its middle, a fourteenth of its height thick
                    let t = (h / 14.0).max(0.6);
                    match kind {
                        Markup::Highlight => Piece::Fill(*r, c, 110),
                        Markup::Underline => Piece::Fill([r[0], r[3] - t * 1.5, r[2], r[3] - t * 0.5], c, 255),
                        Markup::Strikeout => Piece::Fill([r[0], r[1] + h * 0.55 - t / 2.0, r[2], r[1] + h * 0.55 + t / 2.0], c, 255),
                    }
                })
                .collect(),
            Shape::Ink(points, w) => match points.as_slice() {
                [p] => vec![Piece::Line(*p, *p, *w, c)],
                _ => points.windows(2).map(|s| Piece::Line(s[0], s[1], *w, c)).collect(),
            },
            Shape::Note(at, _) => {
                let r = [at[0], at[1], at[0] + NOTE_SIZE, at[1] + NOTE_SIZE];
                let mut v = vec![Piece::Fill(r, c, 255)];
                for i in 0..3 {
                    let y = r[1] + 5.0 + 3.5 * i as f32;
                    v.push(Piece::Line([r[0] + 4.0, y], [r[2] - if i == 2 { 8.0 } else { 4.0 }, y], 1.2, 0x1B1B1B));
                }
                v
            }
            Shape::Rect(r, w) => {
                let k = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]], [r[0], r[1]]];
                k.windows(2).map(|s| Piece::Line(s[0], s[1], *w, c)).collect()
            }
        }
    }

    /// As PDFium adds it, in the page's own coordinates through the page's transform `t` (as in
    /// `Engine::page_transform`).
    pub fn to_pdf(&self, t: [f64; 6]) -> pdfium::NewAnnot {
        let point = |p: [f32; 2]| to_pdf_point(t, p);
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
            Shape::Ink(points, w) => pdfium::Shape::Ink(vec![points.iter().map(|p| point(*p)).collect()], *w),
            Shape::Note(at, text) => pdfium::Shape::Note(rect_of([at[0], at[1], at[0] + NOTE_SIZE, at[1] + NOTE_SIZE]), text.clone()),
            // PDF draws a rectangle's line inside its box: the box goes out by half the line
            Shape::Rect(r, w) => {
                let h = w / 2.0;
                pdfium::Shape::Square(rect_of([r[0].min(r[2]) - h, r[1].min(r[3]) - h, r[0].max(r[2]) + h, r[1].max(r[3]) + h]), *w, false)
            }
        };
        pdfium::NewAnnot { page: self.page, color: self.color, shape }
    }
}

/// A point of the page as shown, in points from its top-left, in the page's own coordinates, through
/// the inverse of its transform `t`.
pub fn to_pdf_point(t: [f64; 6], p: [f32; 2]) -> [f32; 2] {
    let det = t[0] * t[3] - t[1] * t[2];
    if det == 0.0 {
        return p;
    }
    let (x, y) = (f64::from(p[0]) - t[4], f64::from(p[1]) - t[5]);
    [((t[3] * x - t[2] * y) / det) as f32, ((t[0] * y - t[1] * x) / det) as f32]
}

/// How far `p` is from the segment `a`–`b`.
fn distance(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx * dx + dy * dy;
    let k = if len == 0.0 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len).clamp(0.0, 1.0) };
    (a[0] + k * dx - p[0]).hypot(a[1] + k * dy - p[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_hit_near_it_only() {
        let m = Mark { page: 0, color: 0, shape: Shape::Ink(vec![[0.0, 0.0], [100.0, 0.0]], 2.0) };
        assert!(m.contains(50.0, 3.0));
        assert!(!m.contains(50.0, 20.0));
    }

    #[test]
    fn points_go_back_to_the_page_s_own_coordinates() {
        // a letter page: y flips, 792 high
        let t = [1.0, 0.0, 0.0, -1.0, 0.0, 792.0];
        assert_eq!(to_pdf_point(t, [10.0, 2.0]), [10.0, 790.0]);
    }
}
