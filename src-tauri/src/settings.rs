use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub host_dir: Option<String>,
    pub player_dir: Option<String>,
    /// "", "Windowed", "Borderless Fullscreen" or "Fullscreen". Empty leaves the game alone.
    pub display_mode: String,
    pub minimize_on_launch: bool,
    pub warn_mismatch: bool,
    pub auto_restart_headless: bool,
    pub stop_with_game: bool,
    pub last_profile: Option<String>,
    pub launch_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host_dir: None,
            player_dir: None,
            display_mode: String::new(),
            minimize_on_launch: false,
            warn_mismatch: true,
            auto_restart_headless: true,
            stop_with_game: true,
            last_profile: None,
            launch_mode: "full".into(),
        }
    }
}

fn path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("FikaControl").join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, json);
        }
    }
}
