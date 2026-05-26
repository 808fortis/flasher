use std::thread;

use egui::{Color32, Context, CornerRadius, Frame, Margin, Stroke, Vec2};

use crate::app::{App, LogLevel, SlotMode};

pub fn configure_theme(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.override_text_color = Some(Color32::from_rgb(210, 215, 220));

    let bg = Color32::from_rgb(16, 18, 22);
    let surface = Color32::from_rgb(24, 27, 32);
    let border = Color32::from_rgb(40, 44, 50);

    style.visuals.window_fill = surface;
    style.visuals.panel_fill = bg;
    style.visuals.faint_bg_color = surface;

    let w = &mut style.visuals.widgets;
    w.noninteractive.bg_fill = surface;
    w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(140, 145, 150));
    w.noninteractive.bg_stroke = Stroke::new(1.0, border);

    w.inactive.bg_fill = Color32::from_rgb(32, 36, 42);
    w.inactive.fg_stroke = Stroke::new(1.5, Color32::from_rgb(180, 185, 190));
    w.inactive.bg_stroke = Stroke::new(1.0, border);

    w.active.bg_fill = Color32::from_rgb(25, 100, 180);
    w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);

    w.hovered.bg_fill = Color32::from_rgb(42, 46, 52);
    w.hovered.fg_stroke = Stroke::new(2.0, Color32::from_rgb(220, 225, 230));

    style.spacing.item_spacing = Vec2::new(12.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 7.0);
    style.spacing.window_margin = Margin::symmetric(12, 8);

    ctx.set_style(style);
}

fn color_ok() -> Color32 { Color32::from_rgb(40, 200, 80) }
fn color_warn() -> Color32 { Color32::from_rgb(230, 190, 40) }
fn color_err() -> Color32 { Color32::from_rgb(220, 60, 60) }
fn color_accent() -> Color32 { Color32::from_rgb(50, 140, 230) }
fn color_bg_card() -> Color32 { Color32::from_rgb(22, 25, 30) }

fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame {
        fill: color_bg_card(),
        corner_radius: CornerRadius::same(6),
        stroke: Stroke::new(1.0, Color32::from_rgb(38, 42, 48)),
        inner_margin: Margin::symmetric(10, 8),
        ..Default::default()
    }
    .show(ui, add_contents);
}

