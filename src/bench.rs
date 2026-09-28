//! Concurrent DNS server benchmarking.
//!
//! The primary metric is real DNS-query round-trip latency: a hand-built,
//! minimal `A`-record query is sent over UDP to each server's port 53 and
//! timed until a reply with a matching transaction ID arrives (or a timeout
//! elapses). This avoids pulling in a full DNS resolver crate.
//!
//! An optional secondary metric shells out to the system `ping` utility
//! (fixed argument list, IP validated first — never a shell string) for a
//! rough ICMP round-trip number, since raw ICMP sockets require elevated
//! privileges this app should not require just to benchmark.

use crate::api::DnsServer;
use crate::config::{BENCH_CONCURRENCY, BENCH_TIMEOUT_MS, DNS_QUERY_DOMAIN};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BenchStatus {
    Ok,
    Timeout,
    Error(String),
    /// The server's `preferred_ip` did not parse as an IP address.
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub server_name: String,
    pub ip: String,
    pub dns_latency_ms: Option<f64>,
    pub ping_latency_ms: Option<f64>,
    pub status: BenchStatus,
}

/// Builds a minimal, valid DNS query packet for an `A` record lookup of
/// `domain`, using `txn_id` as the transaction ID (used to match replies and
/// reject spoofed/unrelated packets).
fn build_query(domain: &str, txn_id: u16) -> Vec<u8> {
    let mut packet = Vec::with_capacity(32);
    packet.extend_from_slice(&txn_id.to_be_bytes());
    packet.extend_from_slice(&0x0100u16.to_be_bytes()); // standard query, recursion desired
    packet.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in domain.trim_end_matches('.').split('.') {
        let bytes = label.as_bytes();
        debug_assert!(bytes.len() <= 63, "DNS labels must be <= 63 bytes");
        packet.push(bytes.len() as u8);
        packet.extend_from_slice(bytes);
    }
    packet.push(0); // root label
    packet.extend_from_slice(&1u16.to_be_bytes()); // QTYPE  = A
    packet.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
    packet
}

async fn dns_query_latency(ip: IpAddr) -> (Option<f64>, BenchStatus) {
    let bind_addr = match ip {
        IpAddr::V4(_) => "0.0.0.0:0",
        IpAddr::V6(_) => "[::]:0",
    };
    let socket = match UdpSocket::bind(bind_addr).await {
        Ok(s) => s,
        Err(e) => return (None, BenchStatus::Error(e.to_string())),
    };

    let addr = SocketAddr::new(ip, 53);
    let txn_id: u16 = rand::random();
    let query = build_query(DNS_QUERY_DOMAIN, txn_id);

    let start = Instant::now();
    if let Err(e) = socket.send_to(&query, addr).await {
        return (None, BenchStatus::Error(e.to_string()));
    }

    let mut buf = [0u8; 512];
    let recv = tokio::time::timeout(
        Duration::from_millis(BENCH_TIMEOUT_MS),
        socket.recv_from(&mut buf),
    )
    .await;
    match recv {
        Ok(Ok((n, _))) if n >= 2 => {
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            let recv_txn = u16::from_be_bytes([buf[0], buf[1]]);
            if recv_txn == txn_id {
                (Some(elapsed_ms), BenchStatus::Ok)
            } else {
                (
                    None,
                    BenchStatus::Error("received a reply with a mismatched transaction ID".into()),
                )
            }
        }
        Ok(Ok(_)) => (
            None,
            BenchStatus::Error("response too short to be a DNS reply".into()),
        ),
        Ok(Err(e)) => (None, BenchStatus::Error(e.to_string())),
        Err(_) => (None, BenchStatus::Timeout),
    }
}

/// Runs `ping -n 1 -w <timeout>` against `ip` and parses the reported
/// round-trip time. `ip` is always a parsed [`IpAddr`], never raw user text,
/// so this can never inject extra arguments.
async fn ping_latency(ip: IpAddr) -> Option<f64> {
    let ip_str = ip.to_string();
    let output = tokio::process::Command::new("ping")
        .args(["-n", "1", "-w", "1000", &ip_str])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_ping_output(&text)
}

fn parse_ping_output(text: &str) -> Option<f64> {
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix("time=") {
            let digits: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = digits.parse::<f64>() {
                return Some(v);
            }
        }
        if token.starts_with("time<") {
            // Windows reports sub-millisecond round trips as "time<1ms".
            return Some(1.0);
        }
    }
    None
}

