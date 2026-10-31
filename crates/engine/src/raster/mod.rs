//! hayro's rendering layer (the `hayro` crate 0.8.0, https://github.com/LaurenzV/hayro), vendored so
//! the engine can reach its internals. Licensed Apache-2.0 OR MIT; the licenses are next to this file.
//!
//! Changes from upstream:
//! - images: decoded and scaled images are shared between tiles and threads (`shared.rs`)
//! - `record.rs`: display lists, recorded once per page and replayed per tile
//! - `render` and the test helpers are gone; `render_into` takes the shared image cache

use fearless_simd::Level;
use hayro_interpret::Device;
use hayro_interpret::FillRule;
use hayro_interpret::InterpreterCache;
use hayro_interpret::InterpreterSettings;
use hayro_interpret::font::GlyphRun;
use hayro_interpret::hayro_syntax::page::Page;
use hayro_interpret::util::RectExt;
use hayro_interpret::{BlendMode, Context, DrawMode, DrawProps, ImageDrawProps, SoftMask};
use hayro_interpret::{ClipPath, interpret_page};
use kurbo::{Affine, BezPath, Rect, Shape};
use pic_scale::Scaler;
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use vello_cpu::peniko::{Compose, Fill, Mix};
use vello_cpu::{Mask, RenderContext, peniko};

mod clip;
mod glyph;
mod glyphs;
mod image;
mod mask;
mod paint;
mod path;
mod record;
mod shared;

pub(crate) use glyphs::{GlyphCache, SharedGlyphs, Target};
pub(crate) use record::{DisplayList, record};
pub(crate) use shared::ImageCache;

pub(crate) struct GlobalState {
    level: Level,
    outline_cache: Rc<RefCell<FxHashMap<u128, Rc<BezPath>>>>,
    scaler: Scaler,
    force_image_interpolation: bool,
    images: Arc<ImageCache>,
}

impl GlobalState {
    pub(crate) fn new(cache: &RenderCache<'_>, settings: &RenderSettings, images: &Arc<ImageCache>) -> Self {
        Self {
            level: Level::new(),
            outline_cache: cache.outline_cache.clone(),
            scaler: Scaler::new(image::RESAMPLING_FUNCTION),
            force_image_interpolation: settings.force_image_interpolation,
            images: images.clone(),
        }
    }
}

pub(crate) struct Renderer<'a> {
    global: &'a GlobalState,
    pub(crate) ctx: &'a mut RenderContext,
    pub(crate) inside_pattern: bool,
    pub(crate) soft_mask_cache: FxHashMap<u128, Mask>,
    pub(crate) in_type3_glyph: bool,
}

impl<'r> Renderer<'r> {
    pub(crate) fn new(ctx: &'r mut RenderContext, global: &'r GlobalState) -> Self {
        Self {
            global,
            ctx,
            inside_pattern: false,
            soft_mask_cache: FxHashMap::default(),
            in_type3_glyph: false,
        }
    }

    fn child_context(&self, width: u16, height: u16) -> RenderContext {
        RenderContext::new_with(width, height, derive_settings(self.ctx.render_settings()))
    }

    fn apply_draw_props(&mut self, props: &DrawProps<'_>) {
        self.ctx.set_transform(props.transform);
        self.apply_soft_mask(props.soft_mask.as_ref());
        self.ctx
            .set_blend_mode(convert_blend_mode(props.blend_mode));
    }

    fn apply_image_props(&mut self, props: &ImageDrawProps<'_>) {
        self.ctx.set_transform(props.transform);
        self.apply_soft_mask(props.soft_mask.as_ref());
        self.ctx
            .set_blend_mode(convert_blend_mode(props.blend_mode));
    }
}

impl<'a, 'r> Device<'a> for Renderer<'r> {
    fn draw_image(&mut self, image: hayro_interpret::Image<'a, '_>, props: ImageDrawProps<'a>) {
        Self::draw_pdf_image(self, image, props);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        Self::push_clip_path(self, clip_path);
    }

    fn push_clip_rect(&mut self, rect: &Rect) {
        Self::push_clip_rect(self, rect);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'a>>,
        blend_mode: BlendMode,
    ) {
        Self::push_transparency_group(self, opacity, mask, blend_mode);
    }

    fn pop_clip(&mut self) {
        Self::pop_clip(self);
    }

    fn pop_transparency_group(&mut self) {
        Self::pop_transparency_group(self);
    }

    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, draw_mode: &DrawMode) {
        Self::draw_path(self, path, props, draw_mode);
    }

    fn draw_rect(&mut self, rect: &Rect, props: DrawProps<'a>, draw_mode: &DrawMode) {
        Self::draw_rect(self, rect, props, draw_mode);
    }

    fn draw_glyph_run(
        &mut self,
        glyph_run: &GlyphRun<'_, 'a>,
        props: DrawProps<'a>,
        draw_mode: &DrawMode,
    ) {
        Self::draw_glyph_run(self, glyph_run, props, draw_mode);
    }
}

