use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use rusb::{self, Device, DeviceHandle, Direction, TransferType, GlobalContext};

#[derive(Debug)]
pub enum Error {
    Usb(rusb::Error),
    Protocol(String),
    NotFound,
    Timeout,
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usb(e) => write!(f, "usb: {}", e),
            Error::Protocol(s) => write!(f, "protocol: {}", s),
            Error::NotFound => write!(f, "device not found"),
            Error::Timeout => write!(f, "timeout"),
            Error::Io(e) => write!(f, "io: {}", e),
        }
    }
}

impl From<rusb::Error> for Error {
    fn from(e: rusb::Error) -> Self { Error::Usb(e) }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}

pub type Result<T> = std::result::Result<T, Error>;

const CLASS_VENDOR: u8 = 0xff;
const SUBCLASS_FASTBOOT: u8 = 0x42;
const PROTOCOL_FASTBOOT: u8 = 0x03;
const TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Debug)]
pub struct FastbootSession {
    handle: DeviceHandle<GlobalContext>,
    out_ep: u8,
    in_ep: u8,
    interface: u8,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    bus: u8,
    address: u8,
    vendor_id: u16,
    product_id: u16,
}

impl DeviceInfo {
    pub fn id_string(&self) -> String {
        format!("{:03}:{:03} {:04x}:{:04x}", self.bus, self.address, self.vendor_id, self.product_id)
    }
}

fn is_fastboot_device(device: &Device<GlobalContext>) -> bool {
    let desc = match device.device_descriptor() {
        Ok(d) => d,
        Err(_) => return false,
    };
    if desc.class_code() == CLASS_VENDOR && desc.sub_class_code() == SUBCLASS_FASTBOOT && desc.protocol_code() == PROTOCOL_FASTBOOT {
        return true;
    }
    match device.active_config_descriptor() {
        Ok(config) => {
            for interface in config.interfaces() {
                for if_desc in interface.descriptors() {
                    if if_desc.class_code() == CLASS_VENDOR && if_desc.sub_class_code() == SUBCLASS_FASTBOOT && if_desc.protocol_code() == PROTOCOL_FASTBOOT {
                        return true;
                    }
                }
            }
            false
        }
        Err(_) => false,
    }
}

fn find_endpoints(device: &Device<GlobalContext>) -> Option<(u8, u8, u8)> {
    let config = device.active_config_descriptor().ok()?;
    for interface in config.interfaces() {
        for if_desc in interface.descriptors() {
            if if_desc.class_code() == CLASS_VENDOR && if_desc.sub_class_code() == SUBCLASS_FASTBOOT && if_desc.protocol_code() == PROTOCOL_FASTBOOT {
                let mut out_ep = 0u8;
                let mut in_ep = 0u8;
                for ep in if_desc.endpoint_descriptors() {
                    if ep.transfer_type() == TransferType::Bulk {
                        if ep.direction() == Direction::Out { out_ep = ep.address(); }
                        else if ep.direction() == Direction::In { in_ep = ep.address(); }
                    }
                }
                if out_ep != 0 && in_ep != 0 {
                    return Some((if_desc.interface_number(), out_ep, in_ep));
                }
            }
        }
    }
    None
}

pub fn list_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    if let Ok(list) = rusb::DeviceList::new() {
        for device in list.iter() {
            if is_fastboot_device(&device) {
                if let Ok(desc) = device.device_descriptor() {
                    out.push(DeviceInfo {
                        bus: device.bus_number(),
                        address: device.address(),
                        vendor_id: desc.vendor_id(),
                        product_id: desc.product_id(),
                    });
                }
            }
        }
    }
    out
}

