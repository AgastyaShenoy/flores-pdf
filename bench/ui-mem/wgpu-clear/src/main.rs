//! Window cleared by wgpu on every redraw: what loading the GPU stack alone costs.
//! env: WGPU_BACKEND=dx12|vulkan|gl to pick the backend, MEM=low for MemoryHints::MemoryUsage

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

#[derive(Default)]
struct App(Option<Gpu>);

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("wgpu-clear").with_inner_size(LogicalSize::new(1000, 750));
        let window = Arc::new(el.create_window(attrs).unwrap());
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let surface = instance.create_surface(window.clone()).unwrap();
        let options = wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() };
        let adapter = pollster::block_on(instance.request_adapter(&options)).unwrap();
        eprintln!("{:?}", adapter.get_info());
        let memory_hints = match std::env::var("MEM").as_deref() {
            Ok("low") => wgpu::MemoryHints::MemoryUsage,
            _ => wgpu::MemoryHints::Performance,
        };
        let desc = wgpu::DeviceDescriptor { memory_hints, ..Default::default() };
        let (device, queue) = pollster::block_on(adapter.request_device(&desc)).unwrap();
        let size = window.inner_size();
        let config = surface.get_default_config(&adapter, size.width, size.height).unwrap();
        surface.configure(&device, &config);
        self.0 = Some(Gpu { window, surface, device, queue, config });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(gpu) = &mut self.0 else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                gpu.config.width = size.width;
                gpu.config.height = size.height;
                gpu.surface.configure(&gpu.device, &gpu.config);
                gpu.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let frame = gpu.surface.get_current_texture().unwrap();
                let view = frame.texture.create_view(&Default::default());
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                gpu.queue.submit([encoder.finish()]);
                frame.present();
            }
            _ => {}
        }
    }
}

fn main() {
    EventLoop::new().unwrap().run_app(&mut App::default()).unwrap();
}
