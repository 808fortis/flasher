mod app;
mod fastboot;
mod scatter;

use app::App;

fn main() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([800.0, 500.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "flasher",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    ).unwrap();
}
