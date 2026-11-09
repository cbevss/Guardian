// No console window when installed on the parent PC.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! Guardian relay: links one agent (child's PC) with
//!   - one controller (the live screen viewer), and
//!   - any number of parents (approval inbox, e.g. you and your mum).
//! It only forwards messages. The agent checks the pairing code itself,
//! so the relay alone can't unlock anything.

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Query, State},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, Mutex};
use tower_http::services::ServeDir;

type Tx = mpsc::UnboundedSender<Message>;

#[derive(Default)]
struct Room {
    agent: Option<Tx>,
    controller: Option<Tx>,
    parents: HashMap<u64, Tx>,
    next_pid: u64,
}

#[derive(Clone, Default)]
struct AppState {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
}

#[derive(Deserialize)]
struct Params {
    id: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Role { Agent, Controller, Parent(u64) }

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let state = AppState::default();
    // Viewer page: GUARDIAN_WEB, else a "web" folder next to the exe (installed), else relay/web (dev).
    let web_dir = std::env::var("GUARDIAN_WEB").unwrap_or_else(|_| {
        let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("web")));
        match beside { Some(d) if d.join("index.html").exists() => d.to_string_lossy().into_owned(), _ => "relay/web".into() }
    });
    let app = Router::new()
        .route("/ws/agent", get(agent_ws))
        .route("/ws/controller", get(controller_ws))
        .route("/ws/parent", get(parent_ws))
        .fallback_service(ServeDir::new(web_dir))
        .with_state(state);

    let addr = std::env::var("GUARDIAN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => { eprintln!("Can't use {addr}: {e}. Is Guardian already running?"); std::process::exit(1); }
    };
    let port = listener.local_addr().unwrap().port();
    tracing::info!("relay listening on {addr}");

    // Home mode (default): announce on the local network so the agent finds us.
    // On a cloud server run with GUARDIAN_MODE=relay to turn this off.
    let home = std::env::var("GUARDIAN_MODE").map_or(true, |m| m != "relay");
    if home {
        println!("Home mode: open http://localhost:{port} on this PC");
        tokio::spawn(announce(port));
    } else {
        println!("Relay mode: put this behind HTTPS (e.g. Caddy) before using it over the internet");
    }
    axum::serve(listener, app).await.unwrap();
}

fn valid_id(id: &str) -> bool {
    id.len() == 9 && id.chars().all(|c| c.is_ascii_digit())
}

async fn agent_ws(ws: WebSocketUpgrade, Query(p): Query<Params>, State(s): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |sock| async move { if valid_id(&p.id) { bridge(sock, p.id, s, Role::Agent).await } })
}
async fn controller_ws(ws: WebSocketUpgrade, Query(p): Query<Params>, State(s): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |sock| async move { if valid_id(&p.id) { bridge(sock, p.id, s, Role::Controller).await } })
}
async fn parent_ws(ws: WebSocketUpgrade, Query(p): Query<Params>, State(s): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |sock| async move { if valid_id(&p.id) { bridge(sock, p.id, s, Role::Parent(0)).await } })
}

fn txt(v: Value) -> Message { Message::Text(v.to_string()) }

async fn sink_busy(mut sock: WebSocket) -> Result<(), axum::Error> {
    sock.send(txt(json!({"type":"busy"}))).await?;
    sock.close().await
}

async fn bridge(sock: WebSocket, id: String, state: AppState, mut role: Role) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    {
        let mut rooms = state.rooms.lock().await;
        let room = rooms.entry(id.clone()).or_default();
        // Only one parent watches his screen at a time; the other is told who's busy instead of kicking them off.
        if role == Role::Controller && room.controller.as_ref().map_or(false, |c| !c.is_closed()) {
            let _ = sink_busy(sock).await;
            return;
        }
        match role {
            Role::Agent => {
                room.agent = Some(tx.clone());
                // Tell the new agent about anyone already waiting.
                if room.controller.is_some() { let _ = tx.send(txt(json!({"type":"controller_joined"}))); }
                for pid in room.parents.keys() { let _ = tx.send(txt(json!({"type":"parent_joined","pid":pid}))); }
            }
            Role::Controller => {
                room.controller = Some(tx.clone()); // newest viewer replaces the old one
                if let Some(a) = &room.agent { let _ = a.send(txt(json!({"type":"controller_joined"}))); }
            }
            Role::Parent(_) => {
                room.next_pid += 1;
                let pid = room.next_pid;
                role = Role::Parent(pid);
                room.parents.insert(pid, tx.clone());
                if let Some(a) = &room.agent { let _ = a.send(txt(json!({"type":"parent_joined","pid":pid}))); }
            }
        }
    }

    let (mut sink, mut stream) = sock.split();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() { break; }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        if matches!(msg, Message::Close(_)) { break; }
        let rooms = state.rooms.lock().await;
        let Some(room) = rooms.get(&id) else { continue };
        match role {
            Role::Controller => if let Some(a) = &room.agent { let _ = a.send(msg); },
            Role::Parent(pid) => {
                // Wrap so the agent knows which parent is talking.
                let Message::Text(t) = msg else { continue };
                let Ok(inner) = serde_json::from_str::<Value>(&t) else { continue };
                if let Some(a) = &room.agent { let _ = a.send(txt(json!({"type":"from_parent","pid":pid,"msg":inner}))); }
            }
            Role::Agent => match &msg {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(t).unwrap_or(Value::Null);
                    if v["type"] == "to_parent" {
                        if let (Some(pid), Some(p)) = (v["pid"].as_u64(), v.get("msg")) {
                            if let Some(ptx) = room.parents.get(&pid) { let _ = ptx.send(txt(p.clone())); }
                        }
                    } else if let Some(c) = &room.controller { let _ = c.send(msg); }
                }
                _ => if let Some(c) = &room.controller { let _ = c.send(msg); },
            },
        }
    }

    let mut rooms = state.rooms.lock().await;
    if let Some(room) = rooms.get_mut(&id) {
        match role {
            Role::Agent => if room.agent.as_ref().map_or(false, |t| t.same_channel(&tx)) {
                room.agent = None;
                // Let the parent apps know his PC went offline.
                if let Some(c) = &room.controller { let _ = c.send(txt(json!({"type":"agent_left"}))); }
                for p in room.parents.values() { let _ = p.send(txt(json!({"type":"agent_left"}))); }
            },
            Role::Controller => if room.controller.as_ref().map_or(false, |t| t.same_channel(&tx)) {
                room.controller = None;
                if let Some(a) = &room.agent { let _ = a.send(txt(json!({"type":"controller_left"}))); }
            },
            Role::Parent(pid) => {
                room.parents.remove(&pid);
                if let Some(a) = &room.agent { let _ = a.send(txt(json!({"type":"parent_left","pid":pid}))); }
            }
        }
        if room.agent.is_none() && room.controller.is_none() && room.parents.is_empty() { rooms.remove(&id); }
    }
    writer.abort();
}

/// Broadcasts "GUARDIAN-RELAY v1 <port>" every 2 seconds on UDP port 47800.
async fn announce(port: u16) {
    let Ok(sock) = tokio::net::UdpSocket::bind("0.0.0.0:0").await else { return };
    let _ = sock.set_broadcast(true);
    let msg = format!("GUARDIAN-RELAY v1 {port}");
    let target = std::env::var("GUARDIAN_BEACON_TARGET").unwrap_or_else(|_| "255.255.255.255:47800".into());
    loop {
        if let Err(e) = sock.send_to(msg.as_bytes(), &target).await {
            tracing::warn!("announce failed: {e}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}
