use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

use crate::fastboot;
use crate::scatter;

#[derive(Clone, PartialEq)]
pub enum SlotMode { A, B, Both }

#[derive(Clone)]
pub enum LogLevel { Info, Ok, Warn, Err }

pub struct LogEntry {
    pub text: String,
    pub level: LogLevel,
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

pub const DATA_PARTITIONS: &[&str] = &["userdata", "cache", "metadata"];

pub struct App {
    pub scatter_path: Option<PathBuf>,
    pub partitions: Vec<scatter::Partition>,
    pub slot_mode: SlotMode,
    pub device_connected: bool,
    pub device_serial: String,
    pub device_model: String,
    pub device_manufacturer: String,
    pub device_baseband: String,
    pub device_is_supported: bool,
    pub log: Vec<LogEntry>,
    pub progress: Arc<Mutex<Progress>>,
    pub flashing: bool,
    pub formatting: bool,
    pub unlocking: bool,
    pub show_format_confirm: bool,
    pub last_scan: Instant,
    pub flash_log: Arc<Mutex<Vec<String>>>,
    pub pending_load: Arc<Mutex<Option<PathBuf>>>,
    pub spinner: String,
    pub scatter_valid: bool,
    pub scatter_errors: Vec<String>,
    pub scroll_log: bool,
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
            device_manufacturer: String::new(),
            device_baseband: String::new(),
            device_is_supported: false,
            log: Vec::new(),
            progress: Arc::new(Mutex::new(Progress {
                current: 0, total: 0, message: String::new(), text: String::new(),
                done: true, error: None,
            })),
            flashing: false,
            formatting: false,
            unlocking: false,
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

    pub fn add_log(&mut self, text: String, level: LogLevel) {
        if self.log.len() > 500 { self.log.remove(0); }
        self.log.push(LogEntry { text, level });
        self.scroll_log = true;
    }

    fn drain_flash_log(&mut self) {
        let mut flash_log = self.flash_log.lock().unwrap();
        for msg in flash_log.drain(..) {
            if self.log.len() > 500 { self.log.remove(0); }
            self.log.push(LogEntry { text: msg, level: LogLevel::Ok });
            self.scroll_log = true;
        }
    }

    pub fn tick(&mut self) {
        self.spinner = spinner_char(Instant::now()).to_string();
        self.drain_flash_log();

        if self.last_scan.elapsed() >= Duration::from_secs(2) {
            self.scan_devices();
            self.last_scan = Instant::now();
        }

        let pending = self.pending_load.lock().unwrap().take();
        if let Some(ref path) = pending { self.load_scatter(path); }

        self.reset_busy_flags();
    }

    fn reset_busy_flags(&mut self) {
        let prog = self.progress.lock().unwrap();
        if prog.done {
            drop(prog);
            self.flashing = false;
            self.formatting = false;
            self.unlocking = false;
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
                    self.add_log(format!("scatter {} loaded: {} partitions", fmt, self.partitions.len()), LogLevel::Ok);
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
            self.device_serial = serial;
            self.add_log(format!("detected fastboot device [{}]", ids.join(", ")), LogLevel::Ok);
            self.device_model = fastboot::get_device_model().unwrap_or_else(|| "unknown".to_string());
            self.device_manufacturer = fastboot::get_device_manufacturer().unwrap_or_else(|| "unknown".to_string());
            self.device_baseband = fastboot::get_device_baseband().unwrap_or_else(|| "unknown".to_string());

            let mtk = fastboot::is_mediatek(&self.device_baseband, &self.device_model);
            let transsion = fastboot::is_transsion(&self.device_manufacturer);
            self.device_is_supported = transsion && mtk;

            if self.device_is_supported {
                self.add_log(format!("supported: {} {} (MTK)", self.device_manufacturer, self.device_model), LogLevel::Ok);
            } else {
                self.add_log(format!("unsupported: {} {} (need Transsion+MTK)", self.device_manufacturer, self.device_model), LogLevel::Warn);
            }
        } else if !self.device_connected && was {
            self.device_serial.clear();
            self.device_model.clear();
            self.device_manufacturer.clear();
            self.device_baseband.clear();
            self.device_is_supported = false;
            self.add_log("device disconnected".to_string(), LogLevel::Warn);
        }
    }

