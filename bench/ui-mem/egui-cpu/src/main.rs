//! egui drawn on the CPU (egui_software_backend) and blitted with softbuffer: no GPU driver loaded.
//! usage: egui-cpu [pdf]

#[path = "../../shared/egui_ui.rs"]
mod ui;

use egui_software_backend::{SoftwareBackend, SoftwareBackendAppConfiguration};

struct App(ui::Ui);

impl egui_software_backend::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut SoftwareBackend) {
        self.0.show(ctx);
    }
}

fn main() {
    let pdf = std::env::args().nth(1);
    let settings = SoftwareBackendAppConfiguration::new()
        .inner_size(Some(egui::vec2(1000.0, 750.0)))
        .title(Some("egui-cpu".to_string()));
    egui_software_backend::run_app_with_software_backend(settings, |_ctx| App(ui::Ui::new(pdf.clone()))).unwrap();
}
