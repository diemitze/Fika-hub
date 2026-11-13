use crate::{fika, history::now, profiles, schedule::map_name, Core};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEEP: usize = 500;
// the server writes profiles a few seconds after the raid closes
const SETTLE_SECS: u64 = 90;

#[derive(Clone)]
struct Live {
    location: String,
    host: String,
    headless: bool,
    side: i64,
    seen: u64,
    started: Option<u64>,
    players: BTreeSet<String>,
    peak_players: i64,
    hl_pid: u32,
    hl_crashed: bool,
    peak_ram: u64,
    cpu_sum: f64,
    cpu_n: u32,
    /// profile id -> stats when the player was first seen in this raid
    before: HashMap<String, profiles::Profile>,
}

struct Ended {
    raid: Live,
    end: u64,
    why: &'static str,
}

#[derive(Default)]
struct Tracker {
    live: HashMap<(String, String), Live>,
    settling: Vec<Ended>,
    snapped: BTreeSet<String>,
    /// who was online last tick, None until the server first answers
    online: Option<BTreeSet<String>>,
    /// (profile id, nickname) of whoever is picked under Playing as
    me: (String, String),
}

static T: Mutex<Option<Tracker>> = Mutex::new(None);

/// The picked profile's nickname, re-read only when the pick changes.
fn my_nick(core: &Core, t: &mut Tracker) -> String {
    let s = core.settings.lock().unwrap().clone();
    let id = s.last_profile.clone().unwrap_or_default();
    if t.me.0 != id {
        let nick = crate::host_dir(&s)
            .and_then(|h| crate::install::profiles_dir(&h))
            .and_then(|d| profiles::read(&d.join(format!("{id}.json"))))
            .map(|p| p.nickname)
            .unwrap_or_default();
        t.me = (id, nick);
    }
    t.me.1.clone()
}

fn names(list: &[&String]) -> String {
    match list {
        [one] => one.to_string(),
        _ => list.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
    }
}

fn watch_players(core: &Core, t: &mut Tracker, lobby: Option<&fika::Lobby>) {
    let Some(l) = lobby else {
        t.online = None;
        return;
    };
    let me = my_nick(core, t);
    let now: BTreeSet<String> = l.players.iter().map(|p| p.nickname.clone()).filter(|n| !n.is_empty() && *n != me).collect();
    // the first list after the server comes up is a baseline, not a wave of joins
    if let Some(before) = &t.online {
        let joined: Vec<&String> = now.difference(before).collect();
        let left: Vec<&String> = before.difference(&now).collect();
        let open = crate::notify::Open::Dashboard;
        if !joined.is_empty() {
            let title = if joined.len() == 1 { format!("{} connected", joined[0]) } else { format!("{} players connected", joined.len()) };
            let body = format!("{}\n{} online now", names(&joined), now.len());
            crate::notify::alert(core, crate::notify::Kind::Players, &title, &body, open);
        }
        if !left.is_empty() {
            let title = if left.len() == 1 { format!("{} disconnected", left[0]) } else { format!("{} players disconnected", left.len()) };
            let body = format!("{}\n{} online now", names(&left), now.len());
            crate::notify::alert(core, crate::notify::Kind::Players, &title, &body, open);
        }
    }
    t.online = Some(now);
}

fn path() -> PathBuf {
    crate::settings::data_dir().join("raids.jsonl")
}

pub fn start(core: Arc<Core>) {
    std::thread::spawn(move || loop {
        tick(&core);
        std::thread::sleep(Duration::from_secs(5));
    });
}

fn tick(core: &Core) {
    let lobby = crate::schedule::lobby(core);
    let host = crate::host_dir(&core.settings.lock().unwrap());
    // only read profiles when a raid needs them, they can be several MB each
    let mut g = T.lock().unwrap();
    let t = g.get_or_insert_with(Tracker::default);
    watch_players(core, t, lobby.as_ref());
    if lobby.is_none() && t.live.is_empty() && t.settling.is_empty() {
        return;
    }
    let st = crate::procs::status(core);
    let ts = now();

    let current: Vec<&fika::Raid> = lobby.as_ref().map(|l| l.raids.iter().filter(|r| r.status != 2).collect()).unwrap_or_default();
    // snapshot a player's profile once when they enter a raid, forgotten when they leave it
    let in_raid: BTreeSet<String> = lobby
        .as_ref()
        .map(|l| l.players.iter().filter(|p| p.activity == 1).map(|p| p.nickname.clone()).collect())
        .unwrap_or_default();
    let newcomer = in_raid.iter().any(|n| !t.snapped.contains(n));
    t.snapped = in_raid;
    let profs = if newcomer { host.as_deref().map(profiles::list).unwrap_or_default() } else { vec![] };

    for r in &current {
        let key = (r.host.clone(), r.location.to_lowercase());
        let live = t.live.entry(key).or_insert_with(|| Live {
            location: r.location.clone(),
            host: r.host.clone(),
            headless: r.headless,
            side: r.side,
            seen: ts,
            started: None,
            players: BTreeSet::new(),
            peak_players: 0,
            hl_pid: 0,
            hl_crashed: false,
            peak_ram: 0,
            cpu_sum: 0.0,
            cpu_n: 0,
            before: HashMap::new(),
        });
        if r.status == 1 && live.started.is_none() {
            live.started = Some(ts);
        }
        live.peak_players = live.peak_players.max(r.players);
        if let Some(l) = &lobby {
            for p in l.players.iter().filter(|p| p.activity == 1) {
                let here = p.location.as_deref().is_some_and(|loc| loc.eq_ignore_ascii_case(&r.location));
                if !here {
                    continue;
                }
                live.players.insert(p.nickname.clone());
                if let Some(pr) = profs.iter().find(|x| !x.headless && x.nickname == p.nickname) {
                    live.before.entry(pr.id.clone()).or_insert_with(|| pr.clone());
                }
            }
        }
        if live.headless {
            if st.headless.running {
                if live.hl_pid == 0 {
                    live.hl_pid = st.headless.pid;
                } else if live.hl_pid != st.headless.pid {
                    live.hl_crashed = true;
                }
                live.peak_ram = live.peak_ram.max(st.headless.ram);
                live.cpu_sum += st.headless.cpu as f64;
                live.cpu_n += 1;
            } else if live.hl_pid != 0 {
                live.hl_crashed = true;
            }
        }
    }

    let gone: Vec<(String, String)> = t
        .live
        .keys()
        .filter(|k| !current.iter().any(|r| r.host == k.0 && r.location.eq_ignore_ascii_case(&k.1)))
        .cloned()
        .collect();
    for k in gone {
        let raid = t.live.remove(&k).unwrap();
        // a lobby that never reached the raid is a cancelled queue, not worth a row
        if raid.started.is_none() {
            continue;
        }
        let why = if lobby.is_none() {
            "server stopped"
        } else if raid.hl_crashed {
            "headless crashed"
        } else {
            "finished"
        };
        t.settling.push(Ended { raid, end: ts, why });
    }

    if t.settling.is_empty() {
        return;
    }
    let profs = host.as_deref().map(profiles::list).unwrap_or_default();
    let me = my_nick(core, t);
    t.settling.retain(|e| {
        let saved_after = |id: &str| profs.iter().any(|p| p.id == id && ts.saturating_sub(p.saved_ago) >= e.end);
        let all_saved = e.raid.before.keys().all(|id| saved_after(id));
        if !all_saved && ts.saturating_sub(e.end) < SETTLE_SECS {
            return true;
        }
        write(core, e, &profs, &me);
        false
    });
}

