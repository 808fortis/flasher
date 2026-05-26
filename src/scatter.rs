use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Partition {
    pub enabled: bool,
    pub name: String,
    pub filename: String,
    pub linear_start_addr: u64,
    pub partition_size: u64,
    pub physical_start_addr: u64,
    pub partition_index: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct Scatter {
    pub partitions: Vec<Partition>,
    pub platform: String,
    pub project: String,
    pub storage: String,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {}", e),
            Error::Parse(s) => write!(f, "parse: {}", s),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}

pub type Result<T> = std::result::Result<T, Error>;

const IMEI_PARTITIONS: &[&str] = &["nvram", "nvdata", "protect1", "protect2", "seccfg", "persist"];

pub fn imei_partitions() -> &'static [&'static str] { IMEI_PARTITIONS }

fn parse_int(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| Error::Parse(format!("invalid hex '{}': {}", s, e)))
    } else {
        s.parse::<u64>().map_err(|e| Error::Parse(format!("invalid int '{}': {}", s, e)))
    }
}

fn first_of<'a>(map: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a String> {
    keys.iter().find_map(|k| map.get(*k))
}

fn build_partition(map: &HashMap<String, String>, idx: u32) -> Option<Partition> {
    let name = first_of(map, &["name", "partition_name", "partition", "part"])?.clone();
    let filename = first_of(map, &["filename", "file_name", "file", "image", "img"]).cloned()
        .unwrap_or_else(|| format!("{}.img", name));
    let linear_start_addr = first_of(map, &[
        "linear_start_addr", "begin_addr", "start_addr",
        "linear_start_address", "start", "linear_addr",
    ]).and_then(|s| parse_int(s).ok()).unwrap_or(0);
    let partition_size = first_of(map, &[
        "partition_size", "size", "partition_size_",
        "partition_len", "length", "part_size",
    ]).and_then(|s| parse_int(s).ok()).unwrap_or(0);
    let physical_start_addr = first_of(map, &[
        "physical_start_addr", "physical_addr", "physical_start_address",
        "physical", "phys_addr",
    ]).and_then(|s| parse_int(s).ok()).unwrap_or(0);
    Some(Partition {
        enabled: true,
        name,
        filename,
        linear_start_addr,
        partition_size,
        physical_start_addr,
        partition_index: idx,
    })
}

fn parse_txt(path: &Path) -> Result<Scatter> {
    let content = fs::read_to_string(path)?;
    let mut partitions = Vec::new();
    let mut platform = String::new();
    let mut project = String::new();
    let mut storage = String::new();
    let mut in_partition = false;
    let mut current = HashMap::new();
    let mut part_index = 0u32;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") || trimmed.starts_with("--") {
            continue;
        }

        if trimmed.starts_with('[') {
            if in_partition && !current.is_empty() {
                if let Some(part) = build_partition(&current, part_index) {
                    partitions.push(part);
                    part_index += 1;
                }
                current.clear();
            }
            let section = trimmed.trim_matches('[').trim_matches(']').trim();
            in_partition = section.to_lowercase() != "general_info";
            continue;
        }

        if let Some(eq) = trimmed.find('=') {
            let key = trimmed[..eq].trim().to_lowercase();
            let val = trimmed[eq + 1..].trim().trim_matches('"').to_string();
            if !in_partition {
                match key.as_str() {
                    "platform" | "chip" | "chipset" => platform = val,
                    "project" | "model" | "product" => project = val,
                    "storage" | "storage_type" | "flash_type" => storage = val,
                    _ => {}
                }
            } else {
                current.insert(key, val);
            }
        }
    }

    if in_partition && !current.is_empty() {
        if let Some(part) = build_partition(&current, part_index) {
            partitions.push(part);
        }
    }

    Ok(Scatter { partitions, platform, project, storage })
}

