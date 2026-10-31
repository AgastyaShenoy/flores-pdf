//! EPUB e-books, laid out into pages once when the book opens: 6 × 9 inch pages, Windows' Georgia,
//! justified paragraphs, headings, lists, quotes, preformatted text, pictures, and chapters starting
//! on new pages. The book's CSS isn't applied; its structure is. The table of contents becomes the
//! outline, links work, and since every character's place is known, search and selection do too.
//!
//! Only what's needed to draw is kept per page (lines as text runs with their positions), so a long
//! book costs a few megabytes. Characters Georgia doesn't have come from Segoe UI; there's no shaping
//! beyond one glyph per character, which suits Latin, Greek and Cyrillic text.

use crate::images::{self, Decoded, Source};
use crate::{Bytes, DocInfo, Engine, Link, OpenError, OutlineItem, PageRenderer, PageSize, PageText, PixelRect, Target, TextChar};
use kurbo::{Affine, BezPath, Rect};
use memmap2::Mmap;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use vello_cpu::color::AlphaColor;
use vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};

/// The page: 6 × 9 inches, with margins.
const PAGE_W: f32 = 432.0;
const PAGE_H: f32 = 648.0;
const MARGIN_X: f32 = 54.0;
const MARGIN_TOP: f32 = 54.0;
const MARGIN_BOTTOM: f32 = 60.0;
const BODY: f32 = 11.5;
const LEADING: f32 = 1.35;

/// Faces: regular, bold, italic, bold italic, monospace.
const REGULAR: u8 = 0;
const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const MONO: u8 = 4;

pub(crate) fn is_epub(data: &[u8]) -> bool {
    // the first entry is "mimetype", stored, saying application/epub+zip
    data.len() > 60 && data[30..38] == *b"mimetype" && data[38..80.min(data.len())].windows(20).any(|w| w == b"application/epub+zip")
}

/// A run of text in one face, from `x` along a line.
struct Run {
    face: u8,
    x: f32,
    text: String,
}

struct Line {
    baseline: f32,
    size: f32,
    /// Added to each space, to justify the line.
    extra: f32,
    runs: Vec<Run>,
}

#[derive(Default)]
struct Page {
    lines: Vec<Line>,
    /// Pictures: where, and the file in the book.
    images: Vec<(Rect, String)>,
    rules: Vec<Rect>,
    /// Link areas and where they go, resolved after layout.
    links: Vec<(Rect, String)>,
}

pub(crate) struct EpubEngine {
    bytes: Arc<Bytes>,
    entries: HashMap<String, Source>,
    fonts: Fonts,
    pages: Vec<Page>,
    /// `file#id` and `file` to where they are: page and points down it.
    anchors: HashMap<String, (usize, f32)>,
    toc: Vec<OutlineItem>,
    title: Option<String>,
    author: Option<String>,
    decoded: Mutex<Vec<(String, Arc<Decoded>)>>,
}

