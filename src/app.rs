use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

use crate::fastboot;
use crate::scatter;

#[derive(Clone, PartialEq)]
enum SlotMode { A, B, Both }

enum LogLevel { Info, Ok, Warn, Err }

struct LogEntry {
    text: String,
    level: LogLevel,
}

pub struct Progress {
    pub current: usize,
    pub total: usize,
    pub message: String,
    pub text: String,
    pub done: bool,
    pub error: Option<String>,
}

fn spinner_char(t: Instant) -> &'static str {
    match (t.elapsed().as_millis() / 120) % 4 {
        0 => "|", 1 => "/", 2 => "-", _ => "\\"
    }
}

const DATA_PARTITIONS: &[&str] = &["userdata", "cache", "metadata"];

pub struct App {
    scatter_path: Option<PathBuf>,
    partitions: Vec<scatter::Partition>,
    slot_mode: SlotMode,
    device_connected: bool,
    device_serial: String,
    device_model: String,
    log: Vec<LogEntry>,
    progress: Arc<Mutex<Progress>>,
    flashing: bool,
    formatting: bool,
    show_format_confirm: bool,
    last_scan: Instant,
    flash_log: Arc<Mutex<Vec<String>>>,
    pending_load: Arc<Mutex<Option<PathBuf>>>,
    spinner: String,
    scatter_valid: bool,
    scatter_errors: Vec<String>,
    scroll_log: bool,
}

impl App {
    pub fn new() -> Self {
        App {
            scatter_path: None,
            partitions: Vec::new(),
            slot_mode: SlotMode::A,
            device_connected: bool::default(),
            device_serial: String::new(),
            device_model: String::new(),
            log: Vec::new(),
            progress: Arc::new(Mutex::new(Progress {
                current: 0, total: 0, message: String::new(), text: String::new(),
                done: true, error: None,
            })),
            flashing: false,
            formatting: false,
            show_format_confirm: false,
            last_scan: Instant::now(),
            flash_log: Arc::new(Mutex::new(Vec::new())),
            pending_load: Arc::new(Mutex::new(None)),
            spinner: String::new(),
            scatter_valid: false,
            scatter_errors: Vec::new(),
            scroll_log: false,
        }
    }

    fn add_log(&mut self, text: String, level: LogLevel) {
        if self.log.len() > 500 { self.log.remove(0); }
        self.log.push(LogEntry { text, level });
        self.scroll_log = true;
    }

    #[allow(dead_code)]
    fn drain_flash_log(&mut self) {
        let mut flash_log = self.flash_log.lock().unwrap();
        for msg in flash_log.drain(..) {
            if self.log.len() > 500 { self.log.remove(0); }
            self.log.push(LogEntry { text: msg, level: LogLevel::Ok });
            self.scroll_log = true;
        }
    }

    pub fn load_scatter(&mut self, path: &Path) {
        self.add_log(format!("loading scatter: {}", path.display()), LogLevel::Info);
        match scatter::parse_scatter(path) {
            Ok(s) => {
                let errors = scatter::verify_scatter(&s);
                if errors.is_empty() {
                    self.partitions = s.partitions;
                    self.scatter_path = Some(path.to_path_buf());
                    self.scatter_valid = true;
                    self.scatter_errors.clear();
                    let fmt = if path.to_string_lossy().ends_with(".xml") { "xml" } else { "txt" };
                    self.add_log(format!("scatter {} loaded: {} partitions, ok", fmt, self.partitions.len()), LogLevel::Ok);
                } else {
                    self.scatter_valid = false;
                    self.scatter_errors = errors.clone();
                    self.partitions = s.partitions;
                    self.scatter_path = Some(path.to_path_buf());
                    for e in &errors {
                        self.add_log(format!("scatter warning: {}", e), LogLevel::Warn);
                    }
                }
            }
            Err(e) => {
                self.scatter_valid = false;
                self.scatter_errors = vec![e.to_string()];
                self.add_log(format!("scatter failed: {}", e), LogLevel::Err);
            }
        }
    }

