//! Page text for selection, and search across the document, on a background thread.
//!
//! Text is extracted (a quick interpretation of the page, a few ms) for the pages on screen and
//! kept for a handful of them. A search runs through every page once, starting at the current one,
//! and keeps only where the matches are, not the text.

use engine::{Engine, PageText};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

/// Pages whose text is kept, most recently wanted. About 100 KB each for a dense page.
const KEEP: usize = 6;

/// One place the search text was found.
#[derive(Clone, Debug)]
pub struct Match {
    pub page: usize,
    /// Characters of the page's text.
    pub chars: Range<usize>,
    /// Boxes to highlight, one per line it spans, in points.
    pub rects: Vec<[f32; 4]>,
}

#[derive(Default)]
struct Search {
    /// Lowercased, with whitespace runs as one space.
    query: Vec<char>,
    generation: u64,
    start: usize,
    /// Pages searched so far.
    done: usize,
    /// Sorted by page, then position.
    matches: Vec<Match>,
}

#[derive(Default)]
struct State {
    /// Pages the UI wants text for, most important first.
    wanted: Vec<usize>,
    texts: Vec<(usize, Arc<PageText>)>,
    search: Search,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

enum Task {
    Text(usize),
    Search { page: usize, generation: u64, query: Vec<char> },
}

/// A search's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub found: usize,
    pub searched: usize,
    pub pages: usize,
}

impl Progress {
    pub fn finished(&self) -> bool {
        self.searched >= self.pages
    }
}

pub struct TextWorker {
    shared: Arc<Shared>,
    pages: usize,
}

impl TextWorker {
    /// `notify` runs on the worker thread whenever there's new text or new matches.
    pub fn new(engine: Arc<dyn Engine>, notify: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared { state: Mutex::default(), wake: Condvar::new() });
        let pages = engine.page_count();
        let worker = shared.clone();
        thread::Builder::new()
            .name("text".into())
            .spawn(move || work(&worker, &*engine, &notify))
            .expect("starting the text thread");
        Self { shared, pages }
    }

    /// The page's text, if it's been extracted. Ask with `want` first.
    pub fn text(&self, page: usize) -> Option<Arc<PageText>> {
        let s = self.shared.state.lock().unwrap();
        s.texts.iter().find(|(p, _)| *p == page).map(|(_, t)| t.clone())
    }

    /// Sets the pages whose text should be ready, most important first.
    pub fn want(&self, pages: &[usize]) {
        let mut s = self.shared.state.lock().unwrap();
        if s.wanted != pages {
            s.wanted = pages.to_vec();
            drop(s);
            self.shared.wake.notify_one();
        }
    }

    /// Starts searching for `query` from page `from`, dropping the previous search. An empty query
    /// just clears it.
    pub fn search(&self, query: &str, from: usize) {
        let mut s = self.shared.state.lock().unwrap();
        let generation = s.search.generation + 1;
        s.search = Search { query: normalize(query), generation, start: from, ..Default::default() };
        if s.search.query.is_empty() {
            s.search.done = self.pages;
        }
        drop(s);
        self.shared.wake.notify_one();
    }

    pub fn progress(&self) -> Progress {
        let s = self.shared.state.lock().unwrap();
        Progress { found: s.search.matches.len(), searched: s.search.done, pages: self.pages }
    }

    /// Runs `f` on the matches so far, sorted by page and position.
    pub fn with_matches<R>(&self, f: impl FnOnce(&[Match]) -> R) -> R {
        f(&self.shared.state.lock().unwrap().search.matches)
    }
}

impl Drop for TextWorker {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.wake.notify_one();
    }
}

fn work(shared: &Shared, engine: &dyn Engine, notify: &dyn Fn()) {
    loop {
        let task = {
            let mut s = shared.state.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(&page) = s.wanted.iter().find(|p| !s.texts.iter().any(|(q, _)| q == *p)) {
                    break Task::Text(page);
                }
                let search = &s.search;
                if search.done < engine.page_count() {
                    let page = (search.start + search.done) % engine.page_count();
                    break Task::Search { page, generation: search.generation, query: search.query.clone() };
                }
                s = shared.wake.wait(s).unwrap();
            }
        };
        match task {
            Task::Text(page) => {
                let text = Arc::new(engine.page_text(page));
                let mut s = shared.state.lock().unwrap();
                s.texts.retain(|(p, _)| *p != page);
                s.texts.push((page, text));
                // keep the wanted ones and the most recent others
                while s.texts.len() > KEEP {
                    let Some(i) = s.texts.iter().position(|(p, _)| !s.wanted.contains(p)) else { break };
                    s.texts.remove(i);
                }
            }
            Task::Search { page, generation, query } => {
                let cached = shared.state.lock().unwrap().texts.iter().find(|(p, _)| *p == page).map(|(_, t)| t.clone());
                let text = cached.unwrap_or_else(|| Arc::new(engine.page_text(page)));
                let found = find(&text, &query, page);
                let mut s = shared.state.lock().unwrap();
                if s.search.generation != generation {
                    continue;
                }
                let at = s.search.matches.partition_point(|m| m.page < page);
                s.search.matches.splice(at..at, found);
                s.search.done += 1;
            }
        }
        notify();
    }
}

