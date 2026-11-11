use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn dir() -> PathBuf {
    crate::settings::data_dir().join("backups").join("profiles")
}

/// Where SPT's own BackupService writes, per its backup.json.
fn spt_dir(host: &Path) -> Option<PathBuf> {
    let root = crate::install::spt_root(host)?;
    let rel = std::fs::read_to_string(root.join("SPT_Data").join("configs").join("backup.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("directory").and_then(|d| d.as_str()).map(String::from))
        .unwrap_or_else(|| "./user/profiles/backups".into());
    Some(root.join(rel.trim_start_matches("./").replace('/', "\\")))
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub nickname: String,
    pub level: i64,
    pub size: u64,
    #[serde(default)]
    pub headless: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub id: String,
    pub created: u64,
    pub reason: String,
    pub profiles: Vec<Entry>,
    /// "app" for ours, "spt" for the server's own
    #[serde(default)]
    pub source: String,
}

fn profile_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .collect()
}

// SPT backups are full profiles, often near a megabyte; parse each file once
static ENTRY_CACHE: Mutex<Option<HashMap<(PathBuf, u64, u64), Option<Entry>>>> = Mutex::new(None);

fn entry_of(f: &Path) -> Option<Entry> {
    let meta = f.metadata().ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = (f.to_path_buf(), meta.len(), mtime);
    if let Some(hit) = ENTRY_CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return hit.clone();
    }
    // activeMods.json and friends have no "info", so they drop out here
    let e = crate::profiles::read(f).map(|p| Entry {
        id: p.id,
        nickname: p.nickname,
        level: p.level,
        size: meta.len(),
        headless: p.headless,
    });
    ENTRY_CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, e.clone());
    e
}

fn own_list() -> Vec<Backup> {
    let Ok(rd) = std::fs::read_dir(dir()) else { return vec![] };
    let mut out: Vec<Backup> = rd
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("backup.json")).ok())
        .filter_map(|s| serde_json::from_str::<Backup>(&s).ok())
        .map(|mut b| {
            b.source = "app".into();
            b
        })
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

/// "2026-11-10_10-28-16" in UTC, which is how SPT names its folders.
fn parse_spt_stamp(name: &str) -> Option<u64> {
    let (d, t) = name.split_once('_')?;
    let n: Vec<i64> = d.split('-').chain(t.split('-')).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [y, mo, da, h, mi, s] = n[..] else { return None };
    // days from civil, Howard Hinnant's algorithm
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + s).ok()
}

fn spt_list(host: &Path) -> Vec<Backup> {
    let Some(d) = spt_dir(host) else { return vec![] };
    let Ok(rd) = std::fs::read_dir(d) else { return vec![] };
    rd.flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let created = parse_spt_stamp(&name)?;
            let profiles: Vec<Entry> = profile_files(&e.path()).iter().filter_map(|f| entry_of(f)).collect();
            (!profiles.is_empty()).then(|| Backup {
                id: name,
                created,
                reason: "spt".into(),
                profiles,
                source: "spt".into(),
            })
        })
        .collect()
}

/// Both kinds, newest first.
pub fn list_all(host: Option<&Path>) -> Vec<Backup> {
    let mut out = own_list();
    if let Some(h) = host {
        out.extend(spt_list(h));
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

fn same_as_latest(files: &[PathBuf]) -> bool {
    let Some(latest) = own_list().into_iter().next() else {
        return false;
    };
    let d = dir().join(&latest.id);
    files.len() == latest.profiles.len()
        && files.iter().all(|f| {
            let name = f.file_name().unwrap_or_default();
            matches!((std::fs::read(f), std::fs::read(d.join(name))), (Ok(a), Ok(b)) if a == b)
        })
}

pub fn create(host: &Path, reason: &str, keep: usize) -> Result<Option<Backup>, String> {
    let files = crate::install::profiles_dir(host).map(|d| profile_files(&d)).unwrap_or_default();
    if files.is_empty() {
        return Err("No profiles to back up.".into());
    }
    // launching over and over shouldn't fill the disk with identical copies
    if reason == "launch" && same_as_latest(&files) {
        return Ok(None);
    }
    let created = crate::history::now();
    let id = format!("{created}-{}", reason.replace(' ', "-"));
    let d = dir().join(&id);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let mut profiles = vec![];
    for f in &files {
        let name = f.file_name().unwrap_or_default();
        std::fs::copy(f, d.join(name)).map_err(|e| format!("Couldn't copy {}: {e}", f.display()))?;
        let p = crate::profiles::read(f);
        profiles.push(Entry {
            id: f.file_stem().unwrap_or_default().to_string_lossy().into(),
            nickname: p.as_ref().map(|p| p.nickname.clone()).unwrap_or_default(),
            level: p.as_ref().map(|p| p.level).unwrap_or(0),
            size: f.metadata().map(|m| m.len()).unwrap_or(0),
            headless: p.as_ref().is_some_and(|p| p.headless),
        });
    }
    let b = Backup {
        id,
        created,
        reason: reason.into(),
        profiles,
        source: "app".into(),
    };
    std::fs::write(d.join("backup.json"), serde_json::to_string_pretty(&b).unwrap_or_default()).map_err(|e| e.to_string())?;
    prune(keep);
    Ok(Some(b))
}

/// Only automatic backups age out; ones made by hand or before a restore stay.
fn prune(keep: usize) {
    let auto: Vec<Backup> = own_list().into_iter().filter(|b| b.reason == "launch").collect();
    for b in auto.iter().skip(keep.max(1)) {
        let _ = std::fs::remove_dir_all(dir().join(&b.id));
    }
}

/// Ours only. SPT rotates its own backups.
pub fn delete(id: &str) -> Result<(), String> {
    let d = dir().join(safe(id)?);
    std::fs::remove_dir_all(d).map_err(|e| e.to_string())
}

fn safe(s: &str) -> Result<&str, String> {
    // exactly one plain component, so ".", "C:x" and "\\x" can't redirect the join
    let mut c = Path::new(s).components();
    match (c.next(), c.next()) {
        (Some(std::path::Component::Normal(_)), None) if !s.contains(':') => Ok(s),
        _ => Err("Bad name.".into()),
    }
}

/// Copies the chosen profiles from a backup over the live ones. An empty list means all of them.
pub fn restore(host: &Path, source: &str, id: &str, only: &[String], keep: usize) -> Result<usize, String> {
    let src_dir = match source {
        "spt" => spt_dir(host).ok_or("SPT backup folder not found.")?.join(safe(id)?),
        _ => dir().join(safe(id)?),
    };
    let mut picked = vec![];
    for f in profile_files(&src_dir) {
        let stem = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
        if (only.is_empty() || only.contains(&stem)) && entry_of(&f).is_some() {
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&f).map_err(|e| e.to_string())?)
                .map_err(|_| format!("{stem}.json in that backup is damaged, nothing was restored."))?;
            picked.push((stem, f));
        }
    }
    if picked.is_empty() {
        return Err("None of those profiles are in this backup.".into());
    }
    let dst = crate::install::profiles_dir(host).ok_or("Profiles folder not found.")?;
    // keep what's there now, in case the restore was a mistake
    create(host, "before restore", keep)?;
    for (stem, f) in &picked {
        let bytes = std::fs::read(f).map_err(|e| format!("{stem}: {e}"))?;
        crate::settings::write_atomic(&dst.join(format!("{stem}.json")), &bytes).map_err(|e| format!("{stem}: {e}"))?;
    }
    Ok(picked.len())
}

pub fn open_dir() -> PathBuf {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    d
}