fn write(core: &Core, e: &Ended, profs: &[profiles::Profile], me: &str) {
    let r = &e.raid;
    let start = r.started.unwrap_or(r.seen);
    let players: Vec<Value> = r
        .players
        .iter()
        .map(|name| {
            let mut p = json!({ "name": name });
            let before = r.before.values().find(|b| &b.nickname == name);
            let after = before.and_then(|b| profs.iter().find(|x| x.id == b.id));
            if let (Some(b), Some(a)) = (before, after) {
                let d = |f: fn(&profiles::Profile) -> i64| (f(a) - f(b)).max(0);
                let outcome = if d(|x| x.survived) > 0 {
                    "survived"
                } else if d(|x| x.deaths) > 0 {
                    "died"
                } else {
                    ""
                };
                p["outcome"] = outcome.into();
                p["kills"] = d(|x| x.kills).into();
                p["pmcKills"] = d(|x| x.pmc_kills).into();
                p["bossKills"] = d(|x| x.boss_kills).into();
                p["xp"] = d(|x| x.experience).into();
                p["level"] = a.level.into();
            }
            p
        })
        .collect();
    let rec = json!({
        "start": start,
        "end": e.end,
        "loading": start.saturating_sub(r.seen),
        "map": r.location,
        "host": r.host,
        "headless": r.headless,
        "side": r.side,
        "peakPlayers": r.peak_players,
        "players": players,
        "peakRam": r.peak_ram,
        "avgCpu": if r.cpu_n > 0 { (r.cpu_sum / r.cpu_n as f64 * 10.0).round() / 10.0 } else { 0.0 },
        "why": e.why,
    });
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{rec}");
    }
    crate::history::record("raid_end", json!({ "map": r.location, "why": e.why }));

    // crashes already got their own alert
    if e.why == "finished" {
        let mins = e.end.saturating_sub(start) / 60;
        let title = format!("{} finished · {mins} min", map_name(&r.location));
        let mine = players.iter().find(|p| p["name"] == me);
        let body = match mine {
            Some(p) => {
                let mut bits = vec![];
                match p["outcome"].as_str() {
                    Some("survived") => bits.push("You survived".to_string()),
                    Some("died") => bits.push("You died".to_string()),
                    _ => {}
                }
                let kills = p["kills"].as_i64().unwrap_or(0);
                if kills > 0 {
                    bits.push(format!("{kills} kill{}", if kills == 1 { "" } else { "s" }));
                }
                let xp = p["xp"].as_i64().unwrap_or(0);
                if xp > 0 {
                    bits.push(format!("{xp} XP"));
                }
                if bits.is_empty() { "Your raid is over.".into() } else { bits.join(" · ") }
            }
            None => {
                let n = r.players.len();
                format!("{n} player{} in it.", if n == 1 { "" } else { "s" })
            }
        };
        crate::notify::alert(core, crate::notify::Kind::Info, &title, &body, crate::notify::Open::Raids);
    }
}

pub fn read(limit: usize) -> Vec<Value> {
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(limit)..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Raids still running, for the top of the page.
pub fn live() -> Vec<Value> {
    let g = T.lock().unwrap();
    let Some(t) = g.as_ref() else { return vec![] };
    t.live
        .values()
        .map(|r| {
            json!({
                "map": r.location,
                "host": r.host,
                "headless": r.headless,
                "side": r.side,
                "start": r.started,
                "seen": r.seen,
                "players": r.players.iter().collect::<Vec<_>>(),
                "peakPlayers": r.peak_players,
                "peakRam": r.peak_ram,
                "hlCrashed": r.hl_crashed,
            })
        })
        .collect()
}

pub fn clear() {
    let _ = std::fs::remove_file(path());
}

pub fn trim() {
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > KEEP * 2 {
        let _ = std::fs::write(path(), lines[lines.len() - KEEP..].join("\n") + "\n");
    }
}
