use engine::{PageSize, PixelRect};
use std::ops::Range;

/// Tiles are squares of this many pixels per zoom level, like a map viewer. Edge tiles are cut to the page.
pub const TILE: u32 = 512;

/// Where a page sits in the document, in pixels at the layout's scale, turned as the layout is.
#[derive(Clone, Copy, Debug)]
pub struct PagePos {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
    /// Height of the row the page is in: pages side by side share a row, top-aligned.
    pub row_height: u32,
}

/// A rectangle of the document in document pixels, usually the part shown in the window.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    fn bottom(&self) -> i64 {
        self.y + i64::from(self.height)
    }
}

/// How pages sit in their rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spread {
    /// One page per row.
    #[default]
    One,
    /// Two pages side by side: 1 and 2, 3 and 4.
    Two,
    /// Like a book: the first page alone on the right, then 2 and 3, 4 and 5.
    Book,
}

/// Pages in rows at one scale, centered, turned by `rotation` quarter turns clockwise.
pub struct Layout {
    /// Pixels per point.
    pub scale: f32,
    /// Space around and between pages, in pixels.
    pub gap: u32,
    /// Width of the window the pages are centered in.
    pub view_width: u32,
    pub width: i64,
    pub height: i64,
    pub pages: Vec<PagePos>,
    /// Quarter turns clockwise, 0 to 3. Tiles are rendered turned.
    pub rotation: u8,
    pub spread: Spread,
}

impl Layout {
    /// One page per row, not turned.
    pub fn new(sizes: &[PageSize], scale: f32, gap: u32, view_width: u32) -> Self {
        Self::arranged(sizes, scale, gap, view_width, 0, Spread::One)
    }

    /// `sizes` are the pages as they are, before turning.
    pub fn arranged(sizes: &[PageSize], scale: f32, gap: u32, view_width: u32, rotation: u8, spread: Spread) -> Self {
        let px = |pt: f32| ((pt * scale).round() as u32).max(1);
        let turned = |s: &PageSize| if rotation % 2 == 1 { (px(s.height), px(s.width)) } else { (px(s.width), px(s.height)) };
        let dims: Vec<(u32, u32)> = sizes.iter().map(turned).collect();
        let widest = dims.iter().map(|d| d.0).max().unwrap_or(0);
        // rows of page indexes
        let rows: Vec<Vec<usize>> = match spread {
            Spread::One => (0..dims.len()).map(|i| vec![i]).collect(),
            Spread::Two => (0..dims.len()).collect::<Vec<_>>().chunks(2).map(<[usize]>::to_vec).collect(),
            Spread::Book => {
                let mut rows = vec![vec![0]];
                rows.extend((1..dims.len()).collect::<Vec<_>>().chunks(2).map(<[usize]>::to_vec));
                rows.retain(|r| r.iter().all(|&i| i < dims.len()));
                rows
            }
        };
        let width = match spread {
            Spread::One => i64::from((widest + 2 * gap).max(view_width)),
            _ => i64::from((2 * widest + 3 * gap).max(view_width)),
        };
        // a spread's pages meet at the middle, with a gap between them
        let (left_edge, right_edge) = (width / 2 - i64::from(gap / 2), width / 2 + i64::from(gap - gap / 2));
        let mut pages = vec![PagePos { x: 0, y: 0, width: 1, height: 1, row_height: 1 }; dims.len()];
        let mut y = i64::from(gap);
        for row in &rows {
            let row_height = row.iter().map(|&i| dims[i].1).max().unwrap_or(1);
            for (k, &i) in row.iter().enumerate() {
                let (w, h) = dims[i];
                let x = match (spread, row.len(), k) {
                    (Spread::One, _, _) => (width - i64::from(w)) / 2,
                    // the cover sits on the right, as in a book
                    (Spread::Book, 1, _) if i == 0 => right_edge,
                    (_, _, 0) => left_edge - i64::from(w),
                    _ => right_edge,
                };
                pages[i] = PagePos { x, y, width: w, height: h, row_height };
            }
            y += i64::from(row_height + gap);
        }
        Self { scale, gap, view_width, width, height: y, pages, rotation, spread }
    }

