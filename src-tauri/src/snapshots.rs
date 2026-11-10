use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn dir(root_key: &str) -> PathBuf {
    crate::settings::data_dir().join("snapshots").join(root_key)
}

fn safe(s: &str) -> Result<&str, String> {
    // exactly one plain component, so ".", "C:x" and "\\x" can't redirect the join
    let mut c = Path::new(s).components();
    match (c.next(), c.next()) {
        (Some(std::path::Component::Normal(_)), None) if !s.contains(':') => Ok(s),
        _ => Err("Bad name.".into()),
    }
}

fn safe_rel(rel: &str) -> Result<&str, String> {
    let ok = !rel.is_empty() && !rel.contains(':') && Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if !ok {
        return Err("Bad path.".into());
    }
    Ok(rel)
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub created: u64,
    pub files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub rel: String,
    /// "changed", "missing" (deleted since) or "added" (new since)
    pub status: String,
}

pub fn list(root_key: &str) -> Vec<Snapshot> {
    let Ok(rd) = std::fs::read_dir(dir(root_key)) else { return vec![] };
    let mut out: Vec<Snapshot> = rd
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("snapshot.json")).ok())
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

pub fn create(root_key: &str, root: &Path, name: &str) -> Result<Snapshot, String> {
    let created = crate::history::now();
    let id = created.to_string();
    let base = dir(safe(root_key)?).join(&id);
    let files: Vec<String> = crate::configs::scan(root, false).into_iter().map(|e| e.rel).collect();
    if files.is_empty() {
        return Err("No config files to snapshot.".into());
    }
    for rel in &files {
        let dst = base.join("files").join(rel);
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(root.join(rel), &dst).map_err(|e| format!("{rel}: {e}"))?;
    }
    let name = if name.trim().is_empty() {
        "Snapshot".to_string()
    } else {
        name.trim().to_string()
    };
    let s = Snapshot { id, name, created, files };
    std::fs::write(base.join("snapshot.json"), serde_json::to_string_pretty(&s).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(s)
}

fn load(root_key: &str, id: &str) -> Result<(Snapshot, PathBuf), String> {
    let base = dir(safe(root_key)?).join(safe(id)?);
    let s: Snapshot = std::fs::read_to_string(base.join("snapshot.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or("Snapshot not found.")?;
    Ok((s, base.join("files")))
}

pub fn compare(root_key: &str, id: &str, root: &Path) -> Result<Vec<Change>, String> {
    let (snap, files) = load(root_key, id)?;
    let mut out = vec![];
    for rel in &snap.files {
        let old = std::fs::read(files.join(rel)).ok();
        match std::fs::read(root.join(rel)) {
            Err(_) => out.push(Change {
                rel: rel.clone(),
                status: "missing".into(),
            }),
            Ok(now) if Some(&now) != old.as_ref() => out.push(Change {
                rel: rel.clone(),
                status: "changed".into(),
            }),
            _ => {}
        }
    }
    for e in crate::configs::scan(root, false) {
        if !snap.files.contains(&e.rel) {
            out.push(Change {
                rel: e.rel,
                status: "added".into(),
            });
        }
    }
    Ok(out)
}

pub fn read_file(root_key: &str, id: &str, rel: &str) -> Result<String, String> {
    let (_, files) = load(root_key, id)?;
    let b = std::fs::read(files.join(safe_rel(rel)?)).map_err(|_| "Not in this snapshot.".to_string())?;
    Ok(String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b)).into_owned())
}

/// Writes snapshot copies back. Current files get the same .bak treatment as an
/// editor save first, and files added since the snapshot are left alone.
pub fn restore(root_key: &str, id: &str, root: &Path, only: Option<Vec<String>>) -> Result<usize, String> {
    let (snap, files) = load(root_key, id)?;
    let mut n = 0;
    for rel in snap.files.iter().filter(|r| only.as_ref().is_none_or(|o| o.contains(r))) {
        let src = files.join(safe_rel(rel)?);
        let dst = root.join(rel);
        let old = std::fs::read(&dst).ok();
        let new = std::fs::read(&src).map_err(|e| e.to_string())?;
        if old.as_ref() == Some(&new) {
            continue;
        }
        if let (Some(old), Some(parent)) = (&old, dst.parent()) {
            let name = dst.file_name().unwrap_or_default().to_string_lossy().to_string();
            let backups = parent.join(".fikarunner-backups");
            std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            std::fs::write(backups.join(format!("{name}.{ts}.bak")), old).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&dst, &new).map_err(|e| format!("{rel}: {e}"))?;
        n += 1;
    }
    Ok(n)
}

pub fn delete(root_key: &str, id: &str) -> Result<(), String> {
    std::fs::remove_dir_all(dir(safe(root_key)?).join(safe(id)?)).map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBackup {
    pub name: String,
    pub saved: u64,
    pub size: u64,
}

/// The .bak files the editor keeps next to a config on every save.
pub fn file_backups(path: &Path) -> Vec<FileBackup> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return vec![];
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(rd) = std::fs::read_dir(parent.join(".fikarunner-backups")) else {
        return vec![];
    };
    let mut out: Vec<FileBackup> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let ts = n.strip_prefix(&prefix)?.strip_suffix(".bak")?.parse::<u64>().ok()?;
            Some(FileBackup {
                saved: ts / 1000,
                size: e.metadata().map(|m| m.len()).unwrap_or(0),
                name: n,
            })
        })
        .collect();
    out.sort_by(|a, b| b.saved.cmp(&a.saved));
    out
}

pub fn read_file_backup(path: &Path, name: &str) -> Result<String, String> {
    let p = path.parent().ok_or("bad path")?.join(".fikarunner-backups").join(safe(name)?);
    let b = std::fs::read(p).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b)).into_owned())
}
