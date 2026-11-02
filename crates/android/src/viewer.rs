//! One open document on screen: where the view is, the zoom, the tile cache and render threads,
//! search, and drawing a frame. The Android app calls in on its UI thread only.

use crate::gpu::Gpu;
use crate::window::Window;
use engine::{Engine, OutlineItem, PageSize, PageText, Target};
use render::text::{self, TextWorker};
use render::{Layout, Look, RenderPool, Spread, TILE, TileCache, TileKey, Viewport};
use std::sync::Arc;

/// Zoom is relative to the page width that fits the screen: 1 is fit width.
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 12.0;
/// Rendered tiles kept, in screens' worth of raw pixels; most compress several times over. A
/// tablet's screen is 20 MB of pixels, so this is kept lower than on Windows.
const CACHE_SCREENS: usize = 2;
/// Search matches, blended over the page: color and opacity out of 255. As on Windows.
const MATCH_TINT: (u32, u32) = (0x00FF_D400, 110);
const CURRENT_TINT: (u32, u32) = (0x00FF_7A00, 150);
const SELECTION_TINT: (u32, u32) = (0x0033_8CFF, 90);
/// Places kept for going back after following a link.
const BACK: usize = 50;
/// Where the engine failed on a tile. As on Windows.
const FAILED: u32 = 0x00F4_CCCC;
/// Tiles turned into textures in a frame, at most: each takes about half a millisecond. The rest
/// wait for the next frame, shown stretched from another zoom or as blank paper meanwhile.
const UPLOADS: usize = 4;
/// Textures kept, in halves of a screen's worth of pixels: the screen and a half of what's near it.
const TEXTURE_HALF_SCREENS: usize = 3;

/// What a tap on the page did.
pub enum Tap {
    Nothing,
    /// Followed a link within the document.
    Moved,
    /// A link out of it, for the app to open.
    Uri(String),
}

/// Characters `start` up to `end` of a page's text.
#[derive(Clone, Copy)]
struct Selection {
    page: usize,
    start: usize,
    end: usize,
}

pub struct Viewer {
    engine: Arc<dyn Engine>,
    sizes: Vec<PageSize>,
    layout: Layout,
    zoom: f32,
    /// The view's top-left in document pixels.
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    gap: u32,
    /// Room the app's bars take at the top and bottom: the view can scroll this far past each end,
    /// so the first and last pages can be seen clear of them.
    inset_top: f64,
    inset_bottom: f64,
    cache: TileCache,
    pool: RenderPool,
    text: TextWorker,
    look: Look,
    background: u32,
    /// 1 when scrolling down, -1 up: the next screen that way is rendered ahead.
    ahead: i64,
    /// A pinch or zoom animation is going: tiles are stretched, and asked for once it ends rather
    /// than at every scale on the way.
    gesture: bool,
    /// The view changed since the last frame.
    dirty: bool,
    /// Tiles were asked for and haven't all arrived.
    waiting: bool,
    /// Search progress at the last frame, to draw again when more is found.
    searched: (usize, usize),
    /// The match last moved to, by page and first character: more matches arrive while a search
    /// runs, so its place in the list changes.
    current: Option<(usize, usize)>,
    /// Where links were followed from: page and points down from its top.
    back: Vec<(usize, f32)>,
    /// Quarter turns clockwise, and how pages sit in rows.
    rotation: u8,
    spread: Spread,
    /// Selected text, on one page.
    selection: Option<Selection>,
    gpu: Option<Gpu>,
}

