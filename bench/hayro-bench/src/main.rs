//! Same workload as bench_c.py, for hayro.
//! usage: hayro-bench <pdf> <max_pages> <png_out_dir>

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{self, Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{PixmapSettings, RenderCache, RenderSettings, render, render_into};
use rayon::prelude::*;
use std::time::Instant;

const DPI_SCALE: f32 = 150.0 / 72.0;
const ZOOM_SCALE: f32 = 300.0 / 72.0;
const TILE_SCALE: f64 = 4.0;
const TILE_PX: u16 = 1024;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn px(scale: f32) -> PixmapSettings {
    PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, max_pages, out_dir) = (&args[1], args[2].parse::<usize>().unwrap(), &args[3]);
    std::fs::create_dir_all(out_dir).unwrap();
    let interp = InterpreterSettings::default();
    let rs = RenderSettings::default();

    let t = Instant::now();
    let data = std::fs::read(path).unwrap();
    let pdf = Pdf::new(data.clone()).unwrap();
    let pages = pdf.pages();
    let n = pages.len();
    let _sizes: Vec<_> = pages.iter().map(|p| p.render_dimensions()).collect();
    let open_ms = ms(t);
    let count = n.min(max_pages);

    let cache = RenderCache::new();
    let t_all = Instant::now();
    let mut first_page_ms = 0.0;
    for (i, page) in pages.iter().take(count).enumerate() {
        let t = Instant::now();
        let pm = render(page, &cache, &interp, &rs, &px(DPI_SCALE));
        if i == 0 {
            first_page_ms = ms(t);
            std::fs::write(format!("{out_dir}/hayro_p1.png"), pm.into_png().unwrap()).unwrap();
        }
    }
    let render_total_ms = ms(t_all);

    // zoom change: hayro has no display list, so this re-interprets the page (font cache warm)
    let page = &pages[0];
    let t = Instant::now();
    render(page, &cache, &interp, &rs, &px(ZOOM_SCALE));
    let rezoom_ms = ms(t);

    // 1024x1024 tile from the middle of page 1 at 400%
    let (w, h) = page.render_dimensions();
    let mut ctx = RenderContext::new_with(
        TILE_PX,
        TILE_PX,
        vello_cpu::RenderSettings { num_threads: 0, ..Default::default() },
    );
    let mut tile = Pixmap::new(TILE_PX, TILE_PX);
    let half = TILE_PX as f64 / 2.0;
    let t = Instant::now();
    let transform = Affine::translate((half, half))
        * Affine::scale(TILE_SCALE)
        * Affine::translate((-(w as f64) / 2.0, -(h as f64) / 2.0))
        * page.initial_transform(true).to_kurbo();
    render_into(page, &cache, &interp, &rs, &mut ctx, transform);
    ctx.flush();
    ctx.render_with(
        &mut tile,
        &mut Resources::default(),
        RasterizerSettings { target_init: TargetInit::Clear(WHITE), ..Default::default() },
    );
    let tile_ms = ms(t);
    std::fs::write(format!("{out_dir}/hayro_tile.png"), tile.into_png().unwrap()).unwrap();

    // page-level parallelism: every page on its own thread, one Pdf per worker
    let t = Instant::now();
    (0..count).into_par_iter().for_each_init(
        || Pdf::new(data.clone()).unwrap(),
        |pdf, i| {
            let cache = RenderCache::new();
            let pages = pdf.pages();
            render(&pages[i], &cache, &interp, &rs, &px(DPI_SCALE));
        },
    );
    let parallel_total_ms = ms(t);

    println!(
        "{{\"engine\":\"hayro 0.8.0\",\"file\":{:?},\"pages\":{n},\"open_ms\":{open_ms:.2},\
\"render_pages\":{count},\"render_total_ms\":{render_total_ms:.2},\"first_page_ms\":{first_page_ms:.2},\
\"rezoom_uncached_ms\":{rezoom_ms:.2},\"tile_ms\":{tile_ms:.2},\"parallel_total_ms\":{parallel_total_ms:.2},\
\"threads\":{}}}",
        std::path::Path::new(path).file_name().unwrap().to_string_lossy(),
        rayon::current_num_threads()
    );
}
