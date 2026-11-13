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
const MISSES_TO_STOP: u32 = 3;

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
    /// nickname -> profile as it was when they entered their raid
    snaps: HashMap<String, profiles::Profile>,
    /// in a raid but no profile on this server
    unknown: BTreeSet<String>,
    /// lobby polls in a row that got no answer
    misses: u32,
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
    let now: BTreeSet<String> = l
        .players
        .iter()
        .map(|p| p.nickname.clone())
        .filter(|n| !n.is_empty() && *n != me)
        .collect();
    // the first list after the server comes up is a baseline, not a wave of joins
    if let Some(before) = &t.online {
        let joined: Vec<&String> = now.difference(before).collect();
        let left: Vec<&String> = before.difference(&now).collect();
        let open = crate::notify::Open::Dashboard;
        if !joined.is_empty() {
            let title = if joined.len() == 1 {
                format!("{} connected", joined[0])
            } else {
                format!("{} players connected", joined.len())
            };
            let body = format!("{}\n{} online now", names(&joined), now.len());
            crate::notify::alert(core, crate::notify::Kind::Players, &title, &body, open);
        }
        if !left.is_empty() {
            let title = if left.len() == 1 {
                format!("{} disconnected", left[0])
            } else {
                format!("{} players disconnected", left.len())
            };
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
    let in_raid: BTreeSet<String> = lobby
        .as_ref()
        .map(|l| l.players.iter().filter(|p| p.activity == 1).map(|p| p.nickname.clone()).collect())
        .unwrap_or_default();

    // the slow parts (process scan, parsing every profile) run without the lock, so the Raids page never waits on them
    let (busy, need_list) = {
        let mut g = T.lock().unwrap();
        let t = g.get_or_insert_with(Tracker::default);
        watch_players(core, t, lobby.as_ref());
        let busy = lobby.is_some() || !t.live.is_empty() || !t.settling.is_empty();
        (busy, in_raid.iter().any(|n| !t.snaps.contains_key(n) && !t.unknown.contains(n)))
    };
    if !busy {
        return;
    }
    let st = crate::procs::status(core);
    let profs = if need_list {
        host.as_deref().map(profiles::list).unwrap_or_default()
    } else {
        vec![]
    };
    let ts = now();

    let mut g = T.lock().unwrap();
    let t = g.get_or_insert_with(Tracker::default);

    // a profile is snapshotted once when its player enters a raid, before the raid can change it
    for n in &in_raid {
        if t.snaps.contains_key(n) || t.unknown.contains(n) || !need_list {
            continue;
        }
        match profs.iter().find(|x| !x.headless && &x.nickname == n) {
            Some(pr) => {
                t.snaps.insert(n.clone(), pr.clone());
            }
            // a player whose profile isn't on this server, don't rescan for them every tick
            None => {
                t.unknown.insert(n.clone());
            }
        }
    }
    if lobby.is_some() {
        t.snaps.retain(|n, _| in_raid.contains(n));
        t.unknown.retain(|n| in_raid.contains(n));
        t.misses = 0;
    } else {
        t.misses += 1;
    }

    let current: Vec<&fika::Raid> = lobby
        .as_ref()
        .map(|l| l.raids.iter().filter(|r| r.status != 2).collect())
        .unwrap_or_default();
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
                if let Some(pr) = t.snaps.get(&p.nickname) {
                    live.before.entry(pr.id.clone()).or_insert_with(|| pr.clone());
                }
            }
        }
        if live.headless && st.headless.running {
            live.peak_ram = live.peak_ram.max(st.headless.ram);
            live.cpu_sum += st.headless.cpu as f64;
            live.cpu_n += 1;
        }
    }

    // one missed lobby poll (a busy server) mustn't split a raid in two, so only a real answer
    // without the raid, or a server that has been silent for a few polls, ends it
    let silent = lobby.is_none() && t.misses >= MISSES_TO_STOP;
    if lobby.is_some() || silent {
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
            let why = if silent {
                "server stopped"
            } else if hl_crashed(&raid) {
                "headless crashed"
            } else {
                "finished"
            };
            t.settling.push(Ended { raid, end: ts, why });
        }
    }

    if t.settling.is_empty() {
        return;
    }
    let me = my_nick(core, t);
    let dir = host.as_deref().and_then(crate::install::profiles_dir);
    let file = |id: &str| dir.as_ref().map(|d| d.join(format!("{id}.json")));
    // SPT saves profiles every minute, and only a save after the raid closed holds its results.
    // A file mtime is cheap, parsing a multi-MB profile isn't, so parse only once it's saved.
    let saved_after = |id: &str, end: u64| {
        file(id)
            .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_secs() >= end)
    };
    t.settling.retain(|e| {
        let all_saved = e.raid.before.keys().all(|id| saved_after(id, e.end));
        if !all_saved && ts.saturating_sub(e.end) < SETTLE_SECS {
            return true;
        }
        let profs: Vec<profiles::Profile> = e
            .raid
            .before
            .keys()
            .filter_map(|id| file(id).and_then(|p| profiles::read(&p)))
            .collect();
        write(core, e, &profs, &me);
        false
    });
}

/// Fika's own raid-limit restart is a clean quit, so only a crash the supervisor saw counts.
fn hl_crashed(r: &Live) -> bool {
    r.headless && crate::procs::last_headless_crash() >= r.seen
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
    // a Hub left open for weeks never restarts, so trim here too
    trim();

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
                if bits.is_empty() {
                    "Your raid is over.".into()
                } else {
                    bits.join(" · ")
                }
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
                "hlCrashed": hl_crashed(r),
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
