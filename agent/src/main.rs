//! Guardian agent: runs on the monitored PC, connects out to a relay and
//! streams the screen to a paired controller after checking the pairing code.
//!
//! Connection modes (set "mode" in agent.json):
//!   "home"  - finds the relay running on another PC on the same Wi-Fi (free)
//!   "relay" - connects to the relay URL in "relay" (e.g. a cloud server)

use futures::{SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::PathBuf, time::{Duration, Instant}};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

mod apps;
mod capture;
mod clipboard;
mod discovery;
mod input;
mod dns;
mod activity;
mod overlay;
mod tools;
mod policy;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod updater;
mod winsetup;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Mode { Home, Relay }

fn default_mode() -> Mode { Mode::Home }
fn default_upstream() -> String { dns::DEFAULT_UPSTREAM.into() }

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    #[serde(default = "default_mode")]
    mode: Mode,
    relay: String, // used in relay mode, e.g. wss://relay.example.com
    id: String,    // 9-digit device ID
    code: String,  // pairing code, keep secret
    #[serde(default)]
    policy: policy::Policy,
    /// Real DNS server allowed lookups go to. Default: Cloudflare for Families.
    #[serde(default = "default_upstream")]
    dns_upstream: String,
    /// Parent apps allowed in. The first one to pair is added automatically;
    /// after that a new parent app has to be approved by one already on the list.
    #[serde(default)]
    parents: Vec<ParentInfo>,
    /// The email account every parent app sends password-reset codes from.
    /// Set by the admin in their app; shared with approved parent apps only.
    #[serde(default)]
    mail: Option<serde_json::Value>,
    /// The admin's parent app (the first one signed in as ADMIN_EMAIL).
    #[serde(default)]
    admin_id: Option<String>,
}

