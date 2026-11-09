//! Everything Guardian enforces on the child's account, the approval
//! requests, and the parent override.
//!
//! Apps:  an app means its whole install folder (see apps.rs), so approving
//!        or denying Steam covers all of Steam's helper programs too.
//! Sites: a site means the whole domain (youtube.com covers www.youtube.com,
//!        m.youtube.com...). Right after a parent approves a site, the extra
//!        domains it loads (images, video servers) are added to it as helpers.
//! Each one is Approved, Denied, has Temporary Access, or is Unknown.
//! Unknown things are blocked and a request goes to every parent.
//!
//! None of this applies while a parent has "Take control" switched on.

use crate::apps;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

fn yes() -> bool { true }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    #[serde(default)]
    pub enabled: bool,
    /// "mon".."sun" -> list of "HH:MM-HH:MM" windows when the PC can be used.
    #[serde(default)]
    pub days: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Policy {
    /// "Deny": app folders (e.g. "c:/program files (x86)/steam") or exe names.
    #[serde(default)]
    pub blocked_apps: Vec<String>,
    /// "Approve": app folders or exe names.
    #[serde(default)]
    pub allowed_apps: Vec<String>,
    #[serde(default = "yes")]
    pub ask_unknown: bool,
    #[serde(default)]
    pub blocked_sites: Vec<String>,
    #[serde(default)]
    pub allowed_sites: Vec<String>,
    /// Extra domains an approved site needs, e.g. "youtube.com" -> ["ytimg.com", "googlevideo.com"].
    #[serde(default)]
    pub site_helpers: BTreeMap<String, Vec<String>>,
    #[serde(default = "yes")]
    pub ask_unknown_sites: bool,
    #[serde(default)]
    pub schedule: Schedule,
    /// "Unrestricted": every rule, request and the schedule switched off.
    #[serde(default)]
    pub unrestricted: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            blocked_apps: vec![], allowed_apps: vec![], ask_unknown: true,
            blocked_sites: vec![], allowed_sites: vec![], site_helpers: Default::default(), ask_unknown_sites: true,
            schedule: Default::default(), unrestricted: false,
        }
    }
}

/// Programs that must never be blocked, or Windows (or Guardian) stops working.
const PROTECTED: &[&str] = &[
    "explorer.exe", "winlogon.exe", "csrss.exe", "svchost.exe", "lsass.exe", "services.exe",
    "smss.exe", "wininit.exe", "dwm.exe", "system", "guardian-agent.exe", "guardian-agent",
];

/// One app rule: a folder (contains '/') or a bare exe name.
fn clean_app(a: &str) -> Option<String> {
    let a = a.trim();
    if a.is_empty() || a.len() > 260 { return None; }
    if a.contains('/') || a.contains('\\') {
        let f = apps::norm(a);
        if apps::too_broad(&f, "") || f.contains("/guardian") { return None; }
        Some(f)
    } else {
        let n = a.to_lowercase();
        (n.len() <= 64 && !PROTECTED.contains(&n.as_str())).then_some(n)
    }
}

/// One site rule: the registrable domain, e.g. "https://www.YouTube.com/watch" -> "youtube.com".
pub fn clean_site(s: &str) -> Option<String> {
    let mut d = s.trim().to_lowercase();
    for p in ["https://", "http://"] { if let Some(x) = d.strip_prefix(p) { d = x.to_string(); } }
    let d = d.split(['/', '?', '#', ':']).next().unwrap_or("").trim_end_matches('.');
    if d.is_empty() || d.len() > 253 || !d.contains('.') || !d.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        return None;
    }
    Some(site_of(d))
}

/// "www.bbc.co.uk" -> "bbc.co.uk"
pub fn site_of(domain: &str) -> String {
    psl::domain_str(domain).unwrap_or(domain).to_string()
}

fn clean_list(list: Vec<String>, f: fn(&str) -> Option<String>) -> Vec<String> {
    let mut out: Vec<String> = list.iter().filter_map(|a| f(a)).collect();
    out.sort(); out.dedup();
    out
}