    pub fn is_busy(&self) -> bool {
        self.flashing || self.formatting || self.unlocking
    }

    pub fn can_operate(&self) -> bool {
        self.device_connected && self.device_is_supported && !self.is_busy()
    }

    pub fn start_flash(&mut self) {
        let parts: Vec<scatter::Partition> = self.partitions.iter().filter(|p| p.enabled).cloned().collect();
        let base = self.scatter_path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
        let slot_mode = self.slot_mode.clone();
        let progress = self.progress.clone();
        let flash_log = self.flash_log.clone();

        let slot_name = match slot_mode {
            SlotMode::A => "a", SlotMode::B => "b", SlotMode::Both => "all",
        };

        self.flashing = true;
        self.add_log(format!("flash: slot={} {} partitions", slot_name, parts.len()), LogLevel::Info);

        {
            let mut p = progress.lock().unwrap();
            p.done = false; p.current = 0; p.total = parts.len() + 1;
            p.error = None; p.message = "initializing...".to_string(); p.text = String::new();
        }

        thread::spawn(move || {
            let flog = |msg: String| flash_log.lock().unwrap().push(msg);

            flog("connecting...".to_string());
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
                p.message = "backing up imei...".to_string();
            }
            flog("backing up imei...".to_string());
            let imei_names: Vec<String> = scatter::imei_partitions().iter().map(|s| s.to_string()).collect();
            let backup_dir = PathBuf::from("backups");
            match fastboot::backup_imei(&session, &backup_dir, &imei_names.iter().map(|s| s.as_str()).collect::<Vec<_>>()) {
                Ok(dir) => flog(format!("imei backup -> {}", dir.display())),
                Err(_) => flog("imei backup skipped".to_string()),
            }

            let slot_count = session.get_slot_count();
            let slots: Vec<&str> = match slot_mode {
                SlotMode::A => vec!["a"],
                SlotMode::B => vec!["b"],
                SlotMode::Both if slot_count >= 2 => vec!["a", "b"],
                SlotMode::Both => vec!["_a", "_b"],
            };

            let total = parts.len();
            let mut success = 0usize;
            for (i, part) in parts.iter().enumerate() {
                {
                    let mut p = progress.lock().unwrap();
                    p.current = i + 1;
                    p.message = format!("flashing {}...", part.name);
                }

                let img_path = base.join(&part.filename);
                let data = match std::fs::read(&img_path) {
                    Ok(d) => d,
                    Err(e) => {
                        let mut p = progress.lock().unwrap();
                        p.done = true; p.error = Some(format!("read fail {}: {}", part.filename, e));
                        p.message = "failed".to_string();
                        flog(format!("fail: {} - {}", part.filename, e));
                        return;
                    }
                };

                flog(format!("flashing {} ({:.1} mb)...", part.name, data.len() as f64 / 1048576.0));

                let result = if slot_count >= 2 && slot_mode == SlotMode::Both {
                    session.flash_all_slots(&part.name, &data, &slots)
                } else {
                    let slot = if slot_count >= 2 && slot_mode != SlotMode::Both {
                        match slot_mode { SlotMode::A => "a", _ => "b" }
                    } else { "" };
                    session.flash_with_slot(&part.name, slot, &data)
                };

                match result {
                    Ok(_) => { success += 1; flog(format!("ok: {}", part.name)); }
                    Err(e) => {
                        let mut p = progress.lock().unwrap();
                        p.done = true; p.error = Some(format!("{}: {}", part.name, e));
                        p.message = "failed".to_string();
                        flog(format!("fail: {} - {}", part.name, e));
                        return;
                    }
                }
            }

            flog(format!("ok {}/{} partitions", success, total));
            let _ = session.reboot();
            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "done! rebooting...".to_string();
            p.text = format!("{}/{} ok", success, total);
            flog("done".to_string());
        });
    }