impl EpubEngine {
    pub(crate) fn open(bytes: Arc<Bytes>) -> Result<Self, OpenError> {
        let data = bytes.as_ref().as_ref();
        let entries: HashMap<String, Source> = images::zip_entries(data).ok_or(OpenError::Damaged)?.into_iter().collect();
        let read = |name: &str| entries.get(name).and_then(|s| images::read_entry(data, s)).map(|b| String::from_utf8_lossy(&b).into_owned());
        // the container names the package file, which lists the files and their order
        let container = read("META-INF/container.xml").ok_or(OpenError::Damaged)?;
        let opf_path = tokens(&container)
            .iter()
            .find_map(|t| match t {
                Token::Open(name, attrs, _) if local(name) == "rootfile" => attr(attrs, "full-path"),
                _ => None,
            })
            .ok_or(OpenError::Damaged)?;
        let opf = read(&opf_path).ok_or(OpenError::Damaged)?;
        let base = parent(&opf_path);
        let mut manifest = HashMap::new();
        let (mut spine, mut nav, mut ncx) = (Vec::new(), None, None);
        let (mut title, mut author, mut in_tag) = (None, None, "");
        for t in tokens(&opf) {
            match t {
                Token::Open(name, attrs, _) => match local(&name) {
                    "item" => {
                        let (Some(id), Some(href)) = (attr(&attrs, "id"), attr(&attrs, "href")) else { continue };
                        let href = join(&base, &href);
                        if attr(&attrs, "properties").is_some_and(|p| p.split(' ').any(|p| p == "nav")) {
                            nav = Some(href.clone());
                        }
                        if attr(&attrs, "media-type").is_some_and(|m| m == "application/x-dtbncx+xml") {
                            ncx = Some(href.clone());
                        }
                        manifest.insert(id, href);
                    }
                    "itemref" => spine.extend(attr(&attrs, "idref")),
                    "title" => in_tag = "title",
                    "creator" => in_tag = "creator",
                    _ => {}
                },
                Token::Text(text) if !text.trim().is_empty() => match std::mem::take(&mut in_tag) {
                    "title" => title = title.or(Some(text.trim().to_owned())),
                    "creator" => author = author.or(Some(text.trim().to_owned())),
                    _ => {}
                },
                Token::Close(_) => in_tag = "",
                _ => {}
            }
        }
        let fonts = Fonts::load().ok_or(OpenError::Unsupported)?;
        let mut layout = Layout { fonts: &fonts, pages: vec![Page::default()], y: MARGIN_TOP, anchors: HashMap::new(), entries: &entries, data };
        for id in &spine {
            let Some(href) = manifest.get(id) else { continue };
            let Some(xhtml) = read(href) else { continue };
            layout.chapter(href, &xhtml);
        }
        let (mut pages, anchors) = (layout.pages, layout.anchors);
        if pages.last().is_some_and(|p| p.lines.is_empty() && p.images.is_empty()) && pages.len() > 1 {
            pages.pop();
        }
        let toc = match (nav, ncx) {
            (Some(nav), _) => read(&nav).map(|x| nav_toc(&x, &parent(&nav), &anchors)).unwrap_or_default(),
            (None, Some(ncx)) => read(&ncx).map(|x| ncx_toc(&x, &parent(&ncx), &anchors)).unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(Self { bytes, entries, fonts, pages, anchors, toc, title, author, decoded: Mutex::new(Vec::new()) })
    }

    fn image(&self, name: &str) -> Option<Arc<Decoded>> {
        let mut cache = self.decoded.lock().unwrap();
        if let Some(i) = cache.iter().position(|(n, _)| n == name) {
            return Some(cache[i].1.clone());
        }
        let data = images::read_entry(self.bytes.as_ref().as_ref(), self.entries.get(name)?)?;
        let image = Arc::new(images::decode(&data)?);
        cache.push((name.to_owned(), image.clone()));
        if cache.len() > 4 {
            cache.remove(0);
        }
        Some(image)
    }

    fn target(&self, href: &str) -> Option<Target> {
        if href.contains("://") || href.starts_with("mailto:") {
            return Some(Target::Uri(href.to_owned()));
        }
        let &(page, y) = self.anchors.get(href).or_else(|| self.anchors.get(href.split('#').next()?))?;
        Some(Target::Page { page, y: Some(y) })
    }
}

impl Engine for EpubEngine {
    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn page_size(&self, _page: usize) -> PageSize {
        PageSize { width: PAGE_W, height: PAGE_H }
    }

    fn renderer(&self) -> Box<dyn PageRenderer + '_> {
        Box::new(EpubRenderer { engine: self, outlines: HashMap::new() })
    }

    fn trim(&self) {
        self.decoded.lock().unwrap().clear();
    }

    fn page_text(&self, page: usize) -> PageText {
        let Some(p) = self.pages.get(page) else { return PageText::default() };
        let mut chars = Vec::new();
        for line in &p.lines {
            let (top, bottom) = (line.baseline - line.size * 0.9, line.baseline + line.size * 0.25);
            for run in &line.runs {
                let mut x = run.x;
                // a space between runs only where the line has a gap; the runs' own spaces are in them
                if chars.last().is_some_and(|c: &TextChar| c.ch != '\n' && c.ch != ' ' && run.x > c.rect[2] + 0.5) {
                    chars.push(TextChar { ch: ' ', rect: [x, top, x, bottom] });
                }
                for ch in run.text.chars() {
                    let w = self.fonts.advance(run.face, ch, line.size) + if ch == ' ' { line.extra } else { 0.0 };
                    chars.push(TextChar { ch, rect: [x, top, x + w, bottom] });
                    x += w;
                }
            }
            let end = chars.last().map_or(MARGIN_X, |c| c.rect[2]);
            chars.push(TextChar { ch: '\n', rect: [end, top, end, top] });
        }
        PageText { chars }
    }

    fn page_links(&self, page: usize) -> Vec<Link> {
        let Some(p) = self.pages.get(page) else { return Vec::new() };
        p.links
            .iter()
            .filter_map(|(r, href)| Some(Link { rect: [r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32], target: self.target(href)? }))
            .collect()
    }

    fn outline(&self) -> Vec<OutlineItem> {
        self.toc.clone()
    }

