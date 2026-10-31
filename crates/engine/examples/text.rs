//! Prints a page's extracted text, to check spacing and line breaks by eye.
//!
//! usage: cargo run --release -p engine --example text -- <pdf> [page, 1-based]

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: text <pdf> [page]");
    let page = args.next().and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).saturating_sub(1);
    let doc = engine::open(path.as_ref()).expect("opening");
    let t = Instant::now();
    let text = doc.page_text(page);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let s: String = text.chars.iter().map(|c| c.ch).collect();
    println!("{s}");
    eprintln!("-- page {} of {}: {} characters in {ms:.1} ms", page + 1, doc.page_count(), text.chars.len());
}
