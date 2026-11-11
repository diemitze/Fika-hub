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
    fired_at: Mutex<Option<Instant>>,
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
    Local {
        day: format!("{}-{}-{}", t.wYear, t.wMonth, t.wDay),
        minute: t.wHour as i64 * 60 + t.wMinute as i64,
        second: t.wSecond as i64,
    }
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
    let session = crate::profiles::headless_ids(&host)
        .into_iter()
        .next()
        .or(s.last_profile.clone())
        .unwrap_or_default();
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
        if pending {
            crate::history::now()
        } else {
            crate::history::now() + (until * 60 - now.second).max(0) as u64 + if until == 0 && fired_today { 86400 } else { 0 }
        },
        SeqCst,
    );
    if !server_ours(core) {
        return;
    }

    if until == 5 && *st.warned_day.lock().unwrap() != now.day {
        *st.warned_day.lock().unwrap() = now.day.clone();
        note(core, "Scheduled restart in 5 minutes.");
        let why = if s.restart_wait_raids {
            "Waits for running raids to finish (30 min at most)."
        } else {
            "Running raids will be cut off."
        };
        discord_event(core, "Restart in 5 minutes", why, AMBER);
    }
    // a 10 minute window, so a slow tick or a server mid-reboot at that minute doesn't skip the day.
    // fired_at covers a window that crosses midnight, where the day string already changed.
    let since_target = (now.minute - target).rem_euclid(1440);
    let fired_recently = st
        .fired_at
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() < Duration::from_secs(30 * 60));
    if since_target < 10 && !fired_today && !fired_recently && !pending {
        *st.fired_day.lock().unwrap() = now.day.clone();
        *st.fired_at.lock().unwrap() = Some(Instant::now());
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
                discord_event(
                    core,
                    "Restart is waiting",
                    &format!("{busy} raid{} still running, 30 min at most.", if busy == 1 { "" } else { "s" }),
                    AMBER,
                );
            }
            return;
        }
    }
    // a restart already in progress (a crash restart, or the user) gets another try next tick
    if core.stack.restarting.load(SeqCst) {
        return;
    }
    *st.pending_since.lock().unwrap() = None;
    discord_event(core, "Restarting now", "The server will be back in a few minutes.", AMBER);
    if let Err(e) = crate::procs::restart_server(core, "schedule") {
        note(core, &format!("Scheduled restart failed: {e}"));
    }
}

// ---------------- Discord ----------------

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
    url.starts_with("https://discord.com/api/webhooks/")
        || url.starts_with("https://discordapp.com/api/webhooks/")
        || url.starts_with("https://ptb.discord.com/api/webhooks/")
}

fn state_file() -> std::path::PathBuf {
    crate::settings::data_dir().join("discord.json")
}

