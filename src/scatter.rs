use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
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
    Validation(Vec<String>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {}", e),
            Error::Parse(s) => write!(f, "parse: {}", s),
            Error::Validation(errs) => write!(f, "validation: {}", errs.join("; ")),
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
        u64::from_str_radix(hex, 16).map_err(|e| Error::Parse(format!("invalid hex {}: {}", s, e)))
    } else {
        s.parse::<u64>().map_err(|e| Error::Parse(format!("invalid int {}: {}", s, e)))
    }
}

fn build_partition(map: &HashMap<String, String>, idx: u32) -> Option<Partition> {
    let name = map.get("name")?.clone();
    let filename = map.get("filename").cloned().unwrap_or_else(|| format!("{}.img", name));
    let linear_start_addr = map.get("linear_start_addr").and_then(|s| parse_int(s).ok()).unwrap_or(0);
    let partition_size = map.get("partition_size").and_then(|s| parse_int(s).ok()).unwrap_or(0);
    let physical_start_addr = map.get("physical_start_addr").and_then(|s| parse_int(s).ok()).unwrap_or(0);
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
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") { continue; }

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
                    "platform" => platform = val,
                    "project" => project = val,
                    "storage" => storage = val,
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
    let storage = String::new();

    let mut in_partition = false;
    let mut current = HashMap::new();
    let mut tag_stack: Vec<String> = Vec::new();

    let mut i = 0;
    let bytes = content.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'<' { i += 1; continue; }
        let mut close = i + 1;
        while close < bytes.len() && bytes[close] != b'>' { close += 1; }
        if close >= bytes.len() { break; }
        let tag = &content[i + 1..close];
        i = close + 1;

        if let Some(stripped) = tag.strip_prefix('/') {
            let tag_name = stripped.trim().to_lowercase();
            if tag_name == "partition" && in_partition {
                if !current.is_empty() {
                    if let Some(part) = build_partition(&current, partitions.len() as u32) {
                        partitions.push(part);
                    }
                    current.clear();
                }
                in_partition = false;
            }
            if tag_stack.last().map(|s| s == &tag_name).unwrap_or(false) {
                tag_stack.pop();
            }
        } else if !tag.ends_with('/') {
            let tag_parts: Vec<&str> = tag.split_whitespace().collect();
            let tag_name = tag_parts[0].to_lowercase();
            tag_stack.push(tag_name.clone());

            let rest = tag[tag_parts[0].len()..].trim();
            let mut attrs = HashMap::new();
            let mut pos = 0;
            while pos < rest.len() {
                while pos < rest.len() && rest.as_bytes()[pos] == b' ' { pos += 1; }
                if pos >= rest.len() { break; }
                let eq = pos + rest[pos..].find('=').unwrap_or(rest.len() - pos);
                let key = rest[pos..eq].trim().trim_matches('"').to_lowercase();
                pos = eq + 1;
                if pos >= rest.len() || rest.as_bytes()[pos] != b'"' { break; }
                pos += 1;
                let end = pos + rest[pos..].find('"').unwrap_or(rest.len() - pos);
                let value = rest[pos..end].to_string();
                attrs.insert(key, value);
                pos = end + 1;
            }

            match tag_name.as_str() {
                "scatter" | "flasher" => {
                    platform = attrs.get("platform").cloned().unwrap_or_default();
                    project = attrs.get("project").cloned().unwrap_or_default();
                }
                "partition" => {
                    in_partition = true;
                    for (k, v) in attrs { current.insert(k, v); }
                }
                _ => {}
            }
        }
    }

    Ok(Scatter { partitions, platform, project, storage })
}

pub fn parse_scatter(path: &Path) -> Result<Scatter> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "xml" => parse_xml(path),
        _ => parse_txt(path),
    }
}

pub fn verify_scatter(scatter: &Scatter) -> Vec<String> {
    let mut errors = Vec::new();
    if scatter.partitions.is_empty() {
        errors.push("no partitions found".to_string());
        return errors;
    }

    let mut names = std::collections::HashSet::new();
    for p in &scatter.partitions {
        if p.name.is_empty() {
            errors.push(format!("partition #{} has empty name", p.partition_index));
        } else if !names.insert(p.name.clone()) {
            errors.push(format!("duplicate partition name: {}", p.name));
        }
        if p.filename.is_empty() {
            errors.push(format!("partition '{}' has no filename", p.name));
        }
        if p.linear_start_addr == 0 && p.partition_size == 0 {
            errors.push(format!("partition '{}' has zero address and size", p.name));
        }
    }
    errors
}
