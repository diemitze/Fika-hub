use crate::{install, logs, Core};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

/// Logs can be huge after a long session; the tail is what support needs.
const MAX_FILE: u64 = 8 * 1024 * 1024;

fn tail_bytes(p: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().read(true).share_mode(7).open(p).ok()?;
    let len = f.metadata().ok()?.len();
    if len > MAX_FILE {
        f.seek(SeekFrom::Start(len - MAX_FILE)).ok()?;
    }
    let mut buf = vec![];
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn newest(dir: &Path, ext: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

fn listing(dir: &Path) -> String {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return format!("({} not found)\n", dir.display());
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let ver = install::file_version(&e.path()).map(|v| format!("  {v}")).unwrap_or_default();
            if e.path().is_dir() {
                format!("{n}/")
            } else {
                format!("{n}{ver}")
            }
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.join("\n") + "\n"
}

pub fn build(core: &Core, out: &Path) -> Result<(), String> {
    let s = core.settings.lock().unwrap().clone();
    let host = crate::host_dir(&s);
    let player = s.player_dir.clone().map(PathBuf::from);
    let headless = crate::headless_dir(&s);

    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opt = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zip.start_file(name, opt).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())
    };

    // the webhook URL is a secret, everything else in settings helps debugging
    let mut redacted = serde_json::to_value(&s).unwrap_or_default();
    if let Some(v) = redacted
        .get_mut("discordWebhook")
        .filter(|v| v.as_str().is_some_and(|x| !x.is_empty()))
    {
        *v = "(set, removed from bundle)".into();
    }
    add(
        "settings.json",
        serde_json::to_string_pretty(&redacted).unwrap_or_default().as_bytes(),
    )?;

    let mut info = format!("Fika Hub {}\n", env!("CARGO_PKG_VERSION"));
    for (label, dir) in [
        ("host", host.as_deref()),
        ("headless", headless.as_deref()),
        ("player", player.as_deref()),
    ] {
        let Some(d) = dir else { continue };
        info += &format!(
            "{label}: {}\n  SPT {}\n  Fika {}\n",
            d.display(),
            install::spt_version(d).unwrap_or_else(|| "?".into()),
            install::fika_version(d).unwrap_or_else(|| "?".into())
        );
    }
    if let Some(h) = host.as_deref() {
        let (ip, backend) = install::server_bind(h);
        info += &format!("port {}  ip {:?}  backendIp {:?}\n", install::server_port(h), ip, backend);
    }
    add("versions.txt", info.as_bytes())?;
    add("checks.txt", crate::health::report_text(core).as_bytes())?;

    for (i, name) in [(logs::SERVER, "server"), (logs::HEADLESS, "headless"), (logs::GAME, "game")] {
        add(&format!("console/{name}.log"), core.logs.dump(i).as_bytes())?;
    }
    if let Ok(h) = std::fs::read(crate::history::path()) {
        add("history.jsonl", &h)?;
    }

    if let Some(h) = host.as_deref() {
        if let Some(root) = install::spt_root(h) {
            if let Some(b) = newest(&root.join("user").join("logs").join("spt"), "log").and_then(|p| tail_bytes(&p)) {
                add("host/spt-server.log", &b)?;
            }
            if let Some(b) = std::fs::read(root.join("SPT_Data").join("configs").join("http.json")).ok() {
                add("host/http.json", &b)?;
            }
            add("host/server-mods.txt", listing(&root.join("user").join("mods")).as_bytes())?;
        }
    }
    if let Some(h) = headless.as_deref() {
        if let Some(b) = tail_bytes(&install::bepinex_log(h)) {
            add("headless/LogOutput.log", &b)?;
        }
        add("headless/plugins.txt", listing(&h.join("BepInEx").join("plugins")).as_bytes())?;
    }
    if let Some(p) = player.as_deref() {
        if let Some(b) = tail_bytes(&install::bepinex_log(p)) {
            add("game/LogOutput.log", &b)?;
        }
        add("game/plugins.txt", listing(&p.join("BepInEx").join("plugins")).as_bytes())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}
