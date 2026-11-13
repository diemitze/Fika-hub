use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub host_dir: Option<String>,
    pub player_dir: Option<String>,
    /// only set when the headless lives apart from the server install
    pub headless_dir: Option<String>,
    /// "", "Windowed", "Borderless Fullscreen" or "Fullscreen". Empty leaves the game alone.
    pub display_mode: String,
    pub minimize_on_launch: bool,
    pub warn_mismatch: bool,
    pub auto_restart_headless: bool,
    pub auto_restart_server: bool,
    pub stop_with_game: bool,
    pub last_profile: Option<String>,
    pub launch_mode: String,
    /// host[:port] for the game-only and headless-only modes. Empty means this PC.
    pub server_address: String,
    /// headless profile ID for when the server (and its profiles) is on another machine
    pub headless_profile: String,
    pub notify_crash: bool,
    pub notify_players: bool,
    /// server finished booting, raid summaries
    pub notify_ready: bool,
    /// "root|rel" keys of config files
    pub pinned_configs: Vec<String>,
    pub recent_configs: Vec<String>,
    pub check_before_launch: bool,
    pub backup_on_launch: bool,
    pub backup_keep: usize,
    pub restart_enabled: bool,
    /// local "HH:MM"
    pub restart_time: String,
    pub restart_wait_raids: bool,
    pub discord_enabled: bool,
    pub discord_webhook: String,
    pub discord_interval: u32,
    pub discord_events: bool,
    pub discord_crashes: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host_dir: None,
            player_dir: None,
            headless_dir: None,
            display_mode: String::new(),
            minimize_on_launch: false,
            warn_mismatch: true,
            auto_restart_headless: true,
            auto_restart_server: true,
            stop_with_game: true,
            last_profile: None,
            launch_mode: "full".into(),
            server_address: String::new(),
            headless_profile: String::new(),
            notify_crash: true,
            notify_players: false,
            notify_ready: true,
            pinned_configs: vec![],
            recent_configs: vec![],
            check_before_launch: true,
            backup_on_launch: true,
            backup_keep: 20,
            restart_enabled: false,
            restart_time: "05:00".into(),
            restart_wait_raids: true,
            discord_enabled: false,
            discord_webhook: String::new(),
            discord_interval: 5,
            discord_events: true,
            discord_crashes: true,
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()));
    let dir = base.join("FikaHub");
    static MIGRATE: std::sync::Once = std::sync::Once::new();
    MIGRATE.call_once(|| merge_into(&base.join("FikaControl"), &dir));
    dir
}

/// Moves the Fika Control data over, keeping the newer file where both exist.
fn merge_into(old: &std::path::Path, new: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(old) else { return };
    let _ = std::fs::create_dir_all(new);
    let mtime = |p: &std::path::Path| p.metadata().and_then(|m| m.modified()).ok();
    for e in rd.flatten() {
        let (from, to) = (e.path(), new.join(e.file_name()));
        if from.is_dir() && to.is_dir() {
            merge_into(&from, &to);
        } else if !to.exists() || (from.is_file() && to.is_file() && mtime(&from) > mtime(&to)) {
            // rename replaces the file in one step on Windows, so a failure leaves both copies
            let _ = std::fs::rename(&from, &to);
        } else if from.is_file() && to.is_file() {
            let _ = std::fs::remove_file(&from);
        }
    }
    // only goes once everything above actually moved out
    let _ = std::fs::remove_dir(old);
}

fn path() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let Ok(s) = std::fs::read_to_string(path()) else {
            return Self::default();
        };
        serde_json::from_str(&s).unwrap_or_else(|_| {
            // keep the broken file around, the next save would otherwise overwrite it for good
            let _ = std::fs::copy(path(), path().with_extension("json.bad"));
            Self::default()
        })
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = write_atomic(&p, json.as_bytes());
        }
    }
}

/// Writes next to the target and renames over it, so a crash mid-write never leaves a truncated file.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
