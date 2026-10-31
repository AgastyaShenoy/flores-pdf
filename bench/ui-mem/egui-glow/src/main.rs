//! egui through eframe's glow renderer.
//! usage: egui-glow [pdf]

#[path = "../../shared/egui_ui.rs"]
mod ui;

struct App(ui::Ui);

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.0.show(ctx);
    }
}

fn main() -> eframe::Result {
    let pdf = std::env::args().nth(1);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 750.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("egui-glow", options, Box::new(|_cc| Ok(Box::new(App(ui::Ui::new(pdf))))))
}