    /// Pages that overlap the rows `top..bottom`.
    pub fn pages_between(&self, top: i64, bottom: i64) -> Range<usize> {
        let first = self.pages.partition_point(|p| p.y + i64::from(p.row_height) <= top);
        let end = self.pages.partition_point(|p| p.y < bottom);
        first..end.max(first)
    }

    /// The page at row `y`, or the next one when `y` falls between pages; the first of its row
    /// when pages are side by side.
    pub fn page_at(&self, y: i64) -> usize {
        let i = self.pages.partition_point(|p| p.y + i64::from(p.row_height) <= y);
        i.min(self.pages.len().saturating_sub(1))
    }

    /// The page at (x, y), or the nearest one in that row.
    pub fn page_at_point(&self, x: i64, y: i64) -> usize {
        let first = self.page_at(y);
        let Some(row_y) = self.pages.get(first).map(|p| p.y) else { return first };
        let row = self.pages[first..].iter().take_while(|p| p.y == row_y);
        let distance = |p: &PagePos| (p.x - x).max(x - (p.x + i64::from(p.width))).max(0);
        row.enumerate().min_by_key(|(_, p)| distance(p)).map_or(first, |(k, _)| first + k)
    }

    /// The page that takes up most of `view`.
    pub fn current_page(&self, view: &Viewport) -> usize {
        self.pages_between(view.y, view.bottom())
            .min_by_key(|&i| {
                let p = self.pages[i];
                -((p.y + i64::from(p.height)).min(view.bottom()) - p.y.max(view.y))
            })
            .unwrap_or_else(|| self.page_at(view.y))
    }

    /// The tiles of `page` that overlap `rect`: column, row, and the tile's area in page pixels.
    pub fn tiles(&self, page: usize, rect: &Viewport) -> impl Iterator<Item = (u32, u32, PixelRect)> + use<> {
        let p = self.pages[page];
        let (w, h) = (i64::from(p.width), i64::from(p.height));
        let (x0, x1) = ((rect.x - p.x).clamp(0, w), (rect.x + i64::from(rect.width) - p.x).clamp(0, w));
        let (y0, y1) = ((rect.y - p.y).clamp(0, h), (rect.bottom() - p.y).clamp(0, h));
        let t = i64::from(TILE);
        let span = |a: i64, b: i64| if a < b { (a / t) as u32..((b - 1) / t + 1) as u32 } else { 0..0 };
        let (cols, rows) = (span(x0, x1), span(y0, y1));
        rows.flat_map(move |row| {
            cols.clone().map(move |col| {
                let (x, y) = (col * TILE, row * TILE);
                (col, row, PixelRect { x, y, width: TILE.min(p.width - x), height: TILE.min(p.height - y) })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: PageSize = PageSize { width: 100.0, height: 140.0 };

    #[test]
    fn spreads_meet_in_the_middle() {
        let two = Layout::arranged(&[A4; 3], 1.0, 10, 0, 0, Spread::Two);
        let p = &two.pages;
        assert_eq!(two.width, 2 * 100 + 3 * 10);
        // 1 and 2 side by side, the gap between them centered; 3 alone on the left
        assert_eq!((p[0].x, p[0].y, p[1].x, p[1].y), (10, 10, 120, 10));
        assert_eq!((p[2].x, p[2].y), (10, 160));
        assert_eq!((two.page_at_point(150, 50), two.page_at_point(50, 50), two.page_at_point(500, 170)), (1, 0, 2));
        // in a book the cover is alone on the right
        let book = Layout::arranged(&[A4; 3], 1.0, 10, 0, 0, Spread::Book);
        assert_eq!((book.pages[0].x, book.pages[1].x, book.pages[2].x, book.pages[2].y), (120, 10, 120, 160));
        assert_eq!(book.pages_between(0, 20), 0..1);
    }

    #[test]
    fn a_quarter_turn_swaps_width_and_height() {
        let turned = Layout::arranged(&[A4], 1.0, 0, 0, 1, Spread::One);
        assert_eq!((turned.pages[0].width, turned.pages[0].height), (140, 100));
    }
}