impl Viewer {
    pub fn new(engine: Arc<dyn Engine>, gap: u32) -> Self {
        let sizes: Vec<PageSize> = (0..engine.page_count()).map(|i| engine.page_size(i)).collect();
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4);
        // frames are polled while tiles are on their way, so the threads needn't wake anything
        let pool = RenderPool::new(engine.clone(), threads, || {});
        let text = TextWorker::new(engine.clone(), || {});
        let layout = Layout::new(&sizes, 1.0, gap, 1);
        Self {
            engine,
            sizes,
            layout,
            zoom: 1.0,
            x: 0.0,
            y: 0.0,
            width: 1,
            height: 1,
            gap,
            inset_top: 0.0,
            inset_bottom: 0.0,
            cache: TileCache::default(),
            pool,
            text,
            look: Look::Normal,
            background: 0x00E8_E8E8,
            ahead: 1,
            gesture: false,
            dirty: true,
            waiting: false,
            searched: (0, 0),
            current: None,
            back: Vec::new(),
            rotation: 0,
            spread: Spread::One,
            selection: None,
            gpu: None,
        }
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    pub fn title(&self) -> Option<String> {
        self.engine.info().title.filter(|t| !t.trim().is_empty())
    }

    /// The outline as lines of depth, page (-1 for none), points down the page (-1 for its top) and
    /// title, tab-separated.
    pub fn outline(&self) -> String {
        fn walk(items: &[OutlineItem], depth: usize, out: &mut String) {
            for item in items {
                let (page, y) = match item.target {
                    Some(Target::Page { page, y }) => (page as i64, y.unwrap_or(-1.0)),
                    _ => (-1, -1.0),
                };
                let title: String = item.title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                out.push_str(&format!("{depth}\t{page}\t{y}\t{}\n", title.trim()));
                walk(&item.children, depth + 1, out);
            }
        }
        let mut out = String::new();
        walk(&self.engine.outline(), 0, &mut out);
        out
    }

    pub fn set_window(&mut self, window: Option<Window>, width: u32, height: u32) {
        // one GPU surface per window at a time: the old one goes first
        self.gpu = None;
        self.gpu = window.and_then(Gpu::new);
        if width > 0 && height > 0 && (width, height) != (self.width, self.height) {
            // keep the top of the view where it was
            let anchor = (f64::from(self.width) / 2.0, 0.0);
            (self.width, self.height) = (width, height);
            self.relayout(anchor);
        }
        self.dirty = true;
    }

