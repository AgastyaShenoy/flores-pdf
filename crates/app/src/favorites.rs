//! Bookmarked pages, kept in `favorites.txt` beside the settings file (they were called favorites
//! once): one per line, the page (from 0), its name and the file, separated by tabs.

use crate::history::same_file;
use crate::settings;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Debug)]
pub struct Favorite {
    pub path: PathBuf,
    pub page: usize,
    pub name: String,
}

#[derive(Default)]
pub struct Favorites {
    list: Vec<Favorite>,
}

impl Favorites {
    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(settings::favorites_path()).unwrap_or_default())
    }

    fn parse(text: &str) -> Self {
        let list = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|line| {
                let mut f = line.splitn(3, '\t');
                let (page, name, path) = (f.next()?.parse().ok()?, f.next()?, f.next()?);
                (!path.is_empty()).then(|| Favorite { path: PathBuf::from(path), page, name: name.to_owned() })
            })
            .collect();
        Self { list }
    }

    fn format(&self) -> String {
        let mut out = String::from("# Flores PDF's favorite pages: page (from 0), name, file.\n");
        for f in &self.list {
            let Some(path) = f.path.to_str() else { continue };
            let _ = writeln!(out, "{}\t{}\t{path}", f.page, f.name);
        }
        out
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = settings::favorites_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.format())
    }

    /// The favorites in the file at `path`, by page.
    pub fn of(&self, path: &Path) -> Vec<&Favorite> {
        let mut found: Vec<&Favorite> = self.list.iter().filter(|f| same_file(&f.path, path)).collect();
        found.sort_by_key(|f| f.page);
        found
    }

    pub fn has(&self, path: &Path, page: usize) -> bool {
        self.list.iter().any(|f| f.page == page && same_file(&f.path, path))
    }

    /// Marks `page` of `path`, named `name` (tabs and line breaks become spaces).
    pub fn add(&mut self, path: &Path, page: usize, name: &str) {
        if !self.has(path, page) {
            let name = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_owned();
            self.list.push(Favorite { path: path.to_owned(), page, name });
        }
    }

    /// Gives the bookmark on `page` of `path` a new name.
    pub fn rename(&mut self, path: &Path, page: usize, name: &str) {
        let name = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_owned();
        if let Some(f) = self.list.iter_mut().find(|f| f.page == page && same_file(&f.path, path)) {
            f.name = name;
        }
    }

    pub fn remove(&mut self, path: &Path, page: usize) {
        self.list.retain(|f| !(f.page == page && same_file(&f.path, path)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_round_trip_and_sort_by_page() {
        let mut f = Favorites::default();
        f.add(Path::new(r"C:\a.pdf"), 9, "Overview\tand more");
        f.add(Path::new(r"C:\a.pdf"), 2, "Page 3");
        f.add(Path::new(r"C:\b.pdf"), 0, "Cover");
        f.add(Path::new(r"c:\A.PDF"), 2, "again");
        let back = Favorites::parse(&f.format());
        let pages: Vec<(usize, &str)> = back.of(Path::new(r"C:\a.pdf")).iter().map(|f| (f.page, f.name.as_str())).collect();
        assert_eq!(pages, [(2, "Page 3"), (9, "Overview and more")]);
        let mut back = back;
        back.remove(Path::new(r"C:\A.pdf"), 9);
        assert_eq!(back.of(Path::new(r"C:\a.pdf")).len(), 1);
    }
}