const ADMIN_EMAIL: &str = "cbevs@icloud.com";

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ParentInfo { id: String, name: String, added: u64, #[serde(default)] email: String }

fn now_secs() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Who's allowed in, kept in agent.json.
#[derive(Clone)]
struct Roster {
    parents: std::sync::Arc<std::sync::Mutex<Vec<ParentInfo>>>,
    /// Parent apps that typed the right code but still need approving: id -> name.
    waiting: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, String>>>,
    pending_email: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, String>>>,
    save: std::sync::Arc<dyn Fn(&[ParentInfo]) + Send + Sync>,
    mail: std::sync::Arc<std::sync::Mutex<(Option<serde_json::Value>, Option<String>)>>,
    save_mail: std::sync::Arc<dyn Fn(&Option<serde_json::Value>, &Option<String>) + Send + Sync>,
}
impl Roster {
    fn is_parent(&self, id: &str) -> bool { self.parents.lock().unwrap().iter().any(|p| p.id == id) }
    fn list(&self) -> Vec<ParentInfo> { self.parents.lock().unwrap().clone() }
    fn add_with_email(&self, id: &str, name: &str, email: &str) {
        let mut l = self.parents.lock().unwrap();
        if !l.iter().any(|p| p.id == id) { l.push(ParentInfo { id: id.into(), name: clean_name(name), added: now_secs(), email: clean_email(email) }); }
        (self.save)(&l);
        self.waiting.lock().unwrap().remove(id);
    }
    fn set_email(&self, id: &str, email: &str) {
        let mut l = self.parents.lock().unwrap();
        let e = clean_email(email);
        if let Some(p) = l.iter_mut().find(|p| p.id == id) { if p.email != e { p.email = e; (self.save)(&l); } }
    }
    fn email_of(&self, id: &str) -> String { self.parents.lock().unwrap().iter().find(|p| p.id == id).map(|p| p.email.clone()).unwrap_or_default() }
    /// Admin = signed in as ADMIN_EMAIL, and the first parent app that did so (later look-alikes are refused).
    fn is_admin(&self, id: &str) -> bool {
        if self.email_of(id) != ADMIN_EMAIL { return false; }
        let mut m = self.mail.lock().unwrap();
        match &m.1 {
            Some(a) => a == id,
            None => { m.1 = Some(id.into()); (self.save_mail)(&m.0, &m.1); true }
        }
    }
    fn set_mail(&self, v: Option<serde_json::Value>) { let mut m = self.mail.lock().unwrap(); m.0 = v; (self.save_mail)(&m.0, &m.1); }
    fn remove(&self, id: &str) {
        let mut l = self.parents.lock().unwrap();
        l.retain(|p| p.id != id);
        (self.save)(&l);
    }
}
fn clean_email(e: &str) -> String { e.trim().to_lowercase().chars().filter(|c| !c.is_control()).take(100).collect() }
fn clean_name(n: &str) -> String {
    let n: String = n.chars().filter(|c| !c.is_control()).take(40).collect();
    if n.trim().is_empty() { "Parent".into() } else { n.trim().into() }
}

enum Admit { Yes, Waiting, No(&'static str) }

#[allow(dead_code)]
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn config_path() -> PathBuf {
    #[cfg(windows)]
    if INSTALLED.load(std::sync::atomic::Ordering::Relaxed) {
        let dir = service::data_dir();
        std::fs::create_dir_all(&dir).ok();
        return dir.join("agent.json");
    }
    let dir = dirs::config_dir().unwrap_or_else(|| ".".into()).join("Guardian");
    std::fs::create_dir_all(&dir).ok();
    dir.join("agent.json")
}

fn load_or_create_config() -> Config {
    let path = config_path();
    let mut cfg = match std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Config>(&s).ok()) {
        Some(c) => c,
        None => {
            let mut r = rand::thread_rng();
            const ALPHA: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
            Config {
                mode: Mode::Home,
                relay: "wss://your-relay.example.com".into(),
                id: format!("{:09}", r.gen_range(100_000_000u32..1_000_000_000)),
                code: (0..12).map(|_| ALPHA[r.gen_range(0..ALPHA.len())] as char).collect(),
                policy: Default::default(),
                dns_upstream: default_upstream(),
                parents: vec![],
                mail: None,
                admin_id: None,
            }
        }
    };
    // Optional overrides, handy for testing.
    if let Ok(m) = std::env::var("GUARDIAN_MODE") {
        cfg.mode = if m == "relay" { Mode::Relay } else { Mode::Home };
    }
    if let Ok(r) = std::env::var("GUARDIAN_RELAY") { cfg.relay = r; }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).expect("save config");
    cfg
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlMsg {
    ControllerJoined,
    ControllerLeft,
    Auth { code: String, #[serde(default)] parent_id: String, #[serde(default)] name: String, #[serde(default)] email: String },
    SelectMonitor { index: usize },
    SetQuality { fps: u32, quality: u8 },
    /// x, y are 0..1 positions on the currently viewed monitor.
    MouseMove { x: f64, y: f64 },
    MouseButton { button: String, down: bool },
    Wheel { dx: i32, dy: i32 },
    Key { key: String, down: bool },
    Clipboard { text: String },
    /// Parent switched "Take control" on/off: pauses/resumes restrictions.
    Control { on: bool },
    SetPolicy { policy: policy::Policy },
    // From the relay about parent inboxes (you and your mum):
    ParentJoined { pid: u64 },
    ParentLeft { pid: u64 },
    FromParent { pid: u64, msg: serde_json::Value },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ParentMsg {
    Auth { code: String, #[serde(default)] parent_id: String, #[serde(default)] name: String, #[serde(default)] email: String },
    /// decision is "Temporary", "Approve" or "Deny"
    Decide { app: String, decision: policy::Decision },
    SetPolicy { policy: policy::Policy },
    /// Admin only: the shared reset-email account.
    SetMail { mail: Option<serde_json::Value> },
    /// Let a new parent app in, or not.
    ApproveParent { id: String },
    DenyParent { id: String },
    RemoveParent { id: String },
    // ----- tools -----
    Power { action: String },
    Message { text: String },
    ListApps,
    CloseApp { home: String },
    GetActivity { #[serde(default)] day: String },
    AddTime { minutes: u32 },
    ListDir { #[serde(default)] path: String },
    GetFile { id: String, path: String },
    PutFile { id: String, name: String, offset: u64, data: String, #[serde(default)] last: bool },
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    #[cfg(windows)]
    match arg.as_str() {
        "install" => {
            INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed);
            match service::install() {
                Ok(()) => {
                    let cfg = load_or_create_config();
                    println!("Guardian is installed and running.\n");
                    println!("  Device ID:    {}", cfg.id);
                    println!("  Pairing code: {}", cfg.code);
                    println!("\nWrite these down somewhere safe. You need them to connect.");
                }
                Err(e) => { eprintln!("Install failed: {e}"); std::process::exit(1); }
            }
            return;
        }
        "uninstall" => {
            match service::uninstall() { Ok(()) => println!("Guardian removed."), Err(e) => eprintln!("Uninstall failed: {e}") }
            return;
        }
        "service" => { INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed); let _ = service::run_service(); return; }
        "run" => INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed),
        _ => {}
    }
    let installed = arg == "run";
    tokio::runtime::Runtime::new().expect("runtime").block_on(agent(installed));
}

async fn agent(installed: bool) {
    let cfg = load_or_create_config();
    let disk = std::sync::Arc::new(std::sync::Mutex::new(cfg.clone()));
    let save_disk = disk.clone();
    // Guardian's own relay server must always be reachable.
    let mut always = vec![];
    if let Some(h) = cfg.relay.split("://").nth(1).and_then(|r| r.split(['/', ':']).next()) {
        always.push(policy::site_of(h)); always.push(h.to_string());
    }
    activity::init(config_path().parent().map(|p| p.to_path_buf()).unwrap_or_else(std::env::temp_dir));
    let shared = policy::Shared::new(cfg.policy.clone(), always, move |p| {
        let mut c = save_disk.lock().unwrap(); c.policy = p.clone();
        let _ = std::fs::write(config_path(), serde_json::to_string_pretty(&*c).unwrap());
    });
    let roster_disk = disk.clone();
    let roster = Roster {
        parents: std::sync::Arc::new(std::sync::Mutex::new(cfg.parents.clone())),
        waiting: Default::default(),
        pending_email: Default::default(),
        save: std::sync::Arc::new(move |l: &[ParentInfo]| {
            let mut c = roster_disk.lock().unwrap(); c.parents = l.to_vec();
            let _ = std::fs::write(config_path(), serde_json::to_string_pretty(&*c).unwrap());
        }),
        mail: std::sync::Arc::new(std::sync::Mutex::new((cfg.mail.clone(), cfg.admin_id.clone()))),
        save_mail: { let d = disk.clone(); std::sync::Arc::new(move |m: &Option<serde_json::Value>, a: &Option<String>| {
            let mut c = d.lock().unwrap(); c.mail = m.clone(); c.admin_id = a.clone();
            let _ = std::fs::write(config_path(), serde_json::to_string_pretty(&*c).unwrap());
        }) },
    };
    tokio::spawn(policy::enforcer(shared.clone()));
    tokio::spawn(winsetup::schedule_enforcer(shared.clone()));
    // "Hide screen": keep the cover in step with the rules and parent control.
    { let s = shared.clone(); tokio::spawn(async move { loop {
        let hidden = s.policy.read().map(|p| p.screen_hidden).unwrap_or(false);
        overlay::update(hidden, s.parent_override.load(std::sync::atomic::Ordering::SeqCst));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    } }); }

    // Website filter. Installed on Windows it listens on the normal DNS port and
    // Windows is pointed at it; when testing, set GUARDIAN_DNS_ADDR instead.
    let dns_addrs: Vec<String> = match std::env::var("GUARDIAN_DNS_ADDR") {
        Ok(a) => vec![a],
        Err(_) if installed => vec!["127.0.0.1:53".into(), "[::1]:53".into()],
        Err(_) => vec![],
    };
    let upstream = std::env::var("GUARDIAN_DNS_UPSTREAM").unwrap_or_else(|_| cfg.dns_upstream.clone());
    for a in dns_addrs { tokio::spawn(dns::serve(shared.clone(), a, upstream.clone())); }
    if installed { tokio::spawn(winsetup::keep_applied()); }
    println!("Guardian agent");
    println!("  Device ID:    {}", cfg.id);
    println!("  Pairing code: {}", cfg.code);
    println!("  Mode:         {:?}", cfg.mode);
    println!("  Config file:  {}", config_path().display());

    loop {
        let url = match cfg.mode {
            Mode::Relay => cfg.relay.clone(),
            Mode::Home => {
                println!("looking for Guardian on the home network...");
                discovery::find_home_relay().await
            }
        };
        if let Err(e) = run(&cfg, &url, &shared, &roster).await { eprintln!("connection lost: {e}"); }
        tokio::time::sleep(Duration::from_secs(5)).await; // reconnect forever (unattended)
    }
}

struct Session {
    authed: bool,
    failures: u32,
    locked_until: Option<Instant>,
    monitor: usize,
    fps: u32,
    quality: u8,
    last_clip: String,
    /// The viewer's parent app ID, while it waits for approval.
    waiting_id: Option<String>,
    viewer_id: String,
}

fn text(v: serde_json::Value) -> Message { Message::Text(v.to_string()) }
fn to_parent(pid: u64, v: serde_json::Value) -> Message { text(json!({"type":"to_parent","pid":pid,"msg":v})) }

/// Checks a pairing code with lockout. Shared by the viewer and parent inboxes.
fn check_code(s: &mut Session, cfg: &Config, code: &str) -> Result<(), &'static str> {
    if s.locked_until.map_or(false, |t| Instant::now() < t) { return Err("locked"); }
    let ok: bool = code.trim().to_uppercase().as_bytes().ct_eq(cfg.code.as_bytes()).into();
    if ok { s.failures = 0; return Ok(()); }
    s.failures += 1;
    if s.failures >= 5 { s.locked_until = Some(Instant::now() + Duration::from_secs(300)); s.failures = 0; }
    Err("wrong_code")
}

/// Right code + a known parent app = in. Right code + a new parent app = wait
/// for a parent to approve it (unless it's the very first one).
fn admit(s: &mut Session, cfg: &Config, roster: &Roster, code: &str, id: &str, name: &str, email: &str) -> Admit {
    if let Err(r) = check_code(s, cfg, code) { return Admit::No(r); }
    if id.len() < 16 || id.len() > 64 { return Admit::No("update_app"); }
    if roster.list().is_empty() { roster.add_with_email(id, name, email); println!("first parent added: {name}"); return Admit::Yes; }
    if roster.is_parent(id) { roster.set_email(id, email); return Admit::Yes; }
    roster.waiting.lock().unwrap().insert(id.into(), clean_name(name));
    roster.pending_email.lock().unwrap().insert(id.into(), clean_email(email));
    Admit::Waiting
}

fn parent_welcome(shared: &policy::Shared, roster: &Roster, you: &str) -> serde_json::Value {
    let pending: Vec<_> = shared.pending.lock().unwrap().values().cloned().collect();
    let pol = shared.policy.read().unwrap().clone();
    let waiting: Vec<_> = roster.waiting.lock().unwrap().iter().map(|(id, name)| json!({"id":id,"name":name})).collect();
    let mail = roster.mail.lock().unwrap().0.clone();
    // Other parents' emails stay private; only names are shared.
    let parents: Vec<_> = roster.list().into_iter().map(|p| json!({"id":p.id,"name":p.name,"added":p.added})).collect();
    json!({"type":"parent_hello","pending":pending,"policy":pol,"parents":parents,"waiting_parents":waiting,"you":you,
           "mail":mail,"admin":roster.is_admin(you),"agent_version":option_env!("GUARDIAN_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))})
}

#[derive(Clone)]
struct PState { authed: bool, id: String }

/// Sends everyone the latest lists (rules, requests, parents).
async fn broadcast_roster(out: &mpsc::Sender<Message>, parents: &std::collections::HashMap<u64, PState>, shared: &policy::Shared, roster: &Roster) -> Result<(), Box<dyn std::error::Error>> {
    for (pid, p) in parents {
        if p.authed && roster.is_parent(&p.id) { out.send(to_parent(*pid, parent_welcome(shared, roster, &p.id))).await?; }
    }
    Ok(())
}

async fn run(cfg: &Config, relay: &str, shared: &policy::Shared, roster: &Roster) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/ws/agent?id={}", relay.trim_end_matches('/'), cfg.id);
    let (ws, _) = tokio_tungstenite::connect_async(&url).await?;
    println!("connected to {relay}");
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(4);

    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() { break; }
        }
    });

    let mut s = Session { authed: false, failures: 0, locked_until: None, monitor: 0, fps: 8, quality: 60, last_clip: String::new(), waiting_id: None, viewer_id: String::new() };
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / s.fps as u64));
    let mut clip_tick = tokio::time::interval(Duration::from_millis(1000));
    let mut inp = input::Input::new();
    let mut parents: std::collections::HashMap<u64, PState> = Default::default();
    let mut events = shared.events.subscribe();

    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { break };
                let Message::Text(t) = msg? else { continue };
                let Ok(cm) = serde_json::from_str::<ControlMsg>(&t) else { continue };
                match cm {
                    ControlMsg::ControllerJoined => {
                        s.authed = false; s.waiting_id = None; shared.set_override(false);
                        out_tx.send(text(json!({"type":"auth_required"}))).await?;
                    }
                    ControlMsg::ControllerLeft => { s.authed = false; inp.release_all(); shared.set_override(false); println!("controller left"); }
                    ControlMsg::Auth { code, parent_id, name, email } => match admit(&mut s, cfg, roster, &code, &parent_id, &name, &email) {
                        Admit::Waiting => {
                            s.waiting_id = Some(parent_id.clone());
                            out_tx.send(text(json!({"type":"auth_pending"}))).await?;
                            let m = json!({"type":"parent_request","id":parent_id,"name":clean_name(&name)});
                            for (pid, p) in &parents { if p.authed { out_tx.send(to_parent(*pid, m.clone())).await?; } }
                        }
                        Admit::Yes => {
                            s.authed = true; s.waiting_id = None; s.viewer_id = parent_id.clone();
                            s.last_clip = clipboard::get().unwrap_or_default();
                            println!("controller authenticated");
                            let pol = shared.policy.read().unwrap().clone();
                            out_tx.send(text(json!({"type":"hello","monitors":capture::list_monitors(),"policy":pol}))).await?;
                        }
                        Admit::No(reason) => out_tx.send(text(json!({"type":"auth_failed","reason":reason}))).await?,
                    },
                    ControlMsg::ParentJoined { pid } => {
                        parents.insert(pid, PState { authed: false, id: String::new() });
                        out_tx.send(to_parent(pid, json!({"type":"auth_required"}))).await?;
                    }
                    ControlMsg::ParentLeft { pid } => { parents.remove(&pid); }
                    ControlMsg::FromParent { pid, msg } => {
                        let Ok(pm) = serde_json::from_value::<ParentMsg>(msg) else { continue };
                        let me = parents.get(&pid).cloned().unwrap_or(PState { authed: false, id: String::new() });
                        // A parent removed while connected loses access straight away.
                        let authed = me.authed && roster.is_parent(&me.id);
                        match pm {
                            ParentMsg::Auth { code, parent_id, name, email } => match admit(&mut s, cfg, roster, &code, &parent_id, &name, &email) {
                                Admit::Yes => {
                                    parents.insert(pid, PState { authed: true, id: parent_id.clone() });
                                    println!("parent {pid} authenticated");
                                    out_tx.send(to_parent(pid, parent_welcome(shared, roster, &parent_id))).await?;
                                }
                                Admit::Waiting => {
                                    parents.insert(pid, PState { authed: false, id: parent_id.clone() });
                                    out_tx.send(to_parent(pid, json!({"type":"auth_pending"}))).await?;
                                    let m = json!({"type":"parent_request","id":parent_id,"name":clean_name(&name)});
                                    for (p2, st) in &parents { if st.authed { out_tx.send(to_parent(*p2, m.clone())).await?; } }
                                }
                                Admit::No(reason) => out_tx.send(to_parent(pid, json!({"type":"auth_failed","reason":reason}))).await?,
                            },
                            ParentMsg::Decide { app, decision } if authed => { shared.decide(&app, decision); }
                            ParentMsg::SetPolicy { policy } if authed => { shared.set_policy(policy); }
                            ParentMsg::SetMail { mail } if authed && roster.is_admin(&me.id) => {
                                roster.set_mail(mail.filter(|m| m.to_string().len() < 2000));
                                println!("reset email settings updated by admin");
                                broadcast_roster(&out_tx, &parents, shared, roster).await?;
                            }
                            ParentMsg::ApproveParent { id } if authed => {
                                let name = roster.waiting.lock().unwrap().get(&id).cloned();
                                let Some(name) = name else { continue };
                                let em = roster.pending_email.lock().unwrap().remove(&id).unwrap_or_default();
                                roster.add_with_email(&id, &name, &em);
                                println!("parent approved: {name}");
                                // Let them in right away if they're connected.
                                for (p2, st) in parents.iter_mut() {
                                    if st.id == id { st.authed = true; out_tx.send(to_parent(*p2, parent_welcome(shared, roster, &id))).await?; }
                                }
                                if s.waiting_id.as_deref() == Some(&id) {
                                    s.authed = true; s.waiting_id = None; s.viewer_id = id.clone();
                                    let pol = shared.policy.read().unwrap().clone();
                                    out_tx.send(text(json!({"type":"hello","monitors":capture::list_monitors(),"policy":pol}))).await?;
                                }
                                broadcast_roster(&out_tx, &parents, shared, roster).await?;
                            }
                            ParentMsg::Power { action } if authed => {
                                let ok = tools::power(&action);
                                out_tx.send(to_parent(pid, json!({"type":"tool_done","what":"power","ok":ok}))).await?;
                            }
                            ParentMsg::Message { text } if authed => {
                                tools::message(&text);
                                out_tx.send(to_parent(pid, json!({"type":"tool_done","what":"message","ok":true}))).await?;
                            }
                            ParentMsg::ListApps if authed => {
                                let list = tokio::task::spawn_blocking(policy::Shared::running_apps).await.unwrap_or_default();
                                let apps: Vec<_> = list.into_iter().map(|(home, name, pids, mem)| json!({"home":home,"name":name,"count":pids.len(),"memory":mem})).collect();
                                out_tx.send(to_parent(pid, json!({"type":"apps","apps":apps}))).await?;
                            }
                            ParentMsg::CloseApp { home } if authed => {
                                let h = home.clone();
                                let n = tokio::task::spawn_blocking(move || policy::Shared::close_app(&h)).await.unwrap_or(0);
                                if n > 0 { activity::log("closed", format!("{} closed by a parent", apps::display_name(&home))); }
                                out_tx.send(to_parent(pid, json!({"type":"tool_done","what":"close_app","ok":n > 0}))).await?;
                            }
                            ParentMsg::GetActivity { day } if authed => {
                                let items = activity::read(&day);
                                out_tx.send(to_parent(pid, json!({"type":"activity","day":day,"items":items,"days":activity::days(),
                                    "usage":activity::usage_json(shared.limit_today())}))).await?;
                            }
                            ParentMsg::AddTime { minutes } if authed => {
                                let _ = activity::add_bonus(minutes.min(600));
                                out_tx.send(to_parent(pid, json!({"type":"usage","usage":activity::usage_json(shared.limit_today())}))).await?;
                            }
                            ParentMsg::ListDir { path } if authed => {
                                let v = tokio::task::spawn_blocking(move || tools::list_dir(&path)).await.unwrap_or(json!({}));
                                out_tx.send(to_parent(pid, json!({"type":"dir","listing":v}))).await?;
                            }
                            ParentMsg::GetFile { id, path } if authed => {
                                let tx = out_tx.clone();
                                tokio::task::spawn_blocking(move || tools::read_file(&id, &path, |m| tx.blocking_send(to_parent(pid, m)).is_ok()));
                            }
                            ParentMsg::PutFile { id, name, offset, data, last } if authed => {
                                let r = tokio::task::spawn_blocking(move || tools::put_chunk(&name, offset, &data, last)).await.unwrap_or(Err("failed".into()));
                                let msg = match r {
                                    Ok(Some(path)) => json!({"type":"file_saved","id":id,"path":path}),
                                    Ok(None) => json!({"type":"file_ack","id":id,"offset":offset}),
                                    Err(e) => json!({"type":"file_saved","id":id,"error":e}),
                                };
                                out_tx.send(to_parent(pid, msg)).await?;
                            }
                            ParentMsg::DenyParent { id } if authed => {
                                roster.waiting.lock().unwrap().remove(&id);
                                for (p2, st) in &parents { if st.id == id && !st.authed { out_tx.send(to_parent(*p2, json!({"type":"auth_failed","reason":"denied"}))).await?; } }
                                if s.waiting_id.as_deref() == Some(&id) { s.waiting_id = None; out_tx.send(text(json!({"type":"auth_failed","reason":"denied"}))).await?; }
                                broadcast_roster(&out_tx, &parents, shared, roster).await?;
                            }
                            ParentMsg::RemoveParent { id } if authed => {
                                if id == me.id || roster.list().len() <= 1 { continue; } // can't remove yourself or the last parent
                                roster.remove(&id);
                                println!("parent removed");
                                for (p2, st) in parents.iter_mut() {
                                    if st.id == id { st.authed = false; out_tx.send(to_parent(*p2, json!({"type":"auth_failed","reason":"removed"}))).await?; }
                                }
                                if s.viewer_id == id {
                                    s.authed = false; inp.release_all(); shared.set_override(false);
                                    out_tx.send(text(json!({"type":"auth_failed","reason":"removed"}))).await?;
                                }
                                broadcast_roster(&out_tx, &parents, shared, roster).await?;
                            }
                            _ => {}
                        }
                    }
                    // Everything below only works after the pairing code was accepted.
                    _ if !s.authed => {}
                    ControlMsg::SelectMonitor { index } => s.monitor = index,
                    ControlMsg::SetQuality { fps, quality } => {
                        s.fps = fps.clamp(1, 30);
                        s.quality = quality.clamp(20, 95);
                        tick = tokio::time::interval(Duration::from_millis(1000 / s.fps as u64));
                    }
                    ControlMsg::MouseMove { x, y } => {
                        if let Some(r) = capture::monitor_rect(s.monitor) {
                            let px = r.0 + (x.clamp(0.0, 1.0) * (r.2 as f64 - 1.0)) as i32;
                            let py = r.1 + (y.clamp(0.0, 1.0) * (r.3 as f64 - 1.0)) as i32;
                            inp.mouse_move(px, py);
                        }
                    }
                    ControlMsg::MouseButton { button, down } => inp.mouse_button(&button, down),
                    ControlMsg::Wheel { dx, dy } => inp.wheel(dx.clamp(-20, 20), dy.clamp(-20, 20)),
                    ControlMsg::Key { key, down } => inp.key(&key, down),
                    ControlMsg::Control { on } => shared.set_override(on),
                    ControlMsg::SetPolicy { policy } => { shared.set_policy(policy); }
                    ControlMsg::Clipboard { text } => {
                        if text.len() <= 1_000_000 && clipboard::set(&text) { s.last_clip = text; }
                    }
                }
            }
            _ = tick.tick(), if s.authed => {
                // Skip a frame if the network is still busy with the last one.
                if out_tx.capacity() == 0 { continue; }
                let (idx, q) = (s.monitor, s.quality);
                if let Ok(Some(jpeg)) = tokio::task::spawn_blocking(move || capture::capture_jpeg(idx, q)).await {
                    let mut frame = Vec::with_capacity(jpeg.len() + 1);
                    frame.push(idx as u8); // 1-byte header: monitor index
                    frame.extend_from_slice(&jpeg);
                    let _ = out_tx.try_send(Message::Binary(frame));
                }
            }
            ev = events.recv() => {
                let msg = match ev {
                    Ok(policy::Event::NewRequest(req)) => json!({"type":"request","request":req}),
                    Ok(policy::Event::Resolved { app, decision }) => json!({"type":"resolved","app":app,"decision":decision}),
                    Ok(policy::Event::PolicyChanged) | Err(_) => {
                        let pol = shared.policy.read().unwrap().clone();
                        if s.authed { out_tx.send(text(json!({"type":"policy","policy":pol}))).await?; }
                        // Lagged? resend everything to parents
                        broadcast_roster(&out_tx, &parents, shared, roster).await?;
                        continue;
                    }
                };
                for (pid, p) in &parents { if p.authed && roster.is_parent(&p.id) { out_tx.send(to_parent(*pid, msg.clone())).await?; } }
            }
            _ = clip_tick.tick(), if s.authed => {
                if let Some(c) = clipboard::get() {
                    if c != s.last_clip && c.len() <= 1_000_000 {
                        s.last_clip = c.clone();
                        out_tx.send(text(json!({"type":"clipboard","text":c}))).await?;
                    }
                }
            }
        }
    }
    inp.release_all();
    shared.set_override(false); // connection lost: restrictions back on
    writer.abort();
    Ok(())
}
