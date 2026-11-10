#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backups;
mod configs;
mod diagnostics;
mod fika;
mod health;
mod history;
mod install;
mod logs;
mod notify;
mod procs;
mod profiles;
mod schedule;
mod settings;
mod snapshots;

use serde::Serialize;
use settings::Settings;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

pub struct Core {
    pub settings: Mutex<Settings>,
    pub logs: Arc<logs::Logs>,
    pub stack: procs::Stack,
    pub http: reqwest::Client,
    /// verifies certs, unlike `http` which talks to SPT's self-signed server
    pub web: reqwest::Client,
    pub sched: schedule::State,
}

type Ctx<'a> = State<'a, Arc<Core>>;

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// The launcher normally lives inside the host install; a saved folder covers
/// running it from anywhere else.
pub fn host_dir(s: &Settings) -> Option<PathBuf> {
    if let Some(d) = exe_dir().filter(|d| install::host_problem(d).is_none()) {
        return Some(d);
    }
    s.host_dir.as_ref().map(PathBuf::from).filter(|d| install::host_problem(d).is_none())
}

/// Where the game connects. A saved address only applies to game-only mode, since
/// the other modes start the server on this PC.
pub fn game_addr(s: &Settings, mode: &str) -> String {
    let port = host_dir(s).as_deref().map(install::server_port).unwrap_or(6969);
    let saved = s.server_address.trim();
    if mode != "join" || saved.is_empty() {
        return format!("127.0.0.1:{port}");
    }
    let saved = saved.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    if saved.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        saved.to_string()
    } else {
        format!("{saved}:{port}")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Versions {
    spt_host: Option<String>,
    spt_player: Option<String>,
    fika_host: Option<String>,
    fika_player: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Boot {
    host_dir: Option<String>,
    host_from_exe: bool,
    host_problem: Option<String>,
    player_dir: Option<String>,
    player_problem: Option<String>,
    settings: Settings,
    versions: Versions,
    app_version: String,
}

fn boot_info(core: &Core) -> Boot {
    let s = core.settings.lock().unwrap().clone();
    let host = host_dir(&s);
    let host_from_exe = exe_dir().map(|d| install::host_problem(&d).is_none()).unwrap_or(false);
    let host_problem = match (&host, &s.host_dir) {
        (Some(_), _) => None,
        (None, Some(d)) => install::host_problem(Path::new(d)),
        (None, None) => Some("Pick the install that runs the server and headless.".into()),
    };
    let player = s.player_dir.as_ref().map(PathBuf::from);
    let player_problem = player.as_deref().and_then(install::player_problem);
    Boot {
        versions: Versions {
            spt_host: host.as_deref().and_then(install::spt_version),
            spt_player: player.as_deref().and_then(install::spt_version),
            fika_host: host.as_deref().and_then(install::fika_version),
            fika_player: player.as_deref().and_then(install::fika_version),
        },
        host_dir: host.map(|h| h.to_string_lossy().into()),
        host_from_exe,
        host_problem,
        player_dir: s.player_dir.clone(),
        player_problem,
        settings: s,
        app_version: env!("CARGO_PKG_VERSION").into(),
    }
}

#[tauri::command]
fn boot(core: Ctx) -> Boot {
    boot_info(&core)
}

#[tauri::command]
fn set_dir(core: Ctx, kind: String, path: String) -> Result<Boot, String> {
    let p = PathBuf::from(&path);
    let problem = if kind == "host" { install::host_problem(&p) } else { install::player_problem(&p) };
    if let Some(e) = problem {
        return Err(e);
    }
    {
        let mut s = core.settings.lock().unwrap();
        if kind == "host" {
            s.host_dir = Some(path);
        } else {
            s.player_dir = Some(path);
        }
        s.save();
    }
    Ok(boot_info(&core))
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    Ok(app.dialog().file().blocking_pick_folder().and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into()))
}

#[tauri::command]
fn save_settings(core: Ctx, settings: Settings) {
    let mut s = core.settings.lock().unwrap();
    // folders only change through set_dir, which validates them
    let (h, p) = (s.host_dir.clone(), s.player_dir.clone());
    *s = settings;
    s.host_dir = h;
    s.player_dir = p;
    s.save();
    drop(s);
    // refresh the Discord message so a new webhook or restart time shows up right away
    schedule::request_post(&core);
}

#[tauri::command]
fn status(core: Ctx) -> procs::Status {
    procs::status(&core)
}

#[tauri::command]
fn launch(core: Ctx, window: tauri::WebviewWindow, mode: String, profile: Option<String>) -> Result<(), String> {
    let s = core.settings.lock().unwrap().clone();
    let host = host_dir(&s);
    let player = s.player_dir.as_ref().map(PathBuf::from).filter(|p| install::player_problem(p).is_none());

    if mode != "join" && host.is_none() {
        return Err("Set the host install in Settings first.".into());
    }
    if mode != "host" {
        if player.is_none() {
            return Err("Set your game install in Settings first.".into());
        }
        if profile.as_deref().unwrap_or("").is_empty() {
            return Err("Pick who you're playing as.".into());
        }
    }
    let headless_id = host.as_deref().and_then(|h| profiles::headless_ids(h).into_iter().next());
    let port = host.as_deref().map(install::server_port).unwrap_or(6969);
    let addr = game_addr(&s, &mode);
    if mode != "join" && s.backup_on_launch {
        if let Some(h) = host.as_deref() {
            match backups::create(h, "launch", s.backup_keep) {
                Ok(Some(_)) => core.logs.push_line(logs::SERVER, "[Fika Hub] Profiles backed up.".into()),
                Ok(None) => {}
                Err(e) => core.logs.push_line(logs::SERVER, format!("[Fika Hub] Profile backup failed: {e}")),
            }
        }
    }
    history::record(
        "launch",
        serde_json::json!({
            "mode": mode,
            "spt": host.as_deref().and_then(install::spt_version),
            "fika": host.as_deref().and_then(install::fika_version),
            "gameSpt": player.as_deref().and_then(install::spt_version),
            "gameFika": player.as_deref().and_then(install::fika_version),
            "profile": profile.as_deref().and_then(|id| host.as_deref().and_then(|h| profiles::list(h).into_iter().find(|p| p.id == id)).map(|p| p.nickname)),
        }),
    );

    {
        let mut st = core.settings.lock().unwrap();
        st.launch_mode = mode.clone();
        if profile.is_some() {
            st.last_profile = profile.clone();
        }
        st.save();
    }
    procs::launch(core.inner().clone(), procs::LaunchPlan { mode, host, player, profile, headless_id, port, addr })?;
    if s.minimize_on_launch {
        let _ = window.minimize();
    }
    Ok(())
}

#[tauri::command]
async fn stop(core: Ctx<'_>) -> Result<(), String> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || procs::stop_all(&c, "user")).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn start_game(core: Ctx, profile: String) -> Result<(), String> {
    let s = core.settings.lock().unwrap().clone();
    let player = s.player_dir.as_ref().map(PathBuf::from).ok_or("Set your game install in Settings first.")?;
    let addr = game_addr(&s, &core.stack.mode.lock().unwrap());
    if !core.stack.active.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("Start the stack first.".into());
    }
    procs::start_game(core.inner(), procs::current_gen(&core), &player, &profile, &addr)
}

