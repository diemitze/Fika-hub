use crate::install::{self, norm};
use crate::logs::{GAME, HEADLESS, SERVER};
use crate::Core;
use serde::Serialize;
use std::io::BufRead;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
pub struct Stack {
    pub server: Mutex<Option<Child>>,
    pub headless: Mutex<Option<Child>>,
    pub game: Mutex<Option<Child>>,
    pub active: AtomicBool,
    pub gen: AtomicU64,
    pub restarts: AtomicU32,
    pub force_restart: AtomicBool,
    pub stage: Mutex<(String, u8)>,
    pub mode: Mutex<String>,
    pub sys: Mutex<Option<System>>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProcInfo {
    pub running: bool,
    pub owned: bool,
    pub pid: u32,
    pub cpu: f32,
    pub ram: u64,
    pub uptime: u64,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub active: bool,
    pub mode: String,
    pub stage: String,
    pub progress: u8,
    pub port_open: bool,
    pub port: u16,
    pub server: ProcInfo,
    pub headless: ProcInfo,
    pub game: ProcInfo,
    pub restarts: u32,
}

fn alive(core: &Core, my: u64) -> bool {
    core.stack.active.load(SeqCst) && core.stack.gen.load(SeqCst) == my
}

fn set_stage(core: &Core, s: &str, p: u8) {
    *core.stack.stage.lock().unwrap() = (s.to_string(), p);
}

fn note(core: &Core, idx: usize, msg: &str) {
    core.logs.push_line(idx, format!("[Fika Control] {msg}"));
}

fn backend_args(token: &str, port: u16) -> String {
    format!("-token={token} -config={{'BackendUrl':'https://127.0.0.1:{port}','Version':'live','MatchingVersion':'live'}}")
}

fn child_alive(slot: &Mutex<Option<Child>>) -> bool {
    let mut g = slot.lock().unwrap();
    match g.as_mut() {
        Some(c) => matches!(c.try_wait(), Ok(None)),
        None => false,
    }
}

fn kill_slot(slot: &Mutex<Option<Child>>) {
    if let Some(mut c) = slot.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Asks the game window to close first so EFT gets to write its settings,
/// then forces it after a few seconds.
fn close_game(slot: &Mutex<Option<Child>>) {
    if !child_alive(slot) {
        slot.lock().unwrap().take();
        return;
    }
    let Some(pid) = slot.lock().unwrap().as_ref().map(|c| c.id()) else { return };
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string()]).creation_flags(NO_WINDOW).status();
    let until = Instant::now() + Duration::from_secs(6);
    while Instant::now() < until {
        if !child_alive(slot) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    kill_slot(slot);
}

pub fn stop_all(core: &Core) {
    core.stack.active.store(false, SeqCst);
    core.stack.gen.fetch_add(1, SeqCst);
    close_game(&core.stack.game);
    kill_slot(&core.stack.headless);
    kill_slot(&core.stack.server);
    set_stage(core, "", 0);
}

fn wait_port(core: &Core, my: u64, port: u16, watch_server: bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(600) {
        if !alive(core, my) {
            return false;
        }
        if install::port_listening(port) {
            return true;
        }
        if watch_server && !child_alive(&core.stack.server) {
            note(core, SERVER, "SPT server exited before it was ready. Check the output above.");
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    note(core, SERVER, "Server didn't open its port within 10 minutes.");
    false
}

fn spawn_server(core: &Arc<Core>, host: &Path) -> Result<(), String> {
    let root = install::spt_root(host).ok_or("SPT server not found")?;
    let mut child = Command::new(root.join("SPT.Server.exe"))
        .current_dir(&root)
        .env("DISABLE_VIRTUAL_TERMINAL", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .creation_flags(NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Couldn't start SPT.Server.exe: {e}"))?;

    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        let c = core.clone();
        std::thread::spawn(move || {
            let mut r = std::io::BufReader::new(pipe);
            let mut buf = vec![];
            while matches!(r.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
                c.logs.push_text(SERVER, &String::from_utf8_lossy(&buf));
                buf.clear();
            }
        });
    }
    *core.stack.server.lock().unwrap() = Some(child);
    Ok(())
}

fn spawn_eft(dir: &Path, args: &str, hidden: bool) -> Result<Child, String> {
    let mut cmd = Command::new(dir.join("EscapeFromTarkov.exe"));
    cmd.current_dir(dir).raw_arg(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if hidden {
        cmd.creation_flags(NO_WINDOW);
    }
    cmd.spawn().map_err(|e| format!("Couldn't start EscapeFromTarkov.exe: {e}"))
}

fn supervise_headless(core: Arc<Core>, my: u64, host: PathBuf, token: String, port: u16) {
    std::thread::spawn(move || loop {
        if !alive(&core, my) {
            return;
        }
        let args = format!("{} -nographics -batchmode", backend_args(&token, port));
        match spawn_eft(&host, &args, true) {
            Ok(c) => *core.stack.headless.lock().unwrap() = Some(c),
            Err(e) => {
                note(&core, HEADLESS, &e);
                return;
            }
        }
        while child_alive(&core.stack.headless) {
            if !alive(&core, my) {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        core.stack.headless.lock().unwrap().take();
        if !alive(&core, my) {
            return;
        }
        let forced = core.stack.force_restart.swap(false, SeqCst);
        if !forced && !core.settings.lock().unwrap().auto_restart_headless {
            note(&core, HEADLESS, "Headless exited. Auto-restart is off.");
            return;
        }
        note(&core, HEADLESS, if forced { "Restarting headless..." } else { "Headless exited, restarting in 3s." });
        // EFT holds a single-instance mutex for a moment after exit
        std::thread::sleep(Duration::from_secs(3));
        if !forced {
            core.stack.restarts.fetch_add(1, SeqCst);
        }
    });
}

pub fn apply_display_mode(player: &Path, mode: &str) {
    // Unity's FullScreenMode (what EFT reads at boot) and EFT's own Graphics.ini enum differ
    let (unity, ini) = match mode {
        "Windowed" => (3u32, 2),
        "Borderless Fullscreen" => (1, 1),
        "Fullscreen" => (0, 0),
        _ => return,
    };
    if let Ok((key, _)) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(r"Software\Battlestate Games\EscapeFromTarkov") {
        let _ = key.set_value("Screenmanager Fullscreen mode_h3630240806", &unity);
    }
    let Some(path) = install::spt_root(player).map(|r| r.join("user").join("sptSettings").join("Graphics.ini")) else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    if let Some(v) = root.pointer_mut("/DisplaySettings/FullScreenMode") {
        *v = ini.into();
        let mut perms = match std::fs::metadata(&path) {
            Ok(m) => m.permissions(),
            Err(_) => return,
        };
        let was_ro = perms.readonly();
        if was_ro {
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(&path, perms.clone());
        }
        if let Ok(s) = serde_json::to_string_pretty(&root) {
            let _ = std::fs::write(&path, s);
        }
        if was_ro {
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
}

pub fn start_game(core: &Arc<Core>, my: u64, player: &Path, profile: &str, port: u16) -> Result<(), String> {
    let mode = core.settings.lock().unwrap().display_mode.clone();
    apply_display_mode(player, &mode);
    let child = spawn_eft(player, &backend_args(profile, port), false)?;
    *core.stack.game.lock().unwrap() = Some(child);
    note(core, GAME, "Game started.");

    let c = core.clone();
    std::thread::spawn(move || {
        while child_alive(&c.stack.game) {
            if !alive(&c, my) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        c.stack.game.lock().unwrap().take();
        if !alive(&c, my) {
            return;
        }
        let host_mode = c.stack.mode.lock().unwrap().as_str() == "host";
        if c.settings.lock().unwrap().stop_with_game && !host_mode {
            note(&c, GAME, "Game closed, shutting the stack down.");
            stop_all(&c);
        } else {
            note(&c, GAME, "Game closed. Server and headless keep running.");
        }
    });
    Ok(())
}

pub struct LaunchPlan {
    pub mode: String,
    pub host: Option<PathBuf>,
    pub player: Option<PathBuf>,
    pub profile: Option<String>,
    pub headless_id: Option<String>,
    pub port: u16,
}

pub fn launch(core: Arc<Core>, plan: LaunchPlan) -> Result<(), String> {
    if core.stack.active.swap(true, SeqCst) {
        return Err("Already running.".into());
    }
    let my = core.stack.gen.fetch_add(1, SeqCst) + 1;
    core.stack.restarts.store(0, SeqCst);
    *core.stack.mode.lock().unwrap() = plan.mode.clone();
    set_stage(&core, "Starting", 5);

    std::thread::spawn(move || {
        let fail = |msg: &str| {
            note(&core, SERVER, msg);
            stop_all(&core);
        };
        let wants_server = plan.mode != "join";
        let wants_game = plan.mode != "host";

        if wants_server {
            let host = plan.host.clone().unwrap();
            if install::port_listening(plan.port) {
                note(&core, SERVER, "A server is already listening, using that one.");
            } else {
                core.logs.clear(SERVER);
                set_stage(&core, "Starting SPT server", 12);
                if let Err(e) = spawn_server(&core, &host) {
                    return fail(&e);
                }
                set_stage(&core, "Waiting for server", 35);
                if !wait_port(&core, my, plan.port, true) {
                    if alive(&core, my) {
                        stop_all(&core);
                    }
                    return;
                }
            }
            set_stage(&core, "Starting headless", 65);
            match plan.headless_id.clone() {
                Some(id) => supervise_headless(core.clone(), my, host, id, plan.port),
                None => note(&core, HEADLESS, "No headless profile found on the host, skipping headless."),
            }
        } else {
            set_stage(&core, "Waiting for server", 35);
            if !wait_port(&core, my, plan.port, false) {
                if alive(&core, my) {
                    stop_all(&core);
                }
                return;
            }
        }

        if wants_game && alive(&core, my) {
            set_stage(&core, "Launching game", 85);
            if let Err(e) = start_game(&core, my, plan.player.as_ref().unwrap(), plan.profile.as_deref().unwrap_or(""), plan.port) {
                return fail(&e);
            }
        }
        if alive(&core, my) {
            set_stage(&core, "Running", 100);
        }
    });
    Ok(())
}

pub fn current_gen(core: &Core) -> u64 {
    core.stack.gen.load(SeqCst)
}

pub fn restart_headless(core: &Core) {
    core.stack.force_restart.store(true, SeqCst);
    kill_slot(&core.stack.headless);
}

pub fn status(core: &Core) -> Status {
    let (host, player) = {
        let s = core.settings.lock().unwrap();
        (crate::host_dir(&s), s.player_dir.clone().map(PathBuf::from))
    };
    let port = host.as_deref().map(install::server_port).unwrap_or(6969);

    let own = |slot: &Mutex<Option<Child>>| -> Option<u32> {
        if !child_alive(slot) {
            return None;
        }
        slot.lock().unwrap().as_ref().map(|c| c.id())
    };
    let own_server = own(&core.stack.server);
    let own_headless = own(&core.stack.headless);
    let own_game = own(&core.stack.game);

    let server_exe = host.as_deref().and_then(install::spt_root).map(|r| norm(&r.join("SPT.Server.exe")));
    let headless_exe = host.as_deref().map(|h| norm(&h.join("EscapeFromTarkov.exe")));
    let game_exe = player.as_deref().map(|p| norm(&p.join("EscapeFromTarkov.exe")));

    let mut guard = core.stack.sys.lock().unwrap();
    let sys = guard.get_or_insert_with(System::new);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet),
    );
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;

    let info = |owned: Option<u32>, exe: &Option<String>| -> ProcInfo {
        let proc_ = match owned {
            Some(pid) => sys.process(sysinfo::Pid::from_u32(pid)),
            None => exe.as_ref().and_then(|want| {
                sys.processes().values().find(|p| p.exe().map(|e| &norm(e) == want).unwrap_or(false))
            }),
        };
        match proc_ {
            Some(p) => ProcInfo {
                running: true,
                owned: owned.is_some(),
                pid: p.pid().as_u32(),
                cpu: (p.cpu_usage() / cores).min(100.0),
                ram: p.memory(),
                uptime: p.run_time(),
            },
            None if owned.is_some() => ProcInfo { running: true, owned: true, pid: owned.unwrap(), ..Default::default() },
            None => ProcInfo::default(),
        }
    };

    let (stage, progress) = core.stack.stage.lock().unwrap().clone();
    Status {
        active: core.stack.active.load(SeqCst),
        mode: core.stack.mode.lock().unwrap().clone(),
        stage,
        progress,
        port_open: install::port_listening(port),
        port,
        server: info(own_server, &server_exe),
        headless: info(own_headless, &headless_exe),
        game: info(own_game, &game_exe),
        restarts: core.stack.restarts.load(SeqCst),
    }
}
