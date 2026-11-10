use tauri_winrt_notification::Toast;

const APP_ID: &str = "dev.20fpsguy.fikahub";

/// A portable exe has no Start menu shortcut, so Windows drops toasts from it unless
/// the AUMID is registered by hand. Per-user and harmless to repeat.
pub fn register() {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let _ = hkcu.delete_subkey_all(r"Software\Classes\AppUserModelId\dev.20fpsguy.fikacontrol");
    let Ok((key, _)) = hkcu.create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}")) else {
        return;
    };
    let _ = key.set_value("DisplayName", &"Fika Hub");
    let icon = crate::settings::data_dir().join("icon.png");
    let logo: &[u8] = include_bytes!("../../ui/logo.png");
    if std::fs::read(&icon).ok().as_deref() != Some(logo) {
        let _ = std::fs::create_dir_all(icon.parent().unwrap());
        let _ = std::fs::write(&icon, logo);
    }
    let _ = key.set_value("IconUri", &icon.to_string_lossy().to_string());
}

pub fn show(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    // WinRT calls can block for a moment, keep them off the IPC thread
    std::thread::spawn(move || {
        let _ = Toast::new(APP_ID).title(&title).text1(&body).show();
    });
}
