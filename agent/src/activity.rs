//! Activity history and screen-time usage, kept on his PC.
//!
//! Each day gets its own file (activity-YYYY-MM-DD.jsonl) beside agent.json,
//! one event per line. 30 days are kept. Usage (minutes used today and any
//! bonus time) lives in usage.json.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

static DIR: OnceLock<PathBuf> = OnceLock::new();
static RECENT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
static USAGE: Mutex<Option<Usage>> = Mutex::new(None);

pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let _ = DIR.set(dir);
    prune();
}
fn dir() -> PathBuf { DIR.get().cloned().unwrap_or_else(std::env::temp_dir) }
fn today() -> String { chrono::Local::now().format("%Y-%m-%d").to_string() }
fn now_secs() -> i64 { chrono::Local::now().timestamp() }

/// Records one event. `kind` is app, closed, blocked, site, request, decision, screen, power, file, message, time.
pub fn log(kind: &str, text: impl Into<String>) {
    let line = json!({"t": now_secs(), "kind": kind, "text": text.into()}).to_string();
    let path = dir().join(format!("activity-{}.jsonl", today()));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "{line}"); }
}

/// Same as `log`, but at most once per `every` for the same key (e.g. each website).
pub fn log_once(key: &str, every: Duration, kind: &str, text: impl Into<String>) {
    let m = RECENT.get_or_init(|| Mutex::new(HashMap::new()));
    let mut m = m.lock().unwrap();
    if m.get(key).map_or(false, |t| t.elapsed() < every) { return; }
    if m.len() > 5000 { m.clear(); }
    m.insert(key.to_string(), Instant::now());
    drop(m);
    log(kind, text);
}

/// A day's events, newest first. `day` is YYYY-MM-DD (empty = today).
pub fn read(day: &str) -> Vec<Value> {
    let day = if day.len() == 10 && day.chars().all(|c| c.is_ascii_digit() || c == '-') { day.to_string() } else { today() };
    let text = std::fs::read_to_string(dir().join(format!("activity-{day}.jsonl"))).unwrap_or_default();
    let mut v: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v.truncate(3000);
    v
}

/// Days that have history, newest first.
pub fn days() -> Vec<String> {
    let mut d: Vec<String> = std::fs::read_dir(dir()).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("activity-").and_then(|n| n.strip_suffix(".jsonl")).map(String::from)))
        .collect();
    d.sort(); d.reverse(); d
}

fn prune() {
    let cutoff = (chrono::Local::now() - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
    for d in days() { if d < cutoff { let _ = std::fs::remove_file(dir().join(format!("activity-{d}.jsonl"))); } }
}

// ---------- screen-time usage ----------
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Usage { pub day: String, pub seconds: u64, pub bonus_minutes: u32 }

fn load_usage() -> Usage {
    let mut g = USAGE.lock().unwrap();
    let mut u = g.clone().unwrap_or_else(|| std::fs::read_to_string(dir().join("usage.json")).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    if u.day != today() { u = Usage { day: today(), seconds: 0, bonus_minutes: 0 }; prune(); }
    *g = Some(u.clone());
    u
}
fn save_usage(u: &Usage) {
    *USAGE.lock().unwrap() = Some(u.clone());
    let _ = std::fs::write(dir().join("usage.json"), serde_json::to_string(u).unwrap_or_default());
}
pub fn usage() -> Usage { load_usage() }
pub fn add_seconds(s: u64) -> Usage { let mut u = load_usage(); u.seconds += s; save_usage(&u); u }
pub fn add_bonus(minutes: u32) -> Usage {
    let mut u = load_usage(); u.bonus_minutes = (u.bonus_minutes + minutes).min(24 * 60); save_usage(&u);
    log("time", format!("{minutes} minutes of extra time given"));
    u
}

/// For the parent apps: minutes used today, today's limit (if any), bonus.
pub fn usage_json(limit_minutes: Option<u32>) -> Value {
    let u = usage();
    json!({"used_minutes": u.seconds / 60, "limit_minutes": limit_minutes, "bonus_minutes": u.bonus_minutes})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn logs_and_reads_back() {
        init(std::env::temp_dir().join(format!("gact-{}", std::process::id())));
        log("app", "Opened Steam");
        log_once("site:a.com", Duration::from_secs(600), "site", "Visited a.com");
        log_once("site:a.com", Duration::from_secs(600), "site", "Visited a.com");
        let v = read("");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["text"], "Visited a.com");
        add_seconds(125);
        assert_eq!(add_bonus(30).bonus_minutes, 30);
        assert_eq!(usage().seconds, 125);
        assert_eq!(read("../../etc/passwd").len(), 3, "bad day names fall back to today");
    }
}