pub const GREEN: u32 = 0x3fd18b;
pub const AMBER: u32 = 0xf0b232;
const GREY: u32 = 0x4e5058;

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn short_dur(secs: u64) -> String {
    let (h, m) = (secs / 3600, secs / 60 % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Discord caps field values at 1024 characters.
fn clip(s: String) -> String {
    if s.chars().count() <= 1000 {
        return s;
    }
    s.chars().take(997).collect::<String>() + "..."
}

/// RFC 3339 UTC, what an embed timestamp wants.
fn iso_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

fn status_payload(core: &Core, online: bool, last_online: Option<u64>) -> serde_json::Value {
    let now = crate::history::now();
    if !online {
        let desc = last_online.map(|t| format!("Last online <t:{t}:R>")).unwrap_or_default();
        return json!({
            "username": "Fika Hub",
            "embeds": [{ "title": "○ Offline", "color": GREY, "description": with_quip("Offline", &desc) }]
        });
    }

    let l = lobby(core).unwrap_or_default();
    let (in_raid, in_menus): (Vec<&fika::Player>, Vec<&fika::Player>) = l.players.iter().partition(|p| p.activity == 1);

    let mut sub = vec![];
    if let Some(up) = crate::procs::server_uptime(core) {
        sub.push(format!("Up for {}", short_dur(up)));
    }
    if let Some(v) = &l.version {
        sub.push(format!("Fika {v}"));
    }
    let host = crate::host_dir(&core.settings.lock().unwrap());
    if let Some(v) = host.as_deref().and_then(install::spt_version) {
        sub.push(format!("SPT {v}"));
    }

    let mut fields = vec![
        json!({ "name": "In raid", "value": in_raid.len().to_string(), "inline": true }),
        json!({ "name": "In menus", "value": in_menus.len().to_string(), "inline": true }),
    ];
    let next = core.sched.next_restart.load(SeqCst);
    if next > 0 {
        fields.push(json!({ "name": "Next restart", "value": format!("<t:{next}:R>"), "inline": true }));
    }

    // one block per map, with the players grouped by where their raid is
    let mut maps: Vec<(String, Vec<&fika::Player>)> = vec![];
    for p in &in_raid {
        let map = p.location.as_deref().map(map_name).unwrap_or_else(|| "Raid".into());
        match maps.iter_mut().find(|(m, _)| *m == map) {
            Some((_, v)) => v.push(p),
            None => maps.push((map, vec![p])),
        }
    }
    for (map, ps) in maps.iter().take(20) {
        let mut name = format!("{map} · {}", plural(ps.len(), "player"));
        if let Some(t) = ps.iter().map(|p| p.since).filter(|s| *s > 0).min() {
            name += &format!(" · {} min in", now.saturating_sub(t as u64) / 60);
        }
        let names = ps.iter().map(|p| p.nickname.as_str()).collect::<Vec<_>>().join(", ");
        fields.push(json!({ "name": name, "value": clip(format!("└ {names}")) }));
    }
    if !in_menus.is_empty() {
        let names = in_menus
            .iter()
            .map(|p| match p.activity {
                2 => format!("{} (stash)", p.nickname),
                3 => format!("{} (hideout)", p.nickname),
                4 => format!("{} (flea)", p.nickname),
                _ => p.nickname.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        fields.push(json!({ "name": "In menus", "value": clip(format!("└ {names}")) }));
    }

    let title = match l.players.len() {
        0 => "● Online · nobody on".to_string(),
        n => format!("● Online · {}", plural(n, "player")),
    };
    json!({
        "username": "Fika Hub",
        "embeds": [{
            "title": title,
            "color": GREEN,
            "description": if l.players.is_empty() { with_quip("Empty", &sub.join(" · ")) } else { sub.join(" · ") },
            "fields": fields,
            "footer": { "text": "Fika Hub · Last updated" },
            "timestamp": iso_utc(now),
        }]
    })
}

async fn send(http: &reqwest::Client, url: &str, body: &serde_json::Value, edit: Option<&str>) -> Result<Option<String>, String> {
    let req = match edit {
        Some(id) => http.patch(format!("{url}/messages/{id}")),
        None => http.post(format!("{url}?wait=true")),
    };
    let res = req
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
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
    let mut saved: serde_json::Value = std::fs::read_to_string(state_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // the offline card says when it was last up, which has to survive a Hub restart
    if online {
        saved["lastOnline"] = crate::history::now().into();
    }
    let body = status_payload(core, online, saved.get("lastOnline").and_then(|v| v.as_u64()));
    let key = url.rsplit('/').nth(1).unwrap_or("").to_string();
    let old = saved.get(&key).and_then(|v| v.as_str()).map(String::from);

    let id = tauri::async_runtime::block_on(async {
        if let Some(id) = &old {
            if send(&core.web, &url, &body, Some(id)).await?.is_some() {
                return Ok(Some(id.clone()));
            }
        }
        send(&core.web, &url, &body, None).await
    })?;
    let mut m = saved.as_object().cloned().unwrap_or_default();
    if let Some(id) = id {
        m.insert(key, id.into());
    }
    let _ = std::fs::write(state_file(), serde_json::Value::Object(m).to_string());
    Ok(())
}

/// A random line from the list, good enough without pulling in a rand crate.
fn pick(lines: &[&'static str]) -> &'static str {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    lines[n % lines.len()]
}

/// The little joke under each alert, picked by the alert's title.
fn quip(title: &str) -> Option<&'static str> {
    let lines: &[&str] = match title {
        "Server crashed" => &[
            "Someone tell Nikita.",
            "Killed in action. Insurance claim filed.",
            "Killed by: Unknown. Classic.",
            "The server walked into a claymore.",
            "Desync claims another victim.",
        ],
        "Server keeps crashing" => &[
            "At this point it's a lifestyle.",
            "Three strikes, the server is benched.",
            "Tagilla hits less often than this.",
        ],
        "Server is back up" => &[
            "Survived. Extracted. Somehow.",
            "Insurance came through.",
            "Shook it off like a painkiller.",
            "Respawned with full health.",
        ],
        "Server failed to start" => &[
            "Died in the hideout. Impressive.",
            "Stuck on the loading screen, as tradition demands.",
            "Somebody's mod had other plans.",
        ],
        "Headless crashed" => &[
            "The headless lost its head. Again.",
            "The headless got headshot. Ironic.",
            "Headless? More like helpless.",
        ],
        "Headless keeps crashing" => &["The headless has filed for retirement.", "The headless has left the chat."],
        "Restart in 5 minutes" => &[
            "Find an extract, you have 5 minutes.",
            "Finish your raid or forever hold your loot.",
            "Good time to sell that junk on the flea.",
        ],
        "Restart is waiting" => &[
            "Someone is still looting Labs.",
            "Waiting on the guy checking every jacket.",
            "The server is camping the extract for you.",
        ],
        "Restarting now" => &[
            "Turning it off and on again. Works every time.",
            "Brb, server is getting a coffee.",
            "Quick trip to the hideout for a craft.",
        ],
        "Offline" => &[
            "Shhh, the server is sleeping.",
            "Out of raid, out of mind.",
            "Touching grass, back later.",
        ],
        "Empty" => &[
            "Quiet out there. Too quiet.",
            "Plenty of loot, nobody to take it.",
            "The Scavs are getting bored.",
        ],
        _ => return None,
    };
    Some(pick(lines))
}

/// Puts the joke on its own italic line under the real text.
fn with_quip(title: &str, text: &str) -> String {
    match quip(title) {
        Some(q) if text.is_empty() => format!("*{q}*"),
        Some(q) => format!("{text}\n*{q}*"),
        None => text.to_string(),
    }
}

pub fn discord_event(core: &Core, title: &str, text: &str, color: u32) {
    let s = core.settings.lock().unwrap().clone();
    if !s.discord_enabled || !s.discord_events || !valid_hook(s.discord_webhook.trim()) {
        return;
    }
    let body = json!({
        "username": "Fika Hub",
        "embeds": [{ "title": title, "description": with_quip(title, text), "color": color }]
    });
    let _ = tauri::async_runtime::block_on(send(&core.web, s.discord_webhook.trim(), &body, None));
}

pub const RED: u32 = 0xe5484d;

/// Crash and recovery alerts. Posts on its own thread so a slow Discord never delays a restart.
pub fn discord_alert(core: &Core, title: &str, text: &str, color: u32) {
    let s = core.settings.lock().unwrap().clone();
    let url = s.discord_webhook.trim().to_string();
    if !s.discord_enabled || !s.discord_crashes || !valid_hook(&url) {
        return;
    }
    let body = json!({
        "username": "Fika Hub",
        "embeds": [{ "title": title, "description": with_quip(title, text), "color": color, "timestamp": iso_utc(crate::history::now()) }]
    });
    let http = core.web.clone();
    std::thread::spawn(move || {
        let _ = tauri::async_runtime::block_on(send(&http, &url, &body, None));
    });
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
    let online =
        core.stack.active.load(SeqCst) && install::port_listening(crate::host_dir(&s).as_deref().map(install::server_port).unwrap_or(6969));
    let changed = st.was_online.swap(online, SeqCst) != online;
    let due = st
        .last_post
        .lock()
        .unwrap()
        .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.discord_interval.max(1) as u64 * 60));
    // offline doesn't change, so post it once and then stay quiet
    if !(changed || st.force_post.swap(false, SeqCst) || (online && due)) {
        return;
    }
    *st.last_post.lock().unwrap() = Some(Instant::now());
    // no forced retry on failure: a dead webhook would get hit every tick, the interval retries instead
    if let Err(e) = post_status(core) {
        note(core, &format!("Discord update failed: {e}"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_utc_matches_known_dates() {
        assert_eq!(super::iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(super::iso_utc(1_793_738_096), "2026-11-03T20:34:56Z");
    }
}