    fn info(&self) -> DocInfo {
        DocInfo { format: "EPUB".to_owned(), title: self.title.clone(), author: self.author.clone(), ..DocInfo::default() }
    }
}

/// Lays chapters out into pages.
struct Layout<'a> {
    fonts: &'a Fonts,
    pages: Vec<Page>,
    /// Where the next line goes on the last page, from its top.
    y: f32,
    anchors: HashMap<String, (usize, f32)>,
    entries: &'a HashMap<String, Source>,
    data: &'a [u8],
}

/// Text waiting to be laid out as a paragraph: words with their face and link, and whether a
/// space follows.
#[derive(Default)]
struct Para {
    words: Vec<(String, u8, Option<String>, bool)>,
}

/// How the paragraph being collected is set.
#[derive(Clone, Copy)]
struct Block {
    size: f32,
    indent: f32,
    pre: bool,
    justify: bool,
    after: f32,
}

impl Layout<'_> {
    fn chapter(&mut self, file: &str, xhtml: &str) {
        let page = self.pages.last().unwrap();
        if !page.lines.is_empty() || !page.images.is_empty() {
            self.new_page();
        }
        self.anchors.insert(file.to_owned(), (self.pages.len() - 1, 0.0));
        let dir = parent(file);
        let mut para = Para::default();
        let mut block = Block { size: BODY, indent: 0.0, pre: false, justify: true, after: BODY * 0.6 };
        // the open elements, for faces and nesting
        let (mut bold, mut italic, mut mono, mut skip, mut quote) = (0, 0, 0, 0, 0);
        let mut lists: Vec<Option<usize>> = Vec::new();
        let mut link: Option<String> = None;
        let mut in_body = !xhtml.contains("<body");
        for t in tokens(xhtml) {
            match t {
                Token::Open(name, attrs, empty) => {
                    let name = local(&name).to_ascii_lowercase();
                    if let Some(id) = attr(&attrs, "id") {
                        // a link here lands on the line about to be laid out
                        self.flush(&mut para, block);
                        self.anchors.insert(format!("{file}#{id}"), (self.pages.len() - 1, self.y));
                    }
                    match name.as_str() {
                        "body" => in_body = true,
                        "head" | "script" | "style" | "title" if !empty => skip += 1,
                        "b" | "strong" | "th" if !empty => bold += 1,
                        "i" | "em" | "cite" | "var" if !empty => italic += 1,
                        "code" | "tt" | "kbd" | "samp" if !empty => mono += 1,
                        "pre" if !empty => {
                            self.flush(&mut para, block);
                            mono += 1;
                            block = Block { size: BODY * 0.85, pre: true, justify: false, ..block };
                        }
                        "a" if !empty => link = attr(&attrs, "href").map(|h| resolve(&dir, &h)),
                        "br" => {
                            self.flush(&mut para, Block { after: 0.0, ..block });
                        }
                        "hr" => {
                            self.flush(&mut para, block);
                            self.rule();
                        }
                        "img" | "image" => {
                            self.flush(&mut para, block);
                            let src = attr(&attrs, "src").or_else(|| attr(&attrs, "xlink:href")).or_else(|| attr(&attrs, "href"));
                            if let Some(src) = src {
                                self.picture(&resolve(&dir, &src));
                            }
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !empty => {
                            self.flush(&mut para, block);
                            let size = match name.as_str() {
                                "h1" => 21.0,
                                "h2" => 17.0,
                                "h3" => 14.5,
                                _ => 12.5,
                            };
                            self.space(size * 0.8);
                            bold += 1;
                            block = Block { size, justify: false, after: size * 0.5, ..block };
                        }
                        "ul" | "ol" if !empty => {
                            self.flush(&mut para, block);
                            lists.push((name == "ol").then_some(0));
                            block.indent = 20.0 * lists.len() as f32 + 24.0 * quote as f32;
                        }
                        "li" if !empty => {
                            self.flush(&mut para, block);
                            let marker = match lists.last_mut() {
                                Some(Some(n)) => {
                                    *n += 1;
                                    format!("{n}.")
                                }
                                _ => "\u{2022}".to_owned(),
                            };
                            para.words.push((marker, REGULAR, None, true));
                        }
                        "blockquote" if !empty => {
                            self.flush(&mut para, block);
                            quote += 1;
                            block.indent = 20.0 * lists.len() as f32 + 24.0 * quote as f32;
                        }
                        "p" | "div" | "section" | "article" | "tr" | "dt" | "dd" | "figcaption" | "caption" | "aside" | "header" | "footer" => {
                            self.flush(&mut para, block);
                        }
                        "td" => para.words.push((String::new(), REGULAR, None, true)),
                        _ => {}
                    }
                }
                Token::Close(name) => {
                    let name = local(&name).to_ascii_lowercase();
                    match name.as_str() {
                        "head" | "script" | "style" | "title" => skip = (skip - 1).max(0),
                        "b" | "strong" | "th" => bold = (bold - 1).max(0),
                        "i" | "em" | "cite" | "var" => italic = (italic - 1).max(0),
                        "code" | "tt" | "kbd" | "samp" => mono = (mono - 1).max(0),
                        "a" => link = None,
                        "pre" => {
                            self.flush(&mut para, block);
                            mono = (mono - 1).max(0);
                            block = Block { size: BODY, pre: false, justify: true, ..block };
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            self.flush(&mut para, block);
                            bold = (bold - 1).max(0);
                            block = Block { size: BODY, justify: true, after: BODY * 0.6, ..block };
                        }
                        "ul" | "ol" => {
                            self.flush(&mut para, block);
                            lists.pop();
                            block.indent = 20.0 * lists.len() as f32 + 24.0 * quote as f32;
                        }
                        "blockquote" => {
                            self.flush(&mut para, block);
                            quote = (quote - 1).max(0);
                            block.indent = 20.0 * lists.len() as f32 + 24.0 * quote as f32;
                        }
                        "p" | "div" | "li" | "section" | "article" | "tr" | "dt" | "dd" | "figcaption" | "caption" | "aside" | "header"
                        | "footer" => self.flush(&mut para, block),
                        _ => {}
                    }
                }
                Token::Text(text) if skip == 0 && in_body => {
                    let face = if mono > 0 { MONO } else { (if bold > 0 { BOLD } else { REGULAR }) | (if italic > 0 { ITALIC } else { 0 }) };
                    if block.pre {
                        // keep spaces; line breaks become breaks
                        for (i, line) in text.split('\n').enumerate() {
                            if i > 0 {
                                para.words.push(("\n".to_owned(), face, None, false));
                            }
                            para.words.push((line.replace('\t', "    "), face, link.clone(), false));
                        }
                        continue;
                    }
                    let space_before = text.starts_with(char::is_whitespace);
                    if space_before && let Some(last) = para.words.last_mut() {
                        last.3 = true;
                    }
                    let words: Vec<&str> = text.split_whitespace().collect();
                    let space_after = text.ends_with(char::is_whitespace);
                    for (i, w) in words.iter().enumerate() {
                        let space = i + 1 < words.len() || space_after;
                        para.words.push(((*w).to_owned(), face, link.clone(), space));
                    }
                }
                Token::Text(_) => {}
            }
        }
        self.flush(&mut para, block);
    }

    fn new_page(&mut self) {
        self.pages.push(Page::default());
        self.y = MARGIN_TOP;
    }

    fn space(&mut self, h: f32) {
        if self.y > MARGIN_TOP {
            self.y += h;
        }
    }

    fn rule(&mut self) {
        self.space(BODY * 0.5);
        let w = (PAGE_W - 2.0 * MARGIN_X) / 3.0;
        let x = (PAGE_W - w) / 2.0;
        let y = f64::from(self.y);
        self.pages.last_mut().unwrap().rules.push(Rect::new(f64::from(x), y, f64::from(x + w), y + 0.6));
        self.y += BODY;
    }

    fn picture(&mut self, name: &str) {
        let Some(source) = self.entries.get(name) else { return };
        let Some((w, h)) = images::read_entry(self.data, source).and_then(|d| images::image_size(&d)) else { return };
        let (max_w, max_h) = (PAGE_W - 2.0 * MARGIN_X, PAGE_H - MARGIN_TOP - MARGIN_BOTTOM);
        let k = (0.75f32).min(max_w / w as f32).min(max_h / h as f32);
        let (w, h) = (w as f32 * k, h as f32 * k);
        if self.y + h > PAGE_H - MARGIN_BOTTOM {
            self.new_page();
        }
        let x = (PAGE_W - w) / 2.0;
        let r = Rect::new(f64::from(x), f64::from(self.y), f64::from(x + w), f64::from(self.y + h));
        self.pages.last_mut().unwrap().images.push((r, name.to_owned()));
        self.y += h + BODY * 0.6;
    }

    /// Lays the paragraph out in lines, filling pages.
    fn flush(&mut self, para: &mut Para, b: Block) {
        let words = std::mem::take(&mut para.words);
        if words.iter().all(|w| w.0.trim().is_empty() && w.0 != "\n") {
            if !words.is_empty() && b.pre {
                self.y += b.size * LEADING;
            }
            return;
        }
        let avail = PAGE_W - 2.0 * MARGIN_X - b.indent;
        let width = |w: &str, face: u8| w.chars().map(|c| self.fonts.advance(face, c, b.size)).sum::<f32>();
        // lines as ranges of words, breaking where the next word wouldn't fit
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut x = 0.0;
        for (i, (w, face, _, _)) in words.iter().enumerate() {
            if w == "\n" {
                lines.push(Vec::new());
                x = 0.0;
                continue;
            }
            let ww = width(w, *face);
            let gap = match lines.last().unwrap().last() {
                Some(&j) if words[j].3 => width(" ", words[j].1),
                _ => 0.0,
            };
            if !lines.last().unwrap().is_empty() && x + gap + ww > avail && !b.pre {
                lines.push(Vec::new());
                x = 0.0;
            } else {
                x += gap;
            }
            lines.last_mut().unwrap().push(i);
            x += ww;
        }
        let height = b.size * LEADING;
        let count = lines.len();
        for (n, line) in lines.into_iter().enumerate() {
            if self.y + height > PAGE_H - MARGIN_BOTTOM {
                self.new_page();
            }
            let baseline = self.y + b.size * 0.95;
            // runs of one face and link, with the spaces between words in them
            let natural: f32 = line.iter().enumerate().map(|(k, &i)| {
                let gap = if k + 1 < line.len() && words[i].3 { width(" ", words[i].1) } else { 0.0 };
                width(&words[i].0, words[i].1) + gap
            }).sum();
            let spaces = line.iter().take(line.len().saturating_sub(1)).filter(|&&i| words[i].3).count();
            let extra = if b.justify && n + 1 < count && spaces > 0 { ((avail - natural) / spaces as f32).clamp(0.0, b.size) } else { 0.0 };
            let page = self.pages.last_mut().unwrap();
            let mut runs: Vec<Run> = Vec::new();
            let mut x = MARGIN_X + b.indent;
            let mut link_start: Option<(f32, &str)> = None;
            for (k, &i) in line.iter().enumerate() {
                let (w, face, link, space) = &words[i];
                let ww = width(w, *face);
                match runs.last_mut() {
                    Some(run) if run.face == *face && words[line[k - 1]].2 == *link => run.text.push_str(w),
                    _ => runs.push(Run { face: *face, x, text: w.clone() }),
                }
                // link areas, joined across the words of one link
                match (link, link_start) {
                    (Some(l), None) => link_start = Some((x, l)),
                    (Some(l), Some((_, s))) if s != l => {
                        let (start, href) = link_start.take().unwrap();
                        page.links.push((Rect::new(f64::from(start), f64::from(self.y), f64::from(x), f64::from(self.y + height)), href.to_owned()));
                        link_start = Some((x, l));
                    }
                    (None, Some((start, href))) => {
                        page.links.push((Rect::new(f64::from(start), f64::from(self.y), f64::from(x), f64::from(self.y + height)), href.to_owned()));
                        link_start = None;
                    }
                    _ => {}
                }
                x += ww;
                if k + 1 < line.len() && *space {
                    runs.last_mut().unwrap().text.push(' ');
                    x += width(" ", *face) + extra;
                }
            }
            if let Some((start, href)) = link_start {
                page.links.push((Rect::new(f64::from(start), f64::from(self.y), f64::from(x), f64::from(self.y + height)), href.to_owned()));
            }
            page.lines.push(Line { baseline, size: b.size, extra, runs });
            self.y += height;
        }
        self.y += b.after;
    }
}

