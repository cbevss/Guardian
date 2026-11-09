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
}

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
    Auth { code: String },
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
    Auth { code: String },
    /// decision is "Temporary", "Approve" or "Deny"
    Decide { app: String, decision: policy::Decision },
    SetPolicy { policy: policy::Policy },
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
    let save_cfg = cfg.clone();
    // Guardian's own relay server must always be reachable.
    let mut always = vec![];
    if let Some(h) = cfg.relay.split("://").nth(1).and_then(|r| r.split(['/', ':']).next()) {
        always.push(policy::site_of(h)); always.push(h.to_string());
    }
    let shared = policy::Shared::new(cfg.policy.clone(), always, move |p| {
        let mut c = save_cfg.clone(); c.policy = p.clone();
        let _ = std::fs::write(config_path(), serde_json::to_string_pretty(&c).unwrap());
    });
    tokio::spawn(policy::enforcer(shared.clone()));
    tokio::spawn(winsetup::schedule_enforcer(shared.clone()));

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
        if let Err(e) = run(&cfg, &url, &shared).await { eprintln!("connection lost: {e}"); }
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

fn parent_welcome(shared: &policy::Shared) -> serde_json::Value {
    let pending: Vec<_> = shared.pending.lock().unwrap().values().cloned().collect();
    let pol = shared.policy.read().unwrap().clone();
    json!({"type":"parent_hello","pending":pending,"policy":pol})
}

async fn run(cfg: &Config, relay: &str, shared: &policy::Shared) -> Result<(), Box<dyn std::error::Error>> {
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

    let mut s = Session { authed: false, failures: 0, locked_until: None, monitor: 0, fps: 8, quality: 60, last_clip: String::new() };
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / s.fps as u64));
    let mut clip_tick = tokio::time::interval(Duration::from_millis(1000));
    let mut inp = input::Input::new();
    let mut parents: std::collections::HashMap<u64, bool> = Default::default(); // pid -> authed
    let mut events = shared.events.subscribe();

    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { break };
                let Message::Text(t) = msg? else { continue };
                let Ok(cm) = serde_json::from_str::<ControlMsg>(&t) else { continue };
                match cm {
                    ControlMsg::ControllerJoined => {
                        s.authed = false; shared.set_override(false);
                        out_tx.send(text(json!({"type":"auth_required"}))).await?;
                    }
                    ControlMsg::ControllerLeft => { s.authed = false; inp.release_all(); shared.set_override(false); println!("controller left"); }
                    ControlMsg::Auth { code } => match check_code(&mut s, cfg, &code) {
                        Ok(()) => {
                            s.authed = true;
                            s.last_clip = clipboard::get().unwrap_or_default();
                            println!("controller authenticated");
                            let pol = shared.policy.read().unwrap().clone();
                            out_tx.send(text(json!({"type":"hello","monitors":capture::list_monitors(),"policy":pol}))).await?;
                        }
                        Err(reason) => out_tx.send(text(json!({"type":"auth_failed","reason":reason}))).await?,
                    },
                    ControlMsg::ParentJoined { pid } => {
                        parents.insert(pid, false);
                        out_tx.send(to_parent(pid, json!({"type":"auth_required"}))).await?;
                    }
                    ControlMsg::ParentLeft { pid } => { parents.remove(&pid); }
                    ControlMsg::FromParent { pid, msg } => {
                        let Ok(pm) = serde_json::from_value::<ParentMsg>(msg) else { continue };
                        let authed = parents.get(&pid).copied().unwrap_or(false);
                        match pm {
                            ParentMsg::Auth { code } => match check_code(&mut s, cfg, &code) {
                                Ok(()) => {
                                    parents.insert(pid, true);
                                    println!("parent {pid} authenticated");
                                    out_tx.send(to_parent(pid, parent_welcome(shared))).await?;
                                }
                                Err(reason) => out_tx.send(to_parent(pid, json!({"type":"auth_failed","reason":reason}))).await?,
                            },
                            ParentMsg::Decide { app, decision } if authed => { shared.decide(&app, decision); }
                            ParentMsg::SetPolicy { policy } if authed => { shared.set_policy(policy); }
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
                        let w = parent_welcome(shared);
                        for (pid, ok) in &parents { if *ok { out_tx.send(to_parent(*pid, w.clone())).await?; } }
                        continue;
                    }
                };
                for (pid, ok) in &parents { if *ok { out_tx.send(to_parent(*pid, msg.clone())).await?; } }
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
