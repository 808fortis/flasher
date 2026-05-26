mod app;
mod fastboot;
mod scatter;
mod ui;

use app::App;

fn main() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 760.0])
            .with_min_inner_size([950.0, 600.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "tss-helper",
        options,
        Box::new(|cc| {
            ui::configure_theme(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    ).unwrap();
}
