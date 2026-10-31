//! Writes test files in the formats Flores PDF reads besides PDF, from one PNG: a DjVu page, a
//! WebP and a GIF, with the encoders of the libraries that read them.
//!
//! cargo run --release -p engine --example samples -- page.png out-folder

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, out) = (std::path::Path::new(&args[1]), std::path::Path::new(&args[2]));
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(input).unwrap())).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width, info.height);
    // RGBA, whatever the PNG had
    let rgba: Vec<u8> = match info.color_type.samples() {
        3 => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        4 => buf[..w as usize * h as usize * 4].to_vec(),
        _ => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
    };

    let pixmap = djvu_rs::Pixmap { width: w, height: h, data: rgba.clone() };
    // a text layer for the title line, as OCR would give, spread over where the title is on the
    // survey's first page (at 150 dpi); other pages get a text layer that doesn't match them
    use djvu_rs::text::{Rect, TextLayer, TextZone, TextZoneKind};
    let title = "A Survey of Large Language Models";
    let (x0, x1, y, height) = (232u32, 1040u32, 112u32, 44u32);
    let total: usize = title.chars().count();
    let mut at = 0usize;
    let words = title
        .split(' ')
        .map(|word| {
            let n = word.chars().count();
            let left = x0 + (x1 - x0) * at as u32 / total as u32;
            let right = x0 + (x1 - x0) * (at + n) as u32 / total as u32;
            at += n + 1;
            TextZone { kind: TextZoneKind::Word, rect: Rect { x: left, y, width: right - left, height }, text: word.to_owned(), children: Vec::new() }
        })
        .collect();
    let line = TextZone { kind: TextZoneKind::Line, rect: Rect { x: x0, y, width: x1 - x0, height }, text: title.to_owned(), children: words };
    let page = TextZone { kind: TextZoneKind::Page, rect: Rect { x: 0, y: 0, width: w, height: h }, text: title.to_owned(), children: vec![line] };
    let layer = TextLayer { text: title.to_owned(), zones: vec![page] };
    let djvu = djvu_rs::djvu_encode::PageEncoder::from_pixmap(&pixmap).with_dpi(150).with_text_layer(layer).encode().unwrap();
    std::fs::write(out.join("sample.djvu"), &djvu).unwrap();

    let mut webp = Vec::new();
    image_webp::WebPEncoder::new(&mut webp).encode(&rgba, w, h, image_webp::ColorType::Rgba8).unwrap();
    std::fs::write(out.join("sample.webp"), &webp).unwrap();

    let mut gif_bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut gif_bytes, w as u16, h as u16, &[]).unwrap();
        let mut pixels = rgba.clone();
        encoder.write_frame(&gif::Frame::from_rgba_speed(w as u16, h as u16, &mut pixels, 10)).unwrap();
    }
    std::fs::write(out.join("sample.gif"), &gif_bytes).unwrap();
    println!("{w}x{h}: djvu {} bytes, webp {} bytes, gif {} bytes", djvu.len(), webp.len(), gif_bytes.len());
}