struct EpubRenderer<'a> {
    engine: &'a EpubEngine,
    /// Glyph outlines at 1 point, by face and glyph.
    outlines: HashMap<(u8, u32), BezPath>,
}

impl PageRenderer for EpubRenderer<'_> {
    fn render(&mut self, page: usize, scale: f32, area: PixelRect) -> Vec<u32> {
        let mut out = vec![0x00FF_FFFF; area.width as usize * area.height as usize];
        let Some(p) = self.engine.pages.get(page) else { return out };
        let s = f64::from(scale);
        let view = Rect::new(f64::from(area.x), f64::from(area.y), f64::from(area.x + area.width), f64::from(area.y + area.height));
        let base = Affine::translate((-f64::from(area.x), -f64::from(area.y))) * Affine::scale(s);
        let mut ctx = RenderContext::new_with(area.width as u16, area.height as u16, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
        ctx.set_paint(AlphaColor::from_rgba8(0x1B, 0x1B, 0x1B, 255));
        let fonts = &self.engine.fonts;
        for line in &p.lines {
            let (top, bottom) = (f64::from(line.baseline - line.size) * s, f64::from(line.baseline + line.size * 0.4) * s);
            if bottom < view.y0 || top > view.y1 {
                continue;
            }
            for run in &line.runs {
                let mut x = run.x;
                for ch in run.text.chars() {
                    let (face, glyph) = fonts.glyph(run.face, ch);
                    if ch != ' ' {
                        let path = self.outlines.entry((face, glyph.to_u32())).or_insert_with(|| fonts.outline(face, glyph));
                        ctx.set_transform(base * Affine::translate((f64::from(x), f64::from(line.baseline))) * Affine::scale_non_uniform(f64::from(line.size), -f64::from(line.size)));
                        ctx.fill_path(path);
                    }
                    x += fonts.advance(run.face, ch, line.size) + if ch == ' ' { line.extra } else { 0.0 };
                }
            }
        }
        ctx.set_transform(base);
        for r in &p.rules {
            ctx.fill_rect(r);
        }
        ctx.flush();
        let mut pixmap = Pixmap::new(area.width as u16, area.height as u16);
        let white = AlphaColor::from_rgba8(255, 255, 255, 255);
        ctx.render_with(&mut pixmap, &mut Resources::default(), RasterizerSettings { target_init: TargetInit::Clear(white), ..Default::default() });
        for (o, c) in out.iter_mut().zip(pixmap.data_as_u8_slice().chunks_exact(4)) {
            *o = u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]);
        }
        // pictures, scaled as comics are
        for (r, name) in &p.images {
            let on = Rect::new(r.x0 * s, r.y0 * s, r.x1 * s, r.y1 * s).intersect(view);
            if on.area() <= 0.0 {
                continue;
            }
            let Some(img) = self.engine.image(name) else { continue };
            let step = img.width as f64 / (r.width() * s);
            for py in on.y0 as u32..on.y1.ceil() as u32 {
                for px in on.x0 as u32..on.x1.ceil() as u32 {
                    let (ix, iy) = ((f64::from(px) - r.x0 * s) * step, (f64::from(py) - r.y0 * s) * step);
                    let (Some(oy), Some(ox)) = (py.checked_sub(area.y), px.checked_sub(area.x)) else { continue };
                    if ox >= area.width || oy >= area.height {
                        continue;
                    }
                    out[(oy * area.width + ox) as usize] = if step > 1.0 {
                        images::average(&img, ix as f32, iy as f32, (ix + step) as f32, (iy + step) as f32)
                    } else {
                        images::blend(&img, ix as f32, iy as f32)
                    };
                }
            }
        }
        out
    }
}

