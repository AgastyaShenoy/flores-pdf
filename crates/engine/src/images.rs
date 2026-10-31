//! Comics and pictures: a CBZ, CBR or CB7 archive (a zip, RAR or 7-Zip of page images) or a single JPEG, PNG,
//! GIF or WebP picture, each image a page. Pages are decoded when first drawn and kept while they're in use; the render threads
//! drop them with everything else after a few idle seconds.
//!
//! The zip reader is our own and small: archives are read from their central directory, entries are
//! stored or deflated, and zip64 (over 4 GB) isn't read.

use crate::{Bytes, DocInfo, Engine, OpenError, PageRenderer, PageSize, PixelRect};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// UnRAR calls advapi32 (file security, random numbers), which the unrar crate doesn't link itself
#[cfg(windows)]
#[link(name = "advapi32")]
unsafe extern "system" {}

/// Images are measured as if at 96 dpi, as browsers and Sumatra do.
const POINTS_PER_PIXEL: f32 = 72.0 / 96.0;
/// Decoded pages kept at once: the one on screen and its neighbors.
const KEEP: usize = 3;

/// Where a page's encoded image is.
pub(crate) enum Source {
    /// The whole file is the image.
    File,
    /// A zip entry: where its data starts, its sizes, and whether it's deflated.
    Entry { offset: usize, size: usize, deflated: bool },
    /// A RAR entry, by name: UnRAR reads archives from their file, from the start.
    Rar(String),
    /// A 7-Zip entry, by name, read from the archive's file.
    SevenZip(String),
}

pub(crate) struct Decoded {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// 0x00RRGGBB, on white where the image was see-through.
    pub(crate) pixels: Vec<u32>,
}

pub(crate) struct ImageEngine {
    bytes: Arc<Bytes>,
    /// A RAR or 7-Zip archive's file; its pages are read from there rather than from `bytes`.
    rar: Option<PathBuf>,
    pages: Vec<(String, Source, PageSize)>,
    format: &'static str,
    /// Most recently used last.
    decoded: Mutex<Vec<(usize, Arc<Decoded>)>>,
}

impl ImageEngine {
    /// A single JPEG or PNG.
    pub(crate) fn image(bytes: Arc<Bytes>) -> Result<Self, OpenError> {
        let data = bytes.as_ref().as_ref();
        let format = match data {
            d if d.starts_with(b"\x89PNG") => "PNG image",
            d if d.starts_with(b"GIF8") => "GIF image",
            d if is_webp(d) => "WebP image",
            _ => "JPEG image",
        };
        let (w, h) = image_size(data).ok_or(OpenError::Damaged)?;
        let pages = vec![(String::new(), Source::File, size_of(w, h))];
        Ok(Self { bytes, rar: None, pages, format, decoded: Mutex::new(Vec::new()) })
    }

    /// A zip of page images, in the order of their names.
    pub(crate) fn comic(bytes: Arc<Bytes>) -> Result<Self, OpenError> {
        let data = bytes.as_ref().as_ref();
        let mut pages: Vec<(String, Source, PageSize)> = zip_entries(data)
            .ok_or(OpenError::Damaged)?
            .into_iter()
            .filter(|(name, _)| is_image_name(name) && !name.starts_with("__MACOSX/"))
            .filter_map(|(name, source)| {
                let encoded = read_entry(data, &source)?;
                let (w, h) = image_size(&encoded)?;
                Some((name, source, size_of(w, h)))
            })
            .collect();
        if pages.is_empty() {
            return Err(OpenError::Unsupported);
        }
        pages.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        Ok(Self { bytes, rar: None, pages, format: "CBZ comic", decoded: Mutex::new(Vec::new()) })
    }

    /// A RAR of page images. Opening reads each image once, for its size.
    pub(crate) fn rar(path: &Path) -> Result<Self, OpenError> {
        let mut archive = unrar::Archive::new(path).open_for_processing().map_err(|_| OpenError::Damaged)?;
        let mut pages = Vec::new();
        while let Some(header) = archive.read_header().map_err(|_| OpenError::Damaged)? {
            let name = header.entry().filename.to_string_lossy().replace('\\', "/");
            archive = if header.entry().is_file() && is_image_name(&name) {
                let (data, rest) = header.read().map_err(|_| OpenError::Damaged)?;
                if let Some((w, h)) = image_size(&data) {
                    pages.push((name.clone(), Source::Rar(name), size_of(w, h)));
                }
                rest
            } else {
                header.skip().map_err(|_| OpenError::Damaged)?
            };
        }
        if pages.is_empty() {
            return Err(OpenError::Unsupported);
        }
        pages.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        let bytes = Arc::new(Bytes::Read(Vec::new()));
        Ok(Self { bytes, rar: Some(path.to_owned()), pages, format: "CBR comic", decoded: Mutex::new(Vec::new()) })
    }

