use crate::Core;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tauri_winrt_notification::{Sound, Toast};

const APP_ID: &str = "dev.20fpsguy.fikahub";

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn init(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

/// A portable exe has no Start menu shortcut, so Windows drops toasts from it unless
/// the AUMID is registered by hand. Per-user and harmless to repeat.
pub fn register() {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let _ = hkcu.delete_subkey_all(r"Software\Classes\AppUserModelId\dev.20fpsguy.fikacontrol");
    let Ok((key, _)) = hkcu.create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}")) else {
        return;
    };
    let _ = key.set_value("DisplayName", &"Fika Hub");
    let _ = key.set_value("IconUri", &icon().to_string_lossy().to_string());
}

fn icon() -> std::path::PathBuf {
    let icon = crate::settings::data_dir().join("icon.png");
    let logo: &[u8] = include_bytes!("../../ui/logo.png");
    if std::fs::read(&icon).ok().as_deref() != Some(logo) {
        let _ = std::fs::create_dir_all(icon.parent().unwrap());
        let _ = std::fs::write(&icon, logo);
    }
    icon
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Crash,
    Players,
    /// server ready, raid finished
    Info,
}

/// Where a click on the toast takes you.
#[derive(Clone, Copy)]
pub enum Open {
    Log(usize),
    Raids,
    Dashboard,
}

impl Open {
    fn arg(self) -> String {
        match self {
            Open::Log(i) => format!("console:{i}"),
            Open::Raids => "raids".into(),
            Open::Dashboard => "dashboard".into(),
        }
    }
}

fn hub_in_front() -> bool {
    let Some(w) = APP.get().and_then(|a| a.get_webview_window("main")) else {
        return false;
    };
    w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
}

fn bring_up(arg: &str) {
    let Some(app) = APP.get() else { return };
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    let _ = app.emit("open-page", arg.to_string());
}

/// The one way crashes, players and raid results reach the desktop. Runs in the
/// backend so a minimized window, whose timers Windows throttles, can't delay it.
pub fn alert(core: &Core, kind: Kind, title: &str, body: &str, open: Open) {
    let s = core.settings.lock().unwrap().clone();
    let on = match kind {
        Kind::Crash => s.notify_crash,
        Kind::Players => s.notify_players,
        Kind::Info => s.notify_ready,
    };
    // in front, the dashboard already says it
    if !on || hub_in_front() {
        return;
    }
    // a crash loop or a burst of joins shouldn't stack ten toasts
    static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    {
        let mut g = LAST.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        m.retain(|_, t| t.elapsed() < Duration::from_secs(60));
        if m.contains_key(title) {
            return;
        }
        m.insert(title.to_string(), Instant::now());
    }
    show(kind, title, body, open);
}

fn show(kind: Kind, title: &str, body: &str, open: Open) {
    let (title, body, arg) = (title.to_string(), body.to_string(), open.arg());
    // WinRT calls can block for a moment, keep them off the caller's thread
    std::thread::spawn(move || {
        let (line1, line2) = match body.split_once('\n') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (body, String::new()),
        };
        let button = if arg.starts_with("console") { "Show log" } else { "Open" };
        let click = arg.clone();
        let mut t = Toast::new(APP_ID)
            .title(&title)
            .text1(&line1)
            .sound(Some(if kind == Kind::Crash { Sound::Reminder } else { Sound::Default }))
            .add_button(button, &arg)
            .on_activated(move |a| {
                bring_up(a.as_deref().unwrap_or(&click));
                Ok(())
            });
        if !line2.is_empty() {
            t = t.text2(&line2);
        }
        let _ = t.show();
    });
}

/// For the Settings test button.
pub fn test() {
    show(Kind::Info, "Fika Hub", "Notifications work.", Open::Dashboard);
}

/// Windows exit codes that say more than a number.
pub fn exit_reason(code: Option<i32>) -> String {
    let Some(c) = code else {
        return "It was killed or vanished".into();
    };
    let what = match c as u32 {
        0 => "It quit normally",
        0xC000_0005 => "Access violation (a native crash, often a bad mod or driver)",
        0xC000_00FD => "Stack overflow (usually a mod stuck in a loop)",
        0xC000_0409 => "Fast-fail crash (a native error the process couldn't survive)",
        0xC000_0017 | 0xC000_012D => "Out of memory",
        0xE043_4352 => "Unhandled .NET exception",
        0xC000_013A => "Closed with Ctrl+C or a console close",
        0x4001_0004 => "Killed by Windows or Task Manager",
        1 => "Exited with an error",
        _ => "",
    };
    if what.is_empty() {
        format!("Exit code {c}")
    } else {
        format!("{what} (exit code {c})")
    }
}
