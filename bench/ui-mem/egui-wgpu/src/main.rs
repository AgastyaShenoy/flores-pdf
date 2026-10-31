//! egui through eframe's wgpu renderer.
//! usage: egui-wgpu [pdf]   env: MEM=low for MemoryHints::MemoryUsage

#[path = "../../shared/egui_ui.rs"]
mod ui;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use std::sync::Arc;

struct App(ui::Ui);

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.0.show(ctx);
    }
}

fn main() -> eframe::Result {
    let pdf = std::env::args().nth(1);
    let mut setup = WgpuSetupCreateNew::default();
    if std::env::var("MEM").as_deref() == Ok("low") {
        let default_descriptor = setup.device_descriptor.clone();
        setup.device_descriptor = Arc::new(move |adapter| wgpu::DeviceDescriptor {
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..default_descriptor(adapter)
        });
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 750.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..Default::default() },
        ..Default::default()
    };
    eframe::run_native("egui-wgpu", options, Box::new(|_cc| Ok(Box::new(App(ui::Ui::new(pdf))))))
}
