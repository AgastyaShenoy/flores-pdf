/// A token word with this bit set is a run of one color; without it, a span of literal pixels.
const RUN: u32 = 1 << 31;
/// Shorter runs are cheaper to store as literals.
const MIN_RUN: usize = 4;

/// A tile's 0x00RRGGBB pixels, run-length encoded when that saves at least a quarter. Text pages
/// are mostly runs of white, so they shrink several times; scans and photos stay raw.
pub enum Pixels {
    Raw(Vec<u32>),
    /// Each row is a list of tokens: a run word (`RUN | n`) followed by the color, or a literal word
    /// (`n`) followed by n pixels. `rows[y]` is where row y starts in `data`.
    Rle { data: Vec<u32>, rows: Vec<u32> },
}

impl Pixels {
    pub fn encode(pixels: Vec<u32>, width: u32) -> Self {
        let w = width as usize;
        if w == 0 || pixels.is_empty() {
            return Pixels::Raw(pixels);
        }
        let limit = pixels.len() * 3 / 4;
        let mut data = Vec::new();
        let mut rows = Vec::with_capacity(pixels.len() / w);
        for row in pixels.chunks_exact(w) {
            rows.push(data.len() as u32);
            let (mut literal, mut i) = (0, 0);
            while i < w {
                let c = row[i];
                let mut j = i + 1;
                while j < w && row[j] == c {
                    j += 1;
                }
                if j - i >= MIN_RUN {
                    if literal < i {
                        data.push((i - literal) as u32);
                        data.extend_from_slice(&row[literal..i]);
                    }
                    data.extend_from_slice(&[RUN | (j - i) as u32, c]);
                    literal = j;
                }
                i = j;
            }
            if literal < w {
                data.push((w - literal) as u32);
                data.extend_from_slice(&row[literal..]);
            }
            if data.len() + rows.len() > limit {
                return Pixels::Raw(pixels);
            }
        }
        data.shrink_to_fit();
        Pixels::Rle { data, rows }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Pixels::Raw(p) => p.is_empty(),
            Pixels::Rle { rows, .. } => rows.is_empty(),
        }
    }

    pub fn bytes(&self) -> usize {
        4 * match self {
            Pixels::Raw(p) => p.len(),
            Pixels::Rle { data, rows } => data.len() + rows.len(),
        }
    }

    /// Writes pixels `x..x + out.len()` of row `y` into `out`.
    pub fn read_row(&self, width: u32, y: u32, x: u32, out: &mut [u32]) {
        let x = x as usize;
        match self {
            Pixels::Raw(p) => out.copy_from_slice(&p[y as usize * width as usize + x..][..out.len()]),
            Pixels::Rle { data, rows } => {
                let end = x + out.len();
                let (mut i, mut pos) = (rows[y as usize] as usize, 0);
                while pos < end {
                    let n = (data[i] & !RUN) as usize;
                    let (s, e) = (pos.max(x), (pos + n).min(end));
                    if data[i] & RUN != 0 {
                        if s < e {
                            out[s - x..e - x].fill(data[i + 1]);
                        }
                        i += 2;
                    } else {
                        if s < e {
                            out[s - x..e - x].copy_from_slice(&data[i + 1 + s - pos..][..e - s]);
                        }
                        i += 1 + n;
                    }
                    pos += n;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let w = 37u32;
        let mut px = vec![0x00FF_FFFFu32; (w * 5) as usize];
        // a "text" row, a gradient row that won't compress, and some short runs
        for x in 0..w as usize {
            px[w as usize + x] = if x % 7 < 2 { 0 } else { 0x00FF_FFFF };
            px[2 * w as usize + x] = x as u32 * 1000;
            px[3 * w as usize + x] = (x / 3) as u32;
        }
        let enc = Pixels::encode(px.clone(), w);
        assert!(matches!(enc, Pixels::Rle { .. }));
        for y in 0..5 {
            for (x, len) in [(0, w), (5, 10), (30, 7), (36, 1)] {
                let mut out = vec![0; len as usize];
                enc.read_row(w, y, x, &mut out);
                assert_eq!(out, px[(y * w + x) as usize..][..len as usize], "row {y} from {x}");
            }
        }
    }

    #[test]
    fn noise_stays_raw() {
        let px: Vec<u32> = (0..64 * 64u32).map(|i| i.wrapping_mul(2_654_435_761) & 0xFF_FFFF).collect();
        assert!(matches!(Pixels::encode(px, 64), Pixels::Raw(_)));
    }
}