/// Lowercase, with any whitespace as a single space and outer whitespace dropped.
fn normalize(s: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if out.last() != Some(&' ') {
                out.push(' ');
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Every place `query` occurs in `text`. Case and whitespace are ignored, and a word hyphenated
/// across two lines matches as one word.
fn find(text: &PageText, query: &[char], page: usize) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    // the searchable characters, each remembering which character of the page it came from
    let mut hay: Vec<(char, usize)> = Vec::with_capacity(text.chars.len());
    let chars = &text.chars;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].ch;
        if c == '-' && chars.get(i + 1).is_some_and(|n| n.ch == '\n') {
            i += 2;
            continue;
        }
        if c.is_whitespace() {
            if hay.last().is_some_and(|(h, _)| *h != ' ') {
                hay.push((' ', i));
            }
        } else {
            hay.extend(c.to_lowercase().map(|l| (l, i)));
        }
        i += 1;
    }
    let mut found = Vec::new();
    let mut at = 0;
    while at + query.len() <= hay.len() {
        if hay[at..at + query.len()].iter().map(|(c, _)| c).eq(query.iter()) {
            let chars = hay[at].1..hay[at + query.len() - 1].1 + 1;
            found.push(Match { page, rects: line_rects(text, chars.clone()), chars });
            at += query.len();
        } else {
            at += 1;
        }
    }
    found
}

/// Boxes covering characters `range`, one per stretch of a line.
pub fn line_rects(text: &PageText, range: Range<usize>) -> Vec<[f32; 4]> {
    let mut rects: Vec<[f32; 4]> = Vec::new();
    for c in &text.chars[range.start.min(text.chars.len())..range.end.min(text.chars.len())] {
        if c.ch == '\n' {
            continue;
        }
        let r = c.rect;
        match rects.last_mut() {
            // the same line: boxes overlap vertically by more than half, and touch or nearly touch
            Some(l) if overlap(l[1], l[3], r[1], r[3]) > 0.5 * (r[3] - r[1]).min(l[3] - l[1]) && r[0] <= l[2] + (r[3] - r[1]) => {
                *l = [l[0].min(r[0]), l[1].min(r[1]), l[2].max(r[2]), l[3].max(r[3])];
            }
            _ => rects.push(r),
        }
    }
    rects
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// The character under (x, y) in points, if any; line breaks don't count.
pub fn char_at(text: &PageText, x: f32, y: f32) -> Option<usize> {
    text.chars.iter().position(|c| c.ch != '\n' && c.rect[0] <= x && x <= c.rect[2] && c.rect[1] <= y && y <= c.rect[3])
}

/// The selection boundary nearest (x, y): before or after the nearest character, whichever side
/// of its middle the point is on. Distance across lines counts more than along them.
pub fn boundary_at(text: &PageText, x: f32, y: f32) -> usize {
    let distance = |r: &[f32; 4]| {
        let dx = (r[0] - x).max(x - r[2]).max(0.0);
        let dy = (r[1] - y).max(y - r[3]).max(0.0);
        dx + 4.0 * dy
    };
    let nearest = text
        .chars
        .iter()
        .enumerate()
        .filter(|(_, c)| c.ch != '\n')
        .min_by(|(_, a), (_, b)| distance(&a.rect).total_cmp(&distance(&b.rect)));
    match nearest {
        Some((i, c)) if x > (c.rect[0] + c.rect[2]) / 2.0 => i + 1,
        Some((i, _)) => i,
        None => 0,
    }
}

/// The word around character `i`: letters and digits, or just the character itself.
pub fn word_around(text: &PageText, i: usize) -> Range<usize> {
    let is_word = |j: usize| text.chars.get(j).is_some_and(|c| c.ch.is_alphanumeric());
    if !is_word(i) {
        return i..(i + 1).min(text.chars.len());
    }
    let mut start = i;
    while start > 0 && is_word(start - 1) {
        start -= 1;
    }
    let mut end = i + 1;
    while is_word(end) {
        end += 1;
    }
    start..end
}

/// The characters in `range` as a string.
pub fn text_of(text: &PageText, range: Range<usize>) -> String {
    text.chars[range.start.min(text.chars.len())..range.end.min(text.chars.len())].iter().map(|c| c.ch).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::TextChar;

    /// "Hello wor-" / "ld again", 10 points per character, lines 20 points apart.
    fn sample() -> PageText {
        let mut chars = Vec::new();
        for (line, s) in ["Hello wor-", "ld again"].iter().enumerate() {
            if line > 0 {
                chars.push(TextChar { ch: '\n', rect: [100.0, 10.0, 100.0, 10.0] });
            }
            for (i, ch) in s.chars().enumerate() {
                let (x, y) = (i as f32 * 10.0, line as f32 * 20.0);
                chars.push(TextChar { ch, rect: [x, y, x + 10.0, y + 12.0] });
            }
        }
        PageText { chars }
    }

    #[test]
    fn finds_ignoring_case_and_hyphenation() {
        let text = sample();
        let found = find(&text, &normalize("WORLD"), 3);
        assert_eq!(found.len(), 1);
        assert_eq!(text_of(&text, found[0].chars.clone()), "wor-\nld");
        // the match spans two lines, so two boxes
        assert_eq!(found[0].rects.len(), 2);
        assert_eq!(find(&text, &normalize("hello   world"), 0).len(), 1);
        assert!(find(&text, &normalize("planet"), 0).is_empty());
    }

    #[test]
    fn hits_words_and_boundaries() {
        let text = sample();
        assert_eq!(char_at(&text, 15.0, 5.0), Some(1));
        assert_eq!(char_at(&text, 15.0, 15.0), None);
        assert_eq!(boundary_at(&text, 14.0, 5.0), 1);
        assert_eq!(boundary_at(&text, 16.0, 5.0), 2);
        assert_eq!(text_of(&text, word_around(&text, 2)), "Hello");
        assert_eq!(line_rects(&text, 0..5), vec![[0.0, 0.0, 50.0, 12.0]]);
    }
}
