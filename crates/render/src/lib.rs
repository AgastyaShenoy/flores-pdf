//! Everything between the engine and the window: page layout, the tile cache, the render threads
//! and the compositor that copies tiles into the window buffer. No windowing code in here.

mod cache;
mod compose;
mod layout;
mod look;
mod pixels;
mod pool;
mod schedule;
pub mod text;

pub use cache::{Tile, TileCache, TileKey};
pub use compose::{Frame, Style, compose, fill_rect, tint_rect};
pub use layout::{Layout, PagePos, Spread, TILE, Viewport};
pub use look::Look;
pub use pixels::Pixels;
pub use pool::{Done, Job, RenderPool, render_turned};
pub use schedule::wanted_tiles;
