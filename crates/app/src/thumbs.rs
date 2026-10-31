//! The start page's thumbnails: each recent file's first page, small. They're made on a thread of
//! their own, one file at a time, and kept on disk in `%LOCALAPPDATA%\FloresPDF\thumbnails` (as the
//! size, the file's modification time and the deflated pixels), so the start page shows them at once
//! next time. In memory only while the start page is shown.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

/// A first page, fitted into the box asked for: 0x00RRGGBB pixels.
#[derive(PartialEq, Debug)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

enum State {
    Waiting,
    Made(Arc<Thumb>),
    /// Gone, protected, or not readable: the start page shows a plain page instead.
    None,
}

#[derive(Default)]
struct Shared {
    made: HashMap<(PathBuf, u32, u32), State>,
    queue: VecDeque<(PathBuf, u32, u32)>,
    working: bool,
}

pub struct Thumbs {
    shared: Arc<Mutex<Shared>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Thumbs {
    /// `wake` is called from the thread each time a thumbnail is ready.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self { shared: Arc::default(), wake: Arc::new(wake) }
    }

    /// `path`'s thumbnail fitted into width × height pixels, if it's ready; otherwise it's made.
    pub fn get(&self, path: &Path, width: u32, height: u32) -> Option<Arc<Thumb>> {
        let key = (path.to_owned(), width, height);
        let mut shared = self.shared.lock().unwrap();
        match shared.made.get(&key) {
            Some(State::Made(t)) => return Some(t.clone()),
            Some(State::Waiting | State::None) => return None,
            None => {}
        }
        shared.made.insert(key.clone(), State::Waiting);
        shared.queue.push_back(key);
        if !shared.working {
            shared.working = true;
            let (shared, wake) = (self.shared.clone(), self.wake.clone());
            std::thread::spawn(move || work(&shared, &*wake));
        }
        None
    }

    /// Lets go of the thumbnails in memory, once the start page is gone. Ones being made are made.
    pub fn clear(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.made.retain(|_, s| matches!(s, State::Waiting));
    }
}

/// Makes the thumbnails asked for until there are none left, then ends.
fn work(shared: &Mutex<Shared>, wake: &(dyn Fn() + Send + Sync)) {
    loop {
        let Some((path, w, h)) = shared.lock().unwrap().queue.pop_front() else {
            shared.lock().unwrap().working = false;
            return;
        };
        let state = match load_or_make(&path, w, h) {
            Some(t) => State::Made(Arc::new(t)),
            None => State::None,
        };
        shared.lock().unwrap().made.insert((path, w, h), state);
        wake();
    }
}

fn load_or_make(path: &Path, w: u32, h: u32) -> Option<Thumb> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let cache = cache_path(path, w, h);
    if let Some(t) = cache.as_deref().and_then(|c| read(c, modified)) {
        return Some(t);
    }
    let t = make(path, w, h)?;
    if let Some(cache) = cache {
        write(&cache, modified, &t);
    }
    Some(t)
}

/// Draws the first page to fit w × h.
fn make(path: &Path, w: u32, h: u32) -> Option<Thumb> {
    let engine = engine::open(path).ok()?;
    if engine.page_count() == 0 {
        return None;
    }
    let size = engine.page_size(0);
    let scale = (w as f32 / size.width).min(h as f32 / size.height);
    let (width, height) = (((size.width * scale).round() as u32).clamp(1, w), ((size.height * scale).round() as u32).clamp(1, h));
    let pixels = engine.renderer().render(0, scale, engine::PixelRect { x: 0, y: 0, width, height });
    (pixels.len() == (width * height) as usize).then_some(Thumb { width, height, pixels })
}

fn cache_path(path: &Path, w: u32, h: u32) -> Option<PathBuf> {
    // FNV-1a of the path, case folded as Windows compares them, and the size
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().to_lowercase().bytes() {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    Some(crate::pdfium::cache_dir()?.join("thumbnails").join(format!("{hash:016x}-{w}x{h}.thumb")))
}

/// A cached thumbnail, if it's for the file as it is now.
fn read(cache: &Path, modified: u64) -> Option<Thumb> {
    let data = std::fs::read(cache).ok()?;
    let word = |i: usize| Some(u64::from_le_bytes(data.get(i..i + 8)?.try_into().ok()?));
    let (width, height, stamp) = (word(0)? as u32, word(8)? as u32, word(16)?);
    if stamp != modified {
        return None;
    }
    let bytes = miniz_oxide::inflate::decompress_to_vec(data.get(24..)?).ok()?;
    let pixels: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    (pixels.len() == (width * height) as usize).then_some(Thumb { width, height, pixels })
}

fn write(cache: &Path, modified: u64, t: &Thumb) {
    let bytes: Vec<u8> = t.pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
    let mut data = Vec::new();
    for word in [u64::from(t.width), u64::from(t.height), modified] {
        data.extend(word.to_le_bytes());
    }
    data.extend(miniz_oxide::deflate::compress_to_vec(&bytes, 6));
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(cache, data);
}
