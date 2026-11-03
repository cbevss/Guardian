//! Website filter. Guardian runs a small DNS server on his PC, and Windows is
//! set to use it (see winsetup.rs). Every time a browser or app looks up a
//! website, the policy decides; allowed lookups are passed to a real DNS
//! server, everything else gets "no such site".
//!
//! The default upstream is Cloudflare for Families (1.1.1.3), which also
//! blocks adult and malware sites as an extra safety net.

use crate::policy::Shared;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::UdpSocket;

pub const DEFAULT_UPSTREAM: &str = "1.1.1.3:53";

/// Reads the first question's name out of a DNS query.
pub fn query_name(pkt: &[u8]) -> Option<String> {
    if pkt.len() < 12 || u16::from_be_bytes([pkt[4], pkt[5]]) == 0 { return None; }
    let mut i = 12;
    let mut labels = Vec::new();
    loop {
        let len = *pkt.get(i)? as usize;
        if len == 0 { break; }
        if len > 63 { return None; } // compression isn't used in questions
        let label = pkt.get(i + 1..i + 1 + len)?;
        labels.push(String::from_utf8_lossy(label).to_lowercase());
        i += 1 + len;
        if i > 300 { return None; }
    }
    Some(labels.join("."))
}

/// Turns a query into a "no such domain" answer.
pub fn nxdomain(query: &[u8]) -> Vec<u8> {
    let mut r = query.to_vec();
    if r.len() < 12 { return r; }
    r[2] = 0x80 | (r[2] & 0x01);   // response, keep "recursion desired"
    r[3] = 0x80 | 0x03;            // recursion available, NXDOMAIN
    for b in &mut r[6..12] { *b = 0; } // no answer/authority/additional records
    r
}

async fn forward(query: &[u8], upstream: SocketAddr) -> Option<Vec<u8>> {
    let sock = UdpSocket::bind(if upstream.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }).await.ok()?;
    sock.send_to(query, upstream).await.ok()?;
    let mut buf = vec![0u8; 4096];
    let (n, _) = tokio::time::timeout(Duration::from_secs(3), sock.recv_from(&mut buf)).await.ok()?.ok()?;
    buf.truncate(n);
    Some(buf)
}

/// Runs the filter forever. `listen` is normally 127.0.0.1:53.
pub async fn serve(shared: Shared, listen: String, upstream: String) {
    let Ok(upstream) = upstream.parse::<SocketAddr>() else { eprintln!("dns: bad upstream {upstream}"); return };
    let sock = loop {
        match UdpSocket::bind(&listen).await {
            Ok(s) => break Arc::new(s),
            Err(e) => { eprintln!("dns: can't listen on {listen}: {e}"); tokio::time::sleep(Duration::from_secs(10)).await; }
        }
    };
    println!("website filter listening on {listen}");
    let mut buf = vec![0u8; 4096];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
        let query = buf[..n].to_vec();
        let (shared, sock) = (shared.clone(), sock.clone());
        tokio::spawn(async move {
            let Some(name) = query_name(&query) else { return };
            let allowed = { let s = shared.clone(); let n = name.clone(); tokio::task::spawn_blocking(move || s.allow_domain(&n)).await.unwrap_or(false) };
            let reply = if allowed { forward(&query, upstream).await } else { Some(nxdomain(&query)) };
            if let Some(r) = reply { let _ = sock.send_to(&r, from).await; }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(name: &str) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for l in name.split('.') { q.push(l.len() as u8); q.extend_from_slice(l.as_bytes()); }
        q.extend_from_slice(&[0, 0, 1, 0, 1]);
        q
    }
    #[test] fn reads_name() { assert_eq!(query_name(&query("www.YouTube.com")).as_deref(), Some("www.youtube.com")); }
    #[test] fn nx_answer() {
        let r = nxdomain(&query("a.com"));
        assert_eq!(&r[..2], &[0x12, 0x34]);
        assert_eq!(r[3] & 0x0f, 3);
        assert!(r[2] & 0x80 != 0);
    }
    #[test] fn junk_ignored() { assert_eq!(query_name(&[1, 2, 3]), None); }
}
