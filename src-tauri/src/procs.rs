use crate::history::record;
use crate::install::{self, norm};
use crate::logs::{GAME, HEADLESS, SERVER};
use crate::Core;
use serde::Serialize;
use serde_json::json;
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
    /// user stopped the headless on purpose, so the supervisor must not bring it back
    pub headless_hold: AtomicBool,
    pub supervising: AtomicBool,
    pub restarting: AtomicBool,
    pub server_crashes: Mutex<Vec<Instant>>,
    pub headless_plan: Mutex<Option<(PathBuf, String, u16, String)>>,
    pub stage: Mutex<(String, u8)>,
    /// when the current server boot began, for the ETA
    pub boot_start: Mutex<Option<Instant>>,
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
    pub headless_held: bool,
    pub can_start_headless: bool,
    /// seconds into the current server boot, and how long the last few took
    pub boot_elapsed: Option<u64>,
    pub boot_expect: Option<u64>,
}

fn alive(core: &Core, my: u64) -> bool {
    core.stack.active.load(SeqCst) && core.stack.gen.load(SeqCst) == my
}

fn set_stage(core: &Core, s: &str, p: u8) {
    *core.stack.stage.lock().unwrap() = (s.to_string(), p);
}

fn note(core: &Core, idx: usize, msg: &str) {
    core.logs.push_line(idx, format!("[Fika Hub] {msg}"));
}

