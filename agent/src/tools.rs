//! Remote tools for parents: power, messages and files on his PC.

use base64::Engine as _;
use serde_json::{json, Value};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

pub const CHUNK: usize = 256 * 1024;
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024; // 1 GB

// ---------- power ----------
pub fn power(action: &str) -> bool {
    crate::activity::log("power", match action { "lock" => "Locked by a parent", "logoff" => "Signed out by a parent", "restart" => "Restarted by a parent", "shutdown" => "Shut down by a parent", _ => return false });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let run = |args: &[&str]| std::process::Command::new("shutdown.exe").args(args).creation_flags(0x0800_0000).spawn().is_ok();
        return match action {
            "lock" => { crate::winsetup::lock_screen(); true }
            "logoff" => unsafe {
                use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSLogoffSession, WTS_CURRENT_SERVER_HANDLE};
                WTSLogoffSession(WTS_CURRENT_SERVER_HANDLE, WTSGetActiveConsoleSessionId(), 0) != 0
            },
            "restart" => run(&["/r", "/t", "10", "/c", "A parent is restarting this PC."]),
            "shutdown" => run(&["/s", "/t", "10", "/c", "A parent is shutting down this PC."]),
            _ => false,
        };
    }
    #[cfg(not(windows))]
    { println!("[tools] power: {action}"); true }
}

// ---------- message box ----------
pub fn message(text: &str) {
    let text: String = text.chars().filter(|c| !c.is_control() || *c == '\n').take(500).collect();
    crate::activity::log("message", format!("Message sent: \"{text}\""));
    #[cfg(windows)]
    std::thread::spawn(move || unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        MessageBoxW(std::ptr::null_mut(), w(&text).as_ptr(), w("Message from a parent").as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND | MB_SYSTEMMODAL);
    });
    #[cfg(not(windows))]
    println!("[tools] message shown: {text}");
}

// ---------- files ----------
/// His user folder (e.g. C:\Users\Sam), found from the signed-in desktop.
pub fn user_home() -> PathBuf {
    if let Ok(t) = std::env::var("GUARDIAN_TEST_HOME") { return PathBuf::from(t); }
    #[cfg(windows)]
    {
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        let users = sysinfo::Users::new_with_refreshed_list();
        for p in sys.processes().values() {
            if p.name().to_string_lossy().eq_ignore_ascii_case("explorer.exe") {
                if let Some(u) = p.user_id().and_then(|id| users.get_user_by_id(id)) {
                    let home = PathBuf::from(format!(r"C:\Users\{}", u.name()));
                    if home.exists() { return home; }
                }
            }
        }
        PathBuf::from(r"C:\Users\Public")
    }
    #[cfg(not(windows))]
    { dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")) }
}

fn incoming_dir() -> PathBuf { user_home().join("Downloads").join("From Guardian") }

fn safe_name(name: &str) -> String {
    let base = name.rsplit(|c| c == '/' || c == '\\').next().unwrap_or("").to_string();
    let clean: String = base.chars().filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c)).take(150).collect();
    let clean = clean.trim().trim_matches('.').to_string();
    if clean.is_empty() { "file".into() } else { clean }
}

