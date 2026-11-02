//! One open document on screen: where the view is, the zoom, the tile cache and render threads,
//! search, and drawing a frame. The Android app calls in on its UI thread only.

use crate::gpu::Gpu;
use crate::window::Window;
use engine::{Engine, OutlineItem, PageSize, Target};
use render::text::TextWorker;
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

    /// Search matches on the pages in view, tinted over them.
    fn draw_matches(&self, view: &Viewport) {
        let Some(gpu) = &self.gpu else { return };
        let pages = self.layout.pages_between(view.y, view.y + i64::from(view.height));
        let scale = self.layout.scale;
        self.text.with_matches(|matches| {
            for m in matches.iter().filter(|m| pages.contains(&m.page)) {
                let p = self.layout.pages[m.page];
                let (ox, oy) = ((p.x - view.x) as f32, (p.y - view.y) as f32);
                let (color, alpha) = if Some((m.page, m.chars.start)) == self.current { CURRENT_TINT } else { MATCH_TINT };
                for r in &m.rects {
                    let (x0, y0) = ((ox + r[0] * scale).floor(), (oy + r[1] * scale).floor());
                    let (x1, y1) = ((ox + r[2] * scale).ceil(), (oy + r[3] * scale).ceil());
                    gpu.fill(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0), color, alpha);
                }
            }
        });
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
                let key = TileKey { page, scale: scale.to_bits(), col, row, look, rotation: 0 };
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
                let mut others = gpu.others(page, scale.to_bits(), look);
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
        self.draw_matches(&view);
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
        let widest = self.sizes.iter().fold(1.0f32, |m, s| m.max(s.width));
        let fit = self.width.saturating_sub(2 * self.gap).max(1) as f32 / widest;
        // past this, a page's tiles would number in the thousands
        (fit * zoom).min(64.0)
    }

    /// Lays the pages out again at the current zoom and view width, keeping the document point under
    /// `anchor` (a position in the view) where it was.
    fn relayout(&mut self, anchor: (f64, f64)) {
        let scale = self.scale_for(self.zoom);
        let old = &self.layout;
        if scale == old.scale && self.width == old.view_width {
            return;
        }
        let new = Layout::arranged(&self.sizes, scale, self.gap, self.width, 0, Spread::One);
        if !old.pages.is_empty() && !new.pages.is_empty() {
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
        let top = match y {
            Some(y) => p.y as f64 + f64::from(y * self.layout.scale) - f64::from(self.gap),
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
        let (dx, dy) = (self.x + x, self.y + y);
        if self.layout.pages.is_empty() {
            return Tap::Nothing;
        }
        let page = self.layout.page_at_point(dx as i64, dy as i64);
        let p = self.layout.pages[page];
        let s = f64::from(self.layout.scale);
        let (px, py) = (((dx - p.x as f64) / s) as f32, ((dy - p.y as f64) / s) as f32);
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
        if let (Some(p), Some(r)) = (self.layout.pages.get(page), rect) {
            let s = f64::from(self.layout.scale);
            let (x0, y0) = (p.x as f64 + f64::from(r[0]) * s, p.y as f64 + f64::from(r[1]) * s);
            let (x1, y1) = (p.x as f64 + f64::from(r[2]) * s, p.y as f64 + f64::from(r[3]) * s);
            let (w, h) = (f64::from(self.width), f64::from(self.height));
            // only move if it's out of view: to a third of the way down, centered across if need be
            let y = if y0 < self.y || y1 > self.y + h { y0 - h / 3.0 } else { self.y };
            let x = if x0 < self.x || x1 > self.x + w { (x0 + x1 - w) / 2.0 } else { self.x };
            self.scroll_to(x, y);
        }
        self.dirty = true;
        true
    }
}
