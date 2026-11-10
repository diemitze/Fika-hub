use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

const KEEP: usize = 3000;

pub fn path() -> PathBuf {
    crate::settings::data_dir().join("history.jsonl")
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn record(kind: &str, detail: Value) {
    let mut ev = json!({ "t": now(), "kind": kind });
    if let (Some(m), Value::Object(d)) = (ev.as_object_mut(), detail) {
        m.extend(d);
    }
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{ev}");
    }
}

pub fn read(limit: usize) -> Vec<Value> {
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(limit)..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Called once at startup so the file can't grow forever.
pub fn trim() {
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > KEEP * 2 {
        let _ = std::fs::write(path(), lines[lines.len() - KEEP..].join("\n") + "\n");
    }
}