fn section_header(ui: &mut egui::Ui, icon: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.add_space(-4.0);
        ui.label(egui::RichText::new(icon).size(16.0).color(Color32::from_rgb(140, 145, 150)));
        ui.label(egui::RichText::new(title).size(14.0).strong().color(Color32::from_rgb(200, 205, 210)));
    });
    ui.separator();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.tick();

        egui::TopBottomPanel::top("bar")
            .frame(Frame {
                fill: Color32::from_rgb(20, 22, 28),
                inner_margin: Margin::symmetric(14, 6),
                ..Default::default()
            })
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("⛏  tss-helper").size(16.0).strong().color(color_accent()));
                    ui.label(egui::RichText::new("v1.0").size(12.0).color(Color32::from_rgb(100, 105, 110)));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("slot:").size(12.0).color(Color32::from_rgb(130, 135, 140)));
                            ui.radio_value(&mut self.slot_mode, SlotMode::A, "a");
                            ui.radio_value(&mut self.slot_mode, SlotMode::B, "b");
                            ui.radio_value(&mut self.slot_mode, SlotMode::Both, "both");
                        });

                        ui.separator();

                        let (text, col) = if self.device_connected && self.device_is_supported {
                            (format!("● {} {}", self.device_manufacturer, self.device_model), color_ok())
                        } else if self.device_connected {
                            (format!("● {} {} (unsupported)", self.device_manufacturer, self.device_model), color_warn())
                        } else {
                            ("○ no device".to_string(), Color32::from_rgb(100, 105, 110))
                        };
                        ui.colored_label(col, egui::RichText::new(text).size(13.0).strong());
                    });
                });
            });

        if self.show_format_confirm {
            egui::Window::new("⚠ confirm format")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(Frame {
                    fill: color_bg_card(),
                    corner_radius: CornerRadius::same(8),
                    stroke: Stroke::new(1.0, Color32::from_rgb(200, 60, 60)),
                    ..Default::default()
                })
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new("this will format all data partitions").size(14.0));
                    ui.label("userdata · cache · metadata");
                    ui.colored_label(color_err(), egui::RichText::new("all data will be lost!").strong());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(egui::RichText::new("yes, format").color(Color32::WHITE)).clicked() {
                            self.show_format_confirm = false;
                            self.add_log("format confirmed".to_string(), LogLevel::Info);
                            self.start_format_data();
                        }
                        if ui.button("cancel").clicked() {
                            self.show_format_confirm = false;
                        }
                    });
                });
        }

        egui::SidePanel::left("panel_left")
            .resizable(true)
            .default_width(460.0)
            .min_width(300.0)
            .frame(Frame {
                fill: Color32::from_rgb(16, 18, 22),
                inner_margin: Margin::symmetric(10, 6),
                ..Default::default()
            })
            .show(ctx, |ui| {
                section_header(ui, "📁", "scatter file");

                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let load_btn = egui::Button::new(egui::RichText::new("📂 load scatter").size(13.0))
                            .fill(Color32::from_rgb(35, 45, 60))
                            .corner_radius(CornerRadius::same(4));
                        if ui.add(load_btn).clicked() {
                            let pending = self.pending_load.clone();
                            thread::spawn(move || {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("scatter", &["txt", "scatter", "xml"])
                                    .pick_file()
                                {
                                    *pending.lock().unwrap() = Some(path);
                                }
                            });
                        }

                        if let Some(ref p) = self.scatter_path {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(
                                p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                            ).size(12.0).color(Color32::from_rgb(150, 200, 120)));
                            ui.label(egui::RichText::new(format!("{} pts", self.partitions.len()))
                                .size(11.0).color(Color32::from_rgb(100, 105, 110)));
                        } else {
                            ui.label(egui::RichText::new("no scatter loaded").size(12.0).color(Color32::from_rgb(100, 105, 110)));
                        }
                    });
                });

                ui.add_space(8.0);

                if !self.scatter_errors.is_empty() {
                    card(ui, |ui| {
                        for e in &self.scatter_errors {
                            ui.colored_label(color_warn(), egui::RichText::new(format!("⚠ {}", e)).size(12.0));
                        }
                    });
                    ui.add_space(8.0);
                }

                section_header(ui, "🗄", "partitions");

                if self.partitions.is_empty() {
                    ui.add_space(20.0);
                    ui.label(egui::RichText::new("load a scatter file to see partitions")
                        .size(12.0).color(Color32::from_rgb(80, 85, 90)));
                } else {
                    let all = self.partitions.iter().all(|p| p.enabled);
                    let mut all_enabled = all;
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut all_enabled, "all");
                        ui.label(egui::RichText::new(format!("{}/{}",
                            self.partitions.iter().filter(|p| p.enabled).count(),
                            self.partitions.len()))
                        .size(12.0).color(Color32::from_rgb(130, 135, 140)));
                    });
                    if all_enabled != all {
                        for p in &mut self.partitions { p.enabled = all_enabled; }
                    }

                    ui.add_space(4.0);

                    use egui_extras::{Column, TableBuilder};
                    let th = egui::TextStyle::Body.resolve(ui.style()).size + 5.0;

                    let table = TableBuilder::new(ui)
                        .striped(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto().at_least(30.0))
                        .column(Column::auto().resizable(true))
                        .column(Column::auto().resizable(true))
                        .column(Column::remainder());

                    table
                        .header(22.0, |mut h| {
                            h.col(|ui| { ui.label(egui::RichText::new("☑").size(12.0)); });
                            h.col(|ui| { ui.label(egui::RichText::new("partition").size(11.0).strong()); });
                            h.col(|ui| { ui.label(egui::RichText::new("size").size(11.0).strong()); });
                            h.col(|ui| { ui.label(egui::RichText::new("addr").size(11.0).strong()); });
                        })
                        .body(|mut body| {
                            for p in &mut self.partitions {
                                body.row(th, |mut row| {
                                    row.col(|ui| { ui.checkbox(&mut p.enabled, ""); });
                                    row.col(|ui| { ui.label(egui::RichText::new(&p.name).size(12.0)); });
                                    row.col(|ui| {
                                        let s = if p.partition_size >= 1073741824 {
                                            format!("{:.1} gb", p.partition_size as f64 / 1073741824.0)
                                        } else {
                                            format!("{:.1} mb", p.partition_size as f64 / 1048576.0)
                                        };
                                        ui.label(egui::RichText::new(s).size(12.0).color(Color32::from_rgb(150, 155, 160)));
                                    });
                                    row.col(|ui| {
                                        ui.label(egui::RichText::new(format!("0x{:x}", p.linear_start_addr))
                                            .size(11.0).color(Color32::from_rgb(110, 115, 120)));
                                    });
                                });
                            }
                        });
                }
            });

        egui::CentralPanel::default()
            .frame(Frame {
                fill: Color32::from_rgb(16, 18, 22),
                inner_margin: Margin::symmetric(10, 6),
                ..Default::default()
            })
            .show(ctx, |ui| {
                section_header(ui, "📋", "log");

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in &self.log {
                            let col = match entry.level {
                                LogLevel::Info => Color32::from_rgb(160, 165, 170),
                                LogLevel::Ok => color_ok(),
                                LogLevel::Warn => color_warn(),
                                LogLevel::Err => color_err(),
                            };
                            let prefix = match entry.level {
                                LogLevel::Info => "",
                                LogLevel::Ok => "✓ ",
                                LogLevel::Warn => "⚠ ",
                                LogLevel::Err => "✗ ",
                            };
                            ui.label(egui::RichText::new(format!("{}{}", prefix, entry.text))
                                .size(12.0)
                                .color(col));
                        }
                    });

                if self.scroll_log {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    self.scroll_log = false;
                }
            });

        egui::TopBottomPanel::bottom("actions")
            .frame(Frame {
                fill: Color32::from_rgb(20, 22, 28),
                inner_margin: Margin::symmetric(14, 8),
                ..Default::default()
            })
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let busy = self.is_busy();
                    let can = self.can_operate();
                    let has_enabled = !self.partitions.is_empty() && self.partitions.iter().any(|p| p.enabled);

                    let flash_col = if can && has_enabled { Color32::from_rgb(25, 110, 50) } else { Color32::from_rgb(30, 33, 38) };
                    let btn_flash = egui::Button::new(egui::RichText::new("▷ flash").size(13.0))
                        .fill(flash_col)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(Vec2::new(90.0, 28.0));
                    if ui.add_enabled(!busy && can && has_enabled, btn_flash).clicked() {
                        self.add_log("starting flash...".to_string(), LogLevel::Info);
                        self.start_flash();
                    }

                    let fmt_col = if can { Color32::from_rgb(120, 70, 25) } else { Color32::from_rgb(30, 33, 38) };
                    let btn_fmt = egui::Button::new(egui::RichText::new("◎ format data").size(13.0))
                        .fill(fmt_col)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(Vec2::new(110.0, 28.0));
                    if ui.add_enabled(!busy && can, btn_fmt).clicked() {
                        self.show_format_confirm = true;
                    }

                    let unlock_col = if can { Color32::from_rgb(130, 40, 40) } else { Color32::from_rgb(30, 33, 38) };
                    let btn_unlock = egui::Button::new(egui::RichText::new("🔓 unlock bl").size(13.0))
                        .fill(unlock_col)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(Vec2::new(100.0, 28.0));
                    if ui.add_enabled(!busy && can, btn_unlock).clicked() {
                        self.start_unlock();
                    }

                    let reboot_col = if !busy && self.device_connected { Color32::from_rgb(40, 55, 75) } else { Color32::from_rgb(30, 33, 38) };
                    let btn_reboot = egui::Button::new(egui::RichText::new("↻ reboot fb").size(13.0))
                        .fill(reboot_col)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(Vec2::new(105.0, 28.0));
                    if ui.add_enabled(!busy && self.device_connected, btn_reboot).clicked() {
                        self.start_reboot_fastboot();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let prog = self.progress.lock().unwrap();

                        if !prog.done || prog.total > 0 {
                            let frac = if prog.total > 0 { prog.current as f32 / prog.total as f32 } else { 0.0 };
                            let label = if !prog.done && prog.total > 0 {
                                format!("{} {}/{}", self.spinner, prog.current, prog.total)
                            } else {
                                prog.message.clone()
                            };
                            let pb = egui::ProgressBar::new(frac)
                                .text(label)
                                .desired_width(180.0)
                                .fill(Color32::from_rgb(40, 120, 60));
                            ui.add(pb);
                        }

                        if let Some(ref err) = prog.error {
                            ui.colored_label(color_err(), egui::RichText::new(err).size(12.0));
                        }

                        drop(prog);

                        let (status, sc) = if self.is_busy() {
                            ("working...", Color32::from_rgb(160, 165, 170))
                        } else if self.device_connected && self.device_is_supported {
                            ("ready ✓", color_ok())
                        } else if self.device_connected {
                            ("device not supported", color_warn())
                        } else {
                            ("no device", Color32::from_rgb(100, 105, 110))
                        };
                        ui.colored_label(sc, egui::RichText::new(status).size(12.0));
                    });
                });
            });
    }
}