#[tauri::command]
async fn restart_headless(core: Ctx<'_>) -> Result<(), String> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || procs::restart_headless(&c)).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn restart_server(core: Ctx<'_>) -> Result<(), String> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || procs::restart_server(&c, "user")).await.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    lan_ip: Option<String>,
    port: u16,
    bind_ip: Option<String>,
    backend_ip: Option<String>,
    game_addr: String,
}

#[tauri::command]
fn connection_info(core: Ctx) -> Connection {
    let s = core.settings.lock().unwrap().clone();
    let host = host_dir(&s);
    let (bind_ip, backend_ip) = host.as_deref().map(install::server_bind).unwrap_or((None, None));
    Connection {
        lan_ip: install::lan_ip().map(|i| i.to_string()),
        port: host.as_deref().map(install::server_port).unwrap_or(6969),
        bind_ip,
        backend_ip,
        game_addr: game_addr(&s, "join"),
    }
}

#[tauri::command]
async fn check_reach(addr: String) -> bool {
    tauri::async_runtime::spawn_blocking(move || install::reachable(&addr, std::time::Duration::from_secs(3))).await.unwrap_or(false)
}

#[tauri::command]
async fn stop_headless(core: Ctx<'_>) -> Result<(), String> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || procs::stop_headless(&c)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn start_headless(core: Ctx) -> Result<(), String> {
    procs::start_headless(core.inner())
}

