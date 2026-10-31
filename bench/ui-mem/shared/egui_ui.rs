//! The same small UI for every egui backend: a toolbar, a sidebar, and page 1 of a PDF if one was given.

pub struct Ui {
    pdf: Option<String>,
    // keeps the document open, as a real viewer would; the pixels live in the texture
    page: Option<page::Page>,
    texture: Option<egui::TextureHandle>,
    zoom: f32,
    search: String,
}

impl Ui {
    pub fn new(pdf: Option<String>) -> Self {
        Ui { pdf, page: None, texture: None, zoom: 100.0, search: String::new() }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let _ = ui.button("Open");
                ui.add(egui::Slider::new(&mut self.zoom, 10.0..=400.0).text("zoom"));
                ui.text_edit_singleline(&mut self.search);
            });
        });
        egui::SidePanel::left("toc").show(ctx, |ui| {
            for i in 1..=20 {
                ui.label(format!("Chapter {i}"));
            }
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            if let (Some(path), None) = (&self.pdf, &self.texture) {
                let ppp = ui.ctx().pixels_per_point();
                let mut p = page::render_first_page(path, (ui.available_height() * ppp) as u32);
                let image = egui::ColorImage::from_rgba_premultiplied([p.width as usize, p.height as usize], &p.rgba);
                self.texture = Some(ui.ctx().load_texture("page", image, egui::TextureOptions::LINEAR));
                p.rgba = Vec::new();
                self.page = Some(p);
            }
            match &self.texture {
                Some(t) => {
                    ui.image((t.id(), t.size_vec2() / ui.ctx().pixels_per_point()));
                }
                None => {
                    ui.label("No document");
                }
            }
        });
    }
}
