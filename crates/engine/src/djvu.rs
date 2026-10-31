//! DjVu documents, scanned books mostly, through djvu-rs (pure Rust). Pages are drawn for any area
//! at any size, like PDF pages, bookmarks become the outline, and a page's hidden text layer (from
//! OCR) is its text for search and selection.

use crate::{DocInfo, Engine, OpenError, OutlineItem, PageRenderer, PageSize, PageText, PixelRect, Target, TextChar};
use djvu_rs::text::{TextZone, TextZoneKind};
use djvu_rs::djvu_render::{RenderRect, RenderRequest};

pub(crate) struct DjvuEngine {
    doc: djvu_rs::Document,
    sizes: Vec<PageSize>,
}

pub(crate) fn is_djvu(data: &[u8]) -> bool {
    data.starts_with(b"AT&TFORM")
}

impl DjvuEngine {
    pub(crate) fn open(data: Vec<u8>) -> Result<Self, OpenError> {
        let doc = djvu_rs::Document::from_bytes(data).map_err(|_| OpenError::Damaged)?;
        let sizes = (0..doc.page_count())
            .map(|i| match doc.page(i) {
                // points are 1/72 inch; a page knows its resolution
                Ok(p) => {
                    let dpi = f32::from(p.dpi().max(1));
                    PageSize { width: p.display_width() as f32 * 72.0 / dpi, height: p.display_height() as f32 * 72.0 / dpi }
                }
                Err(_) => PageSize { width: 612.0, height: 792.0 },
            })
            .collect();
        Ok(Self { doc, sizes })
    }
}

impl Engine for DjvuEngine {
    fn page_count(&self) -> usize {
        self.sizes.len()
    }

    fn page_size(&self, page: usize) -> PageSize {
        self.sizes.get(page).copied().unwrap_or(PageSize { width: 1.0, height: 1.0 })
    }

    fn renderer(&self) -> Box<dyn PageRenderer + '_> {
        Box::new(DjvuRenderer { engine: self })
    }

    fn outline(&self) -> Vec<OutlineItem> {
        fn item(b: &djvu_rs::djvu_document::DjVuBookmark) -> OutlineItem {
            // internal targets look like "#12", a page number from 1
            let page = b.url.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()).filter(|&n| n > 0);
            OutlineItem {
                title: b.title.clone(),
                target: page.map(|n| Target::Page { page: n - 1, y: None }),
                open: false,
                children: b.children.iter().map(item).collect(),
            }
        }
        self.doc.bookmarks().map(|list| list.iter().map(item).collect()).unwrap_or_default()
    }

    fn info(&self) -> DocInfo {
        DocInfo { format: "DjVu".to_owned(), ..DocInfo::default() }
    }

    /// The text layer, word by word: each word's letters share its box evenly, words of a line
    /// are joined by spaces, and lines end in a line break.
    fn page_text(&self, page: usize) -> PageText {
        let Ok(p) = self.doc.page(page) else { return PageText::default() };
        let Ok(Some(layer)) = p.text_layer() else { return PageText::default() };
        let k = 72.0 / f32::from(p.dpi().max(1));
        let mut lines = Vec::new();
        for zone in &layer.zones {
            collect_lines(zone, &mut lines);
        }
        let mut chars = Vec::new();
        for words in lines {
            for (i, w) in words.iter().enumerate() {
                let n = w.text.chars().count().max(1) as f32;
                let (x, y, width, height) = (w.rect.x as f32, w.rect.y as f32, w.rect.width as f32, w.rect.height as f32);
                for (j, ch) in w.text.chars().enumerate() {
                    let x0 = x + width * j as f32 / n;
                    chars.push(TextChar { ch, rect: [x0 * k, y * k, (x0 + width / n) * k, (y + height) * k] });
                }
                let end = (x + width) * k;
                let gap = match words.get(i + 1) {
                    Some(next) => [end, y * k, next.rect.x as f32 * k, (y + height) * k],
                    // the line break, an empty box at the line's end
                    None => [end, y * k, end, y * k],
                };
                chars.push(TextChar { ch: if i + 1 < words.len() { ' ' } else { '\n' }, rect: gap });
            }
        }
        PageText { chars }
    }
}

/// The page's lines, each as its words; a line with no word zones is one word.
fn collect_lines<'a>(zone: &'a TextZone, out: &mut Vec<Vec<&'a TextZone>>) {
    fn words<'a>(zone: &'a TextZone, out: &mut Vec<&'a TextZone>) {
        match zone.kind {
            TextZoneKind::Word => out.push(zone),
            _ => zone.children.iter().for_each(|c| words(c, out)),
        }
    }
    match zone.kind {
        TextZoneKind::Line => {
            let mut line = Vec::new();
            words(zone, &mut line);
            if line.is_empty() {
                line.push(zone);
            }
            out.push(line.into_iter().filter(|w| !w.text.trim().is_empty()).collect());
        }
        TextZoneKind::Word => out.push(vec![zone]),
        _ => zone.children.iter().for_each(|c| collect_lines(c, out)),
    }
}

struct DjvuRenderer<'a> {
    engine: &'a DjvuEngine,
}

impl PageRenderer for DjvuRenderer<'_> {
    fn render(&mut self, page: usize, scale: f32, area: PixelRect) -> Vec<u32> {
        let blank = || vec![0x00FF_FFFF; area.width as usize * area.height as usize];
        let (Ok(p), Some(size)) = (self.engine.doc.page(page), self.engine.sizes.get(page)) else { return blank() };
        // the whole page at this scale, rounded as the layout rounds, and the part asked for
        let px = |pt: f32| ((pt * scale).round() as u32).max(1);
        let options = p.options_for_size(px(size.width), px(size.height));
        let region = RenderRect { x: area.x, y: area.y, width: area.width, height: area.height };
        let Ok(pixmap) = p.render_request(&RenderRequest::new(options).region(region)) else { return blank() };
        if pixmap.width != area.width || pixmap.height != area.height {
            return blank();
        }
        // RGBA, on white where it's see-through
        pixmap
            .data
            .chunks_exact(4)
            .map(|c| {
                let a = u32::from(c[3]);
                let over = |v: u8| (u32::from(v) * a + 255 * (255 - a)) / 255;
                over(c[0]) << 16 | over(c[1]) << 8 | over(c[2])
            })
            .collect()
    }
}
