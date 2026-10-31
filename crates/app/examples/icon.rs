//! Draws Flores PDF's icon, a white flower on a sea-teal tile (Flores is "flowers" in Portuguese),
//! and writes it as `crates/app/flores.ico` in every size Windows asks for. The build embeds that
//! file; run this again only to change the icon.
//!
//! cargo run --release -p flores-pdf --example icon

use kurbo::{Affine, Circle, Ellipse, RoundedRect, Shape};
use vello_cpu::color::AlphaColor;
use vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};

const SIZES: [u16; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn draw(size: u16) -> Vec<u8> {
    let s = f64::from(size);
    let mut ctx = RenderContext::new_with(size, size, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
    let rgb = |c: u32| AlphaColor::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255);
    // the tile, inset a little at small sizes less, so it fills the space Windows gives it
    let inset = if size <= 24 { 0.5 } else { s / 32.0 };
    ctx.set_paint(rgb(0x0E7C7B));
    ctx.fill_path(&RoundedRect::new(inset, inset, s - inset, s - inset, s * 0.22).to_path(0.01));
    // five petals around the middle
    let mid = (s / 2.0, s / 2.0);
    ctx.set_paint(rgb(0xFFFFFF));
    for i in 0..5 {
        let angle = std::f64::consts::TAU * f64::from(i) / 5.0 - std::f64::consts::FRAC_PI_2;
        let petal = Ellipse::new((0.0, -s * 0.19), (s * 0.13, s * 0.2), 0.0);
        let at = Affine::translate(mid) * Affine::rotate(angle + std::f64::consts::FRAC_PI_2);
        ctx.fill_path(&(at * petal.to_path(0.01)));
    }
    ctx.set_paint(rgb(0xFFC23C));
    ctx.fill_path(&Circle::new(mid, s * 0.11).to_path(0.01));
    ctx.flush();
    let mut pixmap = Pixmap::new(size, size);
    let clear = AlphaColor::from_rgba8(0, 0, 0, 0);
    ctx.render_with(&mut pixmap, &mut Resources::default(), RasterizerSettings {
        target_init: TargetInit::Clear(clear),
        ..Default::default()
    });
    // PNG wants straight alpha; the pixmap is premultiplied
    let mut rgba = pixmap.data_as_u8_slice().to_vec();
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    rgba
}

/// The 256 px image is stored as a PNG; smaller ones as bitmaps, since older Windows code (GDI+,
/// some dialogs) only reads PNG at 256.
fn encode(size: u16, rgba: &[u8]) -> Vec<u8> {
    let n = usize::from(size);
    if size >= 256 {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, u32::from(size), u32::from(size));
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(rgba).unwrap();
        return png;
    }
    // BITMAPINFOHEADER with double height (color rows, then the mask), BGRA rows bottom to top,
    // then a 1-bit mask of fully transparent pixels, rows padded to 4 bytes
    let mask_row = n.div_ceil(32) * 4;
    let mut out = Vec::new();
    out.extend(40u32.to_le_bytes());
    out.extend((n as i32).to_le_bytes());
    out.extend((2 * n as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(((n * n * 4 + mask_row * n) as u32).to_le_bytes());
    out.extend([0u8; 16]);
    for row in rgba.chunks_exact(n * 4).rev() {
        out.extend(row.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
    }
    for row in rgba.chunks_exact(n * 4).rev() {
        let mut bits = vec![0u8; mask_row];
        for (x, p) in row.chunks_exact(4).enumerate() {
            if p[3] == 0 {
                bits[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend(bits);
    }
    out
}

fn main() {
    let images: Vec<(u16, Vec<u8>)> = SIZES.iter().map(|&s| (s, encode(s, &draw(s)))).collect();
    // ICONDIR, one ICONDIRENTRY per size, then the PNGs
    let mut ico = Vec::new();
    ico.extend(0u16.to_le_bytes());
    ico.extend(1u16.to_le_bytes());
    ico.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        let side = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend([side, side, 0, 0]);
        ico.extend(1u16.to_le_bytes());
        ico.extend(32u16.to_le_bytes());
        ico.extend((png.len() as u32).to_le_bytes());
        ico.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        ico.extend(png);
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("flores.ico");
    std::fs::write(&out, &ico).unwrap();
    // the 256 px image on its own, to look at
    std::fs::write(out.with_extension("png"), &images.last().unwrap().1).unwrap();
    println!("wrote {} ({} bytes)", out.display(), ico.len());
}
