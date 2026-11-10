use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Write};

// The Fika routes on a 4.1 server: bodiless routes answer GET, the rest want a
// zlib'd JSON POST, and every response is raw zlib with no Content-Encoding.
// Auth is the profile id in PHPSESSID. The cert is SPT's self-signed one.

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("http client")
}

fn inflate(raw: &[u8]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if raw[0] == 0x78 {
        let mut s = String::new();
        if ZlibDecoder::new(raw).read_to_string(&mut s).is_ok() {
            return Some(s);
        }
    }
    Some(String::from_utf8_lossy(raw).into_owned())
}

async fn call(http: &reqwest::Client, url: String, session: &str, body: Option<&str>) -> Option<Value> {
    let req = match body {
        None => http.get(url),
        Some(json) => {
            let mut enc = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(json.as_bytes()).ok()?;
            http.post(url).header("Content-Type", "application/json").body(enc.finish().ok()?)
        }
    };
    let res = req.header("Cookie", format!("PHPSESSID={session}")).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let bytes = res.bytes().await.ok()?;
    serde_json::from_str(&inflate(&bytes)?).ok()
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub nickname: String,
    pub level: i64,
    pub activity: i64,
    pub since: i64,
    pub location: Option<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Raid {
    pub host: String,
    pub location: String,
    pub side: i64,
    pub status: i64,
    pub players: i64,
    pub headless: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Lobby {
    pub online: bool,
    pub version: Option<String>,
    pub players: Vec<Player>,
    pub raids: Vec<Raid>,
}

fn is_headless_name(s: &str) -> bool {
    s.to_lowercase().starts_with("headless_")
}

pub async fn lobby(http: &reqwest::Client, port: u16, session: &str) -> Lobby {
    let base = format!("https://127.0.0.1:{port}");
    let (presence, raids, version) = tokio::join!(
        call(http, format!("{base}/fika/presence/get"), session, None),
        call(http, format!("{base}/fika/location/raids"), session, Some("{}")),
        call(http, format!("{base}/fika/client/check/version"), session, None),
    );

    let online = presence.is_some() || raids.is_some() || version.is_some();
    let str_of = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let int_of = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);

    // The headless is a real Fika client, so it shows up in presence and in every
    // raid it hosts. It is infrastructure, not a player, so it never counts.
    let players = presence
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|p| !is_headless_name(&str_of(p, "nickname")))
                .map(|p| Player {
                    nickname: str_of(p, "nickname"),
                    level: int_of(p, "level"),
                    activity: int_of(p, "activity"),
                    since: int_of(p, "activityStartedTimestamp"),
                    location: p
                        .get("raidInformation")
                        .and_then(|r| r.get("location"))
                        .and_then(|l| l.as_str())
                        .map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();

    let raids = raids
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    let headless = r.get("isHeadless").and_then(|b| b.as_bool()).unwrap_or(false);
                    // same correction Fika's own matchmaker list applies
                    let players = (int_of(r, "playerCount") - headless as i64).max(0);
                    let requester = str_of(r, "headlessRequesterNickname");
                    Raid {
                        host: if headless && !requester.is_empty() {
                            requester
                        } else {
                            str_of(r, "hostUsername")
                        },
                        location: str_of(r, "location"),
                        side: int_of(r, "side"),
                        status: int_of(r, "status"),
                        players,
                        headless,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Lobby {
        online,
        version: version
            .as_ref()
            .and_then(|v| v.get("version"))
            .and_then(|v| v.as_str())
            .map(String::from),
        players,
        raids,
    }
}