    /// A 7-Zip of page images. Opening reads each image once, for its size.
    pub(crate) fn seven_zip(path: &Path) -> Result<Self, OpenError> {
        let mut archive = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|_| OpenError::Damaged)?;
        let mut pages = Vec::new();
        archive
            .for_each_entries(|entry, reader| {
                let name = entry.name().replace('\\', "/");
                if !entry.is_directory() && is_image_name(&name) {
                    let mut data = Vec::new();
                    reader.read_to_end(&mut data)?;
                    if let Some((w, h)) = image_size(&data) {
                        pages.push((name.clone(), Source::SevenZip(entry.name().to_owned()), size_of(w, h)));
                    }
                }
                Ok(true)
            })
            .map_err(|_| OpenError::Damaged)?;
        if pages.is_empty() {
            return Err(OpenError::Unsupported);
        }
        pages.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        let bytes = Arc::new(Bytes::Read(Vec::new()));
        Ok(Self { bytes, rar: Some(path.to_owned()), pages, format: "CB7 comic", decoded: Mutex::new(Vec::new()) })
    }

    /// A page's encoded image: the whole file, a zip entry, or a RAR or 7-Zip entry.
    fn read_source(&self, source: &Source) -> Option<Vec<u8>> {
        match (source, &self.rar) {
            (Source::SevenZip(name), Some(path)) => {
                let mut archive = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).ok()?;
                archive.read_file(name).ok()
            }
            (Source::Rar(name), Some(path)) => {
                let mut archive = unrar::Archive::new(path).open_for_processing().ok()?;
                while let Some(header) = archive.read_header().ok()? {
                    let here = header.entry().filename.to_string_lossy().replace('\\', "/");
                    archive = if here == *name {
                        return header.read().ok().map(|(data, _)| data);
                    } else {
                        header.skip().ok()?
                    };
                }
                None
            }
            (source, _) => read_entry(self.bytes.as_ref().as_ref(), source),
        }
    }

    /// Page `page` decoded, from the cache if it's there.
    fn decoded(&self, page: usize) -> Option<Arc<Decoded>> {
        {
            let mut cache = self.decoded.lock().unwrap();
            if let Some(i) = cache.iter().position(|(p, _)| *p == page) {
                let hit = cache.remove(i);
                let image = hit.1.clone();
                cache.push(hit);
                return Some(image);
            }
        }
        let encoded = self.read_source(&self.pages.get(page)?.1)?;
        let image = Arc::new(decode(&encoded)?);
        let mut cache = self.decoded.lock().unwrap();
        cache.push((page, image.clone()));
        if cache.len() > KEEP {
            cache.remove(0);
        }
        Some(image)
    }
}

impl Engine for ImageEngine {
    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn page_size(&self, page: usize) -> PageSize {
        self.pages.get(page).map_or(PageSize { width: 1.0, height: 1.0 }, |p| p.2)
    }

    fn renderer(&self) -> Box<dyn PageRenderer + '_> {
        Box::new(ImageRenderer { engine: self })
    }

    fn trim(&self) {
        self.decoded.lock().unwrap().clear();
    }

    fn info(&self) -> DocInfo {
        DocInfo { format: self.format.to_owned(), ..DocInfo::default() }
    }
}

struct ImageRenderer<'a> {
    engine: &'a ImageEngine,
}

impl PageRenderer for ImageRenderer<'_> {
    /// Scales the page's image: averaging the pixels each one covers when shrinking, so text in a
    /// scan stays legible, and blending neighbors when enlarging.
    fn render(&mut self, page: usize, scale: f32, area: PixelRect) -> Vec<u32> {
        let mut out = vec![0x00FF_FFFF; area.width as usize * area.height as usize];
        let Some(img) = self.engine.decoded(page) else { return out };
        // image pixels per output pixel: an image pixel is POINTS_PER_PIXEL points, a point `scale`
        // output pixels
        let step = 1.0 / (scale * POINTS_PER_PIXEL);
        let (iw, ih) = (img.width as f32, img.height as f32);
        for oy in 0..area.height {
            let y0 = (area.y + oy) as f32 * step;
            for ox in 0..area.width {
                let x0 = (area.x + ox) as f32 * step;
                out[(oy * area.width + ox) as usize] = if step > 1.0 {
                    average(&img, x0, y0, (x0 + step).min(iw), (y0 + step).min(ih))
                } else {
                    blend(&img, x0 + step / 2.0 - 0.5, y0 + step / 2.0 - 0.5)
                };
            }
        }
        out
    }
}