/// If an item was just added to one list, take it off the other one.
fn resolve_moves(old_allow: &[String], old_block: &[String], allow: &mut Vec<String>, block: &mut Vec<String>) {
    let newly_allowed: Vec<String> = allow.iter().filter(|a| !old_allow.contains(a)).cloned().collect();
    let newly_blocked: Vec<String> = block.iter().filter(|a| !old_block.contains(a)).cloned().collect();
    block.retain(|a| !(newly_allowed.contains(a) && !newly_blocked.contains(a)));
    allow.retain(|a| !(newly_blocked.contains(a) && !newly_allowed.contains(a)));
}

impl Policy {
    /// Tidies the lists and removes anything that would break the PC.
    pub fn sanitized(mut self) -> Self {
        self.blocked_apps = clean_list(self.blocked_apps, clean_app);
        self.allowed_apps = clean_list(self.allowed_apps, clean_app);
        self.blocked_sites = clean_list(self.blocked_sites, clean_site);
        self.allowed_sites = clean_list(self.allowed_sites, clean_site);
        // Safety net if something is somehow on both lists: Deny wins.
        let (ba, bs) = (self.blocked_apps.clone(), self.blocked_sites.clone());
        self.allowed_apps.retain(|a| !ba.contains(a));
        self.allowed_sites.retain(|a| !bs.contains(a));
        let allowed = self.allowed_sites.clone();
        self.site_helpers.retain(|site, _| allowed.contains(site));
        for h in self.site_helpers.values_mut() { h.retain(|d| !bs.contains(d)); h.sort(); h.dedup(); }
        self.schedule.days.retain(|d, w| {
            w.retain(|x| parse_window(x).is_some());
            ["mon","tue","wed","thu","fri","sat","sun"].contains(&d.as_str())
        });
        self
    }

    /// Applies an edited policy on top of the current one. Anything moved
    /// between Approved and Denied ends up on the list it was just added to.
    pub fn merged_from(old: &Policy, new: Policy) -> Policy {
        let mut new = Policy {
            blocked_apps: clean_list(new.blocked_apps, clean_app),
            allowed_apps: clean_list(new.allowed_apps, clean_app),
            blocked_sites: clean_list(new.blocked_sites, clean_site),
            allowed_sites: clean_list(new.allowed_sites, clean_site),
            // Helpers are learned by Guardian, not typed in, so keep ours.
            site_helpers: old.site_helpers.clone(),
            ..new
        };
        resolve_moves(&old.allowed_apps, &old.blocked_apps, &mut new.allowed_apps, &mut new.blocked_apps);
        resolve_moves(&old.allowed_sites, &old.blocked_sites, &mut new.allowed_sites, &mut new.blocked_sites);
        new.sanitized()
    }

    fn app_rule_matches(rule: &str, name: &str, exe: &str) -> bool {
        if !rule.contains('/') { return rule == name; }
        if !apps::inside(exe, rule) { return false; }
        // A separate app stored inside this folder (e.g. a game in Steam's
        // library) has its own rule, so approving Steam doesn't approve every game.
        let home = apps::app_home(exe, "");
        !(home != rule && apps::inside(&home, rule))
    }
}

/// "16:00-20:30" -> (960, 1230) minutes past midnight
pub fn parse_window(w: &str) -> Option<(u32, u32)> {
    let (a, b) = w.trim().split_once('-')?;
    let m = |t: &str| -> Option<u32> {
        let (h, mm) = t.trim().split_once(':')?;
        let (h, mm): (u32, u32) = (h.parse().ok()?, mm.parse().ok()?);
        (h <= 24 && mm < 60 && h * 60 + mm <= 1440).then_some(h * 60 + mm)
    };
    let (s, e) = (m(a)?, m(b)?);
    (s < e).then_some((s, e))
}

