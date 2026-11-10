use crate::{install, profiles, Core};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// "ok", "warn" or "bad"
    pub level: &'static str,
    pub title: String,
    pub fix: Option<String>,
    /// page the UI can jump to for the fix
    pub go: Option<&'static str>,
}

fn ok(title: impl Into<String>) -> Check {
    Check {
        level: "ok",
        title: title.into(),
        fix: None,
        go: None,
    }
}
fn warn(title: impl Into<String>, fix: impl Into<String>, go: Option<&'static str>) -> Check {
    Check {
        level: "warn",
        title: title.into(),
        fix: Some(fix.into()),
        go,
    }
}
fn bad(title: impl Into<String>, fix: impl Into<String>, go: Option<&'static str>) -> Check {
    Check {
        level: "bad",
        title: title.into(),
        fix: Some(fix.into()),
        go,
    }
}

fn exe_running(sys: &sysinfo::System, exe: &Path) -> bool {
    let want = install::norm(exe);
    sys.processes().values().any(|p| p.exe().is_some_and(|e| install::norm(e) == want))
}

pub fn run(core: &Core, mode: &str, profile: Option<&str>) -> Vec<Check> {
    let s = core.settings.lock().unwrap().clone();
    let host = crate::host_dir(&s);
    let player = s.player_dir.as_ref().map(PathBuf::from);
    let headless = crate::headless_dir(&s);
    let wants_host = mode != "join";
    let wants_game = mode != "host";
    let mut out = vec![];

    match (&host, &s.host_dir) {
        (Some(h), _) => out.push(ok(format!("Host install found at {}", h.display()))),
        (None, Some(d)) if wants_host => out.push(bad(
            install::host_problem(Path::new(d)).unwrap_or_else(|| "Host install isn't usable.".into()),
            "Pick the folder with SPT_Runtime in Settings.",
            Some("settings"),
        )),
        (None, _) if wants_host => out.push(bad("No host install set.", "Pick it in Settings.", Some("settings"))),
        _ => {}
    }
    if wants_host {
        match (&headless, &s.headless_dir) {
            (Some(h), _) => out.push(ok(format!("Headless install found at {}", h.display()))),
            (None, Some(d)) => out.push(warn(
                install::headless_problem(Path::new(d)).unwrap_or_else(|| "Headless install isn't usable.".into()),
                "Pick the folder with EscapeFromTarkov.exe and Fika.Headless.dll in Settings, or the headless is skipped.",
                Some("settings"),
            )),
            (None, None) => out.push(warn(
                "No Fika headless install found.",
                "If the headless lives in its own folder, set it under Settings > Installs, or the headless is skipped.",
                Some("settings"),
            )),
        }
    }
    if wants_game {
        match player.as_deref().map(|p| (p, install::player_problem(p))) {
            Some((_, None)) => out.push(ok("Game install found")),
            Some((_, Some(e))) => out.push(bad(e, "Pick the copy you play on in Settings.", Some("settings"))),
            None => out.push(bad(
                "No game install set.",
                "Pick the copy you play on in Settings.",
                Some("settings"),
            )),
        }
    }

    if let (Some(h), Some(p)) = (host.as_deref(), player.as_deref()) {
        if wants_game && wants_host {
            let (sh, sp) = (install::spt_version(h), install::spt_version(p));
            let (fh, fp) = (install::fika_version(headless.as_deref().unwrap_or(h)), install::fika_version(p));
            match (sh == sp || sp.is_none(), fh == fp) {
                (true, true) => out.push(ok(format!(
                    "Versions match (SPT {}, Fika {})",
                    sh.unwrap_or_default(),
                    fh.unwrap_or_default()
                ))),
                _ => out.push(bad(
                    format!("Versions differ: host SPT {:?} Fika {:?}, game SPT {:?} Fika {:?}", sh, fh, sp, fp),
                    "Update the older install so both match, or you won't connect.",
                    None,
                )),
            }
        }
    }

    let port = host.as_deref().map(install::server_port).unwrap_or(6969);
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
    );
    let active = core.stack.active.load(std::sync::atomic::Ordering::SeqCst);
    if wants_host && !active {
        let spt_running = host
            .as_deref()
            .and_then(install::spt_root)
            .is_some_and(|r| exe_running(&sys, &r.join("SPT.Server.exe")));
        if spt_running {
            out.push(warn(
                "An SPT server from this install is already running outside Fika Hub.",
                "Fika Hub will use it, but can't restart or watch it. Close it first for full control.",
                None,
            ));
        } else if install::port_listening(port) {
            out.push(bad(
                format!("Something else is already using port {port}."),
                "Close whatever holds that port, or change the port in SPT's http.json.",
                None,
            ));
        } else {
            out.push(ok(format!("Port {port} is free")));
        }
        if headless
            .as_deref()
            .is_some_and(|h| exe_running(&sys, &h.join("EscapeFromTarkov.exe")))
        {
            out.push(warn(
                "The headless EscapeFromTarkov.exe is already running.",
                "A leftover headless blocks a new one from starting. End it in Task Manager.",
                None,
            ));
        }
    }
    if wants_game
        && !active
        && player
            .as_deref()
            .is_some_and(|p| exe_running(&sys, &p.join("EscapeFromTarkov.exe")))
    {
        out.push(warn(
            "Your game is already running.",
            "Close it before launching, EFT only allows one copy.",
            None,
        ));
    }

    if let Some(h) = host.as_deref().filter(|_| wants_host) {
        let list = profiles::list(h);
        if list.iter().any(|p| p.headless) {
            out.push(ok("Headless profile found"));
        } else {
            out.push(warn(
                "No headless profile on the host.",
                "Start the server once with Fika's headless setup so it creates one, or the headless is skipped.",
                None,
            ));
        }
        if let Some(id) = profile.filter(|_| wants_game) {
            match list.iter().find(|p| p.id == id) {
                Some(p) if p.valid => out.push(ok(format!("Profile {} looks complete", p.nickname))),
                Some(p) => out.push(bad(
                    format!("Profile {} is missing parts of its save.", p.nickname),
                    "Restore it from a backup on the History page, or log in through the SPT launcher once.",
                    Some("history"),
                )),
                None => out.push(bad(
                    "The selected profile doesn't exist on the host.",
                    "Pick another one, or create it in the SPT launcher.",
                    None,
                )),
            }
        }
        let (ip, _) = install::server_bind(h);
        let loopback = ip.as_deref().is_none_or(|i| i.starts_with("127.") || i == "localhost");
        if mode == "host" && loopback {
            out.push(warn(
                "SPT only listens on this PC, friends can't join.",
                "Set ip to 0.0.0.0 in SPT_Data\\configs\\http.json. The Connection check in Settings explains more.",
                Some("settings"),
            ));
        }
    }

    if mode == "join" && !s.server_address.trim().is_empty() {
        let addr = crate::game_addr(&s, "join");
        if install::reachable(&addr, std::time::Duration::from_secs(2)) {
            out.push(ok(format!("Server at {addr} answers")));
        } else {
            out.push(warn(
                format!("Nothing answers at {addr} yet."),
                "Fine if your friend hasn't started it. Otherwise check the address in Settings.",
                Some("settings"),
            ));
        }
    }

    if wants_host && s.backup_on_launch {
        match crate::backups::list_all(None).first() {
            Some(b) => out.push(ok(format!("Profile backups on, last one {}", ago(b.created)))),
            None => out.push(ok("Profile backups on, the first one is made at launch")),
        }
    } else if wants_host {
        out.push(warn(
            "Profile backups before launch are off.",
            "Turn them on in Settings so a bad raid or mod can be rolled back.",
            Some("settings"),
        ));
    }
    out
}

fn ago(t: u64) -> String {
    let s = crate::history::now().saturating_sub(t);
    match s {
        0..=89 => "just now".into(),
        90..=5399 => format!("{} min ago", s / 60),
        5400..=172_799 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

pub fn report_text(core: &Core) -> String {
    let s = core.settings.lock().unwrap().clone();
    run(core, &s.launch_mode, s.last_profile.as_deref())
        .iter()
        .map(|c| {
            format!(
                "[{}] {}{}\n",
                c.level,
                c.title,
                c.fix.as_ref().map(|f| format!("\n      {f}")).unwrap_or_default()
            )
        })
        .collect()
}
