use crate::pixels::Pixels;
use std::cell::Cell;
use std::collections::HashMap;

/// One tile: a page, a zoom level, and a position in that page's grid of tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub page: usize,
    /// Pixels per point, as `f32::to_bits` so it can be hashed.
    pub scale: u32,
    pub col: u32,
    pub row: u32,
    /// How the page is colored (`Look`), so tiles of different looks never mix.
    pub look: u8,
    /// Quarter turns clockwise; `col` and `row` count in the turned page.
    pub rotation: u8,
}

pub struct Tile {
    pub width: u32,
    pub height: u32,
    /// Empty when the engine failed on this tile.
    pub pixels: Pixels,
    last_used: Cell<u64>,
}

/// Rendered tiles under a byte budget. Tiles drawn or wanted in the current frame are never evicted,
/// so a big window can push it over budget rather than make tiles render over and over.
pub struct TileCache {
    tiles: HashMap<TileKey, Tile>,
    bytes: usize,
    budget: usize,
    frame: u64,
}

impl Default for TileCache {
    fn default() -> Self {
        Self { tiles: HashMap::new(), bytes: 0, budget: 0, frame: 1 }
    }
}

impl TileCache {
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn insert(&mut self, key: TileKey, width: u32, height: u32, pixels: Pixels) {
        self.bytes += pixels.bytes();
        let tile = Tile { width, height, pixels, last_used: Cell::new(self.frame) };
        if let Some(old) = self.tiles.insert(key, tile) {
            self.bytes -= old.pixels.bytes();
        }
    }

    /// Looks up a tile and marks it used in this frame.
    pub fn get(&self, key: &TileKey) -> Option<&Tile> {
        let tile = self.tiles.get(key)?;
        self.mark_used(tile);
        Some(tile)
    }

    pub fn mark_used(&self, tile: &Tile) {
        tile.last_used.set(self.frame);
    }

    /// Cached tiles of `page` in `look` at scales other than `scale`, to stretch while sharp ones
    /// render. Not marked used: the caller marks the ones it draws.
    pub fn other_scales(&self, page: usize, scale: u32, look: u8, rotation: u8) -> impl Iterator<Item = (&TileKey, &Tile)> {
        self.tiles.iter().filter(move |(k, t)| {
            k.page == page && k.scale != scale && k.look == look && k.rotation == rotation && !t.pixels.is_empty()
        })
    }

    /// Drops the least recently used tiles until the cache fits its budget, sparing this frame's.
    pub fn evict(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut old: Vec<(u64, TileKey)> = self
            .tiles
            .iter()
            .filter(|(_, t)| t.last_used.get() < self.frame)
            .map(|(k, t)| (t.last_used.get(), *k))
            .collect();
        old.sort_unstable_by_key(|&(used, _)| used);
        for (_, key) in old {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(tile) = self.tiles.remove(&key) {
                self.bytes -= tile.pixels.bytes();
            }
        }
    }
}