/// Windows' fonts for books: Georgia in four faces, Consolas, and Segoe UI for characters they
/// lack. Memory-mapped, so they don't count as ours.
struct Fonts {
    faces: Vec<Mmap>,
}

impl Fonts {
    fn load() -> Option<Self> {
        let dir = std::env::var_os("WINDIR").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from).join("Fonts");
        let open = |names: &[&str]| {
            names.iter().find_map(|n| {
                let file = std::fs::File::open(dir.join(n)).ok()?;
                // SAFETY: font files in the Windows folder aren't changed while in use.
                let map = unsafe { Mmap::map(&file) }.ok()?;
                FontRef::new(&map).ok()?;
                Some(map)
            })
        };
        let faces = vec![
            open(&["georgia.ttf", "times.ttf", "segoeui.ttf"])?,
            open(&["georgiab.ttf", "timesbd.ttf", "segoeuib.ttf", "georgia.ttf"])?,
            open(&["georgiai.ttf", "timesi.ttf", "segoeuii.ttf", "georgia.ttf"])?,
            open(&["georgiaz.ttf", "timesbi.ttf", "segoeuiz.ttf", "georgia.ttf"])?,
            open(&["consola.ttf", "cour.ttf", "georgia.ttf"])?,
            open(&["segoeui.ttf", "arial.ttf", "georgia.ttf"])?,
        ];
        Some(Self { faces })
    }

    fn font(&self, face: u8) -> FontRef<'_> {
        FontRef::new(&self.faces[usize::from(face)]).expect("checked when loaded")
    }

    /// The face and glyph that draw `ch`: the face asked for, or Segoe UI if it lacks it.
    fn glyph(&self, face: u8, ch: char) -> (u8, GlyphId) {
        match self.font(face).charmap().map(ch) {
            Some(g) if g != GlyphId::NOTDEF => (face, g),
            _ => (5, self.font(5).charmap().map(ch).unwrap_or(GlyphId::NOTDEF)),
        }
    }

    fn advance(&self, face: u8, ch: char, size: f32) -> f32 {
        let (face, glyph) = self.glyph(face, ch);
        self.font(face).glyph_metrics(Size::new(size), LocationRef::default()).advance_width(glyph).unwrap_or(size * 0.5)
    }

    /// A glyph's outline at 1 point, y up.
    fn outline(&self, face: u8, glyph: GlyphId) -> BezPath {
        let mut pen = Pen(BezPath::new());
        if let Some(o) = self.font(face).outline_glyphs().get(glyph) {
            let _ = o.draw(DrawSettings::unhinted(Size::new(1.0), LocationRef::default()), &mut pen);
        }
        pen.0
    }
}