/// The mean of the pixels in [x0, x1) × [y0, y1).
pub(crate) fn average(img: &Decoded, x0: f32, y0: f32, x1: f32, y1: f32) -> u32 {
    let (xa, ya) = (x0 as u32, y0 as u32);
    let (xb, yb) = ((x1.ceil() as u32).clamp(xa + 1, img.width), (y1.ceil() as u32).clamp(ya + 1, img.height));
    if xa >= img.width || ya >= img.height {
        return 0x00FF_FFFF;
    }
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for y in ya..yb {
        for &p in &img.pixels[(y * img.width + xa) as usize..(y * img.width + xb) as usize] {
            (r, g, b, n) = (r + (p >> 16 & 0xFF), g + (p >> 8 & 0xFF), b + (p & 0xFF), n + 1);
        }
    }
    (r / n) << 16 | (g / n) << 8 | b / n
}

/// The image at (x, y) between pixel centers, blended from the four around it.
pub(crate) fn blend(img: &Decoded, x: f32, y: f32) -> u32 {
    let (x, y) = (x.clamp(0.0, img.width as f32 - 1.0), y.clamp(0.0, img.height as f32 - 1.0));
    let (xa, ya) = (x as u32, y as u32);
    let (xb, yb) = ((xa + 1).min(img.width - 1), (ya + 1).min(img.height - 1));
    let (fx, fy) = (x - xa as f32, y - ya as f32);
    let at = |x: u32, y: u32| img.pixels[(y * img.width + x) as usize];
    let (p00, p10, p01, p11) = (at(xa, ya), at(xb, ya), at(xa, yb), at(xb, yb));
    let channel = |shift: u32| {
        let c = |p: u32| (p >> shift & 0xFF) as f32;
        let top = c(p00) + (c(p10) - c(p00)) * fx;
        let bottom = c(p01) + (c(p11) - c(p01)) * fx;
        ((top + (bottom - top) * fy).round() as u32).min(255)
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

fn size_of(w: u32, h: u32) -> PageSize {
    PageSize { width: w as f32 * POINTS_PER_PIXEL, height: h as f32 * POINTS_PER_PIXEL }
}

pub(crate) fn is_image(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG") || data.starts_with(&[0xFF, 0xD8, 0xFF]) || data.starts_with(b"GIF8") || is_webp(data)
}

fn is_webp(data: &[u8]) -> bool {
    data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP"
}

pub(crate) fn is_7z(data: &[u8]) -> bool {
    data.starts_with(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C])
}

pub(crate) fn is_rar(data: &[u8]) -> bool {
    data.starts_with(b"Rar!\x1A\x07")
}

fn is_image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".gif", ".webp"].iter().any(|ext| lower.ends_with(ext))
}

/// An image's width and height in pixels, from its header.
pub(crate) fn image_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.starts_with(b"\x89PNG") {
        let reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().ok()?;
        let info = reader.info();
        return Some((info.width, info.height));
    }
    if data.starts_with(b"GIF8") {
        // the logical screen's size, right after the signature
        let w = u16::from_le_bytes([*data.get(6)?, *data.get(7)?]);
        let h = u16::from_le_bytes([*data.get(8)?, *data.get(9)?]);
        return (w > 0 && h > 0).then_some((u32::from(w), u32::from(h)));
    }
    if is_webp(data) {
        return image_webp::WebPDecoder::new(std::io::Cursor::new(data)).ok().map(|d| d.dimensions());
    }
    let mut decoder = zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(data));
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    Some((u32::from(info.width), u32::from(info.height)))
}

