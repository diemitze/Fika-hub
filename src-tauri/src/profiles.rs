use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const ROUBLE_TPL: &str = "5449016a4bdc2d6f028b456f";

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub side: String,
    pub edition: String,
    pub level: i64,
    pub experience: i64,
    pub prestige: i64,
    pub raids: i64,
    pub survived: i64,
    pub kills: i64,
    pub deaths: i64,
    pub pmc_kills: i64,
    pub boss_kills: i64,
    pub scav_kills: i64,
    pub headshots: i64,
    pub win_streak: i64,
    pub longest_streak: i64,
    pub lifetime_secs: i64,
    pub bodies_looted: i64,
    pub roubles: i64,
    pub last_session: i64,
    pub headless: bool,
    pub valid: bool,
    pub saved_ago: u64,
}

pub fn read(path: &Path) -> Option<Profile> {
    let file = std::fs::File::open(path).ok()?;
    let root: Value = serde_json::from_reader(std::io::BufReader::new(file)).ok()?;

    let info = root.get("info")?;
    let pmc = root.pointer("/characters/pmc");
    let pinfo = pmc.and_then(|p| p.get("Info"));

    let s = |v: Option<&Value>, k: &str| -> String {
        v.and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let n = |v: Option<&Value>, k: &str| -> i64 { v.and_then(|v| v.get(k)).and_then(|x| x.as_i64()).unwrap_or(0) };

    let username = s(Some(info), "username");
    let id = s(Some(info), "id");
    if id.is_empty() {
        return None;
    }

    // Counters are a flat list of {Key:[...], Value}; match whole keys so "Kills"
    // doesn't swallow "KilledBoss" and friends.
    let mut c: HashMap<String, i64> = HashMap::new();
    if let Some(items) = pmc.and_then(|p| p.pointer("/Stats/Eft/OverallCounters/Items")).and_then(|v| v.as_array()) {
        for it in items {
            let key = it
                .get("Key")
                .and_then(|k| k.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("/"))
                .unwrap_or_default();
            let val = it.get("Value").and_then(|v| v.as_i64()).unwrap_or(0);
            c.insert(key, val);
        }
    }
    let ctr = |k: &str| *c.get(k).unwrap_or(&0);

    let roubles = pmc
        .and_then(|p| p.pointer("/Inventory/items"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|i| i.get("_tpl").and_then(|t| t.as_str()) == Some(ROUBLE_TPL))
                .map(|i| i.pointer("/upd/StackObjectsCount").and_then(|v| v.as_i64()).unwrap_or(1))
                .sum()
        })
        .unwrap_or(0);

    let valid = pmc.is_some()
        && root.pointer("/characters/scav").is_some()
        && root.get("dialogues").is_some()
        && root.get("inraid").is_some()
        && root.get("insurance").is_some();

    let saved_ago = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let nickname = s(pinfo, "Nickname");
    Some(Profile {
        headless: username.to_lowercase().starts_with("headless_"),
        nickname: if nickname.is_empty() { username.clone() } else { nickname },
        id,
        username,
        side: s(pinfo, "Side"),
        edition: s(Some(info), "edition"),
        level: n(pinfo, "Level"),
        experience: n(pinfo, "Experience"),
        prestige: n(pinfo, "PrestigeLevel"),
        raids: ctr("Sessions/Pmc"),
        survived: ctr("ExitStatus/Survived/Pmc"),
        kills: ctr("Kills"),
        deaths: ctr("Deaths"),
        pmc_kills: ctr("KilledPmc"),
        boss_kills: ctr("KilledBoss"),
        scav_kills: ctr("KilledSavage"),
        headshots: ctr("HeadShots"),
        win_streak: ctr("CurrentWinStreak/Pmc"),
        longest_streak: ctr("LongestWinStreak/Pmc"),
        lifetime_secs: ctr("LifeTime/Pmc"),
        bodies_looted: ctr("BodiesLooted"),
        roubles,
        last_session: pmc.and_then(|p| p.pointer("/Stats/Eft/LastSessionDate")).and_then(|v| v.as_i64()).unwrap_or(0),
        valid,
        saved_ago,
    })
}

pub fn list(host: &Path) -> Vec<Profile> {
    let Some(dir) = crate::install::profiles_dir(host) else { return vec![] };
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<Profile> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("json")).unwrap_or(false))
        .filter_map(|p| read(&p))
        .collect();
    out.sort_by_key(|p| (p.headless, p.saved_ago));
    out
}

pub fn headless_ids(host: &Path) -> Vec<String> {
    list(host).into_iter().filter(|p| p.headless).map(|p| p.id).collect()
}
