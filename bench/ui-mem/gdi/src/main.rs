//! Window drawn on the CPU and blitted with GDI (softbuffer), the way Sumatra draws.
//! usage: gdi [pdf]   with a pdf, shows page 1 fitted to the window height

use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const GRAY: u32 = 0x00_80_80_80;

struct App {
    pdf: Option<String>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    // the open document plus its page as 0RGB pixels
    page: Option<(page::Page, Vec<u32>)>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("gdi").with_inner_size(LogicalSize::new(1000, 750));
        let window = Rc::new(el.create_window(attrs).unwrap());
        let context = softbuffer::Context::new(window.clone()).unwrap();
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).unwrap());
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => {
                let size = self.window.as_ref().unwrap().inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
                    return;
                };
                if let Some(path) = &self.pdf
                    && self.page.as_ref().is_none_or(|(p, _)| p.height != size.height)
                {
                    let mut p = page::render_first_page(path, size.height);
                    let xrgb = p.rgba.chunks_exact(4).map(|c| u32::from_be_bytes([0, c[0], c[1], c[2]])).collect();
                    p.rgba = Vec::new();
                    self.page = Some((p, xrgb));
                }
                let surface = self.surface.as_mut().unwrap();
                surface.resize(w, h).unwrap();
                let mut buf = surface.buffer_mut().unwrap();
                buf.fill(GRAY);
                if let Some((p, xrgb)) = &self.page {
                    let (bw, pw) = (size.width as usize, p.width as usize);
                    let (x0, cols) = (bw.saturating_sub(pw) / 2, pw.min(bw));
                    for y in 0..(p.height.min(size.height) as usize) {
                        buf[y * bw + x0..][..cols].copy_from_slice(&xrgb[y * pw..][..cols]);
                    }
                }
                buf.present().unwrap();
            }
            _ => {}
        }
    }
}

fn main() {
    let mut app = App { pdf: std::env::args().nth(1), window: None, surface: None, page: None };
    EventLoop::new().unwrap().run_app(&mut app).unwrap();
}
