use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([560.0, 260.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        vault_core::PRODUCT_NAME,
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            let mut cjk = egui::FontData::from_static(include_bytes!(
                "../../../assets/fonts/NotoSansCJK-Regular.ttc"
            ));
            cjk.index = 2; // Noto Sans CJK SC in the unmodified, licensed TTC collection.
            fonts
                .font_data
                .insert("noto-cjk".into(), std::sync::Arc::new(cjk));
            for family in fonts.families.values_mut() {
                family.push("noto-cjk".into());
            }
            cc.egui_ctx.set_fonts(fonts);
            Ok(Box::new(VaultApp::default()))
        }),
    )
}

#[derive(Default)]
struct VaultApp {
    phrase: zeroize::Zeroizing<String>,
    show_password: bool,
}

impl eframe::App for VaultApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(vault_core::PRODUCT_NAME);
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.separator();
            ui.label(vault_core::DEVELOPMENT_STATUS);
            ui.label("中文输入验证（不创建保险库）：");
            ui.add(egui::TextEdit::singleline(&mut *self.phrase).password(!self.show_password));
            ui.checkbox(&mut self.show_password, "显示输入以核对");
            if ui.button("Close").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}