impl Schedule {
    /// Is the PC allowed to be used at this weekday (0 = Monday) and minute of day?
    pub fn allows(&self, weekday: u32, minute: u32) -> bool {
        if !self.enabled { return true; }
        let day = ["mon","tue","wed","thu","fri","sat","sun"][weekday as usize % 7];
        self.days.get(day).map_or(false, |ws| ws.iter().filter_map(|w| parse_window(w)).any(|(s, e)| minute >= s && minute < e))
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Request {
    /// What a decision applies to: an app folder/exe, or "site:example.com".
    pub app: String,
    /// "app" or "site"
    pub kind: String,
    /// Friendly name for the popup.
    pub name: String,
    pub path: String,
    pub requested_at: u64,
}

#[derive(Clone, Debug)]
pub enum Event {
    NewRequest(Request),
    Resolved { app: String, decision: String },
    PolicyChanged,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Decision { Temporary, Approve, Deny }

struct TempGrant { granted: Instant, seen_running: bool }

/// How long he has to open an app after getting Temporary Access.
const TEMP_START_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How long Temporary Access to a website lasts.
pub const TEMP_SITE: Duration = Duration::from_secs(60 * 60);
/// After a parent approves a site, new domains in this window are its helpers.
fn helper_window_len() -> Duration {
    Duration::from_secs(std::env::var("GUARDIAN_TEST_HELPER_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60))
}

/// Domains Windows, browsers and most websites need to work at all.
const INFRA: &[&str] = &[
    "microsoft.com", "windows.com", "windowsupdate.com", "msftconnecttest.com", "msftncsi.com",
    "msedge.net", "digicert.com", "lencr.org", "letsencrypt.org", "sectigo.com", "globalsign.com",
    "usertrust.com", "gstatic.com", "googleapis.com", "cloudflare.com", "akamaized.net", "akamaihd.net",
    "akamai.net", "cloudfront.net", "fastly.net", "jsdelivr.net", "mozilla.net", "arpa",
];

#[derive(Clone)]
pub struct Shared {
    pub policy: Arc<RwLock<Policy>>,
    /// true while a parent is actively controlling: restrictions paused.
    pub parent_override: Arc<AtomicBool>,
    pub pending: Arc<Mutex<BTreeMap<String, Request>>>,
    temp: Arc<Mutex<HashMap<String, TempGrant>>>,
    temp_sites: Arc<Mutex<HashMap<String, Instant>>>,
    /// (site, until, permanent): helper-learning window after an approval.
    helper_window: Arc<Mutex<Option<(String, Instant, bool)>>>,
    /// Extra domains Guardian itself needs (e.g. the relay server).
    pub always_allow: Arc<Vec<String>>,
    pub events: broadcast::Sender<Event>,
    save: Arc<dyn Fn(&Policy) + Send + Sync>,
}

fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

impl Shared {
    pub fn new(p: Policy, always_allow: Vec<String>, save: impl Fn(&Policy) + Send + Sync + 'static) -> Self {
        Self {
            policy: Arc::new(RwLock::new(p.sanitized())),
            parent_override: Arc::new(AtomicBool::new(false)),
            pending: Default::default(),
            temp: Default::default(),
            temp_sites: Default::default(),
            helper_window: Default::default(),
            always_allow: Arc::new(always_allow),
            events: broadcast::channel(64).0,
            save: Arc::new(save),
        }
    }

    /// True while a parent is in control, or the PC is set to Unrestricted.
    pub fn overridden(&self) -> bool {
        self.parent_override.load(Ordering::SeqCst) || self.policy.read().map(|p| p.unrestricted).unwrap_or(false)
    }

    pub fn set_override(&self, on: bool) {
        let was = self.parent_override.swap(on, Ordering::SeqCst);
        if was != on {
            println!("[policy] {}", if on { "parent in control: restrictions paused" } else { "control handed back: restrictions active" });
            if !on && !self.overridden() { self.enforce_apps(); } // close blocked apps straight away
        }
    }

    pub fn set_policy(&self, p: Policy) -> Policy {
        let old = self.policy.read().unwrap().clone();
        let p = Policy::merged_from(&old, p);
        for a in p.allowed_apps.iter().chain(&p.allowed_sites).filter(|a| old.blocked_apps.contains(a) || old.blocked_sites.contains(a)) {
            println!("[policy] {a}: moved from Denied to Approved");
        }
        for a in p.blocked_apps.iter().chain(&p.blocked_sites).filter(|a| old.allowed_apps.contains(a) || old.allowed_sites.contains(a)) {
            println!("[policy] {a}: moved from Approved to Denied");
        }
        *self.policy.write().unwrap() = p.clone();
        (self.save)(&p);
        // Anything now on a list no longer needs asking about.
        self.pending.lock().unwrap().retain(|key, _| {
            let k = key.strip_prefix("site:").unwrap_or(key);
            !p.unrestricted && ![&p.allowed_apps, &p.blocked_apps, &p.allowed_sites, &p.blocked_sites].iter().any(|l| l.iter().any(|x| x == k))
        });
        if old.unrestricted != p.unrestricted {
            println!("[policy] {}", if p.unrestricted { "Unrestricted: all rules off" } else { "restrictions back on" });
            if !p.unrestricted { self.enforce_apps(); }
        }
        let _ = self.events.send(Event::PolicyChanged);
        p
    }

    fn request(&self, key: String, kind: &str, name: String, path: String) {
        if self.overridden() { return; }
        let mut pending = self.pending.lock().unwrap();
        if pending.contains_key(&key) { return; }
        let req = Request { app: key.clone(), kind: kind.into(), name, path, requested_at: now_secs() };
        pending.insert(key, req.clone());
        let _ = self.events.send(Event::NewRequest(req));
    }

    /// A parent pressed Temporary Access, Approve or Deny.
    pub fn decide(&self, key: &str, d: Decision) -> bool {
        let key = key.trim().to_lowercase();
        // Only the first parent to answer counts.
        if self.pending.lock().unwrap().remove(&key).is_none() { return false; }
        let mut p = self.policy.read().unwrap().clone();
        if let Some(site) = key.strip_prefix("site:") {
            let site = site.to_string();
            match d {
                Decision::Temporary => {
                    self.temp_sites.lock().unwrap().insert(site.clone(), Instant::now() + TEMP_SITE);
                    *self.helper_window.lock().unwrap() = Some((site, Instant::now() + helper_window_len(), false));
                }
                Decision::Approve => {
                    p.allowed_sites.push(site.clone());
                    self.set_policy(p);
                    *self.helper_window.lock().unwrap() = Some((site, Instant::now() + helper_window_len(), true));
                }
                Decision::Deny => { p.blocked_sites.push(site); self.set_policy(p); }
            }
        } else {
            match d {
                Decision::Temporary => {
                    self.temp.lock().unwrap().insert(key.clone(), TempGrant { granted: Instant::now(), seen_running: false });
                }
                Decision::Approve => { p.allowed_apps.push(key.clone()); self.set_policy(p); }
                Decision::Deny => { p.blocked_apps.push(key.clone()); self.set_policy(p); }
            }
        }
        println!("[policy] {key}: {d:?}");
        let label = match d { Decision::Temporary => "Temporary Access", Decision::Approve => "Approve", Decision::Deny => "Deny" };
        let _ = self.events.send(Event::Resolved { app: key, decision: label.into() });
        true
    }

    // ------------------------------------------------------------- apps

    fn enforce_apps(&self) {
        if self.overridden() { return; }
        let pol = self.policy.read().unwrap().clone();
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let mut running_homes: Vec<String> = Vec::new();
        for p in sys.processes().values() {
            let name = p.name().to_string_lossy().to_lowercase();
            if PROTECTED.contains(&name.as_str()) { continue; }
            let exe = p.exe().map(|e| apps::norm(&e.to_string_lossy())).unwrap_or_default();

            if pol.blocked_apps.iter().any(|r| Policy::app_rule_matches(r, &name, &exe)) {
                if p.kill() { println!("[policy] closed denied app {name}"); }
                continue;
            }
            if pol.allowed_apps.iter().any(|r| Policy::app_rule_matches(r, &name, &exe)) || !pol.ask_unknown { continue; }
            if exe.is_empty() || !is_user_app(&exe, p) { continue; }

            let home = apps::app_home(&exe, "");
            if let Some(g) = self.temp.lock().unwrap().get_mut(&home) {
                g.seen_running = true;
                running_homes.push(home);
                continue;
            }
            // Unknown: close it and ask a parent (once per app folder).
            if p.kill() { println!("[policy] closed unknown app {name}, asking parents"); }
            self.request(home.clone(), "app", apps::display_name(&home), exe);
        }

        // Temporary Access ends when the whole app is closed (or never opened in time).
        self.temp.lock().unwrap().retain(|home, g| {
            let keep = if g.seen_running { running_homes.contains(home) } else { g.granted.elapsed() < TEMP_START_WINDOW };
            if !keep { println!("[policy] temporary access for {home} ended"); }
            keep
        });
    }

    // ------------------------------------------------------------- sites

    /// Called by the DNS filter for every lookup. true = let it through.
    pub fn allow_domain(&self, domain: &str) -> bool {
        let domain = domain.trim_end_matches('.').to_lowercase();
        if !domain.contains('.') || domain.ends_with(".local") || domain.ends_with(".lan") || domain.ends_with(".home.arpa") {
            return true; // local network names
        }
        let site = site_of(&domain);
        if INFRA.contains(&site.as_str()) || self.always_allow.iter().any(|a| *a == site || *a == domain) { return true; }
        if self.overridden() { return true; }

        let pol = self.policy.read().unwrap().clone();
        if pol.blocked_sites.contains(&site) { return false; }
        if pol.allowed_sites.contains(&site) { return true; }
        if pol.site_helpers.values().any(|h| h.contains(&site)) { return true; }
        {
            let mut t = self.temp_sites.lock().unwrap();
            t.retain(|_, until| Instant::now() < *until);
            if t.contains_key(&site) { return true; }
        }
        if !pol.ask_unknown_sites { return true; }

        // Just after a parent said yes to a site: new domains are what it needs to load.
        let window = self.helper_window.lock().unwrap().clone();
        if let Some((main, until, permanent)) = window {
            if Instant::now() < until && !self.pending.lock().unwrap().contains_key(&format!("site:{site}")) {
                if permanent {
                    let mut p = pol.clone();
                    if p.allowed_sites.contains(&main) {
                        p.site_helpers.entry(main.clone()).or_default().push(site.clone());
                        let p = p.sanitized();
                        *self.policy.write().unwrap() = p.clone();
                        (self.save)(&p);
                        println!("[policy] {site} added as part of {main}");
                    }
                } else {
                    self.temp_sites.lock().unwrap().insert(site.clone(), Instant::now() + TEMP_SITE);
                    println!("[policy] {site} allowed temporarily as part of {main}");
                }
                return true;
            }
        }
        self.request(format!("site:{site}"), "site", site.clone(), domain);
        false
    }

    // ------------------------------------------------------------- schedule

    /// Is the PC allowed to be used right now?
    pub fn schedule_allows_now(&self) -> bool {
        if self.overridden() { return true; }
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        self.policy.read().unwrap().schedule.allows(now.weekday().num_days_from_monday(), now.hour() * 60 + now.minute())
    }
}

/// Is this a program he opened (worth asking about), rather than part of
/// Windows, Guardian, or a background service?
#[cfg(windows)]
fn is_user_app(exe: &str, p: &sysinfo::Process) -> bool {
    let windir = apps::norm(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    if apps::inside(exe, &windir) || exe.contains("/program files/guardian/") { return false; }
    // Skip system accounts (SYSTEM, LOCAL SERVICE, NETWORK SERVICE).
    match p.user_id() {
        Some(uid) => !matches!(uid.to_string().as_str(), "S-1-5-18" | "S-1-5-19" | "S-1-5-20"),
        None => false,
    }
}

/// Elsewhere (testing only): programs inside GUARDIAN_TEST_APPS count as his apps.
#[cfg(not(windows))]
fn is_user_app(exe: &str, _p: &sysinfo::Process) -> bool {
    std::env::var("GUARDIAN_TEST_APPS").map_or(false, |d| apps::inside(exe, &apps::norm(&d)))
}

/// Closes blocked apps forever in the background, independent of any connection.
pub async fn enforcer(shared: Shared) {
    loop {
        if !shared.overridden() {
            let s = shared.clone();
            let _ = tokio::task::spawn_blocking(move || s.enforce_apps()).await;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pol(a: &[&str], b: &[&str]) -> Policy {
        Policy { allowed_apps: a.iter().map(|s| s.to_string()).collect(), blocked_apps: b.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }
    #[test] fn denied_then_approved_moves_to_approved() {
        let p = Policy::merged_from(&pol(&[], &["steam.exe"]), pol(&["Steam.exe"], &["steam.exe"]));
        assert_eq!((p.allowed_apps, p.blocked_apps), (vec!["steam.exe".to_string()], vec![]));
    }
    #[test] fn approved_then_denied_moves_to_denied() {
        let p = Policy::merged_from(&pol(&["roblox.exe"], &[]), pol(&["roblox.exe"], &["roblox.exe"]));
        assert_eq!((p.allowed_apps, p.blocked_apps), (vec![], vec!["roblox.exe".to_string()]));
    }
    #[test] fn added_to_both_at_once_denies() {
        let p = Policy::merged_from(&pol(&[], &[]), pol(&["x.exe"], &["x.exe"]));
        assert_eq!((p.allowed_apps, p.blocked_apps), (vec![], vec!["x.exe".to_string()]));
    }
    #[test] fn other_apps_untouched() {
        let p = Policy::merged_from(&pol(&["a.exe"], &["b.exe", "c.exe"]), pol(&["a.exe", "c.exe"], &["b.exe", "c.exe"]));
        assert_eq!((p.allowed_apps, p.blocked_apps), (vec!["a.exe".to_string(), "c.exe".to_string()], vec!["b.exe".to_string()]));
    }
    #[test] fn folder_rules_are_normalised_and_broad_ones_refused() {
        let p = pol(&[r"C:\Program Files (x86)\Steam\", r"C:\Program Files", r"C:\Windows\System32", "C:\\"], &[]).sanitized();
        assert_eq!(p.allowed_apps, vec!["c:/program files (x86)/steam".to_string()]);
    }
    #[test] fn folder_rule_covers_helpers() {
        assert!(Policy::app_rule_matches("c:/program files (x86)/steam", "steamwebhelper.exe", "c:/program files (x86)/steam/bin/cef/steamwebhelper.exe"));
        assert!(!Policy::app_rule_matches("c:/program files (x86)/steam", "x.exe", "c:/program files (x86)/steamy/x.exe"));
    }
    #[test] fn launcher_rule_does_not_cover_its_games() {
        let steam = "c:/program files (x86)/steam";
        assert!(!Policy::app_rule_matches(steam, "rocketleague.exe", "c:/program files (x86)/steam/steamapps/common/rocket league/rocketleague.exe"));
        assert!(Policy::app_rule_matches("c:/program files (x86)/steam/steamapps/common/rocket league", "rocketleague.exe",
            "c:/program files (x86)/steam/steamapps/common/rocket league/binaries/rocketleague.exe"));
    }
    #[test] fn sites_are_cleaned_to_domains() {
        assert_eq!(clean_site("https://www.YouTube.com/watch?v=1").as_deref(), Some("youtube.com"));
        assert_eq!(clean_site("m.bbc.co.uk").as_deref(), Some("bbc.co.uk"));
        assert_eq!(clean_site("not a site"), None);
    }
    #[test] fn schedule_windows() {
        let mut s = Schedule { enabled: true, days: Default::default() };
        s.days.insert("mon".into(), vec!["16:00-20:30".into()]);
        assert!(s.allows(0, 17 * 60));
        assert!(!s.allows(0, 21 * 60));
        assert!(!s.allows(1, 17 * 60)); // no windows on Tuesday = not allowed
        s.enabled = false;
        assert!(s.allows(1, 17 * 60));
        assert_eq!(parse_window("20:00-16:00"), None);
    }
}

#[cfg(test)]
mod unrestricted_tests {
    use super::*;
    #[test] fn unrestricted_allows_everything() {
        let s = Shared::new(Policy { unrestricted: true, ask_unknown_sites: true, ..Default::default() }, vec![], |_: &Policy| {});
        assert!(s.allow_domain("anything-unknown.example"));
        assert!(s.schedule_allows_now());
        assert!(s.pending.lock().unwrap().is_empty());
    }
}
