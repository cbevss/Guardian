//! Home mode: the relay on your PC announces itself on the local network
//! (UDP broadcast on port 47800) and the agent picks it up automatically.
use tokio::net::UdpSocket;

pub const PORT: u16 = 47800;
pub const MAGIC: &str = "GUARDIAN-RELAY v1";

/// Waits until a relay announces itself, then returns its WebSocket URL.
pub async fn find_home_relay() -> String {
    loop {
        match UdpSocket::bind(("0.0.0.0", PORT)).await {
            Ok(sock) => {
                let mut buf = [0u8; 128];
                loop {
                    let Ok((n, from)) = sock.recv_from(&mut buf).await else { break };
                    let msg = String::from_utf8_lossy(&buf[..n]);
                    if let Some(port) = msg.strip_prefix(MAGIC).map(str::trim).and_then(|p| p.parse::<u16>().ok()) {
                        let url = format!("ws://{}:{}", from.ip(), port);
                        println!("found Guardian at {url}");
                        return url;
                    }
                }
            }
            Err(e) => eprintln!("discovery: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}