/// A cache used by the renderer.
///
/// Ideally, such a cache should be constructed once per PDF and then reused across
/// multiple render invocations on the same document.
#[derive(Clone, Default)]
pub(crate) struct RenderCache<'a> {
    pub(crate) interpreter_cache: InterpreterCache<'a>,
    pub(crate) outline_cache: Rc<RefCell<FxHashMap<u128, Rc<BezPath>>>>,
}

impl RenderCache<'_> {
    pub(crate) fn new() -> Self {
        Self {
            interpreter_cache: InterpreterCache::new(),
            outline_cache: Rc::new(RefCell::new(FxHashMap::default())),
        }
    }
}

/// Settings for rendering a page.
#[derive(Clone, Copy, Default)]
pub(crate) struct RenderSettings {
    /// Whether all images should forcibly be rendered with bilinear interpolation.
    pub(crate) force_image_interpolation: bool,
}

/// Render a page into the given [`RenderContext`] with the given transform.
pub(crate) fn render_into<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    interpreter_settings: &InterpreterSettings,
    render_settings: &RenderSettings,
    images: &Arc<ImageCache>,
    ctx: &mut RenderContext,
    transform: Affine,
) {
    let mut state = Context::new(
        transform,
        Rect::new(0.0, 0.0, ctx.width() as f64, ctx.height() as f64),
        &cache.interpreter_cache,
        page.xref(),
        interpreter_settings.clone(),
    );
    ctx.take_current_state();
    ctx.reset_mask();
    ctx.reset_filter_effect();

    let global = GlobalState::new(cache, render_settings, images);
    let mut device = Renderer::new(ctx, &global);
    let mut clip_path = page.intersected_crop_box().to_kurbo().to_path(0.1);
    clip_path.apply_affine(transform);
    device.push_clip_path(&ClipPath {
        path: clip_path,
        fill: FillRule::NonZero,
    });

    interpret_page(page, &mut state, &mut device);

    device.pop_clip();
}

/// What a `width` × `height` tile covers, in recording space, with room for antialiasing and
/// minimum line widths.
fn tile_view(transform: Affine, width: f64, height: f64) -> Rect {
    transform.inverse().transform_rect_bbox(Rect::new(-2.0, -2.0, width + 2.0, height + 2.0))
}

/// Draws a recorded page into `ctx`. `transform` maps the page at 1 pixel per point to the context.
/// With `defer`, the text that `stamp_text` will add afterwards is left out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay(
    list: &DisplayList,
    cache: &RenderCache<'_>,
    render_settings: &RenderSettings,
    images: &Arc<ImageCache>,
    ctx: &mut RenderContext,
    transform: Affine,
    defer: bool,
) {
    ctx.take_current_state();
    ctx.reset_mask();
    ctx.reset_filter_effect();
    let view = tile_view(transform, ctx.width() as f64, ctx.height() as f64);

    let global = GlobalState::new(cache, render_settings, images);
    let mut device = Renderer::new(ctx, &global);
    list.replay(&mut device, transform, view, defer);
}

/// Stamps a recorded page's deferred text into the tile vello_cpu has rendered.
pub(crate) fn stamp_text(list: &DisplayList, target: &mut Target, transform: Affine, glyphs: &mut GlyphCache) {
    let view = tile_view(transform, target.width as f64, target.height as f64);
    list.stamp_text(target, transform, view, glyphs);
}

pub(crate) fn derive_settings(settings: &vello_cpu::RenderSettings) -> vello_cpu::RenderSettings {
    vello_cpu::RenderSettings {
        num_threads: 0,
        ..*settings
    }
}

fn convert_fill_rule(fill_rule: FillRule) -> Fill {
    match fill_rule {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

fn convert_blend_mode(blend_mode: BlendMode) -> peniko::BlendMode {
    let mix = match blend_mode {
        BlendMode::Normal => Mix::Normal,
        BlendMode::Multiply => Mix::Multiply,
        BlendMode::Screen => Mix::Screen,
        BlendMode::Overlay => Mix::Overlay,
        BlendMode::Darken => Mix::Darken,
        BlendMode::Lighten => Mix::Lighten,
        BlendMode::ColorDodge => Mix::ColorDodge,
        BlendMode::ColorBurn => Mix::ColorBurn,
        BlendMode::HardLight => Mix::HardLight,
        BlendMode::SoftLight => Mix::SoftLight,
        BlendMode::Difference => Mix::Difference,
        BlendMode::Exclusion => Mix::Exclusion,
        BlendMode::Hue => Mix::Hue,
        BlendMode::Saturation => Mix::Saturation,
        BlendMode::Color => Mix::Color,
        BlendMode::Luminosity => Mix::Luminosity,
    };

    peniko::BlendMode::new(mix, Compose::SrcOver)
}
