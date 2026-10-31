use crate::cache::{TileCache, TileKey};
use crate::layout::{Layout, Viewport};
use crate::look::Look;
use crate::pool::Job;
use std::collections::HashSet;

/// Tiles to render for `view`, most important first: the visible ones nearest the center of the
/// view, then the next screen in the direction of scrolling (`ahead` is 1 for down, -1 for up).
/// Cached tiles are left out and marked used, so a prefetched tile isn't evicted before it's needed.
pub fn wanted_tiles(layout: &Layout, view: &Viewport, ahead: i64, look: Look, cache: &TileCache) -> Vec<Job> {
    let next = Viewport { y: view.y + ahead * i64::from(view.height), ..*view };
    let center = (view.x + i64::from(view.width / 2), view.y + i64::from(view.height / 2));
    let scale = layout.scale.to_bits();
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for region in [view, &next] {
        let start = jobs.len();
        for page in layout.pages_between(region.y, region.y + i64::from(region.height)) {
            for (col, row, area) in layout.tiles(page, region) {
                let key = TileKey { page, scale, col, row, look: look as u8, rotation: layout.rotation };
                if seen.insert(key) && cache.get(&key).is_none() {
                    jobs.push(Job { key, area });
                }
            }
        }
        jobs[start..].sort_by_key(|j| {
            let p = layout.pages[j.key.page];
            let dx = p.x + i64::from(j.area.x + j.area.width / 2) - center.0;
            let dy = p.y + i64::from(j.area.y + j.area.height / 2) - center.1;
            dx * dx + dy * dy
        });
    }
    jobs
}