struct Pen(BezPath);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((f64::from(x), f64::from(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((f64::from(x), f64::from(y)));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0.quad_to((f64::from(cx), f64::from(cy)), (f64::from(x), f64::from(y)));
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.0.curve_to((f64::from(ax), f64::from(ay)), (f64::from(bx), f64::from(by)), (f64::from(x), f64::from(y)));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// The EPUB 3 table of contents: the `toc` nav's nested lists of links.
fn nav_toc(xhtml: &str, dir: &str, anchors: &HashMap<String, (usize, f32)>) -> Vec<OutlineItem> {
    let mut stack: Vec<Vec<OutlineItem>> = vec![Vec::new()];
    let (mut in_toc, mut depth, mut href, mut title) = (false, 0, None::<String>, String::new());
    let mut in_link = false;
    for t in tokens(xhtml) {
        match t {
            Token::Open(name, attrs, empty) => match local(&name) {
                "nav" if attr(&attrs, "epub:type").is_some_and(|t| t.contains("toc")) || attr(&attrs, "role").is_some_and(|r| r == "doc-toc") => in_toc = true,
                "ol" if in_toc && !empty => {
                    depth += 1;
                    if depth > 1 {
                        stack.push(Vec::new());
                    }
                }
                "a" if in_toc && !empty => (in_link, href, title) = (true, attr(&attrs, "href").map(|h| resolve(dir, &h)), String::new()),
                _ => {}
            },
            Token::Text(text) if in_link => title.push_str(&text),
            Token::Close(name) => match local(&name) {
                "a" if in_link => {
                    in_link = false;
                    let target = href.take().and_then(|h| anchor_target(&h, anchors));
                    let words: Vec<&str> = title.split_whitespace().collect();
                    stack.last_mut().unwrap().push(OutlineItem { title: words.join(" "), target, open: false, children: Vec::new() });
                }
                "ol" if in_toc => {
                    depth -= 1;
                    if depth >= 1 {
                        let children = stack.pop().unwrap();
                        if let Some(parent) = stack.last_mut().unwrap().last_mut() {
                            parent.children = children;
                        }
                    }
                }
                "nav" if in_toc => break,
                _ => {}
            },
            _ => {}
        }
    }
    stack.into_iter().next().unwrap_or_default()
}

/// The EPUB 2 table of contents: the NCX's nested navPoints.
fn ncx_toc(xml: &str, dir: &str, anchors: &HashMap<String, (usize, f32)>) -> Vec<OutlineItem> {
    let mut stack: Vec<OutlineItem> = Vec::new();
    let mut top = Vec::new();
    let mut in_label = false;
    for t in tokens(xml) {
        match t {
            Token::Open(name, attrs, empty) => match local(&name) {
                "navPoint" if !empty => stack.push(OutlineItem { title: String::new(), target: None, open: false, children: Vec::new() }),
                "text" => in_label = true,
                "content" => {
                    if let (Some(item), Some(src)) = (stack.last_mut(), attr(&attrs, "src")) {
                        item.target = anchor_target(&resolve(dir, &src), anchors);
                    }
                }
                _ => {}
            },
            Token::Text(text) if in_label => {
                if let Some(item) = stack.last_mut() {
                    item.title.push_str(text.trim());
                }
            }
            Token::Close(name) => match local(&name) {
                "text" => in_label = false,
                "navPoint" => {
                    let item = stack.pop().unwrap();
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(item),
                        None => top.push(item),
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    top
}

fn anchor_target(href: &str, anchors: &HashMap<String, (usize, f32)>) -> Option<Target> {
    let &(page, y) = anchors.get(href).or_else(|| anchors.get(href.split('#').next()?))?;
    Some(Target::Page { page, y: Some(y) })
}

enum Token {
    /// Name, attributes, and whether it closes itself.
    Open(String, Vec<(String, String)>, bool),
    Close(String),
    Text(String),
}

/// A small XML reader: tags with attributes, text with entities decoded; comments, processing
/// instructions, doctypes and CDATA markers are passed over.
fn tokens(src: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            out.push(Token::Text(entities(rest)));
            break;
        };
        if lt > 0 {
            out.push(Token::Text(entities(&rest[..lt])));
        }
        rest = &rest[lt..];
        fn skip_to<'a>(rest: &'a str, end: &str) -> &'a str {
            rest.find(end).map_or("", |i| &rest[i + end.len()..])
        }
        if rest.starts_with("<!--") {
            rest = skip_to(rest, "-->");
        } else if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            let end = cdata.find("]]>").unwrap_or(cdata.len());
            out.push(Token::Text(cdata[..end].to_owned()));
            rest = cdata.get(end + 3..).unwrap_or("");
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = skip_to(rest, ">");
        } else if let Some(close) = rest.strip_prefix("</") {
            let end = close.find('>').unwrap_or(close.len());
            out.push(Token::Close(close[..end].trim().to_owned()));
            rest = close.get(end + 1..).unwrap_or("");
        } else {
            // a start tag: quoted values may hold '>'
            let mut end = 1;
            let mut quote = None;
            for (i, c) in rest.char_indices().skip(1) {
                match (c, quote) {
                    ('"' | '\'', None) => quote = Some(c),
                    (q, Some(open)) if q == open => quote = None,
                    ('>', None) => {
                        end = i;
                        break;
                    }
                    _ => end = i + c.len_utf8(),
                }
            }
            let inner = &rest[1..end];
            let empty = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
            let mut attrs = Vec::new();
            let mut a = &inner[name_end..];
            while let Some(eq) = a.find('=') {
                let key = a[..eq].trim().to_owned();
                let v = a[eq + 1..].trim_start();
                let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
                let Some(close) = v[1..].find(q) else { break };
                attrs.push((key, entities(&v[1..1 + close])));
                a = &v[close + 2..];
            }
            out.push(Token::Open(inner[..name_end].to_owned(), attrs, empty));
            rest = rest.get(end + 1..).unwrap_or("");
        }
    }
    out
}

fn entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|&i| i < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{A0}'),
            "mdash" => Some('\u{2014}'),
            "ndash" => Some('\u{2013}'),
            "hellip" => Some('\u{2026}'),
            "lsquo" => Some('\u{2018}'),
            "rsquo" => Some('\u{2019}'),
            "ldquo" => Some('\u{201C}'),
            "rdquo" => Some('\u{201D}'),
            "copy" => Some('\u{A9}'),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=semi]),
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn attr(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs.iter().find(|(k, _)| k == key || local(k) == key).map(|(_, v)| v.clone())
}