/// A JPEG or PNG decoded to 0x00RRGGBB, see-through parts on white.
pub(crate) fn decode(data: &[u8]) -> Option<Decoded> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let (width, height, channels, bytes) = if data.starts_with(b"GIF8") {
        // the first frame, placed on the logical screen
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(std::io::Cursor::new(data)).ok()?;
        let (w, h) = (u32::from(decoder.width()), u32::from(decoder.height()));
        let mut screen = vec![255u8; w as usize * h as usize * 4];
        if let Some(frame) = decoder.read_next_frame().ok()? {
            let (fl, ft, fw) = (usize::from(frame.left), usize::from(frame.top), usize::from(frame.width));
            for (row, line) in frame.buffer.chunks_exact(fw * 4).enumerate() {
                let y = ft + row;
                if y >= h as usize || fl >= w as usize {
                    continue;
                }
                let n = (fw).min(w as usize - fl) * 4;
                let at = (y * w as usize + fl) * 4;
                screen[at..at + n].copy_from_slice(&line[..n]);
            }
        }
        (w, h, 4, screen)
    } else if is_webp(data) {
        let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).ok()?;
        let (w, h) = decoder.dimensions();
        let channels = if decoder.has_alpha() { 4 } else { 3 };
        let mut buf = vec![0; decoder.output_buffer_size()?];
        decoder.read_image(&mut buf).ok()?;
        (w, h, channels, buf)
    } else if data.starts_with(b"\x89PNG") {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        buf.truncate(info.buffer_size());
        (info.width, info.height, info.color_type.samples(), buf)
    } else {
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut decoder = zune_jpeg::JpegDecoder::new_with_options(zune_jpeg::zune_core::bytestream::ZCursor::new(data), options);
        let bytes = decoder.decode().ok()?;
        let info = decoder.info()?;
        let channels = decoder.output_colorspace()?.num_components();
        (u32::from(info.width), u32::from(info.height), channels, bytes)
    };
    let to_rgb = |p: &[u8]| -> u32 {
        let (r, g, b, a) = match p.len() {
            1 => (p[0], p[0], p[0], 255),
            2 => (p[0], p[0], p[0], p[1]),
            3 => (p[0], p[1], p[2], 255),
            _ => (p[0], p[1], p[2], p[3]),
        };
        // on white: c * a + 255 * (1 - a)
        let over = |c: u8| (u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a))) / 255;
        over(r) << 16 | over(g) << 8 | over(b)
    };
    let pixels: Vec<u32> = bytes.chunks_exact(channels.max(1)).map(to_rgb).collect();
    (pixels.len() == width as usize * height as usize).then_some(Decoded { width, height, pixels })
}

/// The entries of a zip archive, from its central directory: name and where the data is.
pub(crate) fn zip_entries(data: &[u8]) -> Option<Vec<(String, Source)>> {
    let u16_at = |i: usize| Some(u16::from_le_bytes(data.get(i..i + 2)?.try_into().ok()?) as usize);
    let u32_at = |i: usize| Some(u32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize);
    // the end record is in the last 64 KB, after a comment of up to that size
    let from = data.len().saturating_sub(22 + 65_535);
    let end = (from..data.len().saturating_sub(21)).rev().find(|&i| data[i..].starts_with(&[0x50, 0x4B, 0x05, 0x06]))?;
    let (count, mut at) = (u16_at(end + 10)?, u32_at(end + 16)?);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4B50 {
            return None;
        }
        let (method, size) = (u16_at(at + 10)?, u32_at(at + 20)?);
        let (name_len, extra_len, comment_len) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?);
        let local = u32_at(at + 42)?;
        let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + name_len)?).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        // the data follows the local header, whose name and extra field can differ in length
        if u32_at(local)? != 0x0403_4B50 || (method != 0 && method != 8) || name.ends_with('/') {
            continue;
        }
        let offset = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        entries.push((name, Source::Entry { offset, size, deflated: method == 8 }));
    }
    Some(entries)
}

/// A page's encoded image from the file's bytes: the whole file, or a zip entry, inflated if it
/// was deflated.
pub(crate) fn read_entry(data: &[u8], source: &Source) -> Option<Vec<u8>> {
    match source {
        Source::File => Some(data.to_vec()),
        Source::Rar(_) | Source::SevenZip(_) => None,
        Source::Entry { offset, size, deflated } => {
            let raw = data.get(*offset..offset.checked_add(*size)?)?;
            if *deflated { miniz_oxide::inflate::decompress_to_vec(raw).ok() } else { Some(raw.to_vec()) }
        }
    }
}

/// Compares names the way people number pages: "page2" before "page10".
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(d) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(d);
                        it.next();
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (m, n) = (number(&mut a), number(&mut b));
                let order = m.len().cmp(&n.len()).then(m.cmp(&n));
                if order.is_ne() {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if order.is_ne() {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;

    #[test]
    fn pages_sort_by_number() {
        let mut names = vec!["p10.jpg", "p2.jpg", "P1.jpg", "p02b.jpg", "cover.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["cover.jpg", "P1.jpg", "p2.jpg", "p02b.jpg", "p10.jpg"]);
    }
}