    fn scan_devices(&mut self) {
        let devices = fastboot::list_devices();
        let was = self.device_connected;
        self.device_connected = !devices.is_empty();
        if self.device_connected && !was {
            let serial = fastboot::get_device_serial().unwrap_or_else(|| "unknown".to_string());
            let ids: Vec<String> = devices.iter().map(|d| d.id_string()).collect();
            self.device_serial = serial.clone();
            self.add_log(format!("detected fastboot device ({}) [{}]", serial, ids.join(", ")), LogLevel::Ok);
            self.device_model = fastboot::get_device_model().unwrap_or_else(|| "unknown".to_string());
        } else if !self.device_connected && was {
            self.device_serial.clear();
            self.device_model.clear();
            self.add_log("device disconnected".to_string(), LogLevel::Warn);
        }
    }

    fn start_flash(&mut self) {
        let parts: Vec<scatter::Partition> = self.partitions.iter().filter(|p| p.enabled).cloned().collect();
        let base = self.scatter_path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
        let slot_mode = self.slot_mode.clone();
        let progress = self.progress.clone();
        let total = parts.len();
        let device_model = self.device_model.clone();
        let flash_log = self.flash_log.clone();
        let imei_names: Vec<String> = scatter::imei_partitions().iter().map(|s| s.to_string()).collect();

        let slot_name = match slot_mode {
            SlotMode::A => "a", SlotMode::B => "b", SlotMode::Both => "all",
        };

        self.flashing = true;
        self.add_log(format!("selected slot: {}", slot_name), LogLevel::Info);
        self.add_log(format!("device model: {}", device_model), LogLevel::Info);

        {
            let mut p = progress.lock().unwrap();
            p.done = false; p.current = 0; p.total = total + 1;
            p.error = None; p.message = "initializing...".to_string(); p.text = String::new();
        }

        thread::spawn(move || {
            let flog = |msg: String| { flash_log.lock().unwrap().push(msg); };

            flog("connecting to fastboot device...".to_string());
            let session = match fastboot::connect() {
                Ok(s) => s,
                Err(e) => {
                    let mut p = progress.lock().unwrap();
                    p.done = true; p.error = Some(format!("connection failed: {}", e));
                    p.message = "failed".to_string();
                    flog(format!("fail: {}", e));
                    return;
                }
            };

            {
                let mut p = progress.lock().unwrap();
                p.message = "backing up imei partitions...".to_string();
            }
            flog("backing up imei partitions...".to_string());
            let backup_dir = PathBuf::from("backups");
            match fastboot::backup_imei(&session, &backup_dir, &imei_names.iter().map(|s| s.as_str()).collect::<Vec<_>>()) {
                Ok(dir) => flog(format!("imei backup saved to {}", dir.display())),
                Err(_) => flog("imei backup skipped (no imei partitions found)".to_string()),
            }

            flog("connected. reading slot info...".to_string());
            let slot_count = session.get_slot_count();
            let slots: Vec<&str> = match slot_mode {
                SlotMode::A => vec!["a"],
                SlotMode::B => vec!["b"],
                SlotMode::Both if slot_count >= 2 => vec!["a", "b"],
                SlotMode::Both => vec!["_a", "_b"],
            };

            let mut success = 0usize;
            for (i, part) in parts.iter().enumerate() {
                {
                    let mut p = progress.lock().unwrap();
                    p.current = i + 1;
                    p.message = format!("flashing {}...", part.name);
                    p.text = String::new();
                }

                let img_path = base.join(&part.filename);
                let data = match std::fs::read(&img_path) {
                    Ok(d) => d,
                    Err(e) => {
                        let mut p = progress.lock().unwrap();
                        p.done = true; p.error = Some(format!("cannot read {}: {}", part.filename, e));
                        p.message = "failed".to_string();
                        flog(format!("fail: {} - {}", part.filename, e));
                        return;
                    }
                };

                let size_mb = data.len() as f64 / 1048576.0;
                flog(format!("flashing {} ({:.2} mb)...", part.name, size_mb));

                let result = if slot_count >= 2 && slot_mode == SlotMode::Both {
                    session.flash_all_slots(&part.name, &data, &slots)
                } else {
                    let slot = if slot_count >= 2 && slot_mode != SlotMode::Both {
                        if slot_mode == SlotMode::A { "a" } else { "b" }
                    } else { "" };
                    session.flash_with_slot(&part.name, slot, &data)
                };

                match result {
                    Ok(_) => {
                        success += 1;
                        flog(format!("ok: {}", part.name));
                    }
                    Err(e) => {
                        let mut p = progress.lock().unwrap();
                        p.done = true; p.error = Some(format!("{}: {}", part.name, e));
                        p.message = "failed".to_string();
                        flog(format!("fail: {} - {}", part.name, e));
                        return;
                    }
                }
            }

            flog(format!("ok {}/{} partition", success, total));
            let _ = session.reboot();
            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "done! device rebooting...".to_string();
            p.text = format!("ok {}/{} partitions", success, total);
            flog("done! device rebooting.".to_string());
        });
    }