/// The folder part of a path in the book, with its slash.
fn parent(path: &str) -> String {
    path.rfind('/').map_or_else(String::new, |i| path[..=i].to_owned())
}

/// `href` from a file in `dir`: an address outside the book as it is, a fragment kept.
fn resolve(dir: &str, href: &str) -> String {
    if href.contains("://") || href.starts_with("mailto:") {
        return href.to_owned();
    }
    join(dir, href)
}

/// `href` joined to `dir`, with `..` and `.` worked out and %-escapes decoded.
fn join(dir: &str, href: &str) -> String {
    let (path, fragment) = href.split_once('#').map_or((href, None), |(p, f)| (p, Some(f)));
    let full = if path.is_empty() { dir.trim_end_matches('/').to_owned() } else if path.starts_with('/') { path[1..].to_owned() } else { format!("{dir}{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in full.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    let mut out = percent_decode(&parts.join("/"));
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_text_and_entities() {
        let t = tokens(r#"<?xml version="1.0"?><!-- c --><p class="a&amp;b">Tom &amp; Jerry&#8212;<br/>x</p>"#);
        assert!(matches!(&t[0], Token::Open(n, a, false) if n == "p" && a[0] == ("class".to_owned(), "a&b".to_owned())));
        assert!(matches!(&t[1], Token::Text(s) if s == "Tom & Jerry\u{2014}"));
        assert!(matches!(&t[2], Token::Open(n, _, true) if n == "br"));
        assert!(matches!(&t[4], Token::Close(n) if n == "p"));
    }

    #[test]
    fn paths_resolve_in_the_book() {
        assert_eq!(join("OEBPS/text/", "../images/a%20b.jpg"), "OEBPS/images/a b.jpg");
        assert_eq!(join("OEBPS/", "ch1.xhtml#s2"), "OEBPS/ch1.xhtml#s2");
        assert_eq!(resolve("OEBPS/", "https://example.org/x"), "https://example.org/x");
    }
}