/// Benchmarks every server's `preferred_ip` concurrently, bounded to
/// [`BENCH_CONCURRENCY`] in-flight probes at a time, each capped at
/// [`BENCH_TIMEOUT_MS`]. Results are sorted by DNS latency ascending, with
/// unreachable/unresolved servers pushed to the end (ties broken by name).
pub async fn benchmark_all(servers: &[DnsServer], with_ping: bool) -> Vec<BenchResult> {
    let semaphore = Arc::new(Semaphore::new(BENCH_CONCURRENCY));
    let mut handles = Vec::with_capacity(servers.len());

    for server in servers.iter().cloned() {
        let sem = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem
                .acquire_owned()
                .await
                .expect("semaphore closed unexpectedly");
            let ip: IpAddr = match server.preferred_ip.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return BenchResult {
                        server_name: server.name,
                        ip: server.preferred_ip,
                        dns_latency_ms: None,
                        ping_latency_ms: None,
                        status: BenchStatus::Unresolved,
                    }
                }
            };
            let (dns_latency_ms, status) = dns_query_latency(ip).await;
            let ping_latency_ms = if with_ping {
                ping_latency(ip).await
            } else {
                None
            };
            BenchResult {
                server_name: server.name,
                ip: server.preferred_ip,
                dns_latency_ms,
                ping_latency_ms,
                status,
            }
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(r) = h.await {
            results.push(r);
        }
    }

    sort_results(&mut results);
    results
}

fn sort_results(results: &mut [BenchResult]) {
    results.sort_by(|a, b| match (a.dns_latency_ms, b.dns_latency_ms) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.server_name.cmp(&b.server_name),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_packet_has_expected_header_and_question() {
        let packet = build_query("example.com.", 0xABCD);
        assert_eq!(&packet[0..2], &[0xAB, 0xCD]); // transaction id
        assert_eq!(&packet[2..4], &[0x01, 0x00]); // flags: recursion desired
        assert_eq!(&packet[4..6], &[0x00, 0x01]); // QDCOUNT = 1
        assert_eq!(&packet[6..8], &[0x00, 0x00]); // ANCOUNT = 0
                                                  // "example" label
        assert_eq!(packet[12], 7);
        assert_eq!(&packet[13..20], b"example");
        // "com" label
        assert_eq!(packet[20], 3);
        assert_eq!(&packet[21..24], b"com");
        assert_eq!(packet[24], 0); // root label
        let qtype_qclass = &packet[25..29];
        assert_eq!(qtype_qclass, &[0x00, 0x01, 0x00, 0x01]); // A / IN
    }

    #[test]
    fn parses_normal_ping_time() {
        let sample = "Reply from 1.1.1.1: bytes=32 time=14ms TTL=57";
        assert_eq!(parse_ping_output(sample), Some(14.0));
    }

    #[test]
    fn parses_sub_millisecond_ping_time() {
        let sample = "Reply from 127.0.0.1: bytes=32 time<1ms TTL=128";
        assert_eq!(parse_ping_output(sample), Some(1.0));
    }

    #[test]
    fn sorts_ok_before_failed_and_failed_by_name() {
        let mut results = vec![
            BenchResult {
                server_name: "Zeta".into(),
                ip: "1.1.1.1".into(),
                dns_latency_ms: None,
                ping_latency_ms: None,
                status: BenchStatus::Timeout,
            },
            BenchResult {
                server_name: "Alpha".into(),
                ip: "8.8.8.8".into(),
                dns_latency_ms: Some(40.0),
                ping_latency_ms: None,
                status: BenchStatus::Ok,
            },
            BenchResult {
                server_name: "Beta".into(),
                ip: "9.9.9.9".into(),
                dns_latency_ms: None,
                ping_latency_ms: None,
                status: BenchStatus::Unresolved,
            },
            BenchResult {
                server_name: "Gamma".into(),
                ip: "1.0.0.1".into(),
                dns_latency_ms: Some(10.0),
                ping_latency_ms: None,
                status: BenchStatus::Ok,
            },
        ];
        sort_results(&mut results);
        let names: Vec<&str> = results.iter().map(|r| r.server_name.as_str()).collect();
        assert_eq!(names, vec!["Gamma", "Alpha", "Beta", "Zeta"]);
    }
}