/// Receives one piece of a file a parent is sending. Returns the saved path when done.
pub fn put_chunk(name: &str, offset: u64, data_b64: &str, last: bool) -> Result<Option<String>, String> {
    use std::io::{Seek, SeekFrom, Write};
    let data = base64::engine::general_purpose::STANDARD.decode(data_b64).map_err(|_| "bad data")?;
    let dir = incoming_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.part", safe_name(name)));
    let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(offset == 0).open(&path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    f.write_all(&data).map_err(|e| e.to_string())?;
    if !last { return Ok(None); }
    drop(f);
    // Don't overwrite: "photo.jpg", "photo (2).jpg", ...
    let name = safe_name(name);
    let (stem, ext) = match name.rsplit_once('.') { Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")), _ => (name.clone(), String::new()) };
    let mut dest = dir.join(&name);
    let mut n = 2;
    while dest.exists() { dest = dir.join(format!("{stem} ({n}){ext}")); n += 1; }
    std::fs::rename(&path, &dest).map_err(|e| e.to_string())?;
    crate::activity::log("file", format!("A parent sent {}", dest.file_name().unwrap_or_default().to_string_lossy()));
    Ok(Some(dest.to_string_lossy().to_string()))
}

/// Lists a folder on his PC. Empty path = his main folders and drives.
pub fn list_dir(path: &str) -> Value {
    if path.is_empty() {
        let home = user_home();
        #[allow(unused_mut)]
        let mut entries: Vec<Value> = ["Desktop", "Documents", "Downloads", "Pictures", "Videos", "Music"].iter()
            .map(|n| home.join(n)).filter(|p| p.exists())
            .map(|p| json!({"name": p.file_name().unwrap().to_string_lossy(), "path": p.to_string_lossy(), "dir": true})).collect();
        #[cfg(windows)]
        for d in b'C'..=b'Z' {
            let root = format!("{}:\\", d as char);
            if Path::new(&root).exists() { entries.push(json!({"name": format!("Drive {}:", d as char), "path": root, "dir": true})); }
        }
        return json!({"path": "", "parent": null, "entries": entries});
    }
    let p = PathBuf::from(path);
    let mut entries: Vec<Value> = Vec::new();
    match std::fs::read_dir(&p) {
        Ok(rd) => for e in rd.flatten().take(5000) {
            let Ok(md) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('$') || name.eq_ignore_ascii_case("desktop.ini") { continue; }
            let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            entries.push(json!({"name": name, "path": e.path().to_string_lossy(), "dir": md.is_dir(), "size": md.len(), "mtime": mtime}));
        },
        Err(e) => return json!({"path": path, "error": e.to_string(), "entries": []}),
    }
    entries.sort_by(|a, b| (!a["dir"].as_bool().unwrap_or(false), a["name"].as_str().unwrap_or("").to_lowercase())
        .cmp(&(!b["dir"].as_bool().unwrap_or(false), b["name"].as_str().unwrap_or("").to_lowercase())));
    let parent = p.parent().map(|x| x.to_string_lossy().to_string());
    json!({"path": path, "parent": parent, "entries": entries})
}

/// Reads a file from his PC in pieces; `send` gets each piece as a message.
pub fn read_file(id: &str, path: &str, mut send: impl FnMut(Value) -> bool) {
    use std::io::Read;
    let fail = |send: &mut dyn FnMut(Value) -> bool, e: String| { send(json!({"type":"file_data","id":id,"error":e})); };
    let p = PathBuf::from(path);
    let Ok(md) = std::fs::metadata(&p) else { return fail(&mut send, "File not found".into()) };
    if md.is_dir() { return fail(&mut send, "That's a folder".into()); }
    if md.len() > MAX_DOWNLOAD { return fail(&mut send, "Files over 1 GB can't be downloaded".into()); }
    let Ok(mut f) = std::fs::File::open(&p) else { return fail(&mut send, "Can't open that file (it may be in use)".into()) };
    let (size, mut offset, mut buf) = (md.len(), 0u64, vec![0u8; CHUNK]);
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    loop {
        let n = match f.read(&mut buf) { Ok(n) => n, Err(e) => return fail(&mut send, e.to_string()) };
        let done = n == 0 || offset + n as u64 >= size;
        let msg = json!({"type":"file_data","id":id,"name":name,"size":size,"offset":offset,
            "data": base64::engine::general_purpose::STANDARD.encode(&buf[..n]),"done":done});
        if !send(msg) { return; }
        offset += n as u64;
        if done { break; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn names_are_made_safe() {
        assert_eq!(safe_name("../../evil.exe"), "evil.exe");
        assert_eq!(safe_name("C:\\x\\a:b?.txt"), "ab.txt");
        assert_eq!(safe_name(".."), "file");
    }
    #[test] fn sends_files_into_downloads_without_overwriting() {
        let home = std::env::temp_dir().join(format!("ghome-{}", std::process::id()));
        std::env::set_var("GUARDIAN_TEST_HOME", &home);
        let b = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
        assert_eq!(put_chunk("hi.txt", 0, &b(b"hel"), false).unwrap(), None);
        let p1 = put_chunk("hi.txt", 3, &b(b"lo"), true).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&p1).unwrap(), "hello");
        let p2 = put_chunk("hi.txt", 0, &b(b"x"), true).unwrap().unwrap();
        assert!(p2.ends_with("hi (2).txt"));
        let mut got = Vec::new();
        read_file("1", &p1, |m| { got.push(m); true });
        assert_eq!(got.last().unwrap()["done"], true);
        assert!(list_dir(home.join("Downloads").to_str().unwrap())["entries"].as_array().unwrap().len() == 1);
    }
}