    fn start_format_data(&mut self) {
        let partitions: Vec<&str> = DATA_PARTITIONS.to_vec();
        let progress = self.progress.clone();
        let total = partitions.len();
        let flash_log = self.flash_log.clone();

        self.formatting = true;
        self.add_log("formatting data partitions...".to_string(), LogLevel::Info);

        {
            let mut p = progress.lock().unwrap();
            p.done = false; p.current = 0; p.total = total;
            p.error = None; p.message = "formatting...".to_string(); p.text = String::new();
        }

        thread::spawn(move || {
            let flog = |msg: String| { flash_log.lock().unwrap().push(msg); };

            flog("connecting to fastboot device...".to_string());
            let session = match fastboot::connect() {
                Ok(s) => s,
                Err(e) => {
                    let mut p = progress.lock().unwrap();
                    p.done = true; p.error = Some(format!("connection failed: {}", e));
                    p.message = "failed".to_string();
                    flog(format!("fail: {}", e));
                    return;
                }
            };

            let mut success = 0usize;
            for (i, part) in partitions.iter().enumerate() {
                {
                    let mut p = progress.lock().unwrap();
                    p.current = i + 1;
                    p.message = format!("formatting {}...", part);
                }

                flog(format!("formatting {}...", part));
                match session.format(part) {
                    Ok(_) => {
                        success += 1;
                        flog(format!("ok: {}", part));
                    }
                    Err(e) => {
                        flog(format!("skip: {} ({})", part, e));
                    }
                }
            }

            flog(format!("done. {} partitions formatted", success));
            let _ = session.reboot();
            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "format complete! device rebooting...".to_string();
            p.text = format!("ok {}/{} partitions", success, total);
            flog("done! device rebooting.".to_string());
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_scan.elapsed() >= Duration::from_secs(2) {
            self.scan_devices();
            self.last_scan = Instant::now();
        }

        let pending = { self.pending_load.lock().unwrap().take() };
        if let Some(ref path) = pending { self.load_scatter(path); }

        self.spinner = spinner_char(Instant::now()).to_string();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("flasher");
                ui.label("v0.1");
                ui.separator();

                let status = if self.device_connected {
                    format!("{} ({})", self.device_serial, self.device_model)
                } else {
                    "no device".to_string()
                };
                let col = if self.device_connected {
                    egui::Color32::from_rgb(25, 200, 75)
                } else {
                    egui::Color32::GRAY
                };
                ui.label("device:");
                ui.colored_label(col, &status);

                ui.separator();

                let slot_name = match self.slot_mode {
                    SlotMode::A => "a", SlotMode::B => "b", SlotMode::Both => "both",
                };
                ui.label(format!("slot: {}", slot_name));

                if let Some(ref path) = self.scatter_path {
                    ui.separator();
                    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                    ui.label(format!("scatter: {}", name));
                }
            });
        });

        if self.show_format_confirm {
            egui::Window::new("confirm format")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("this will format all data partitions");
                    ui.label("(userdata, cache, metadata)");
                    ui.label("all data will be lost!");
                    ui.colored_label(egui::Color32::RED, "are you sure?");
                    ui.horizontal(|ui| {
                        if ui.button("yes, format").clicked() {
                            self.show_format_confirm = false;
                            self.add_log("formatting data partitions...".to_string(), LogLevel::Info);
                            self.start_format_data();
                        }
                        if ui.button("cancel").clicked() {
                            self.show_format_confirm = false;
                        }
                    });
                });
        }

        egui::TopBottomPanel::bottom("controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("load scatter").clicked() {
                    let pending = self.pending_load.clone();
                    thread::spawn(move || {
                        if let Some(path) = rfd::FileDialog::new().add_filter("scatter", &["txt", "scatter", "xml"]).pick_file() {
                            *pending.lock().unwrap() = Some(path);
                        }
                    });
                }

                ui.separator();

                ui.label("slot");
                ui.radio_value(&mut self.slot_mode, SlotMode::A, "a");
                ui.radio_value(&mut self.slot_mode, SlotMode::B, "b");
                ui.radio_value(&mut self.slot_mode, SlotMode::Both, "both");

                ui.separator();

                let can_flash = !self.flashing && !self.formatting && self.device_connected && !self.partitions.is_empty()
                    && self.partitions.iter().any(|p| p.enabled);
                if ui.add_enabled(can_flash, egui::Button::new("flash")).clicked() {
                    self.add_log("initializing flash sequence...".to_string(), LogLevel::Info);
                    self.start_flash();
                }

                let can_format = !self.formatting && !self.flashing && self.device_connected;
                if ui.add_enabled(can_format, egui::Button::new("format data")).clicked() {
                    self.show_format_confirm = true;
                }

                let prog = self.progress.lock().unwrap();
                if !prog.done || prog.total > 0 || !prog.text.is_empty() {
                    let frac = if prog.total > 0 { prog.current as f32 / prog.total as f32 } else { 0.0 };
                    let overlay = if prog.done && prog.total > 0 {
                        format!("{} {}/{} {}", self.spinner, prog.current, prog.total, prog.text)
                    } else if !prog.done && prog.total > 0 {
                        format!("{} {}/{} {}", self.spinner, prog.current, prog.total, prog.message)
                    } else {
                        prog.message.clone()
                    };
                    ui.add(egui::ProgressBar::new(frac).text(overlay).desired_width(200.0));
                }
                if let Some(ref err) = prog.error {
                    ui.colored_label(egui::Color32::RED, err);
                }
                drop(prog);
            });
        });

        egui::SidePanel::left("table_panel")
            .resizable(true)
            .default_width(480.0)
            .min_width(280.0)
            .show(ctx, |ui| {
                ui.label("partitions");
                ui.separator();

                if self.partitions.is_empty() {
                    ui.label("drop scatter file or click load below");
                } else {
                    if !self.scatter_valid {
                        ui.colored_label(egui::Color32::YELLOW, "scatter has warnings");
                        for e in &self.scatter_errors {
                            ui.colored_label(egui::Color32::YELLOW, e);
                        }
                        ui.separator();
                    }

                    let all_enabled = self.partitions.iter().all(|p| p.enabled);
                    let mut all = all_enabled;
                    ui.checkbox(&mut all, "all");
                    if all != all_enabled {
                        for p in &mut self.partitions { p.enabled = all; }
                    }
                    ui.label(format!("{}/{}",
                        self.partitions.iter().filter(|p| p.enabled).count(),
                        self.partitions.len()));

                    use egui_extras::{Column, TableBuilder};
                    let text_height = egui::TextStyle::Body.resolve(ui.style()).size + 4.0;
                    TableBuilder::new(ui)
                        .striped(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto().resizable(true))
                        .column(Column::auto().resizable(true))
                        .column(Column::auto().resizable(true))
                        .column(Column::remainder())
                        .header(20.0, |mut header| {
                            header.col(|ui| { ui.label("on"); });
                            header.col(|ui| { ui.label("partition"); });
                            header.col(|ui| { ui.label("size"); });
                            header.col(|ui| { ui.label("addr"); });
                        })
                        .body(|mut body| {
                            for p in &mut self.partitions {
                                body.row(text_height, |mut row| {
                                    row.col(|ui| { ui.checkbox(&mut p.enabled, ""); });
                                    row.col(|ui| { ui.label(&p.name); });
                                    row.col(|ui| { ui.label(format!("{:.1} mb", p.partition_size as f64 / 1048576.0)); });
                                    row.col(|ui| { ui.label(format!("0x{:x}", p.linear_start_addr)); });
                                });
                            }
                        });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("log");
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for entry in &self.log {
                    let col = match entry.level {
                        LogLevel::Info => egui::Color32::LIGHT_GRAY,
                        LogLevel::Ok => egui::Color32::from_rgb(25, 200, 75),
                        LogLevel::Warn => egui::Color32::YELLOW,
                        LogLevel::Err => egui::Color32::RED,
                    };
                    ui.colored_label(col, &entry.text);
                }
                if self.scroll_log {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    self.scroll_log = false;
                }
            });
        });
    }
}