pub fn get_device_serial() -> Option<String> {
    let session = connect().ok()?;
    session.getvar("serial").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn get_device_model() -> Option<String> {
    let session = connect().ok()?;
    session.getvar("product").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn connect() -> Result<FastbootSession> {
    let list = rusb::DeviceList::new()?;
    for device in list.iter() {
        if !is_fastboot_device(&device) { continue; }
        let (interface, out_ep, in_ep) = find_endpoints(&device).ok_or(Error::NotFound)?;
        let handle = device.open()?;
        handle.set_active_configuration(1).ok();
        handle.claim_interface(interface)?;
        return Ok(FastbootSession { handle, out_ep, in_ep, interface });
    }
    Err(Error::NotFound)
}

impl FastbootSession {
    fn send(&self, cmd: &str) -> Result<()> {
        let mut buf = cmd.as_bytes().to_vec();
        buf.push(0);
        let mut offset = 0;
        while offset < buf.len() {
            let written = self.handle.write_bulk(self.out_ep, &buf[offset..], TIMEOUT)?;
            if written == 0 { return Err(Error::Timeout); }
            offset += written;
        }
        Ok(())
    }

    fn read_response(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; 64];
        let mut read = 0;
        loop {
            if read >= buf.len() {
                buf.resize(buf.len() + 4096, 0);
            }
            let n = self.handle.read_bulk(self.in_ep, &mut buf[read..], TIMEOUT)?;
            if n == 0 { break; }
            read += n;
            if read >= 4 {
                let h = &buf[..4];
                if h == b"OKAY" || h == b"FAIL" || (h == b"DATA" && read >= 8) {
                    break;
                }
            }
        }
        buf.truncate(read);
        Ok(buf)
    }

    fn parse_response(&self, raw: &[u8]) -> Result<String> {
        if raw.len() < 4 {
            return Err(Error::Protocol("empty response".to_string()));
        }
        let header = &raw[..4];
        let body = if raw.len() > 4 {
            String::from_utf8_lossy(&raw[4..]).trim_end_matches('\0').to_string()
        } else { String::new() };
        match header {
            b"OKAY" => Ok(body),
            b"FAIL" => Err(Error::Protocol(body)),
            b"INFO" => {
                let rest = self.read_response()?;
                self.parse_response(&rest)
            }
            b"DATA" => Ok(body),
            _ => Err(Error::Protocol(format!("unknown response: {}", String::from_utf8_lossy(raw)))),
        }
    }

    fn command(&self, cmd: &str) -> Result<String> {
        self.send(cmd)?;
        let raw = self.read_response()?;
        self.parse_response(&raw)
    }

    pub fn getvar(&self, var: &str) -> Result<String> {
        self.command(&format!("getvar:{}", var))
    }

    pub fn get_slot_count(&self) -> usize {
        self.getvar("slot-count").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1)
    }

    pub fn get_max_download_size(&self) -> usize {
        self.getvar("max-download-size").ok()
            .and_then(|v| usize::from_str_radix(v.trim(), 16).ok())
            .unwrap_or(256 * 1024 * 1024)
    }

    pub fn download(&self, data: &[u8]) -> Result<()> {
        let size = data.len();
        self.command(&format!("download:{:08x}", size))?;

        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + CHUNK_SIZE).min(data.len());
            let written = self.handle.write_bulk(self.out_ep, &data[offset..end], TIMEOUT)?;
            if written == 0 { return Err(Error::Timeout); }
            offset += written;
        }

        let raw = self.read_response()?;
        self.parse_response(&raw).map(|_| ())
    }

    pub fn flash(&self, partition: &str) -> Result<()> {
        self.command(&format!("flash:{}", partition)).map(|_| ())
    }

    pub fn flash_with_slot(&self, partition: &str, slot: &str, data: &[u8]) -> Result<()> {
        let part = if slot.is_empty() { partition.to_string() } else { format!("{}:{}", partition, slot) };
        self.download(data)?;
        self.flash(&part)
    }

    pub fn flash_all_slots(&self, partition: &str, data: &[u8], slots: &[&str]) -> Result<()> {
        for slot in slots {
            self.download(data)?;
            self.flash(&format!("{}:{}", partition, slot))?;
        }
        Ok(())
    }

    pub fn erase(&self, partition: &str) -> Result<()> {
        self.command(&format!("erase:{}", partition)).map(|_| ())
    }

    pub fn reboot(&self) -> Result<()> {
        self.send("reboot").ok();
        Ok(())
    }

    pub fn backup_partitions(&self, partitions: &[String], base_path: &Path) -> Result<()> {
        fs::create_dir_all(base_path)?;
        for name in partitions {
            match self.command(&format!("flash:{}", name)) {
                Ok(_) => {}
                Err(_) => {
                    if let Ok(data) = self.read_partition_raw(name) {
                        let path = base_path.join(format!("{}.img", name));
                        fs::write(&path, &data)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn read_partition_raw(&self, name: &str) -> Result<Vec<u8>> {
        let size_str = self.getvar(&format!("partition-size:{}", name))?;
        let size = usize::from_str_radix(size_str.trim(), 16).map_err(|_| Error::Protocol("bad partition-size".to_string()))?;
        if size == 0 { return Err(Error::Protocol("zero size".to_string())); }
        let pos_str = self.getvar(&format!("partition-type:{}", name))?;
        let _ = pos_str;
        self.command(&format!("flash:{}", name))?;
        Ok(Vec::new())
    }
}

pub fn backup_imei(session: &FastbootSession, backup_dir: &Path, imei_parts: &[&str]) -> Result<PathBuf> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let dir = backup_dir.join(format!("imei_backup_{}", ts));
    fs::create_dir_all(&dir)?;

    let mut found = 0u32;
    for name in imei_parts {
        let path = dir.join(format!("{}.img", name));
        match session.command(&format!("flash:{}", name)) {
            Ok(_) => {}
            Err(e) => {
                let err_msg = e.to_string();
                if !err_msg.contains("No such partition") && !err_msg.contains("doesn't exist") {
                    fs::write(path, format!("backup_error: {}\n", err_msg))?;
                }
            }
        }
        found += 1;
    }

    let manifest = dir.join("_manifest.txt");
    let meta = format!(
        "backup_time: {}\npartitions_attempted: {}\npartitions_found: {}\n",
        ts, imei_parts.len(), found
    );
    fs::write(manifest, meta)?;

    if found > 0 { Ok(dir) } else { Err(Error::NotFound) }
}
