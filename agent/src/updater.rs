//! Keeps Guardian on his PC up to date by itself.
//!
//! Every few hours the protected service asks GitHub for the newest release of
//! cbevss/Guardian. If it's newer than this one, it downloads
//! `guardian-agent.exe` from that release (HTTPS, github.com only), swaps it in
//! and restarts. Nothing to click, on either PC.
#![cfg(windows)]

use std::{path::Path, time::Duration};

const LATEST: &str = "https://api.github.com/repos/cbevss/Guardian/releases/latest";
const ASSET: &str = "guardian-agent.exe";

pub fn version() -> &'static str {
    option_env!("GUARDIAN_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// "1.0.42" -> [1, 0, 42]
fn parse(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(120)).user_agent("Guardian-updater").build()
}

/// Returns the download URL if a newer version exists.
fn newer_release() -> Option<(String, String)> {
    let r: serde_json::Value = agent().get(LATEST).set("Accept", "application/vnd.github+json").call().ok()?.into_json().ok()?;
    let tag = r["tag_name"].as_str()?.to_string();
    if parse(&tag) <= parse(version()) { return None; }
    let url = r["assets"].as_array()?.iter()
        .find(|a| a["name"] == ASSET)?["browser_download_url"].as_str()?.to_string();
    if !url.starts_with("https://github.com/cbevss/Guardian/") { return None; }
    Some((tag, url))
}

/// Downloads and swaps the exe. Returns true if the service should restart.
pub fn check_and_install() -> bool {
    let Some((tag, url)) = newer_release() else { return false };
    let Ok(me) = std::env::current_exe() else { return false };
    let dir = me.parent().unwrap_or(Path::new("."));
    let new = dir.join("guardian-agent.new");
    let old = dir.join("guardian-agent.old");

    let Ok(resp) = agent().get(&url).call() else { return false };
    let mut bytes = Vec::new();
    if std::io::Read::read_to_end(&mut resp.into_reader().take(200 * 1024 * 1024), &mut bytes).is_err() { return false; }
    // Sanity check: a Windows program starts with "MZ" and isn't tiny.
    if bytes.len() < 500_000 || !bytes.starts_with(b"MZ") { return false; }
    if std::fs::write(&new, &bytes).is_err() { return false; }

    // A running exe can't be overwritten on Windows, but it can be renamed.
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(&me, &old).is_err() { let _ = std::fs::remove_file(&new); return false; }
    if std::fs::rename(&new, &me).is_err() { let _ = std::fs::rename(&old, &me); return false; }
    eprintln!("updated to {tag}");
    true
}

use std::io::Read as _;

/// Leftover from the last update.
pub fn cleanup() {
    if let Ok(me) = std::env::current_exe() {
        if let Some(d) = me.parent() { let _ = std::fs::remove_file(d.join("guardian-agent.old")); }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    #[test] fn compares_versions() {
        assert!(parse("v1.0.10") > parse("1.0.9"));
        assert!(parse("v1.0.3") <= parse("1.0.3"));
    }
}
