use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const EXTS: &[&str] = &["json", "jsonc", "json5", "ini", "cfg", "conf", "toml", "yaml", "yml", "xml", "properties"];
const SKIP_DIRS: &[&str] = &["node_modules", ".git", ".fikarunner-backups", "bin", "obj"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub key: String,
    pub label: String,
    pub path: String,
    pub exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub group: String,
    pub rel: String,
    pub name: String,
    pub size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    pub text: String,
    pub stamp: String,
    pub read_only: Option<String>,
}

pub fn roots(host: Option<&Path>, player: Option<&Path>) -> Vec<Root> {
    let mut out = vec![];
    let mut add = |key: &str, label: &str, p: Option<PathBuf>| {
        if let Some(p) = p {
            out.push(Root { key: key.into(), label: label.into(), exists: p.is_dir(), path: p.to_string_lossy().into() });
        }
    };
    add("server", "Server", host.and_then(crate::install::spt_root).map(|r| r.join("user").join("mods")));
    add("headless", "Headless", host.map(|h| h.join("BepInEx").join("config")));
    add("game", "Game", player.map(|p| p.join("BepInEx").join("config")));
    out
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn scan(root: &Path, all: bool) -> Vec<Entry> {
    let mut out = vec![];
    walk(root, root, all, &mut out);
    out.sort_by(|a, b| (a.group.to_lowercase(), a.rel.to_lowercase()).cmp(&(b.group.to_lowercase(), b.rel.to_lowercase())));
    out
}

fn walk(root: &Path, dir: &Path, all: bool, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(root, &p, all, out);
            }
            continue;
        }
        let ext = ext_of(&p);
        if !EXTS.contains(&ext.as_str()) || meta.len() > MAX_BYTES {
            continue;
        }
        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_string();
        let parts: Vec<&str> = rel.split('\\').collect();
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let likely = stem.contains("config")
            || stem.contains("setting")
            || parts.iter().skip(1).any(|x| matches!(x.to_lowercase().as_str(), "config" | "configs" | "settings"))
            || matches!(ext.as_str(), "ini" | "cfg" | "conf" | "toml" | "properties");
        if !all && !likely {
            continue;
        }
        let group = if parts.len() > 1 {
            parts[0].to_string()
        } else {
            // BepInEx config files sit flat; group by the plugin's own prefix
            stem_group(&p)
        };
        out.push(Entry {
            group,
            name: p.file_name().unwrap_or_default().to_string_lossy().into(),
            rel,
            size: meta.len(),
        });
    }
}

fn stem_group(p: &Path) -> String {
    let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
    stem.split(['.', '_']).next().unwrap_or(&stem).to_string()
}

/// Resolves a relative path and refuses anything that escapes the root.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let full = root.join(rel);
    let canon_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let canon = std::fs::canonicalize(&full).map_err(|e| e.to_string())?;
    if !canon.starts_with(&canon_root) {
        return Err("That file is outside the config folder.".into());
    }
    Ok(full)
}

fn stamp(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:x}-{}", h.finish(), bytes.len())
}

pub fn read(path: &Path) -> Result<Doc, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("File is over the 2 MB editor limit.".into());
    }
    let st = stamp(&bytes);
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok(Doc { text: String::new(), stamp: st, read_only: Some("UTF-16 file, open it in a text editor.".into()) });
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    match std::str::from_utf8(body) {
        Ok(t) => Ok(Doc { text: t.to_string(), stamp: st, read_only: None }),
        Err(_) => Ok(Doc { text: String::from_utf8_lossy(body).into(), stamp: st, read_only: Some("Not valid UTF-8, editing could damage it.".into()) }),
    }
}

/// JSON with comments is common in SPT configs, so strip them before parsing.
fn strip_json_comments(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

pub fn validate(name: &str, text: &str) -> Result<String, String> {
    let ext = ext_of(Path::new(name));
    match ext.as_str() {
        "json" | "jsonc" => serde_json::from_str::<serde_json::Value>(&strip_json_comments(text))
            .map(|_| "Valid JSON.".to_string())
            .map_err(|e| format!("JSON error at line {}, column {}: {}", e.line(), e.column(), e)),
        "xml" => roxmltree::Document::parse(text).map(|_| "Valid XML.".to_string()).map_err(|e| format!("XML error: {e}")),
        _ => Ok(format!(".{ext} isn't syntax-checked.")),
    }
}

pub fn save(path: &Path, text: &str, expected: &str) -> Result<String, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    validate(&name, text)?;
    let current = std::fs::read(path).map_err(|e| e.to_string())?;
    let body = current.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&current);
    if current.starts_with(&[0xFF, 0xFE]) || current.starts_with(&[0xFE, 0xFF]) || std::str::from_utf8(body).is_err() {
        return Err("This file isn't plain UTF-8, so it can't be saved from here.".into());
    }
    if stamp(&current) != expected {
        return Err("The file changed on disk since you opened it. Reload first.".into());
    }
    let bom = current.starts_with(&[0xEF, 0xBB, 0xBF]);
    let mut bytes = if bom { vec![0xEF, 0xBB, 0xBF] } else { vec![] };
    bytes.extend_from_slice(text.as_bytes());

    let dir = path.parent().ok_or("bad path")?;
    let backups = dir.join(".fikarunner-backups");
    std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let backup = backups.join(format!("{name}.{ts}.bak"));
    std::fs::write(&backup, &current).map_err(|e| e.to_string())?;

    let tmp = dir.join(format!("{name}.{ts}.tmp"));
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(stamp(&bytes))
}
