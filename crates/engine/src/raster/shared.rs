//! Not from hayro: images shared between tiles and render threads, so a page's images are decoded
//! once and scaled once per zoom level instead of once per tile.

use hayro_interpret::{ImageData, LumaData};
use kurbo::Affine;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use vello_cpu::Pixmap;
use vello_cpu::peniko::ImageQuality;

/// Over this many bytes, the least recently used entries go. The whole cache is also dropped
/// when rendering has been idle for a while (`Engine::trim`).
const BUDGET: usize = 48 << 20;

/// An image decoded at full resolution while recording a page.
pub(crate) enum Decoded {
    Raster { data: ImageData, alpha: Option<LumaData> },
    Stencil(LumaData),
}

impl Decoded {
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Decoded::Raster { data, alpha } => {
                let d = match data {
                    ImageData::Rgb(d) => d.data.len(),
                    ImageData::Luma(d) => d.data.len(),
                };
                d + alpha.as_ref().map_or(0, |a| a.data.len())
            }
            Decoded::Stencil(s) => s.data.len(),
        }
    }
}

/// An image converted and scaled for one zoom level, ready to draw.
#[derive(Clone)]
pub(crate) struct Prepared {
    pub(crate) pixmap: Arc<Pixmap>,
    pub(crate) quality: ImageQuality,
    /// Maps the pixmap's pixels to the image's own pixel grid.
    pub(crate) transform: Affine,
}

/// An image at one size. `color` tells stencil masks apart by the color they're painted with.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PixKey {
    pub(crate) image: u128,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color: [u8; 4],
}

#[derive(Default)]
pub(crate) struct ImageCache(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    decoded: HashMap<u128, (Arc<Decoded>, u64)>,
    prepared: HashMap<PixKey, (Prepared, u64)>,
    bytes: usize,
    clock: u64,
}

impl ImageCache {
    pub(crate) fn decoded(&self, key: u128) -> Option<Arc<Decoded>> {
        let mut inner = self.0.lock().unwrap();
        inner.clock += 1;
        let now = inner.clock;
        inner.decoded.get_mut(&key).map(|(d, used)| {
            *used = now;
            d.clone()
        })
    }

    pub(crate) fn put_decoded(&self, key: u128, decoded: Arc<Decoded>) {
        let mut inner = self.0.lock().unwrap();
        inner.clock += 1;
        inner.bytes += decoded.bytes();
        let now = inner.clock;
        if let Some((old, _)) = inner.decoded.insert(key, (decoded, now)) {
            inner.bytes -= old.bytes();
        }
        inner.evict();
    }

    pub(crate) fn prepared(&self, key: &PixKey) -> Option<Prepared> {
        let mut inner = self.0.lock().unwrap();
        inner.clock += 1;
        let now = inner.clock;
        inner.prepared.get_mut(key).map(|(p, used)| {
            *used = now;
            p.clone()
        })
    }

    pub(crate) fn put_prepared(&self, key: PixKey, prepared: Prepared) {
        let mut inner = self.0.lock().unwrap();
        inner.clock += 1;
        inner.bytes += pixmap_bytes(&prepared);
        let now = inner.clock;
        if let Some((old, _)) = inner.prepared.insert(key, (prepared, now)) {
            inner.bytes -= pixmap_bytes(&old);
        }
        inner.evict();
    }

    pub(crate) fn clear(&self) {
        *self.0.lock().unwrap() = Inner::default();
    }
}

impl Inner {
    fn evict(&mut self) {
        while self.bytes > BUDGET {
            let oldest_decoded = self.decoded.iter().map(|(k, (_, t))| (*t, *k)).min_by_key(|(t, _)| *t);
            let oldest_prepared = self.prepared.iter().map(|(k, (_, t))| (*t, *k)).min_by_key(|(t, _)| *t);
            match (oldest_decoded, oldest_prepared) {
                (Some((td, kd)), Some((tp, _))) if td <= tp => self.drop_decoded(kd),
                (_, Some((_, kp))) => {
                    let (p, _) = self.prepared.remove(&kp).unwrap();
                    self.bytes -= pixmap_bytes(&p);
                }
                (Some((_, kd)), None) => self.drop_decoded(kd),
                (None, None) => break,
            }
        }
    }

    fn drop_decoded(&mut self, key: u128) {
        let (d, _) = self.decoded.remove(&key).unwrap();
        self.bytes -= d.bytes();
    }
}

fn pixmap_bytes(p: &Prepared) -> usize {
    usize::from(p.pixmap.width()) * usize::from(p.pixmap.height()) * 4
}
