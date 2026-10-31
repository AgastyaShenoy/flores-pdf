//! Recently opened files and where the reader was in each, so a file reopens at the same place,
//! the start page can list them, and the tabs open at exit can come back.
//!
//! Kept in `history.txt` beside the settings file, newest first, one file per line: page, offset,
//! zoom, tab, view and path, separated by tabs (lines written before views were kept have no
//! view). Windows paths can't contain tabs or line breaks, so nothing needs quoting.

use crate::settings;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Files remembered.
const KEEP: usize = 50;

#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    pub path: PathBuf,
    /// The page at the top of the view, and how far down it the view started, in points.
    pub page: usize,
    pub y: f32,
    /// As the -zoom flag takes it: "fit-page", "fit-width" or a percentage.
    pub zoom: String,
    /// Rotation and spread, as "90" or "0-two".
    pub view: String,
    /// Place among the tabs open when the reader closed, from 1; 0 if it wasn't open.
    pub tab: usize,
    /// It was the tab being shown.
    pub active: bool,
}

#[derive(Default)]
pub struct History {
    /// Most recently looked at first.
    pub entries: Vec<Entry>,
}

impl History {
    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(settings::history_path()).unwrap_or_default())
    }

    /// Lines that don't parse are skipped.
    fn parse(text: &str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let fields: Vec<&str> = line.splitn(6, '\t').collect();
            let (page, y, zoom, tab, view, path) = match fields[..] {
                [page, y, zoom, tab, view, path] => (page, y, zoom, tab, view, path),
                [page, y, zoom, tab, path] => (page, y, zoom, tab, "", path),
                _ => continue,
            };
            let (Ok(page), Ok(y)) = (page.parse(), y.parse::<f32>()) else { continue };
            if path.is_empty() || !y.is_finite() || entries.len() == KEEP {
                continue;
            }
            let active = tab.ends_with('*');
            let tab = tab.trim_end_matches('*').parse().unwrap_or(0);
            entries.push(Entry { path: PathBuf::from(path), page, y, zoom: zoom.to_owned(), view: view.to_owned(), tab, active });
        }
        Self { entries }
    }

    fn format(&self) -> String {
        let mut out = String::from("# Flores PDF's recent files, newest first: page, offset in points, zoom, tab (* = shown), view, path.\n");
        for e in &self.entries {
            // a path that isn't valid Unicode couldn't be read back the same
            let Some(path) = e.path.to_str() else { continue };
            let active = if e.active { "*" } else { "" };
            let _ = writeln!(out, "{}\t{}\t{}\t{}{active}\t{}\t{path}", e.page, e.y, e.zoom, e.tab, e.view);
        }
        out
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = settings::history_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.format())
    }

    /// Forgets every file, and deletes the history file.
    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = std::fs::remove_file(settings::history_path());
    }

    pub fn find(&self, path: &Path) -> Option<&Entry> {
        self.entries.iter().find(|e| same_file(&e.path, path))
    }

    /// Puts `entry` first, replacing what was remembered for its file.
    pub fn remember(&mut self, entry: Entry) {
        self.forget(&entry.path);
        self.entries.insert(0, entry);
        self.entries.truncate(KEEP);
    }

    pub fn forget(&mut self, path: &Path) {
        self.entries.retain(|e| !same_file(&e.path, path));
    }
}

/// Whether two absolute paths name the same file. Windows paths ignore case.
pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, tab: usize) -> Entry {
        Entry { path: PathBuf::from(path), page: 11, y: -6.5, zoom: "fit-width".into(), view: "90-two".into(), tab, active: tab == 2 }
    }

    #[test]
    fn entries_round_trip_through_the_file() {
        let h = History { entries: vec![entry(r"C:\Papers\A survey.pdf", 2), entry(r"D:\b.pdf", 0)] };
        assert_eq!(History::parse(&h.format()).entries, h.entries);
        // broken lines are skipped, not fatal
        let text = "garbage\n3\tx\tfit-page\t0\tC:\\a.pdf\n4\t0\t150\t1\tC:\\c.pdf\n";
        assert_eq!(History::parse(text).entries.len(), 1);
        // a line from before views were kept reads with no view
        assert_eq!(History::parse(text).entries[0].view, "");
    }

    #[test]
    fn remembering_moves_a_file_to_the_front_once() {
        let mut h = History::default();
        h.remember(entry(r"C:\a.pdf", 0));
        h.remember(entry(r"C:\b.pdf", 0));
        h.remember(entry(r"c:\A.PDF", 0));
        let paths: Vec<_> = h.entries.iter().map(|e| e.path.to_str().unwrap()).collect();
        assert_eq!(paths, [r"c:\A.PDF", r"C:\b.pdf"]);
        for i in 0..KEEP + 5 {
            h.remember(entry(&format!(r"C:\{i}.pdf"), 0));
        }
        assert_eq!(h.entries.len(), KEEP);
    }
}