    /// Search matches and the selection on the pages in view, tinted over them.
    fn draw_highlights(&self, view: &Viewport) {
        let Some(gpu) = &self.gpu else { return };
        let pages = self.layout.pages_between(view.y, view.y + i64::from(view.height));
        let tint = |page: usize, r: &[f32; 4], (color, alpha): (u32, u32)| {
            let (x0, y0, x1, y1) = self.rect_to_doc(page, r);
            let (x0, y0) = ((x0 - view.x as f64).floor() as f32, (y0 - view.y as f64).floor() as f32);
            let (x1, y1) = ((x1 - view.x as f64).ceil() as f32, (y1 - view.y as f64).ceil() as f32);
            gpu.fill(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0), color, alpha);
        };
        self.text.with_matches(|matches| {
            for m in matches.iter().filter(|m| pages.contains(&m.page)) {
                let style = if Some((m.page, m.chars.start)) == self.current { CURRENT_TINT } else { MATCH_TINT };
                for r in &m.rects {
                    tint(m.page, r, style);
                }
            }
        });
        if let Some((sel, t)) = self.selection.and_then(|s| Some((s, self.text.text(s.page)?))) {
            for r in text::line_rects(&t, sel.start..sel.end) {
                tint(sel.page, &r, SELECTION_TINT);
            }
        }
    }

    pub fn set_look(&mut self, look: Look, background: u32) {
        (self.look, self.background) = (look, background);
        self.dirty = true;
    }

    /// Draws a frame if anything changed. Returns whether more frames are wanted: tiles or search
    /// results are still on their way.
    pub fn draw(&mut self) -> bool {
        let finished = self.pool.take_finished();
        let arrived = !finished.is_empty();
        for done in finished {
            self.cache.insert(done.key, done.width, done.height, done.pixels);
        }
        let progress = self.text.progress();
        if (progress.found, progress.searched) != self.searched {
            self.searched = (progress.found, progress.searched);
            self.dirty = true;
        }
        if !(self.dirty || arrived) {
            return self.waiting || !progress.finished() || self.gesture;
        }
        let Some(gpu) = &mut self.gpu else { return false };
        let view = Viewport { x: self.x.round() as i64, y: self.y.round() as i64, width: self.width, height: self.height };
        if !gpu.begin(view.width, view.height, self.background) {
            return true;
        }
        self.cache.set_budget(CACHE_SCREENS * self.width as usize * self.height as usize * 4);
        self.cache.begin_frame();
        let scale = self.layout.scale;
        let look = self.look as u8;
        let mut uploads = 0;
        // Like render::compose, which the Windows app uses: blank paper, then where a sharp tile isn't
        // ready, tiles of the page at other zoom levels stretched over the gap, then the sharp ones.
        for page in self.layout.pages_between(view.y, view.y + i64::from(view.height)) {
            let pos = self.layout.pages[page];
            let (ox, oy) = ((pos.x - view.x) as f32, (pos.y - view.y) as f32);
            gpu.fill(ox, oy, pos.width as f32, pos.height as f32, self.look.paper(), 255);
            let mut sharp = Vec::new();
            let mut missing = false;
            for (col, row, area) in self.layout.tiles(page, &view) {
                let key = TileKey { page, scale: scale.to_bits(), col, row, look, rotation: self.layout.rotation };
                let cached = self.cache.get(&key);
                if !gpu.has(&key) {
                    match cached {
                        Some(t) if t.pixels.is_empty() => {
                            gpu.fill(ox + area.x as f32, oy + area.y as f32, area.width as f32, area.height as f32, FAILED, 255);
                            continue;
                        }
                        Some(t) if uploads < UPLOADS => {
                            gpu.upload(key, t);
                            uploads += 1;
                        }
                        Some(_) => self.dirty = true,
                        None => {}
                    }
                }
                if gpu.has(&key) {
                    sharp.push((key, area));
                } else {
                    missing = true;
                }
            }
            if missing {
                // farthest zoom first, so the closest ends up on top
                let mut others = gpu.others(page, scale.to_bits(), look, self.layout.rotation);
                let off = |k: &TileKey| (f32::from_bits(k.scale) / scale).ln().abs();
                others.sort_by(|a, b| off(&b.0).total_cmp(&off(&a.0)));
                for (key, w, h) in others {
                    let r = scale / f32::from_bits(key.scale);
                    let (x, y) = ((key.col * TILE) as f32 * r, (key.row * TILE) as f32 * r);
                    gpu.image(&key, ox + x, oy + y, w as f32 * r, h as f32 * r);
                }
            }
            for (key, area) in sharp {
                gpu.image(&key, ox + area.x as f32, oy + area.y as f32, area.width as f32, area.height as f32);
            }
        }
        self.draw_highlights(&view);
        // text for selecting on the pages in view, ready before a finger asks for it
        let visible: Vec<usize> = self.layout.pages_between(view.y, view.y + i64::from(view.height)).collect();
        self.text.want(&visible);
        if let Some(gpu) = &mut self.gpu {
            gpu.finish(TEXTURE_HALF_SCREENS * self.width as usize * self.height as usize * 2);
        }
        let more_uploads = std::mem::take(&mut self.dirty);
        if !self.gesture {
            let jobs = render::wanted_tiles(&self.layout, &view, self.ahead, self.look, &self.cache);
            self.waiting = !jobs.is_empty();
            self.pool.set_jobs(jobs);
        }
        self.cache.evict();
        self.dirty = more_uploads;
        self.waiting || more_uploads || !progress.finished() || self.gesture
    }


    /// Pixels per point at `zoom`.
    fn scale_for(&self, zoom: f32) -> f32 {
        let turned = |s: &PageSize| if self.rotation % 2 == 1 { s.height } else { s.width };
        let widest = self.sizes.iter().fold(1.0f32, |m, s| m.max(turned(s)));
        // a spread is two pages wide, with a gap between them
        let across = if self.spread == Spread::One { 1 } else { 2 };
        let fit = self.width.saturating_sub((across + 1) * self.gap).max(1) as f32 / (widest * across as f32);
        // past this, a page's tiles would number in the thousands
        (fit * zoom).min(64.0)
    }

    /// Lays the pages out again at the current zoom, view width, rotation and spread, keeping the
    /// document point under `anchor` (a position in the view) where it was.
    fn relayout(&mut self, anchor: (f64, f64)) {
        let scale = self.scale_for(self.zoom);
        let old = &self.layout;
        let same_view = old.rotation == self.rotation && old.spread == self.spread;
        if scale == old.scale && self.width == old.view_width && same_view {
            return;
        }
        let new = Layout::arranged(&self.sizes, scale, self.gap, self.width, self.rotation, self.spread);
        // a new rotation or spread moves pages about: the caller shows the page again
        if !old.pages.is_empty() && !new.pages.is_empty() && same_view {
            let (ax, ay) = (self.x + anchor.0, self.y + anchor.1);
            let i = old.page_at(ay as i64);
            let (p, q) = (old.pages[i], new.pages[i]);
            let k = f64::from(scale) / f64::from(old.scale);
            self.x = q.x as f64 + (ax - p.x as f64) * k - anchor.0;
            self.y = q.y as f64 + (ay - p.y as f64) * k - anchor.1;
        }
        self.layout = new;
        self.clamp();
        self.dirty = true;
    }

    /// Turns pages `rotation` quarter turns clockwise and sets them in rows of one (`spread` 0), two
    /// (1), or as a book (2), keeping the current page in view.
    pub fn set_view(&mut self, rotation: u8, spread: u8) {
        let spread = match spread {
            1 => Spread::Two,
            2 => Spread::Book,
            _ => Spread::One,
        };
        if (rotation % 4, spread) == (self.rotation, self.spread) {
            return;
        }
        let page = self.current_page();
        (self.rotation, self.spread) = (rotation % 4, spread);
        self.selection = None;
        self.relayout((0.0, 0.0));
        self.go_to_page(page);
    }

    /// A point of `page`, in points from its top-left as it is, in document pixels.
    fn to_doc(&self, page: usize, x: f32, y: f32) -> (f64, f64) {
        let (p, s) = (self.layout.pages[page], self.layout.scale);
        let PageSize { width: w, height: h } = self.sizes[page];
        let (tx, ty) = match self.layout.rotation % 4 {
            0 => (x, y),
            1 => (h - y, x),
            2 => (w - x, h - y),
            _ => (y, w - x),
        };
        (p.x as f64 + f64::from(tx * s), p.y as f64 + f64::from(ty * s))
    }

    /// The point of `page`, in points from its top-left as it is, at (dx, dy) in document pixels.
    fn from_doc(&self, page: usize, dx: f64, dy: f64) -> (f32, f32) {
        let (p, s) = (self.layout.pages[page], f64::from(self.layout.scale));
        let PageSize { width: w, height: h } = self.sizes[page];
        let (tx, ty) = (((dx - p.x as f64) / s) as f32, ((dy - p.y as f64) / s) as f32);
        match self.layout.rotation % 4 {
            0 => (tx, ty),
            1 => (ty, h - tx),
            2 => (w - tx, h - ty),
            _ => (w - ty, tx),
        }
    }

    /// A rectangle of `page` (x0, y0, x1, y1 in points of the page as it is) in document pixels.
    fn rect_to_doc(&self, page: usize, r: &[f32; 4]) -> (f64, f64, f64, f64) {
        let (ax, ay) = self.to_doc(page, r[0], r[1]);
        let (bx, by) = self.to_doc(page, r[2], r[3]);
        (ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
    }

    /// The page under (x, y) in the view, and the point on it in points.
    fn page_point(&self, x: f64, y: f64) -> Option<(usize, f32, f32)> {
        if self.layout.pages.is_empty() {
            return None;
        }
        let (dx, dy) = (self.x + x, self.y + y);
        let page = self.layout.page_at_point(dx as i64, dy as i64);
        let (px, py) = self.from_doc(page, dx, dy);
        Some((page, px, py))
    }

    fn max_scroll(&self) -> (f64, f64) {
        (
            (self.layout.width - i64::from(self.width)).max(0) as f64,
            (self.layout.height - i64::from(self.height)).max(0) as f64,
        )
    }

    fn clamp(&mut self) {
        let (mx, my) = self.max_scroll();
        self.x = self.x.clamp(0.0, mx);
        self.y = self.y.clamp(-self.inset_top, my + self.inset_bottom);
    }

    pub fn set_insets(&mut self, top: f64, bottom: f64) {
        let first = self.y <= -self.inset_top;
        (self.inset_top, self.inset_bottom) = (top.max(0.0), bottom.max(0.0));
        // a view at the very top stays there
        if first {
            self.y = -self.inset_top;
        }
        self.clamp();
        self.dirty = true;
    }

    /// x, y, the largest x and y, and the smallest y.
    pub fn scroll_state(&self) -> [f32; 5] {
        let (mx, my) = self.max_scroll();
        [self.x as f32, self.y as f32, mx as f32, (my + self.inset_bottom) as f32, -self.inset_top as f32]
    }

    pub fn scroll_to(&mut self, x: f64, y: f64) {
        if y != self.y {
            self.ahead = if y > self.y { 1 } else { -1 };
        }
        (self.x, self.y) = (x, y);
        self.clamp();
        self.dirty = true;
    }

    pub fn scroll_by(&mut self, dx: f64, dy: f64) {
        self.scroll_to(self.x + dx, self.y + dy);
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Zooms by `factor` around (fx, fy) in the view.
    pub fn zoom_by(&mut self, factor: f32, fx: f64, fy: f64) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.relayout((fx, fy));
    }

    pub fn set_gesture(&mut self, on: bool) {
        self.gesture = on;
        self.dirty = true;
    }

    pub fn current_page(&self) -> usize {
        let view = Viewport { x: self.x as i64, y: self.y as i64, width: self.width, height: self.height };
        self.layout.current_page(&view)
    }

    pub fn go_to_page(&mut self, page: usize) {
        self.go_to(page, None);
    }

    /// Shows `page`, from `y` points down it if given, else from its top.
    pub fn go_to(&mut self, page: usize, y: Option<f32>) {
        let Some(p) = self.layout.pages.get(page.min(self.sizes.len().saturating_sub(1))) else { return };
        // a line the PDF names, where it is on the turned page; a page turned on its side from its top
        let page = page.min(self.sizes.len() - 1);
        let line = y.filter(|_| self.layout.rotation % 2 == 0).map(|y| self.to_doc(page, 0.0, y).1);
        let top = match line {
            Some(y) => y - f64::from(self.gap),
            None => (p.y - i64::from(self.gap)) as f64,
        } - self.inset_top;
        self.scroll_to(self.x, top);
    }

    /// The page at the top of the view, and how far down it the view starts, in points.
    pub fn position(&self) -> (usize, f32) {
        let page = self.layout.page_at(self.y as i64);
        let top = self.layout.pages.get(page).map_or(0, |p| p.y);
        (page, ((self.y - top as f64) / f64::from(self.layout.scale)) as f32)
    }

    pub fn restore(&mut self, page: usize, offset: f32, zoom: f32) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.relayout((0.0, 0.0));
        let Some(p) = self.layout.pages.get(page.min(self.sizes.len().saturating_sub(1))) else { return };
        let y = p.y as f64 + f64::from(offset * self.layout.scale);
        let x = (self.layout.width - i64::from(self.width)).max(0) as f64 / 2.0;
        self.scroll_to(x, y);
    }

    /// Follows the link at (x, y) in the view, if there is one.
    pub fn tap(&mut self, x: f64, y: f64) -> Tap {
        let Some((page, px, py)) = self.page_point(x, y) else { return Tap::Nothing };
        // a little slack around small links, for fingers
        let slack = (4.0 / self.layout.scale).max(2.0);
        let links = self.engine.page_links(page);
        let hit = links.into_iter().find(|l| {
            let r = l.rect;
            px >= r[0].min(r[2]) - slack && px <= r[0].max(r[2]) + slack && py >= r[1].min(r[3]) - slack && py <= r[1].max(r[3]) + slack
        });
        match hit.map(|l| l.target) {
            Some(Target::Page { page, y }) => {
                self.back.push(self.position());
                if self.back.len() > BACK {
                    self.back.remove(0);
                }
                self.go_to(page, y);
                Tap::Moved
            }
            Some(Target::Uri(uri)) => Tap::Uri(uri),
            None => Tap::Nothing,
        }
    }

    /// Goes back to where the last link was followed from. False when there's nowhere to go.
    pub fn back(&mut self) -> bool {
        let Some((page, offset)) = self.back.pop() else { return false };
        let Some(p) = self.layout.pages.get(page) else { return false };
        let y = p.y as f64 + f64::from(offset * self.layout.scale);
        self.scroll_to(self.x, y);
        true
    }

    pub fn search(&mut self, query: &str) {
        self.current = None;
        self.text.search(query, self.current_page());
        self.dirty = true;
    }

    /// Matches found, pages searched, pages, and the current match (-1 for none).
    pub fn search_status(&self) -> [i32; 4] {
        let p = self.text.progress();
        let current = self.text.with_matches(|m| self.current.and_then(|c| m.iter().position(|x| (x.page, x.chars.start) == c)));
        [p.found as i32, p.searched as i32, p.pages as i32, current.map_or(-1, |c| c as i32)]
    }

    /// Moves to the next match (`dir` 1) or the previous one (-1), and shows it.
    pub fn search_step(&mut self, dir: i32) -> bool {
        let page = self.current_page();
        let found = self.text.with_matches(|m| {
            if m.is_empty() {
                return None;
            }
            let n = m.len();
            let current = self.current.and_then(|c| m.iter().position(|x| (x.page, x.chars.start) == c));
            let i = match current {
                Some(c) if dir >= 0 => (c + 1) % n,
                Some(c) => (c + n - 1) % n,
                // the first one on this page or after it, or the last one on it or before it
                None if dir >= 0 => m.iter().position(|x| x.page >= page).unwrap_or(0),
                None => m.iter().rposition(|x| x.page <= page).unwrap_or(n - 1),
            };
            Some(((m[i].page, m[i].chars.start), m[i].page, m[i].rects.first().copied()))
        });
        let Some((key, page, rect)) = found else { return false };
        self.current = Some(key);
        if let Some(r) = rect.filter(|_| page < self.sizes.len()) {
            let (x0, y0, x1, y1) = self.rect_to_doc(page, &r);
            let (w, h) = (f64::from(self.width), f64::from(self.height));
            // only move if it's out of view: to a third of the way down, centered across if need be
            let y = if y0 < self.y || y1 > self.y + h { y0 - h / 3.0 } else { self.y };
            let x = if x0 < self.x || x1 > self.x + w { (x0 + x1 - w) / 2.0 } else { self.x };
            self.scroll_to(x, y);
        }
        self.dirty = true;
        true
    }

    /// Selects the word at (x, y) in the view. False when there's no text there, or the page's text
    /// isn't ready yet (it's asked for, so a moment later it may be).
    pub fn select_word(&mut self, x: f64, y: f64) -> bool {
        let Some((page, px, py)) = self.page_point(x, y) else { return false };
        let Some(t) = self.page_text(page) else { return false };
        // a finger is broad: the character under it, or failing that the nearest one close by
        let reach = (12.0 / self.layout.scale).max(3.0);
        let near = || {
            let distance = |r: &[f32; 4]| (r[0] - px).max(px - r[2]).max(0.0) + 2.0 * (r[1] - py).max(py - r[3]).max(0.0);
            let (i, c) = t.chars.iter().enumerate().filter(|(_, c)| c.ch != '\n').min_by(|a, b| distance(&a.1.rect).total_cmp(&distance(&b.1.rect)))?;
            (distance(&c.rect) <= reach).then_some(i)
        };
        let Some(i) = text::char_at(&t, px, py).or_else(near) else { return false };
        let r = text::word_around(&t, i);
        self.selection = Some(Selection { page, start: r.start, end: r.end });
        self.dirty = true;
        true
    }

    /// Moves the selection's start (`handle` 0) or end (1) to the text nearest (x, y) in the view.
    /// Returns the handle being moved, which swaps when one is dragged past the other.
    pub fn select_to(&mut self, handle: i32, x: f64, y: f64) -> i32 {
        let Some(mut sel) = self.selection else { return handle };
        let Some(t) = self.page_text(sel.page) else { return handle };
        let (px, py) = self.from_doc(sel.page, self.x + x, self.y + y);
        let b = text::boundary_at(&t, px, py);
        let mut handle = handle;
        if handle == 0 {
            sel.start = b;
        } else {
            sel.end = b;
        }
        if sel.start > sel.end {
            (sel.start, sel.end) = (sel.end, sel.start);
            handle = 1 - handle;
        }
        // never empty: at least the character under the finger
        if sel.start == sel.end {
            sel.end = (sel.start + 1).min(t.chars.len());
        }
        self.selection = Some(sel);
        self.dirty = true;
        handle
    }

    pub fn clear_selection(&mut self) {
        if self.selection.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        let sel = self.selection?;
        let t = self.text.text(sel.page)?;
        Some(text::text_of(&t, sel.start..sel.end).trim().to_owned()).filter(|s| !s.is_empty())
    }

    /// Where the selection's handles go, in view pixels: the start's bottom-left corner, then the
    /// end's bottom-right.
    pub fn selection_handles(&self) -> Option<[f32; 4]> {
        let sel = self.selection?;
        let t = self.text.text(sel.page)?;
        let rects = text::line_rects(&t, sel.start..sel.end);
        let (first, last) = (rects.first()?, rects.last()?);
        let (ax0, _, _, ay1) = self.rect_to_doc(sel.page, first);
        let (_, _, bx1, by1) = self.rect_to_doc(sel.page, last);
        Some([(ax0 - self.x) as f32, (ay1 - self.y) as f32, (bx1 - self.x) as f32, (by1 - self.y) as f32])
    }

    fn page_text(&self, page: usize) -> Option<Arc<PageText>> {
        let t = self.text.text(page);
        if t.is_none() {
            self.text.want(&[page]);
        }
        t
    }

    /// What the document says about itself, as lines of label and value, tab-separated.
    pub fn info(&self) -> String {
        let i = self.engine.info();
        let mut out = format!("Format\t{}\nPages\t{}\n", i.format, self.sizes.len());
        let fields = [
            ("Title", &i.title),
            ("Author", &i.author),
            ("Subject", &i.subject),
            ("Keywords", &i.keywords),
            ("Created with", &i.creator),
            ("Converted by", &i.producer),
            ("Created", &i.created),
            ("Modified", &i.modified),
        ];
        for (label, value) in fields {
            if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                let v: String = v.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                out.push_str(&format!("{label}\t{v}\n"));
            }
        }
        if let Some(size) = self.sizes.first() {
            let (w, h) = (size.width / 72.0 * 25.4, size.height / 72.0 * 25.4);
            out.push_str(&format!("Page size\t{w:.0} × {h:.0} mm\n"));
        }
        out
    }

    /// The engine, for work on another thread such as a thumbnail.
    pub fn engine(&self) -> Arc<dyn Engine> {
        self.engine.clone()
    }
}
