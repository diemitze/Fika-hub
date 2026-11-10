use crate::{fika, install, logs, Core};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_DEFER: Duration = Duration::from_secs(30 * 60);

#[derive(Default)]
pub struct State {
    pub next_restart: AtomicU64,
    fired_day: Mutex<String>,
    warned_day: Mutex<String>,
    pending_since: Mutex<Option<Instant>>,
    said_deferred: AtomicBool,
    last_post: Mutex<Option<Instant>>,
    was_online: AtomicBool,
    force_post: AtomicBool,
}

struct Local {
    day: String,
    minute: i64,
    second: i64,
}

fn local_now() -> Local {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    Local { day: format!("{}-{}-{}", t.wYear, t.wMonth, t.wDay), minute: t.wHour as i64 * 60 + t.wMinute as i64, second: t.wSecond as i64 }
}

fn parse_hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

fn note(core: &Core, msg: &str) {
    core.logs.push_line(logs::SERVER, format!("[Fika Hub] {msg}"));
}

fn server_ours(core: &Core) -> bool {
    core.stack.active.load(SeqCst) && core.stack.stage.lock().unwrap().1 >= 100 && {
        let mut g = core.stack.server.lock().unwrap();
        g.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
}

fn lobby(core: &Core) -> Option<fika::Lobby> {
    let s = core.settings.lock().unwrap().clone();
    let host = crate::host_dir(&s)?;
    let port = install::server_port(&host);
    if !install::port_listening(port) {
        return None;
    }
    let session = crate::profiles::headless_ids(&host).into_iter().next().or(s.last_profile.clone()).unwrap_or_default();
    let l = tauri::async_runtime::block_on(fika::lobby(&core.http, port, &session));
    l.online.then_some(l)
}

pub fn start(core: Arc<Core>) {
    std::thread::spawn(move || loop {
        tick_restart(&core);
        tick_discord(&core);
        std::thread::sleep(Duration::from_secs(10));
    });
}

fn tick_restart(core: &Arc<Core>) {
    let s = core.settings.lock().unwrap().clone();
    let st = &core.sched;
    let Some(target) = parse_hhmm(&s.restart_time).filter(|_| s.restart_enabled) else {
        st.next_restart.store(0, SeqCst);
        *st.pending_since.lock().unwrap() = None;
        return;
    };
    let now = local_now();
    let until = (target - now.minute).rem_euclid(1440);
    let fired_today = *st.fired_day.lock().unwrap() == now.day;
    let pending = st.pending_since.lock().unwrap().is_some();
    st.next_restart.store(
        if pending { crate::history::now() } else { crate::history::now() + (until * 60 - now.second).max(0) as u64 + if until == 0 && fired_today { 86400 } else { 0 } },
        SeqCst,
    );
    if !server_ours(core) {
        return;
    }

    if until == 5 && *st.warned_day.lock().unwrap() != now.day {
        *st.warned_day.lock().unwrap() = now.day.clone();
        note(core, "Scheduled restart in 5 minutes.");
        discord_event(core, "Server restarts in 5 minutes.");
    }
    if until == 0 && !fired_today && !pending {
        *st.fired_day.lock().unwrap() = now.day.clone();
        *st.pending_since.lock().unwrap() = Some(Instant::now());
        st.said_deferred.store(false, SeqCst);
    }

    let Some(since) = *st.pending_since.lock().unwrap() else { return };
    if s.restart_wait_raids && since.elapsed() < MAX_DEFER {
        // if Fika doesn't answer, raids may still be running, so keep waiting
        let busy = lobby(core).map(|l| l.raids.iter().filter(|r| r.status != 2).count()).unwrap_or(1);
        if busy > 0 {
            if !st.said_deferred.swap(true, SeqCst) {
                note(core, "Scheduled restart waits for running raids to finish (30 min at most).");
                discord_event(core, "Restart is waiting for running raids to finish.");
            }
            return;
        }
    }
    *st.pending_since.lock().unwrap() = None;
    discord_event(core, "Server is restarting now.");
    if let Err(e) = crate::procs::restart_server(core, "schedule") {
        note(core, &format!("Scheduled restart failed: {e}"));
    }
}

// ---------------- Discord ----------------

const ACTIVITY: [&str; 5] = ["In menu", "In raid", "In stash", "In hideout", "On the flea"];

fn map_name(id: &str) -> String {
    match id.to_lowercase().as_str() {
        "bigmap" => "Customs",
        "factory4_day" => "Factory",
        "factory4_night" => "Night Factory",
        "interchange" => "Interchange",
        "laboratory" => "The Lab",
        "lighthouse" => "Lighthouse",
        "rezervbase" => "Reserve",
        "shoreline" => "Shoreline",
        "woods" => "Woods",
        "tarkovstreets" => "Streets",
        "sandbox" | "sandbox_high" => "Ground Zero",
        "labyrinth" => "Labyrinth",
        _ => id,
    }
    .to_string()
}

fn valid_hook(url: &str) -> bool {
    url.starts_with("https://discord.com/api/webhooks/") || url.starts_with("https://discordapp.com/api/webhooks/") || url.starts_with("https://ptb.discord.com/api/webhooks/")
}

fn state_file() -> std::path::PathBuf {
    crate::settings::data_dir().join("discord.json")
}

fn status_payload(core: &Core, online: bool) -> serde_json::Value {
    let now = crate::history::now();
    let mut fields = vec![];
    let mut desc = format!("Updated <t:{now}:R>");
    if online {
        let l = lobby(core).unwrap_or_default();
        let players = if l.players.is_empty() {
            "Nobody online".to_string()
        } else {
            l.players
                .iter()
                .map(|p| {
                    let act = ACTIVITY.get(p.activity as usize).copied().unwrap_or("Online");
                    let at = p.location.as_deref().filter(|_| p.activity == 1).map(|m| format!(", {}", map_name(m))).unwrap_or_default();
                    format!("**{}** · {act}{at}", p.nickname)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        fields.push(json!({ "name": format!("Players ({})", l.players.len()), "value": players }));
        if !l.raids.is_empty() {
            let raids = l
                .raids
                .iter()
                .map(|r| {
                    let st = ["Loading", "In raid", "Finished"].get(r.status as usize).copied().unwrap_or("?");
                    format!("{} · {} player{} · {st}", map_name(&r.location), r.players, if r.players == 1 { "" } else { "s" })
                })
                .collect::<Vec<_>>()
                .join("\n");
            fields.push(json!({ "name": format!("Raids ({})", l.raids.len()), "value": raids }));
        }
        if let Some(v) = l.version {
            desc += &format!(" · Fika {v}");
        }
    }
    let next = core.sched.next_restart.load(SeqCst);
    if online && next > 0 {
        fields.push(json!({ "name": "Next restart", "value": format!("<t:{next}:t> (<t:{next}:R>)") }));
    }
    json!({
        "username": "Fika Hub",
        "embeds": [{
            "title": if online { "Server online" } else { "Server offline" },
            "color": if online { 0x3fd18b } else { 0x5b6572 },
            "description": desc,
            "fields": fields,
        }]
    })
}

async fn send(core: &Core, url: &str, body: &serde_json::Value, edit: Option<&str>) -> Result<Option<String>, String> {
    let http = &core.web;
    let req = match edit {
        Some(id) => http.patch(format!("{url}/messages/{id}")),
        None => http.post(format!("{url}?wait=true")),
    };
    let res = req.header("Content-Type", "application/json").body(body.to_string()).send().await.map_err(|e| e.to_string())?;
    let code = res.status();
    if code.as_u16() == 404 && edit.is_some() {
        return Ok(None);
    }
    if !code.is_success() {
        return Err(format!("Discord said {code}"));
    }
    let v: serde_json::Value = serde_json::from_slice(&res.bytes().await.unwrap_or_default()).unwrap_or_default();
    Ok(v.get("id").and_then(|x| x.as_str()).map(String::from))
}

/// Keeps one status message per webhook and edits it in place, so the channel
/// doesn't fill up with copies.
pub fn post_status(core: &Core) -> Result<(), String> {
    static ONE: Mutex<()> = Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let url = core.settings.lock().unwrap().discord_webhook.trim().to_string();
    if !valid_hook(&url) {
        return Err("That isn't a Discord webhook URL.".into());
    }
    let online = core.stack.active.load(SeqCst) && lobby(core).is_some();
    let body = status_payload(core, online);
    let saved: serde_json::Value = std::fs::read_to_string(state_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let key = url.rsplit('/').nth(1).unwrap_or("").to_string();
    let old = saved.get(&key).and_then(|v| v.as_str()).map(String::from);

    let id = tauri::async_runtime::block_on(async {
        if let Some(id) = &old {
            if send(core, &url, &body, Some(id)).await?.is_some() {
                return Ok(Some(id.clone()));
            }
        }
        send(core, &url, &body, None).await
    })?;
    if let Some(id) = id {
        let mut m = saved.as_object().cloned().unwrap_or_default();
        m.insert(key, id.into());
        let _ = std::fs::write(state_file(), serde_json::Value::Object(m).to_string());
    }
    Ok(())
}

pub fn discord_event(core: &Core, text: &str) {
    let s = core.settings.lock().unwrap().clone();
    if !s.discord_enabled || !s.discord_events || !valid_hook(s.discord_webhook.trim()) {
        return;
    }
    let body = json!({ "username": "Fika Hub", "content": text });
    let _ = tauri::async_runtime::block_on(send(core, s.discord_webhook.trim(), &body, None));
}

pub fn request_post(core: &Core) {
    core.sched.force_post.store(true, SeqCst);
}

fn tick_discord(core: &Core) {
    let s = core.settings.lock().unwrap().clone();
    if !s.discord_enabled || !valid_hook(s.discord_webhook.trim()) {
        return;
    }
    let st = &core.sched;
    let online = core.stack.active.load(SeqCst) && install::port_listening(crate::host_dir(&s).as_deref().map(install::server_port).unwrap_or(6969));
    let changed = st.was_online.swap(online, SeqCst) != online;
    let due = st.last_post.lock().unwrap().is_none_or(|t| t.elapsed() >= Duration::from_secs(s.discord_interval.max(1) as u64 * 60));
    // offline doesn't change, so post it once and then stay quiet
    if !(changed || st.force_post.swap(false, SeqCst) || (online && due)) {
        return;
    }
    *st.last_post.lock().unwrap() = Some(Instant::now());
    if let Err(e) = post_status(core) {
        note(core, &format!("Discord update failed: {e}"));
        st.force_post.store(true, SeqCst);
    }
}
