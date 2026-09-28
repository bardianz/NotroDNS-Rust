//! Integration test: simulates the "API is unreachable, fall back to
//! cache" and "apply then restore" flows purely through the data layer
//! (no network access, no real `netsh`/Windows calls — those are exercised
//! manually per the checklist in README.md, and unit-tested at the parsing
//! level inside `src/windows/*`).

use notrodns::api::{sanitize_servers, DnsServer};
use notrodns::cache::DnsCache;
use notrodns::restore_state::{AdapterSnapshot, RestoreState};

#[test]
fn api_response_is_sanitized_then_cached_then_reloadable() {
    // Simulate a raw API payload containing one bad entry.
    let raw = vec![
        DnsServer {
            name: "Cloudflare".into(),
            preferred_ip: "1.1.1.1".into(),
            alternate_ip: "1.0.0.1".into(),
        },
        DnsServer {
            name: "Broken".into(),
            preferred_ip: "not-an-ip".into(),
            alternate_ip: "".into(),
        },
        DnsServer {
            name: "Quad9".into(),
            preferred_ip: "9.9.9.9".into(),
            alternate_ip: "149.112.112.112".into(),
        },
    ];
    let clean = sanitize_servers(raw);
    assert_eq!(clean.len(), 2);

    let dir = tempfile::tempdir().unwrap();
    let cache_path = dir.path().join("cache.json");

    let cache = DnsCache::new(clean.clone());
    cache.save_to(&cache_path).unwrap();

    // Simulate an app restart with the API unreachable: only the cache is available.
    let reloaded = DnsCache::load_from(&cache_path).expect("cache must survive a reload");
    assert_eq!(reloaded.servers, clean);
    assert!(
        reloaded.age_secs() < 5,
        "a freshly written cache should be a few seconds old at most"
    );
}

#[test]
fn apply_then_restore_previous_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.json");

    // Step 1: before ever touching "Ethernet", NotroDNS snapshots whatever
    // was there (here: DHCP).
    let mut state = RestoreState::default();
    state.record_if_absent(
        "Ethernet",
        AdapterSnapshot {
            dhcp: true,
            ipv4_servers: vec![],
        },
    );
    state.save_to(&state_path).unwrap();

    // Step 2: user applies a DNS server, then applies a *different* one.
    // Neither apply should ever overwrite the original DHCP snapshot.
    let mut state = RestoreState::load_from(&state_path);
    state.record_if_absent(
        "Ethernet",
        AdapterSnapshot {
            dhcp: false,
            ipv4_servers: vec!["1.1.1.1".into()],
        },
    );
    state.save_to(&state_path).unwrap();

    let mut state = RestoreState::load_from(&state_path);
    state.record_if_absent(
        "Ethernet",
        AdapterSnapshot {
            dhcp: false,
            ipv4_servers: vec!["8.8.8.8".into()],
        },
    );
    state.save_to(&state_path).unwrap();

    // Step 3: "Restore previous DNS" must return the *original* config.
    let mut state = RestoreState::load_from(&state_path);
    let snapshot = state
        .take("Ethernet")
        .expect("a snapshot should have been recorded");
    assert!(
        snapshot.dhcp,
        "restoring previous DNS should recover the original DHCP setting"
    );
    assert!(snapshot.ipv4_servers.is_empty());

    // Step 4: once restored, the snapshot is consumed — a second restore
    // has nothing adapter-specific left to undo.
    state.save_to(&state_path).unwrap();
    let state = RestoreState::load_from(&state_path);
    assert!(!state.has("Ethernet"));
}