fn backend_args(token: &str, addr: &str) -> String {
    format!("-token={token} -config={{'BackendUrl':'https://{addr}','Version':'live','MatchingVersion':'live'}}")
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
    let Some(pid) = slot.lock().unwrap().as_ref().map(|c| c.id()) else {
        return;
    };
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .creation_flags(NO_WINDOW)
        .status();
    let until = Instant::now() + Duration::from_secs(6);
    while Instant::now() < until {
        if !child_alive(slot) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    kill_slot(slot);
}

fn exit_code(slot: &Mutex<Option<Child>>) -> Option<i32> {
    slot.lock()
        .unwrap()
        .take()
        .and_then(|mut c| c.try_wait().ok().flatten())
        .and_then(|s| s.code())
}

pub fn stop_all(core: &Core, why: &str) {
    if core.stack.active.swap(false, SeqCst) {
        record("stop", json!({ "why": why }));
    }
    core.stack.gen.fetch_add(1, SeqCst);
    core.stack.supervising.store(false, SeqCst);
    close_game(&core.stack.game);
    kill_slot(&core.stack.headless);
    kill_slot(&core.stack.server);
    core.stack.boot_start.lock().unwrap().take();
    set_stage(core, "", 0);
}

fn boot_file() -> PathBuf {
    crate::settings::data_dir().join("boot_times.json")
}

fn boot_times() -> Vec<u64> {
    std::fs::read_to_string(boot_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn record_boot(secs: u64) {
    if secs < 3 {
        return;
    }
    let mut v = boot_times();
    v.push(secs);
    let skip = v.len().saturating_sub(5);
    let v: Vec<u64> = v.into_iter().skip(skip).collect();
    let _ = std::fs::write(boot_file(), serde_json::to_string(&v).unwrap_or_default());
}

/// Median of the last boots, so one slow first-start-after-update doesn't skew it.
fn boot_expect() -> Option<u64> {
    let mut v = boot_times();
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

fn wait_port(core: &Core, my: u64, port: u16, watch_server: bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(600) {
        if !alive(core, my) {
            return false;
        }
        if install::port_listening(port) {
            if let Some(t) = core.stack.boot_start.lock().unwrap().take().filter(|_| watch_server) {
                record_boot(t.elapsed().as_secs());
            }
            return true;
        }
        if watch_server && !child_alive(&core.stack.server) {
            core.stack.boot_start.lock().unwrap().take();
            note(core, SERVER, "SPT server exited before it was ready. Check the output above.");
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    core.stack.boot_start.lock().unwrap().take();
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

    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
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
    let pid = child.id();
    *core.stack.server.lock().unwrap() = Some(child);
    if !core.stack.active.load(SeqCst) {
        kill_slot(&core.stack.server);
        return Err("Stopped while the server was starting.".into());
    }
    core.logs.mark_run(SERVER, "server started");
    *core.stack.boot_start.lock().unwrap() = Some(Instant::now());

    // A restart or stop takes the child out of the slot first, so only an exit
    // that leaves this exact child behind is a crash.
    let c = core.clone();
    let my = c.stack.gen.load(SeqCst);
    std::thread::spawn(move || {
        let code = loop {
            std::thread::sleep(Duration::from_secs(1));
            let mut g = c.stack.server.lock().unwrap();
            match g.as_mut() {
                Some(ch) if ch.id() == pid => {
                    if let Ok(Some(st)) = ch.try_wait() {
                        break st.code();
                    }
                }
                _ => return,
            }
        };
        if !alive(&c, my) || c.stack.stage.lock().unwrap().1 < 100 {
            return;
        }
        let code_txt = code.map(|x| x.to_string()).unwrap_or_else(|| "?".into());
        note(&c, SERVER, &format!("SPT server exited on its own (exit code {code_txt})."));
        record("server_crash", json!({ "code": code }));
        if !c.settings.lock().unwrap().auto_restart_server {
            return;
        }
        // a server that dies on boot every time would otherwise loop forever
        let recent = {
            let mut v = c.stack.server_crashes.lock().unwrap();
            v.retain(|t: &Instant| t.elapsed() < Duration::from_secs(600));
            v.push(Instant::now());
            v.len()
        };
        if recent > 3 {
            note(&c, SERVER, "Server crashed 3 times in 10 minutes, not restarting it again.");
            return;
        }
        note(&c, SERVER, "Restarting server in 5s.");
        std::thread::sleep(Duration::from_secs(5));
        if alive(&c, my) {
            let _ = restart_server(&c, "crash");
        }
    });
    Ok(())
}

fn spawn_eft(dir: &Path, args: &str, hidden: bool) -> Result<Child, String> {
    let mut cmd = Command::new(dir.join("EscapeFromTarkov.exe"));
    cmd.current_dir(dir)
        .raw_arg(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if hidden {
        cmd.creation_flags(NO_WINDOW);
    }
    cmd.spawn().map_err(|e| format!("Couldn't start EscapeFromTarkov.exe: {e}"))
}

fn supervise_headless(core: Arc<Core>, my: u64, host: PathBuf, token: String, port: u16, addr: String) {
    *core.stack.headless_plan.lock().unwrap() = Some((host.clone(), token.clone(), port, addr.clone()));
    core.stack.headless_hold.store(false, SeqCst);
    core.stack.supervising.store(true, SeqCst);
    std::thread::spawn(move || {
        supervise_loop(&core, my, &host, &token, port, &addr);
        if core.stack.gen.load(SeqCst) == my {
            core.stack.supervising.store(false, SeqCst);
        }
    });
}

/// The local TCP table only knows about this PC, a remote server needs a real connect.
fn server_up(port: u16, addr: &str) -> bool {
    if addr.starts_with("127.") || addr.starts_with("localhost:") {
        install::port_listening(port)
    } else {
        install::reachable(addr, Duration::from_secs(2))
    }
}

fn supervise_loop(core: &Arc<Core>, my: u64, host: &Path, token: &str, port: u16, addr: &str) {
    let core = core.clone();
    loop {
        if !alive(&core, my) {
            return;
        }
        // after a server restart the headless would just fail to connect, so hold it until the port is back
        while !server_up(port, addr) {
            if !alive(&core, my) {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let args = format!("{} -nographics -batchmode", backend_args(token, addr));
        match spawn_eft(host, &args, true) {
            Ok(c) => {
                *core.stack.headless.lock().unwrap() = Some(c);
                // a stop may have emptied the slot between the spawn and here
                if !alive(&core, my) {
                    kill_slot(&core.stack.headless);
                    return;
                }
            }
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
        let code = exit_code(&core.stack.headless);
        if !alive(&core, my) {
            return;
        }
        if core.stack.headless_hold.load(SeqCst) {
            note(&core, HEADLESS, "Headless stopped.");
            record("headless_stop", json!({}));
            return;
        }
        let forced = core.stack.force_restart.swap(false, SeqCst);
        record(if forced { "headless_restart" } else { "headless_crash" }, json!({ "code": code }));
        if !forced && !core.settings.lock().unwrap().auto_restart_headless {
            note(&core, HEADLESS, "Headless exited. Auto-restart is off.");
            return;
        }
        note(
            &core,
            HEADLESS,
            if forced {
                "Restarting headless..."
            } else {
                "Headless exited, restarting in 3s."
            },
        );
        // EFT holds a single-instance mutex for a moment after exit
        std::thread::sleep(Duration::from_secs(3));
        if !forced {
            core.stack.restarts.fetch_add(1, SeqCst);
        }
    }
}

pub fn stop_headless(core: &Core) -> Result<(), String> {
    if !child_alive(&core.stack.headless) {
        return Err("The headless isn't running from here.".into());
    }
    core.stack.headless_hold.store(true, SeqCst);
    kill_slot(&core.stack.headless);
    Ok(())
}

pub fn start_headless(core: &Arc<Core>) -> Result<(), String> {
    if !core.stack.active.load(SeqCst) {
        return Err("Start the stack first.".into());
    }
    if core.stack.supervising.load(SeqCst) {
        return Err("The headless is already running.".into());
    }
    let (host, token, port, addr) = core
        .stack
        .headless_plan
        .lock()
        .unwrap()
        .clone()
        .ok_or("No headless profile on the host.")?;
    note(core, HEADLESS, "Starting headless...");
    supervise_headless(core.clone(), core.stack.gen.load(SeqCst), host, token, port, addr);
    Ok(())
}

pub fn apply_display_mode(player: &Path, mode: &str) {
    // Unity's FullScreenMode (what EFT reads at boot) and EFT's own Graphics.ini enum differ
    let (unity, ini) = match mode {
        "Windowed" => (3u32, 2),
        "Borderless Fullscreen" => (1, 1),
        "Fullscreen" => (0, 0),
        _ => return,
    };
    if let Ok((key, _)) =
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(r"Software\Battlestate Games\EscapeFromTarkov")
    {
        let _ = key.set_value("Screenmanager Fullscreen mode_h3630240806", &unity);
    }
    let Some(path) = install::spt_root(player).map(|r| r.join("user").join("sptSettings").join("Graphics.ini")) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
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

pub fn start_game(core: &Arc<Core>, my: u64, player: &Path, profile: &str, addr: &str) -> Result<(), String> {
    let mode = core.settings.lock().unwrap().display_mode.clone();
    apply_display_mode(player, &mode);
    let child = spawn_eft(player, &backend_args(profile, addr), false)?;
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
        let code = exit_code(&c.stack.game);
        if !alive(&c, my) {
            return;
        }
        record("game_exit", json!({ "code": code }));
        let host_mode = c.stack.mode.lock().unwrap().as_str() == "host";
        if c.settings.lock().unwrap().stop_with_game && !host_mode {
            note(&c, GAME, "Game closed, shutting the stack down.");
            stop_all(&c, "game closed");
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
    pub headless: Option<PathBuf>,
    pub profile: Option<String>,
    pub headless_id: Option<String>,
    pub port: u16,
    /// host:port the game or headless connects to. Only differs from 127.0.0.1 in the remote modes.
    pub addr: String,
}

pub fn launch(core: Arc<Core>, plan: LaunchPlan) -> Result<(), String> {
    if core.stack.active.swap(true, SeqCst) {
        return Err("Already running.".into());
    }
    let my = core.stack.gen.fetch_add(1, SeqCst) + 1;
    core.stack.restarts.store(0, SeqCst);
    core.stack.server_crashes.lock().unwrap().clear();
    *core.stack.headless_plan.lock().unwrap() = None;
    *core.stack.mode.lock().unwrap() = plan.mode.clone();
    set_stage(&core, "Starting", 5);

    std::thread::spawn(move || {
        let fail = |msg: &str| {
            note(&core, SERVER, msg);
            stop_all(&core, "launch failed");
        };
        let wants_server = plan.mode == "full" || plan.mode == "host";
        let wants_game = plan.mode == "full" || plan.mode == "join";

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
                        stop_all(&core, "server didn't come up");
                    }
                    return;
                }
            }
            set_stage(&core, "Starting headless", 65);
            match (plan.headless.clone(), plan.headless_id.clone()) {
                (Some(dir), Some(id)) => {
                    supervise_headless(core.clone(), my, dir, id, plan.port, format!("127.0.0.1:{}", plan.port))
                }
                (None, _) => note(
                    &core,
                    HEADLESS,
                    "No Fika headless install found, skipping headless. Set its folder in Settings.",
                ),
                (_, None) => note(&core, HEADLESS, "No headless profile found on the server, skipping headless."),
            }
        } else {
            set_stage(&core, "Waiting for server", 35);
            if !wait_remote(&core, my, &plan.addr) {
                if alive(&core, my) {
                    stop_all(&core, "server unreachable");
                }
                return;
            }
            if plan.mode == "headless" {
                set_stage(&core, "Starting headless", 65);
                match (plan.headless.clone(), plan.headless_id.clone()) {
                    (Some(dir), Some(id)) => supervise_headless(core.clone(), my, dir, id, plan.port, plan.addr.clone()),
                    _ => return fail("Headless only mode needs a headless install and its profile ID."),
                }
            }
        }

        if wants_game && alive(&core, my) {
            set_stage(&core, "Launching game", 85);
            if let Err(e) = start_game(
                &core,
                my,
                plan.player.as_ref().unwrap(),
                plan.profile.as_deref().unwrap_or(""),
                &plan.addr,
            ) {
                return fail(&e);
            }
        }
        if alive(&core, my) {
            set_stage(&core, "Running", 100);
        }
    });
    Ok(())
}

/// The remote modes may point at another PC, where the local TCP table says nothing.
fn wait_remote(core: &Core, my: u64, addr: &str) -> bool {
    let start = Instant::now();
    let mut said = false;
    while start.elapsed() < Duration::from_secs(600) {
        if !alive(core, my) {
            return false;
        }
        if install::reachable(addr, Duration::from_secs(2)) {
            return true;
        }
        if !said {
            note(core, SERVER, &format!("Waiting for the server at {addr}..."));
            said = true;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    note(core, SERVER, &format!("Couldn't reach {addr} within 10 minutes."));
    false
}

pub fn current_gen(core: &Core) -> u64 {
    core.stack.gen.load(SeqCst)
}

pub fn restart_headless(core: &Core) {
    if child_alive(&core.stack.headless) {
        core.stack.force_restart.store(true, SeqCst);
        kill_slot(&core.stack.headless);
    }
}

/// Restarts only the server. The headless goes down with it and comes back once
/// the port is open again, since it can't survive losing its backend.
pub fn restart_server(core: &Arc<Core>, by: &str) -> Result<(), String> {
    if core.stack.restarting.compare_exchange(false, true, SeqCst, SeqCst).is_err() {
        return Err("The server is already restarting.".into());
    }
    let r = restart_server_inner(core, by);
    core.stack.restarting.store(false, SeqCst);
    r
}

fn restart_server_inner(core: &Arc<Core>, by: &str) -> Result<(), String> {
    // a crashed child stays in the slot, so this also covers restarting after a crash
    if !core.stack.active.load(SeqCst) || core.stack.server.lock().unwrap().is_none() {
        return Err("The server wasn't started from here.".into());
    }
    let host = crate::host_dir(&core.settings.lock().unwrap()).ok_or("Host install isn't set.")?;
    let port = install::server_port(&host);
    let my = core.stack.gen.load(SeqCst);

    note(core, SERVER, "Restarting server...");
    record("server_restart", json!({ "by": by }));
    set_stage(core, "Restarting server", 35);
    kill_slot(&core.stack.server);
    if core.stack.headless.lock().unwrap().is_some() {
        core.stack.force_restart.store(true, SeqCst);
        kill_slot(&core.stack.headless);
    }
    let gone = Instant::now();
    while install::port_listening(port) && gone.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if let Err(e) = spawn_server(core, &host) {
        note(core, SERVER, &e);
        stop_all(core, "restart failed");
        return Err(e);
    }

    let c = core.clone();
    std::thread::spawn(move || {
        if wait_port(&c, my, port, true) {
            set_stage(&c, "Running", 100);
        } else if alive(&c, my) {
            stop_all(&c, "server didn't come back after restart");
        }
    });
    Ok(())
}

pub fn status(core: &Core) -> Status {
    let (host, player, headless) = {
        let s = core.settings.lock().unwrap();
        (
            crate::host_dir(&s),
            s.player_dir.clone().map(PathBuf::from),
            crate::headless_dir(&s),
        )
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
    let headless_exe = headless.as_deref().map(|h| norm(&h.join("EscapeFromTarkov.exe")));
    let game_exe = player.as_deref().map(|p| norm(&p.join("EscapeFromTarkov.exe")));

    let mut guard = core.stack.sys.lock().unwrap();
    let sys = guard.get_or_insert_with(System::new);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;

    let info = |owned: Option<u32>, exe: &Option<String>| -> ProcInfo {
        let proc_ = match owned {
            Some(pid) => sys.process(sysinfo::Pid::from_u32(pid)),
            None => exe.as_ref().and_then(|want| {
                sys.processes()
                    .values()
                    .find(|p| p.exe().map(|e| &norm(e) == want).unwrap_or(false))
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
            None if owned.is_some() => ProcInfo {
                running: true,
                owned: true,
                pid: owned.unwrap(),
                ..Default::default()
            },
            None => ProcInfo::default(),
        }
    };

    let (stage, progress) = core.stack.stage.lock().unwrap().clone();
    let boot_elapsed = core.stack.boot_start.lock().unwrap().map(|t| t.elapsed().as_secs());
    Status {
        boot_expect: boot_elapsed.and_then(|_| boot_expect()),
        boot_elapsed,
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
        headless_held: core.stack.headless_hold.load(SeqCst),
        can_start_headless: core.stack.active.load(SeqCst)
            && !core.stack.supervising.load(SeqCst)
            && core.stack.headless_plan.lock().unwrap().is_some(),
    }
}
