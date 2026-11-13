use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};

#[derive(Serialize, Deserialize, Clone, Copy, Default)]
pub struct Geom {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    max: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    window: Option<Geom>,
    #[serde(default)]
    ui: Value,
}

static FILE: Mutex<Option<File>> = Mutex::new(None);

fn path() -> std::path::PathBuf {
    crate::settings::data_dir().join("ui.json")
}

fn with<R>(f: impl FnOnce(&mut File) -> R) -> R {
    let mut g = FILE.lock().unwrap();
    let file = g.get_or_insert_with(|| {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    f(file)
}

fn save() {
    let text = with(|f| serde_json::to_string_pretty(f).unwrap_or_default());
    let _ = std::fs::create_dir_all(crate::settings::data_dir());
    let _ = std::fs::write(path(), text);
}

pub fn get() -> Value {
    with(|f| f.ui.clone())
}

pub fn set(ui: Value) {
    with(|f| f.ui = ui);
    save();
}

// Tarkov shoves the window onto its own screen, so moves while it runs aren't the user's
static GAME_UP: AtomicBool = AtomicBool::new(false);

pub fn set_game_up(up: bool) {
    GAME_UP.store(up, Relaxed);
}

/// Called on move and resize. Keeps the last normal size while maximized so un-maximizing lands right.
pub fn track(w: &tauri::Window) {
    if GAME_UP.load(Relaxed) || w.is_minimized().unwrap_or(false) {
        return;
    }
    let max = w.is_maximized().unwrap_or(false);
    with(|f| {
        let mut g = f.window.unwrap_or_default();
        g.max = max;
        if !max {
            if let (Ok(p), Ok(s)) = (w.outer_position(), w.inner_size()) {
                (g.x, g.y, g.w, g.h) = (p.x, p.y, s.width, s.height);
            }
        }
        f.window = Some(g);
    });
}

pub fn flush() {
    if with(|f| f.window.is_some()) {
        save();
    }
}

pub fn restore(w: &WebviewWindow) {
    let Some(g) = with(|f| f.window) else { return };
    // skip a spot that's now off every monitor, e.g. an unplugged second screen
    let on_screen = w.available_monitors().unwrap_or_default().iter().any(|m| {
        let (p, s) = (m.position(), m.size());
        g.x + 80 > p.x && g.x < p.x + s.width as i32 - 80 && g.y >= p.y - 10 && g.y < p.y + s.height as i32 - 60
    });
    if g.w >= 600 && g.h >= 400 {
        let _ = w.set_size(PhysicalSize::new(g.w, g.h));
    }
    if on_screen {
        let _ = w.set_position(PhysicalPosition::new(g.x, g.y));
    }
    if g.max {
        let _ = w.maximize();
    }
}

/// On focus while the game runs, puts the window back on the screen it was saved on.
pub fn snap_back(w: &tauri::Window) {
    if !GAME_UP.load(Relaxed) {
        return;
    }
    let w = w.clone();
    // let the restore from minimized settle before reading where it landed
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let Some(g) = with(|f| f.window) else { return };
        if w.is_minimized().unwrap_or(false) {
            return;
        }
        let (cx, cy) = (g.x + g.w as i32 / 2, g.y + g.h as i32 / 2);
        let contains = |m: &tauri::Monitor| {
            let (p, s) = (m.position(), m.size());
            cx >= p.x && cx < p.x + s.width as i32 && cy >= p.y && cy < p.y + s.height as i32
        };
        let monitors = w.available_monitors().unwrap_or_default();
        let Some(home) = monitors.iter().find(|m| contains(m)) else { return };
        let Ok(Some(now)) = w.current_monitor() else { return };
        if now.position() == home.position() {
            return;
        }
        let was_max = w.is_maximized().unwrap_or(false);
        if was_max {
            let _ = w.unmaximize();
        }
        let _ = w.set_position(PhysicalPosition::new(g.x, g.y));
        if g.max || was_max {
            let _ = w.maximize();
        }
    });
}