fn parse_xml(path: &Path) -> Result<Scatter> {
    let content = fs::read_to_string(path)?;
    let mut partitions = Vec::new();
    let mut platform = String::new();
    let mut project = String::new();

    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    let mut in_partition = false;
    let mut in_comment = false;
    let mut current: HashMap<String, String> = HashMap::new();

    while i < len {
        // skip comments
        if i + 3 < len && &bytes[i..i+4] == b"<!--" {
            in_comment = true;
            i += 4;
            continue;
        }
        if in_comment {
            if i + 2 < len && &bytes[i..i+3] == b"-->" {
                in_comment = false;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }

        if bytes[i] != b'<' { i += 1; continue; }

        if i + 1 < len && bytes[i + 1] == b'/' {
            // closing tag
            let close = i + 2;
            let mut end = close;
            while end < len && bytes[end] != b'>' { end += 1; }
            if end >= len { break; }
            let tag_name = content[close..end].trim().to_lowercase();
            if tag_name == "partition" && in_partition {
                if !current.is_empty() {
                    if let Some(part) = build_partition(&current, partitions.len() as u32) {
                        partitions.push(part);
                    }
                    current.clear();
                }
                in_partition = false;
            }
            i = end + 1;
            continue;
        }

        // opening or self-closing tag
        let mut close = i + 1;
        while close < len && bytes[close] != b'>' { close += 1; }
        if close >= len { break; }
        let raw_tag = &content[i + 1..close];
        i = close + 1;

        let self_closing = raw_tag.ends_with('/');
        let tag_str = if self_closing { &raw_tag[..raw_tag.len() - 1] } else { raw_tag };
        let tag_str = tag_str.trim();
        if tag_str.is_empty() { continue; }

        let name_end = tag_str.find(|c: char| c.is_whitespace()).unwrap_or(tag_str.len());
        let tag_name = tag_str[..name_end].to_lowercase();

        // parse attributes
        let rest = tag_str[name_end..].trim();
        let mut attrs = HashMap::new();
        let mut pos = 0;
        let rbytes = rest.as_bytes();
        while pos < rest.len() {
            while pos < rest.len() && rbytes[pos] == b' ' { pos += 1; }
            if pos >= rest.len() { break; }
            let eq = pos + rest[pos..].find('=').unwrap_or(rest.len() - pos);
            if eq >= rest.len() { break; }
            let key = rest[pos..eq].trim().to_lowercase();
            if key.is_empty() { break; }
            pos = eq + 1;
            while pos < rest.len() && rbytes[pos] == b' ' { pos += 1; }
            if pos >= rest.len() || rbytes[pos] != b'"' { break; }
            pos += 1;
            let end = pos + rest[pos..].find('"').unwrap_or(rest.len() - pos);
            let value = rest[pos..end].to_string();
            attrs.insert(key, value);
            pos = end + 1;
        }

        match tag_name.as_str() {
            "scatter" | "flasher" | "config" | "mtk_scatter" => {
                platform = attrs.get("platform").or(attrs.get("chip")).cloned().unwrap_or_default();
                project = attrs.get("project").or(attrs.get("model")).cloned().unwrap_or_default();
            }
            "partition" | "part" => {
                in_partition = true;
                for (k, v) in attrs { current.insert(k, v); }
                if self_closing {
                    if let Some(part) = build_partition(&current, partitions.len() as u32) {
                        partitions.push(part);
                    }
                    current.clear();
                    in_partition = false;
                }
            }
            _ => {}
        }
    }

    Ok(Scatter { partitions, platform, project, storage: String::new() })
}

pub fn parse_scatter(path: &Path) -> Result<Scatter> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "xml" => parse_xml(path),
        _ => parse_txt(path),
    }
}

pub fn verify_scatter(scatter: &Scatter) -> Vec<String> {
    let mut warnings = Vec::new();
    if scatter.partitions.is_empty() {
        warnings.push("no partitions found".to_string());
        return warnings;
    }

    let mut names = std::collections::HashSet::new();

    for p in &scatter.partitions {
        if p.name.is_empty() {
            warnings.push(format!("partition #{}: empty name", p.partition_index));
        } else if !names.insert(p.name.clone()) {
            warnings.push(format!("duplicate: '{}'", p.name));
        }
        if p.filename.is_empty() {
            warnings.push(format!("'{}': no filename", p.name));
        }
        if p.linear_start_addr == 0 && p.partition_size == 0 {
            warnings.push(format!("'{}': zero addr & size", p.name));
        }
        if p.partition_size > 0 && p.partition_size % 512 != 0 {
            warnings.push(format!("'{}': size {} not aligned to 512", p.name, p.partition_size));
        }

    }

    if let Some(ref p) = scatter.partitions.first() {
        if p.linear_start_addr != 0 {
            warnings.push(format!("first partition '{}' should start at 0x0, not 0x{:x}", p.name, p.linear_start_addr));
        }
    }

    warnings
}
