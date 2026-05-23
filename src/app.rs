use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;
use imgui::*;

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

fn check_char() -> &'static str { "\u{2713}" }

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
    last_scan: Instant,
    flash_log: Arc<Mutex<Vec<String>>>,
    pending_load: Arc<Mutex<Option<PathBuf>>>,
    spinner: String,
    scatter_valid: bool,
    scatter_errors: Vec<String>,
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
            last_scan: Instant::now(),
            flash_log: Arc::new(Mutex::new(Vec::new())),
            pending_load: Arc::new(Mutex::new(None)),
            spinner: String::new(),
            scatter_valid: false,
            scatter_errors: Vec::new(),
        }
    }

    fn add_log(&mut self, text: String, level: LogLevel) {
        if self.log.len() > 500 { self.log.remove(0); }
        self.log.push(LogEntry { text, level });
    }

    #[allow(dead_code)]
    fn drain_flash_log(&mut self) {
        let mut flash_log = self.flash_log.lock().unwrap();
        for msg in flash_log.drain(..) {
            if self.log.len() > 500 { self.log.remove(0); }
            self.log.push(LogEntry { text: msg, level: LogLevel::Ok });
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

            flog(format!("{} flashed {}/{} partition", check_char(), success, total));
            let _ = session.reboot();
            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "done! device rebooting...".to_string();
            p.text = format!("{} flashed {}/{} partitions", check_char(), success, total);
            flog("done! device rebooting.".to_string());
        });
    }

    pub fn render(&mut self, ui: &Ui, win_w: f32, win_h: f32) {
        if self.last_scan.elapsed() >= Duration::from_secs(2) {
            self.scan_devices();
            self.last_scan = Instant::now();
        }

        let pending = { self.pending_load.lock().unwrap().take() };
        if let Some(ref path) = pending { self.load_scatter(path); }

        self.spinner = spinner_char(Instant::now()).to_string();

        ui.window("flasher")
            .no_decoration()
            .flags(WindowFlags::NO_MOVE | WindowFlags::NO_COLLAPSE | WindowFlags::NO_SCROLLBAR)
            .size([win_w, win_h], Condition::Always)
            .position([0.0, 0.0], Condition::Always)
            .build(|| {
                self.draw_toolbar(ui);
                ui.separator();
                let avail = ui.content_region_avail();
                let log_w = (avail[0] * 0.3).max(200.0);
                let table_w = avail[0] - log_w - 8.0;
                let height = (avail[1] - 60.0).max(100.0);
                self.draw_log(ui, log_w, height);
                ui.same_line();
                self.draw_table(ui, table_w, height);
                ui.separator();
                self.draw_bottom(ui);
            });
    }

    fn draw_toolbar(&self, ui: &Ui) {
        ui.text_colored([0.2, 0.6, 1.0, 1.0], "flasher");
        ui.same_line();
        let dev_text = if self.device_connected {
            format!("device: {} ({})", self.device_serial, self.device_model)
        } else {
            "no device".to_string()
        };
        let col = if self.device_connected { [0.0, 1.0, 0.0, 1.0] } else { [0.5, 0.5, 0.5, 1.0] };
        ui.text_colored(col, dev_text);
    }

    fn draw_log(&mut self, ui: &Ui, w: f32, h: f32) {
        ui.child_window("log").size([w, h]).border(true).build(|| {
            ui.text_colored([0.5, 0.5, 0.5, 1.0], "[ output ]");
            ui.separator();
            for entry in &self.log {
                let col = match entry.level {
                    LogLevel::Info => [0.7, 0.7, 0.7, 1.0],
                    LogLevel::Ok => [0.0, 1.0, 0.0, 1.0],
                    LogLevel::Warn => [1.0, 0.8, 0.0, 1.0],
                    LogLevel::Err => [1.0, 0.2, 0.2, 1.0],
                };
                ui.text_colored(col, &entry.text);
            }
            ui.set_scroll_here_y();
        });
    }

    fn draw_table(&mut self, ui: &Ui, w: f32, h: f32) {
        ui.child_window("parts").size([w, h]).border(true).build(|| {
            ui.text_colored([0.5, 0.5, 0.5, 1.0], "[ partitions ]");
            ui.separator();

            if self.partitions.is_empty() {
                ui.text_disabled("drop scatter file or click load below");
            } else {
                if !self.scatter_valid {
                    ui.text_colored([1.0, 0.8, 0.0, 1.0], "scatter has warnings");
                    if !self.scatter_errors.is_empty() {
                        for e in &self.scatter_errors {
                            ui.text_colored([1.0, 0.4, 0.0, 1.0], e);
                        }
                    }
                    ui.separator();
                }

                let all_enabled = self.partitions.iter().all(|p| p.enabled);
                let mut all = all_enabled;
                if ui.checkbox("all", &mut all) {
                    for p in &mut self.partitions { p.enabled = all; }
                }

                let flags = TableFlags::SCROLL_Y | TableFlags::RESIZABLE
                    | TableFlags::BORDERS_INNER_V | TableFlags::BORDERS_OUTER_V;
                if let Some(_t) = ui.begin_table_header_with_sizing("tbl", [
                    TableColumnSetup::new("on"),
                    TableColumnSetup::new("partition"),
                    TableColumnSetup::new("size"),
                    TableColumnSetup::new("address"),
                ], flags, [w, h - 80.0], 0.0) {
                    for p in &mut self.partitions {
                        ui.table_next_row();
                        ui.table_set_column_index(0);
                        ui.checkbox(format!("##{}", p.name), &mut p.enabled);
                        ui.table_set_column_index(1);
                        ui.text(&p.name);
                        ui.table_set_column_index(2);
                        ui.text(format!("{:.1} mb", p.partition_size as f64 / 1048576.0));
                        ui.table_set_column_index(3);
                        ui.text(format!("0x{:x}", p.linear_start_addr));
                    }
                }
            }
        });
    }

    fn draw_bottom(&mut self, ui: &Ui) {
        if ui.button_with_size("load scatter", [120.0, 24.0]) {
            let pending = self.pending_load.clone();
            thread::spawn(move || {
                if let Some(path) = rfd::FileDialog::new().add_filter("scatter", &["txt", "scatter", "xml"]).pick_file() {
                    *pending.lock().unwrap() = Some(path);
                }
            });
        }

        ui.same_line();
        ui.text("slot:");
        ui.same_line();
        if ui.radio_button_bool("a", self.slot_mode == SlotMode::A) { self.slot_mode = SlotMode::A; }
        ui.same_line();
        if ui.radio_button_bool("b", self.slot_mode == SlotMode::B) { self.slot_mode = SlotMode::B; }
        ui.same_line();
        if ui.radio_button_bool("both", self.slot_mode == SlotMode::Both) { self.slot_mode = SlotMode::Both; }

        ui.same_line();
        let can_flash = !self.flashing && self.device_connected && !self.partitions.is_empty()
            && self.partitions.iter().any(|p| p.enabled);
        if can_flash && ui.button_with_size("flash", [100.0, 24.0]) {
            self.add_log("initializing flash sequence...".to_string(), LogLevel::Info);
            self.start_flash();
        }

        let prog = self.progress.lock().unwrap();
        if !prog.done || prog.total > 0 || !prog.text.is_empty() {
            ui.same_line();
            let frac = if prog.total > 0 { prog.current as f32 / prog.total as f32 } else { 0.0 };
            let overlay = if prog.done && prog.total > 0 {
                format!("{} {}/{} {}", self.spinner, prog.current, prog.total, prog.text)
            } else if !prog.done && prog.total > 0 {
                format!("{} {}/{} {}", self.spinner, prog.current, prog.total, prog.message)
            } else {
                prog.message.clone()
            };
            ProgressBar::new(frac).overlay_text(&overlay).size([250.0, 20.0]).build(ui);
        }
        if let Some(ref err) = prog.error {
            ui.same_line();
            ui.text_colored([1.0, 0.2, 0.2, 1.0], err);
        }
        drop(prog);
    }
}
