use std::path::{Path, PathBuf};

pub fn spt_root(install: &Path) -> Option<PathBuf> {
    // 4.1 renamed SPT to SPT_Runtime; support both so one build serves either layout
    ["SPT_Runtime", "SPT"]
        .iter()
        .map(|n| install.join(n))
        .find(|c| c.join("SPT.Server.exe").is_file())
}

pub fn fika_dir(install: &Path) -> PathBuf {
    install.join("BepInEx").join("plugins").join("Fika")
}

pub fn has_eft(install: &Path) -> bool {
    install.join("EscapeFromTarkov.exe").is_file()
}

pub fn has_fika_core(install: &Path) -> bool {
    fika_dir(install).join("Fika.Core.dll").is_file()
}

pub fn has_fika_headless(install: &Path) -> bool {
    fika_dir(install).join("Fika.Headless.dll").is_file()
}

/// None when the folder can host the stack, else a human reason.
pub fn host_problem(p: &Path) -> Option<String> {
    if !p.is_dir() {
        return Some("Folder doesn't exist.".into());
    }
    if !has_eft(p) {
        return Some("No EscapeFromTarkov.exe in that folder.".into());
    }
    if spt_root(p).is_none() {
        return Some("No SPT server (SPT_Runtime\\SPT.Server.exe) found.".into());
    }
    if !has_fika_core(p) {
        return Some("Fika isn't installed here.".into());
    }
    if !has_fika_headless(p) {
        return Some("Fika headless plugin (Fika.Headless.dll) isn't installed here.".into());
    }
    None
}

pub fn player_problem(p: &Path) -> Option<String> {
    if !p.is_dir() {
        return Some("Folder doesn't exist.".into());
    }
    if !has_eft(p) {
        return Some("No EscapeFromTarkov.exe in that folder.".into());
    }
    if !has_fika_core(p) {
        return Some("Fika isn't installed here.".into());
    }
    None
}

pub fn bepinex_log(install: &Path) -> PathBuf {
    install.join("BepInEx").join("LogOutput.log")
}

pub fn profiles_dir(host: &Path) -> Option<PathBuf> {
    spt_root(host).map(|r| r.join("user").join("profiles"))
}

/// Opens and drops a TCP connection. Only used for addresses the TCP table can't answer for.
pub fn reachable(addr: &str, timeout: std::time::Duration) -> bool {
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .map(|a| std::net::TcpStream::connect_timeout(&a, timeout).is_ok())
        .unwrap_or(false)
}

/// The address other PCs on the LAN would use. Connecting a UDP socket sends nothing,
/// it just makes Windows pick the outgoing interface.
pub fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// SPT's http.json "ip" and "backendIp".
pub fn server_bind(host: &Path) -> (Option<String>, Option<String>) {
    let v = spt_root(host)
        .map(|r| r.join("SPT_Data").join("configs").join("http.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let get = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str()).map(String::from);
    (get("ip"), get("backendIp"))
}

/// Port from SPT's http.json, so a changed port doesn't silently break the launcher.
pub fn server_port(host: &Path) -> u16 {
    spt_root(host)
        .map(|r| r.join("SPT_Data").join("configs").join("http.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("port").and_then(|p| p.as_u64()))
        .map(|p| p as u16)
        .unwrap_or(6969)
}

pub fn spt_version(install: &Path) -> Option<String> {
    spt_root(install).and_then(|r| file_version(&r.join("SPT.Server.exe")))
}

pub fn fika_version(install: &Path) -> Option<String> {
    file_version(&fika_dir(install).join("Fika.Core.dll"))
}

pub fn file_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };

    if !path.is_file() {
        return None;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let root = [b'\\' as u16, 0];
        if VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut ptr, &mut len) == 0 || len == 0 {
            return None;
        }
        let info = &*(ptr as *const VS_FIXEDFILEINFO);
        let (ms, ls) = (info.dwFileVersionMS, info.dwFileVersionLS);
        Some(format!("{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16))
    }
}

/// True when something is listening on the port, read from the TCP table so we
/// never open a connection to the server just to check.
pub fn port_listening(port: u16) -> bool {
    use windows_sys::Win32::NetworkManagement::IpHelper::GetTcpTable;

    unsafe {
        let mut size = 0u32;
        GetTcpTable(std::ptr::null_mut(), &mut size, 0);
        if size == 0 {
            return false;
        }
        let mut buf = vec![0u32; (size as usize / 4) + 1];
        if GetTcpTable(buf.as_mut_ptr().cast(), &mut size, 0) != 0 {
            return false;
        }
        let n = buf[0] as usize;
        // MIB_TCPROW is five u32s: state, local addr, local port, remote addr, remote port
        for i in 0..n {
            let row = &buf[1 + i * 5..1 + i * 5 + 5];
            let state = row[0];
            let local_port = u16::from_be((row[2] & 0xffff) as u16);
            if state == 2 && local_port == port {
                return true;
            }
        }
        false
    }
}

pub fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}
