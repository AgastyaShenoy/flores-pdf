//! Highlights of documents that aren't PDFs (e-books, DjVu, comics), kept in `highlights.txt` beside
//! the settings file. PDFs' highlights go into the PDF; ones made before that are kept here until
//! the PDF is next saved. One highlight per
//! line: page (from 0), color, its rectangles in points of the page as it is, and the file,
//! separated by tabs; rectangles are `x0,y0,x1,y1` joined by `;`.

use crate::history::same_file;
use crate::settings;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Debug)]
pub struct Highlight {
    pub path: PathBuf,
    pub page: usize,
    /// 0x00RRGGBB.
    pub color: u32,
    pub rects: Vec<[f32; 4]>,
}

impl Highlight {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rects.iter().any(|r| r[0] <= x && x <= r[2] && r[1] <= y && y <= r[3])
    }
}

#[derive(Default)]
pub struct Highlights {
    list: Vec<Highlight>,
}

impl Highlights {
    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(settings::highlights_path()).unwrap_or_default())
    }

    fn parse(text: &str) -> Self {
        let list = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|line| {
                let mut f = line.splitn(4, '\t');
                let (page, color, rects, path) = (f.next()?.parse().ok()?, f.next()?, f.next()?, f.next()?);
                let color = u32::from_str_radix(color.trim_start_matches('#'), 16).ok()?;
                let rects: Vec<[f32; 4]> = rects
                    .split(';')
                    .filter_map(|r| {
                        let v: Vec<f32> = r.split(',').filter_map(|n| n.parse().ok()).collect();
                        <[f32; 4]>::try_from(v).ok()
                    })
                    .collect();
                (!rects.is_empty() && !path.is_empty()).then(|| Highlight { path: PathBuf::from(path), page, color, rects })
            })
            .collect();
        Self { list }
    }

    fn format(&self) -> String {
        let mut out = String::from("# Flores PDF's highlights: page (from 0), color, rectangles in points, file.\n");
        for h in &self.list {
            let Some(path) = h.path.to_str() else { continue };
            let rects: Vec<String> = h.rects.iter().map(|r| format!("{:.2},{:.2},{:.2},{:.2}", r[0], r[1], r[2], r[3])).collect();
            let _ = writeln!(out, "{}\t#{:06X}\t{}\t{path}", h.page, h.color, rects.join(";"));
        }
        out
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = settings::highlights_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.format())
    }

    /// The highlights on `page` of the file at `path`.
    pub fn on(&self, path: &Path, page: usize) -> impl Iterator<Item = &Highlight> {
        self.list.iter().filter(move |h| h.page == page && same_file(&h.path, path))
    }

    /// Every highlight in the file at `path`.
    pub fn of(&self, path: &Path) -> Vec<&Highlight> {
        self.list.iter().filter(|h| same_file(&h.path, path)).collect()
    }

    /// Drops the highlights of the file at `path`: they've been saved into it.
    pub fn forget(&mut self, path: &Path) {
        self.list.retain(|h| !same_file(&h.path, path));
    }

    pub fn add(&mut self, highlight: Highlight) {
        if !highlight.rects.is_empty() {
            self.list.push(highlight);
        }
    }

    /// Removes the highlight at (x, y) on `page`, if there's one. Returns whether there was.
    pub fn remove_at(&mut self, path: &Path, page: usize, x: f32, y: f32) -> bool {
        let before = self.list.len();
        let mut removed = false;
        self.list.retain(|h| {
            let hit = !removed && h.page == page && same_file(&h.path, path) && h.contains(x, y);
            removed |= hit;
            !hit
        });
        self.list.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_round_trip_and_remove_by_point() {
        let mut h = Highlights::default();
        let rects = vec![[10.0, 20.0, 110.0, 32.5], [10.0, 34.0, 60.0, 46.0]];
        h.add(Highlight { path: PathBuf::from(r"C:\a.pdf"), page: 3, color: 0xFFE14D, rects });
        let mut back = Highlights::parse(&h.format());
        assert_eq!(back.list, h.list);
        assert_eq!(back.on(Path::new(r"c:\A.pdf"), 3).count(), 1);
        assert!(!back.remove_at(Path::new(r"C:\a.pdf"), 3, 200.0, 25.0));
        assert!(back.remove_at(Path::new(r"C:\a.pdf"), 3, 30.0, 40.0));
        assert_eq!(back.on(Path::new(r"C:\a.pdf"), 3).count(), 0);
    }
}
