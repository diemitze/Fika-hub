use crate::profiles::{counters, read, Profile};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Serialize, Default)]
pub struct Row {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Default)]
pub struct Group {
    pub title: String,
    pub rows: Vec<Row>,
}

#[derive(Serialize, Default)]
pub struct Bar {
    pub label: String,
    pub value: f64,
    pub text: String,
}

/// Everything the stats window shows, read from one profile file.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub profile: Profile,
    pub survivor_class: String,
    pub registered: i64,
    pub killed: i64,
    pub other_exits: i64,
    pub avg_life_secs: i64,
    pub groups: Vec<Group>,
    pub weapons: Vec<Bar>,
    pub targets: Vec<Bar>,
    pub damage_taken: Vec<Bar>,
    pub skills: Vec<Bar>,
}

fn grouped(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn nice_name(raw: &str) -> String {
    let mut out = String::new();
    for (i, ch) in raw.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

pub fn detail(host: &Path, id: &str) -> Option<Detail> {
    let path = crate::install::profiles_dir(host)?.join(format!("{id}.json"));
    let mut profile = read(&path)?;
    let root: Value = serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(&path).ok()?)).ok()?;
    let pmc = root.pointer("/characters/pmc");
    let c = counters(pmc);
    let k = |key: &str| *c.get(key).unwrap_or(&0);
    let row = |label: &str, v: i64| Row {
        label: label.into(),
        value: grouped(v),
    };
    let text_row = |label: &str, v: String| Row {
        label: label.into(),
        value: v,
    };

    let eft = pmc.and_then(|p| p.pointer("/Stats/Eft"));
    let survivor_class = eft
        .and_then(|e| e.get("SurvivorClass"))
        .and_then(|v| v.as_str())
        .filter(|v| *v != "Unknown")
        .unwrap_or("")
        .to_string();
    let registered = pmc
        .and_then(|p| p.pointer("/Info/RegistrationDate"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let killed = k("ExitStatus/Killed/Pmc");
    let other_exits = (profile.raids - profile.survived - killed).max(0);
    let avg_life = if profile.raids > 0 { k("LifeTime/Pmc") / profile.raids } else { 0 };

    let shots = k("AmmoUsed");
    let hit_rate = if shots > 0 {
        format!("{}%", k("AmmoReached") * 100 / shots)
    } else {
        "-".into()
    };
    let hs_rate = if profile.kills > 0 {
        format!("{}%", profile.headshots * 100 / profile.kills)
    } else {
        "-".into()
    };
    let combat = Group {
        title: "Combat".into(),
        rows: vec![
            row("Headshots", profile.headshots),
            text_row("Headshot rate", hs_rate),
            row("Shots fired", shots),
            text_row("Hit rate", hit_rate),
            text_row("Longest shot", format!("{:.1} m", k("LongestShot") as f64 / 100.0)),
            text_row("Longest kill", format!("{:.1} m", k("LongestKillShot") as f64 / 100.0)),
            row("Best kill streak", k("LongestKillStreak")),
            row("Damage dealt", k("CombatDamage")),
            row("Absorbed by armor", k("CauseArmorDamage")),
        ],
    };
    let health = Group {
        title: "Health".into(),
        rows: vec![
            row("Blood lost", k("BloodLoss")),
            row("Limbs lost", k("BodyPartsDestroyed")),
            row("HP healed", k("Heal")),
            row("Fractures", k("Fractures")),
            row("Concussions", k("Contusions")),
            row("Exhaustions", k("Exhaustions")),
            row("Medicine used", k("Medicines")),
            row("Food and drinks", k("UsedFoods") + k("UsedDrinks")),
        ],
    };
    let looting = Group {
        title: "Looting".into(),
        rows: vec![
            text_row("Distance travelled", format!("{:.1} km", k("Pedometer") as f64 / 1000.0)),
            row("Bodies looted", profile.bodies_looted),
            row("Safes opened", k("SafeLooted")),
            row("Weapons found", k("Weapons")),
            row("Ammo found", k("Ammunitions")),
            row("Mods found", k("Mods")),
            row("Keys found", k("Keys")),
            row("Roubles found", k("Money/RUB")),
            row("Dollars found", k("Money/USD")),
            row("Euros found", k("Money/EUR")),
        ],
    };
    let experience = Group {
        title: "Experience".into(),
        rows: vec![
            row("From kills", k("ExpKill")),
            row("From looting", k("ExpLooting")),
            row("From healing", k("Exp/ExpHeal")),
            row("From raid exits", k("Exp/ExpExitStatus")),
        ],
    };

    let bar = |label: &str, v: i64| Bar {
        label: label.into(),
        value: v as f64,
        text: grouped(v),
    };
    let mut weapons = vec![
        bar("Assault rifle", k("KilledWithAssaultRifle")),
        bar("SMG", k("KilledWithSmg")),
        bar("Sniper rifle", k("KilledWithSniperRifle")),
        bar("Shotgun", k("KilledWithShotgun")),
        bar("Marksman rifle", k("KilledWithMarksmanRifle")),
        bar("Pistol", k("KilledWithPistol")),
        bar("Grenade", k("KilledWithThrowWeapon")),
        bar("Machine gun", k("KilledWithMachineGun")),
        bar("Carbine", k("KilledWithAssaultCarbine")),
        bar("Knife", k("KilledWithKnife")),
    ];
    weapons.retain(|b| b.value > 0.0);
    weapons.sort_by(|a, b| b.value.total_cmp(&a.value));

    let mut targets = vec![
        bar("Scavs", k("KilledSavage")),
        bar("USEC", k("KilledUsec")),
        bar("BEAR", k("KilledBear")),
        bar("Bosses", profile.boss_kills),
    ];
    targets.retain(|b| b.value > 0.0);

    let parts = [
        ("Head", "Head"),
        ("Chest", "Chest"),
        ("Stomach", "Stomach"),
        ("Left arm", "LeftArm"),
        ("Right arm", "RightArm"),
        ("Left leg", "LeftLeg"),
        ("Right leg", "RightLeg"),
    ];
    let damage_taken: Vec<Bar> = parts
        .iter()
        .map(|(label, key)| bar(label, k(&format!("BodyPartDamage/{key}"))))
        .collect();

    let mut skills: Vec<(String, f64)> = pmc
        .and_then(|p| p.pointer("/Skills/Common"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| Some((s.get("Id")?.as_str()?.to_string(), s.get("Progress")?.as_f64()?)))
                .collect()
        })
        .unwrap_or_default();
    skills.sort_by(|a, b| b.1.total_cmp(&a.1));
    let skills = skills
        .into_iter()
        .take(6)
        .filter(|(_, p)| *p >= 100.0)
        .map(|(id, p)| Bar {
            label: nice_name(&id),
            value: p.min(5100.0),
            text: format!("Lv {}", (p / 100.0).floor() as i64),
        })
        .collect();

    let in_game = eft.and_then(|e| e.get("TotalInGameTime")).and_then(|v| v.as_i64()).unwrap_or(0);
    profile.lifetime_secs = profile.lifetime_secs.max(in_game);
    Some(Detail {
        profile,
        survivor_class,
        registered,
        killed,
        other_exits,
        avg_life_secs: avg_life,
        groups: vec![combat, health, looting, experience],
        weapons,
        targets,
        damage_taken,
        skills,
    })
}