    pub fn start_format_data(&mut self) {
        let partitions: Vec<&str> = DATA_PARTITIONS.to_vec();
        let progress = self.progress.clone();
        let flash_log = self.flash_log.clone();

        self.formatting = true;
        self.add_log("formatting data partitions...".to_string(), LogLevel::Info);

        {
            let mut p = progress.lock().unwrap();
            p.done = false; p.current = 0; p.total = partitions.len();
            p.error = None; p.message = "formatting...".to_string();
        }

        thread::spawn(move || {
            let flog = |msg: String| flash_log.lock().unwrap().push(msg);

            flog("connecting...".to_string());
            let session = match fastboot::connect() {
                Ok(s) => s,
                Err(e) => {
                    let mut p = progress.lock().unwrap();
                    p.done = true; p.error = Some(format!("connection failed: {}", e));
                    p.message = "failed".to_string();
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
                    Ok(_) => { success += 1; flog(format!("ok: {}", part)); }
                    Err(e) => { flog(format!("skip: {} ({})", part, e)); }
                }
            }

            flog(format!("done: {}/{} formatted", success, partitions.len()));
            let _ = session.reboot();
            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "format complete!".to_string();
            p.text = format!("{}/{} ok", success, partitions.len());
        });
    }

    pub fn start_unlock(&mut self) {
        self.add_log("unlocking bootloader...".to_string(), LogLevel::Info);
        let progress = self.progress.clone();
        let flash_log = self.flash_log.clone();

        self.unlocking = true;

        {
            let mut p = progress.lock().unwrap();
            p.done = false; p.current = 0; p.total = 4;
            p.error = None; p.message = "unlocking...".to_string();
        }

        thread::spawn(move || {
            let flog = |msg: String| flash_log.lock().unwrap().push(msg);

            flog("connecting...".to_string());
            let session = match fastboot::connect() {
                Ok(s) => s,
                Err(e) => {
                    let mut p = progress.lock().unwrap();
                    p.done = true; p.error = Some(format!("connection failed: {}", e));
                    p.message = "failed".to_string();
                    return;
                }
            };

            {
                let mut p = progress.lock().unwrap();
                p.current = 1; p.message = "flashing unlock...".to_string();
            }
            flog("flashing unlock...".to_string());
            match session.flashing_unlock() {
                Ok(_) => flog("ok".to_string()),
                Err(e) => {
                    flog(format!("flashing unlock fail: {}", e));
                    flog("trying oem unlock...".to_string());
                    match session.oem_unlock() {
                        Ok(_) => flog("oem unlock ok".to_string()),
                        Err(e2) => {
                            let mut p = progress.lock().unwrap();
                            p.done = true;
                            p.error = Some(format!("unlock failed: {}", e2));
                            p.message = "failed".to_string();
                            return;
                        }
                    }
                }
            }

            {
                let mut p = progress.lock().unwrap();
                p.current = 2; p.message = "unlock_critical...".to_string();
            }
            flog("unlock_critical...".to_string());
            let _ = session.flashing_unlock_critical();

            {
                let mut p = progress.lock().unwrap();
                p.current = 3; p.message = "verifying...".to_string();
            }
            if let Ok(v) = session.getvar("unlocked") {
                flog(format!("unlocked: {}", v));
            }

            {
                let mut p = progress.lock().unwrap();
                p.current = 4; p.message = "reboot bootloader...".to_string();
            }
            let _ = session.reboot_bootloader();

            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "unlock done!".to_string();
            p.text = "unlocked".to_string();
            flog("unlock complete".to_string());
        });
    }

    pub fn start_reboot_fastboot(&mut self) {
        self.add_log("rebooting to fastboot...".to_string(), LogLevel::Info);
        let progress = self.progress.clone();
        let flash_log = self.flash_log.clone();

        self.flashing = true;
        {
            let mut p = progress.lock().unwrap();
            p.done = false; p.current = 0; p.total = 1;
            p.error = None; p.message = "rebooting...".to_string();
        }

        thread::spawn(move || {
            let flog = |msg: String| flash_log.lock().unwrap().push(msg);
            flog("connecting...".to_string());
            match fastboot::connect() {
                Ok(s) => {
                    flog("reboot-bootloader...".to_string());
                    match s.reboot_bootloader() {
                        Ok(_) => flog("ok".to_string()),
                        Err(e) => flog(format!("fail: {}", e)),
                    }
                }
                Err(e) => flog(format!("connection failed: {}", e)),
            }
            let mut p = progress.lock().unwrap();
            p.current = p.total; p.done = true;
            p.message = "reboot sent".to_string();
        });
    }
}
