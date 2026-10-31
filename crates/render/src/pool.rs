use crate::cache::TileKey;
use crate::look::Look;
use crate::pixels::Pixels;
use engine::{Engine, PageRenderer, PixelRect};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// How long a render thread keeps its recorded pages and caches with nothing to do. Long enough to
/// cover a pause while scrolling or zooming, short enough that an idle reader stays small.
const KEEP_IDLE: Duration = Duration::from_secs(3);
/// How far down the queue a thread looks for a tile of the page it last rendered.
const AFFINITY_WINDOW: usize = 16;

/// A tile to render. `area` is the tile's part of the page, in page pixels at the key's scale.
#[derive(Clone, Copy, Debug)]
pub struct Job {
    pub key: TileKey,
    pub area: PixelRect,
}

/// A rendered tile. `pixels` is empty when the engine panicked on it.
pub struct Done {
    pub key: TileKey,
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

struct State {
    /// Most important first. Replaced as a whole on every frame.
    jobs: VecDeque<Job>,
    rendering: Vec<TileKey>,
    finished: Vec<Done>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// Render threads for one document. Tiles of all pages render in parallel, since hayro is
/// thread-safe; Sumatra renders one page of a document at a time.
pub struct RenderPool {
    shared: Arc<Shared>,
}

impl RenderPool {
    /// `notify` runs on a render thread each time a tile finishes.
    pub fn new(engine: Arc<dyn Engine>, threads: usize, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State { jobs: VecDeque::new(), rendering: Vec::new(), finished: Vec::new(), shutdown: false }),
            wake: Condvar::new(),
        });
        let notify = Arc::new(notify);
        for i in 0..threads {
            let (shared, engine, notify) = (shared.clone(), engine.clone(), notify.clone());
            thread::Builder::new()
                .name(format!("render {i}"))
                // reserved, not committed: deeply nested pages can recurse far
                .stack_size(8 << 20)
                .spawn(move || work(&shared, &*engine, &*notify))
                .expect("starting a render thread");
        }
        Self { shared }
    }

    /// The tiles finished since the last call.
    pub fn take_finished(&self) -> Vec<Done> {
        std::mem::take(&mut self.shared.state.lock().unwrap().finished)
    }

    /// Replaces the pending jobs, most important first. Jobs that are already rendering, or finished
    /// but not yet taken, are skipped so nothing renders twice.
    pub fn set_jobs(&self, jobs: Vec<Job>) {
        let mut s = self.shared.state.lock().unwrap();
        let jobs: VecDeque<Job> = jobs
            .into_iter()
            .filter(|j| !s.rendering.contains(&j.key) && !s.finished.iter().any(|d| d.key == j.key))
            .collect();
        let any = !jobs.is_empty();
        s.jobs = jobs;
        drop(s);
        if any {
            self.shared.wake.notify_all();
        }
    }
}

impl Drop for RenderPool {
    /// Threads finish the tile they're on and exit. Nothing waits for them.
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.wake.notify_all();
    }
}

/// The next job: a tile of `page` near the front of the queue if there is one, since this thread has
/// that page recorded, otherwise the front.
fn take_job(jobs: &mut VecDeque<Job>, page: Option<usize>) -> Option<Job> {
    let near = page.and_then(|p| jobs.iter().take(AFFINITY_WINDOW).position(|j| j.key.page == p));
    jobs.remove(near.unwrap_or(0))
}

fn work(shared: &Shared, engine: &dyn Engine, notify: &dyn Fn()) {
    let mut renderer: Option<Box<dyn PageRenderer + '_>> = None;
    let mut last_page = None;
    loop {
        let job = {
            let mut s = shared.state.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(job) = take_job(&mut s.jobs, last_page) {
                    s.rendering.push(job.key);
                    break job;
                }
                if renderer.is_none() {
                    s = shared.wake.wait(s).unwrap();
                    continue;
                }
                let (guard, wait) = shared.wake.wait_timeout(s, KEEP_IDLE).unwrap();
                s = guard;
                if wait.timed_out() && s.jobs.is_empty() {
                    // Idle a while: give back recorded pages, caches and scratch buffers.
                    renderer = None;
                    last_page = None;
                    if s.rendering.is_empty() {
                        drop(s);
                        engine.trim();
                        s = shared.state.lock().unwrap();
                    }
                }
            }
        };
        last_page = Some(job.key.page);
        let r = renderer.get_or_insert_with(|| engine.renderer());
        let scale = f32::from_bits(job.key.scale);
        let pixels = match catch_unwind(AssertUnwindSafe(|| render_turned(&mut **r, engine, job.key.page, scale, job.key.rotation, job.area))) {
            Ok(mut pixels) => {
                Look::from_u8(job.key.look).apply(&mut pixels);
                Pixels::encode(pixels, job.area.width)
            }
            Err(_) => {
                // its state can't be trusted after a panic
                renderer = None;
                Pixels::Raw(Vec::new())
            }
        };
        let mut s = shared.state.lock().unwrap();
        s.rendering.retain(|k| *k != job.key);
        s.finished.push(Done { key: job.key, width: job.area.width, height: job.area.height, pixels });
        drop(s);
        notify();
    }
}

/// Renders `area` of the page turned `rotation` quarter turns clockwise: the matching area of the
/// page as it is, then its pixels turned. The engine always draws upright, so its recordings and
/// glyph cache work the same at any rotation.
pub fn render_turned(r: &mut dyn PageRenderer, engine: &dyn Engine, page: usize, scale: f32, rotation: u8, area: PixelRect) -> Vec<u32> {
    if rotation % 4 == 0 {
        return r.render(page, scale, area);
    }
    // the page's size in pixels as it is, rounded as the layout rounds
    let size = engine.page_size(page);
    let px = |pt: f32| ((pt * scale).round() as u32).max(1);
    let (pw, ph) = (px(size.width), px(size.height));
    let (x, y, w, h) = (area.x, area.y, area.width, area.height);
    let upright = match rotation % 4 {
        1 => PixelRect { x: y, y: ph.saturating_sub(x + w), width: h, height: w },
        2 => PixelRect { x: pw.saturating_sub(x + w), y: ph.saturating_sub(y + h), width: w, height: h },
        _ => PixelRect { x: pw.saturating_sub(y + h), y: x, width: h, height: w },
    };
    turn(&r.render(page, scale, upright), w, h, rotation)
}

/// Turns pixels drawn upright into a `w` × `h` block turned `rotation` quarter turns clockwise.
fn turn(src: &[u32], w: u32, h: u32, rotation: u8) -> Vec<u32> {
    let (w, h) = (w as usize, h as usize);
    let mut out = Vec::with_capacity(w * h);
    for oy in 0..h {
        for ox in 0..w {
            out.push(match rotation % 4 {
                // the upright block is h wide here
                1 => src[(w - 1 - ox) * h + oy],
                2 => src[(h - 1 - oy) * w + (w - 1 - ox)],
                _ => src[ox * h + (h - 1 - oy)],
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::turn;

    #[test]
    fn turns_a_block() {
        // upright, 3 wide and 2 tall:  1 2 3 / 4 5 6
        let up = [1, 2, 3, 4, 5, 6];
        // a quarter turn clockwise is 2 wide and 3 tall: 4 1 / 5 2 / 6 3
        assert_eq!(turn(&up, 2, 3, 1), [4, 1, 5, 2, 6, 3]);
        assert_eq!(turn(&up, 3, 2, 2), [6, 5, 4, 3, 2, 1]);
        assert_eq!(turn(&up, 2, 3, 3), [3, 6, 2, 5, 1, 4]);
    }
}