#[tauri::command]
fn history(limit: usize) -> Vec<serde_json::Value> {
    history::read(limit.min(5000))
}

#[tauri::command]
async fn export_diagnostics(core: Ctx<'_>, app: tauri::AppHandle) -> Result<bool, String> {
    let stamp = history::now();
    let Some(path) = app.dialog().file().set_file_name(format!("fika-hub-diagnostics-{stamp}.zip")).add_filter("Zip", &["zip"]).blocking_save_file() else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || diagnostics::build(&c, &path)).await.map_err(|e| e.to_string())??;
    Ok(true)
}

#[tauri::command]
async fn health_check(core: Ctx<'_>, mode: String, profile: Option<String>) -> Result<Vec<health::Check>, String> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || health::run(&c, &mode, profile.as_deref())).await.map_err(|e| e.to_string())
}

fn host_or_err(core: &Core) -> Result<PathBuf, String> {
    host_dir(&core.settings.lock().unwrap()).ok_or_else(|| "Set the host install first.".to_string())
}

#[tauri::command]
async fn backup_list(core: Ctx<'_>) -> Result<Vec<backups::Backup>, String> {
    let host = host_dir(&core.settings.lock().unwrap());
    tauri::async_runtime::spawn_blocking(move || backups::list_all(host.as_deref())).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn backup_create(core: Ctx) -> Result<Option<backups::Backup>, String> {
    let keep = core.settings.lock().unwrap().backup_keep;
    backups::create(&host_or_err(&core)?, "manual", keep)
}

#[tauri::command]
async fn backup_restore(core: Ctx<'_>, source: String, id: String, profiles: Vec<String>) -> Result<usize, String> {
    let host = host_or_err(&core)?;
    if install::port_listening(install::server_port(&host)) {
        return Err("Stop the server first. It keeps profiles in memory and would write them straight back over the backup.".into());
    }
    let keep = core.settings.lock().unwrap().backup_keep;
    let n = tauri::async_runtime::spawn_blocking(move || backups::restore(&host, &source, &id, &profiles, keep)).await.map_err(|e| e.to_string())??;
    history::record("profile_restore", serde_json::json!({ "count": n }));
    Ok(n)
}

#[tauri::command]
fn backup_delete(id: String) -> Result<(), String> {
    backups::delete(&id)
}

#[tauri::command]
fn backup_open() -> Result<(), String> {
    std::process::Command::new("explorer.exe").arg(backups::open_dir()).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn snapshot_list(root: String) -> Vec<snapshots::Snapshot> {
    snapshots::list(&root)
}

#[tauri::command]
async fn snapshot_create(core: Ctx<'_>, root: String, name: String) -> Result<snapshots::Snapshot, String> {
    let path = config_root(&core, &root)?;
    tauri::async_runtime::spawn_blocking(move || snapshots::create(&root, &path, &name)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn snapshot_compare(core: Ctx<'_>, root: String, id: String) -> Result<Vec<snapshots::Change>, String> {
    let path = config_root(&core, &root)?;
    tauri::async_runtime::spawn_blocking(move || snapshots::compare(&root, &id, &path)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn snapshot_read(root: String, id: String, rel: String) -> Result<String, String> {
    snapshots::read_file(&root, &id, &rel)
}

#[tauri::command]
fn snapshot_restore(core: Ctx, root: String, id: String, only: Option<Vec<String>>) -> Result<usize, String> {
    let path = config_root(&core, &root)?;
    snapshots::restore(&root, &id, &path, only)
}

#[tauri::command]
fn snapshot_delete(root: String, id: String) -> Result<(), String> {
    snapshots::delete(&root, &id)
}

#[tauri::command]
fn config_backups(core: Ctx, root: String, rel: String) -> Result<Vec<snapshots::FileBackup>, String> {
    let r = config_root(&core, &root)?;
    Ok(snapshots::file_backups(&configs::resolve(&r, &rel)?))
}

#[tauri::command]
fn config_backup_read(core: Ctx, root: String, rel: String, name: String) -> Result<String, String> {
    let r = config_root(&core, &root)?;
    snapshots::read_file_backup(&configs::resolve(&r, &rel)?, &name)
}

#[tauri::command]
fn next_restart(core: Ctx) -> u64 {
    core.sched.next_restart.load(std::sync::atomic::Ordering::SeqCst)
}

#[tauri::command]
async fn discord_test(core: Ctx<'_>) -> Result<(), String> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || schedule::post_status(&c)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn notify(title: String, body: String) {
    notify::show(&title, &body);
}

#[tauri::command]
fn list_profiles(core: Ctx) -> Vec<profiles::Profile> {
    let s = core.settings.lock().unwrap().clone();
    host_dir(&s).map(|h| profiles::list(&h)).unwrap_or_default()
}

#[tauri::command]
async fn lobby(core: Ctx<'_>, session: Option<String>) -> Result<fika::Lobby, String> {
    let s = core.settings.lock().unwrap().clone();
    let Some(host) = host_dir(&s) else { return Ok(fika::Lobby::default()) };
    let port = install::server_port(&host);
    if !install::port_listening(port) {
        return Ok(fika::Lobby::default());
    }
    let heads = profiles::headless_ids(&host);
    let session = session.filter(|x| !x.is_empty()).or_else(|| heads.first().cloned()).unwrap_or_default();
    Ok(fika::lobby(&core.http, port, &session).await)
}

#[tauri::command]
fn logs_since(core: Ctx, stream: usize, after: i64) -> logs::Chunk {
    core.logs.since(stream.min(2), after)
}

#[tauri::command]
fn clear_logs(core: Ctx, stream: usize) {
    core.logs.clear(stream.min(2));
}

#[tauri::command]
async fn save_text(app: tauri::AppHandle, name: String, text: String) -> Result<bool, String> {
    let Some(path) = app.dialog().file().set_file_name(&name).add_filter("Text", &["txt", "log"]).blocking_save_file() else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn open_folder(core: Ctx, which: String) -> Result<(), String> {
    let s = core.settings.lock().unwrap().clone();
    let host = host_dir(&s);
    let player = s.player_dir.as_ref().map(PathBuf::from);
    let target = match which.as_str() {
        "host" => host,
        "player" => player,
        "mods" => host.as_deref().and_then(install::spt_root).map(|r| r.join("user").join("mods")),
        "profiles" => host.as_deref().and_then(install::profiles_dir),
        "headless-log" => host.map(|h| h.join("BepInEx")),
        "game-log" => player.map(|p| p.join("BepInEx")),
        other => {
            // config files are opened by root key + relative path
            let (root, rel) = other.split_once('|').ok_or("unknown folder")?;
            let roots = configs::roots(host_dir(&s).as_deref(), s.player_dir.as_ref().map(Path::new));
            let r = roots.iter().find(|r| r.key == root).ok_or("unknown root")?;
            configs::resolve(Path::new(&r.path), rel)?.parent().map(Path::to_path_buf)
        }
    }
    .ok_or("That folder isn't set.")?;
    std::process::Command::new("explorer.exe").arg(target).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn config_root(core: &Core, key: &str) -> Result<PathBuf, String> {
    let s = core.settings.lock().unwrap().clone();
    configs::roots(host_dir(&s).as_deref(), s.player_dir.as_ref().map(Path::new))
        .into_iter()
        .find(|r| r.key == key)
        .map(|r| PathBuf::from(r.path))
        .ok_or_else(|| "That config location isn't set.".to_string())
}

#[tauri::command]
fn config_roots(core: Ctx) -> Vec<configs::Root> {
    let s = core.settings.lock().unwrap().clone();
    configs::roots(host_dir(&s).as_deref(), s.player_dir.as_ref().map(Path::new))
}

#[tauri::command]
async fn config_list(core: Ctx<'_>, root: String, all: bool) -> Result<Vec<configs::Entry>, String> {
    let r = config_root(&core, &root)?;
    tauri::async_runtime::spawn_blocking(move || configs::scan(&r, all)).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn config_read(core: Ctx, root: String, rel: String) -> Result<configs::Doc, String> {
    let r = config_root(&core, &root)?;
    configs::read(&configs::resolve(&r, &rel)?)
}

#[tauri::command]
fn config_validate(rel: String, text: String) -> Result<String, String> {
    configs::validate(&rel, &text)
}

#[tauri::command]
fn config_save(core: Ctx, root: String, rel: String, text: String, stamp: String) -> Result<String, String> {
    let r = config_root(&core, &root)?;
    configs::save(&configs::resolve(&r, &rel)?, &text, &stamp)
}

#[tauri::command]
async fn quit(core: Ctx<'_>, app: tauri::AppHandle) -> Result<(), String> {
    let c = core.inner().clone();
    let _ = tauri::async_runtime::spawn_blocking(move || procs::stop_all(&c, "quit")).await;
    app.exit(0);
    Ok(())
}

fn main() {
    let logs = Arc::new(logs::Logs::default());
    let core = Arc::new(Core {
        settings: Mutex::new(Settings::load()),
        logs: logs.clone(),
        stack: procs::Stack::default(),
        http: fika::client(),
        web: reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().expect("http client"),
        sched: schedule::State::default(),
    });
    history::trim();
    schedule::start(core.clone());

    // Tails run for the app's whole life so a stack started elsewhere still shows up
    let c = core.clone();
    logs::tail(logs.clone(), logs::HEADLESS, move || host_dir(&c.settings.lock().unwrap()).map(|h| install::bepinex_log(&h)));
    let c = core.clone();
    logs::tail(logs.clone(), logs::GAME, move || c.settings.lock().unwrap().player_dir.as_ref().map(|p| install::bepinex_log(Path::new(p))));

    notify::register();

    let on_close = core.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(core)
        .on_window_event(move |_, event| {
            if let tauri::WindowEvent::Destroyed = event {
                procs::stop_all(&on_close, "window closed");
            }
        })
        .invoke_handler(tauri::generate_handler![
            boot,
            set_dir,
            pick_folder,
            save_settings,
            status,
            launch,
            stop,
            start_game,
            restart_headless,
            restart_server,
            connection_info,
            check_reach,
            notify,
            stop_headless,
            start_headless,
            history,
            export_diagnostics,
            health_check,
            backup_list,
            backup_create,
            backup_restore,
            backup_delete,
            backup_open,
            snapshot_list,
            snapshot_create,
            snapshot_compare,
            snapshot_read,
            snapshot_restore,
            snapshot_delete,
            config_backups,
            config_backup_read,
            next_restart,
            discord_test,
            list_profiles,
            lobby,
            logs_since,
            clear_logs,
            save_text,
            open_folder,
            config_roots,
            config_list,
            config_read,
            config_validate,
            config_save,
            quit
        ])
        .setup(|app| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start Fika Hub");
}
