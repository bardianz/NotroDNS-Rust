//! The GUI layer: an [`eframe::App`] implementation plus a background
//! worker thread that owns a Tokio runtime.
//!
//! `eframe`/`egui` are synchronous and run on the main thread, so all async
//! work (HTTP fetch, DNS benchmarking) and blocking work (`netsh` calls)
//! happens on a dedicated worker thread. The GUI thread and the worker
//! communicate purely by message-passing (`std::sync::mpsc`), so the UI
//! never blocks.

use crate::api::{self, DnsServer};
use crate::bench::{self, BenchResult, BenchStatus};
use crate::cache::DnsCache;
use crate::error::AppResult;
use crate::restore_state::{AdapterSnapshot, RestoreState};
use crate::windows;
use eframe::egui;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// High-level UI state, surfaced as a colored badge in the top bar.
#[derive(Debug, Clone, PartialEq)]
pub enum AppState {
    Loading,
    Ready,
    Benchmarking,
    Applying,
    /// Server list came from the local cache because the live API was
    /// unreachable; the app remains fully usable.
    Offline,
    Error(String),
}

enum Command {
    RefreshServers,
    ListAdapters,
    Benchmark {
        servers: Vec<DnsServer>,
        with_ping: bool,
    },
    ApplyDns {
        adapter: String,
        primary: String,
        secondary: Option<String>,
        ipv6: bool,
    },
    RestoreDhcp {
        adapter: String,
    },
    RestorePrevious {
        adapter: String,
    },
}

enum Event {
    ServersLoaded {
        servers: Vec<DnsServer>,
        from_cache: bool,
        cache_age_secs: Option<u64>,
    },
    ServersError(String),
    AdaptersLoaded(Vec<windows::Adapter>),
    AdaptersError(String),
    BenchDone(Vec<BenchResult>),
    Applied {
        adapter: String,
    },
    ApplyError(String),
    Restored {
        adapter: String,
    },
    RestoreError(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingAction {
    Apply,
    RestoreDhcp,
    RestorePrevious,
}

pub struct NotroDnsApp {
    state: AppState,
    servers: Vec<DnsServer>,
    bench_results: Vec<BenchResult>,
    adapters: Vec<windows::Adapter>,
    selected_adapter: Option<String>,
    selected_server: Option<usize>,
    with_ping: bool,
    use_ipv6: bool,
    cache_age_secs: Option<u64>,
    status_message: String,
    pending_elevated_action: Option<PendingAction>,
    cmd_tx: Sender<Command>,
    evt_rx: Receiver<Event>,
}

impl NotroDnsApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (evt_tx, evt_rx) = std::sync::mpsc::channel();
        spawn_worker(cmd_rx, evt_tx);

        let _ = cmd_tx.send(Command::ListAdapters);
        let _ = cmd_tx.send(Command::RefreshServers);

        Self {
            state: AppState::Loading,
            servers: Vec::new(),
            bench_results: Vec::new(),
            adapters: Vec::new(),
            selected_adapter: None,
            selected_server: None,
            with_ping: false,
            use_ipv6: false,
            cache_age_secs: None,
            status_message: "Loading DNS server list…".to_string(),
            pending_elevated_action: None,
            cmd_tx,
            evt_rx,
        }
    }

    fn is_offline(&self) -> bool {
        matches!(self.state, AppState::Offline)
    }

    fn drain_events(&mut self) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                Event::ServersLoaded {
                    servers,
                    from_cache,
                    cache_age_secs,
                } => {
                    self.servers = servers;
                    self.cache_age_secs = cache_age_secs;
                    self.state = if from_cache {
                        AppState::Offline
                    } else {
                        AppState::Ready
                    };
                    self.status_message = if from_cache {
                        "Live server list unavailable — using cached data.".to_string()
                    } else {
                        format!("Loaded {} DNS servers.", self.servers.len())
                    };
                }
                Event::ServersError(e) => {
                    self.state = AppState::Error(e.clone());
                    self.status_message = format!("Could not load DNS servers: {e}");
                }
                Event::AdaptersLoaded(adapters) => {
                    if self.selected_adapter.is_none() {
                        self.selected_adapter = adapters
                            .iter()
                            .find(|a| a.state.eq_ignore_ascii_case("connected"))
                            .or_else(|| adapters.first())
                            .map(|a| a.name.clone());
                    }
                    self.adapters = adapters;
                }
                Event::AdaptersError(e) => {
                    log::warn!("failed to list adapters: {e}");
                    self.status_message = format!("Could not list network adapters: {e}");
                }
                Event::BenchDone(results) => {
                    self.bench_results = results;
                    self.state = if self.is_offline() {
                        AppState::Offline
                    } else {
                        AppState::Ready
                    };
                    self.status_message = "Benchmark complete.".to_string();
                }
                Event::Applied { adapter } => {
                    self.state = if self.is_offline() {
                        AppState::Offline
                    } else {
                        AppState::Ready
                    };
                    self.status_message = format!("DNS applied on \"{adapter}\".");
                }
                Event::ApplyError(e) => {
                    self.state = if self.is_offline() {
                        AppState::Offline
                    } else {
                        AppState::Ready
                    };
                    self.status_message = format!("Failed to apply DNS: {e}");
                }
                Event::Restored { adapter } => {
                    self.state = if self.is_offline() {
                        AppState::Offline
                    } else {
                        AppState::Ready
                    };
                    self.status_message = format!("DNS restored on \"{adapter}\".");
                }
                Event::RestoreError(e) => {
                    self.state = if self.is_offline() {
                        AppState::Offline
                    } else {
                        AppState::Ready
                    };
                    self.status_message = format!("Failed to restore DNS: {e}");
                }
            }
        }
    }

    fn request_apply(&mut self) {
        if !windows::is_elevated() {
            self.pending_elevated_action = Some(PendingAction::Apply);
            return;
        }
        let Some(adapter) = self.selected_adapter.clone() else {
            return;
        };
        let Some(idx) = self.selected_server else {
            return;
        };
        let Some(server) = self.servers.get(idx).cloned() else {
            return;
        };

        self.state = AppState::Applying;
        self.status_message = format!("Applying DNS from \"{}\" to \"{adapter}\"…", server.name);
        let secondary = if server.alternate_ip.trim().is_empty() {
            None
        } else {
            Some(server.alternate_ip.clone())
        };
        let _ = self.cmd_tx.send(Command::ApplyDns {
            adapter,
            primary: server.preferred_ip.clone(),
            secondary,
            ipv6: self.use_ipv6,
        });
    }

    fn request_restore_dhcp(&mut self) {
        if !windows::is_elevated() {
            self.pending_elevated_action = Some(PendingAction::RestoreDhcp);
            return;
        }
        let Some(adapter) = self.selected_adapter.clone() else {
            return;
        };
        self.state = AppState::Applying;
        self.status_message = format!("Restoring automatic DNS on \"{adapter}\"…");
        let _ = self.cmd_tx.send(Command::RestoreDhcp { adapter });
    }

    fn request_restore_previous(&mut self) {
        if !windows::is_elevated() {
            self.pending_elevated_action = Some(PendingAction::RestorePrevious);
            return;
        }
        let Some(adapter) = self.selected_adapter.clone() else {
            return;
        };
        self.state = AppState::Applying;
        self.status_message = format!("Restoring previous DNS on \"{adapter}\"…");
        let _ = self.cmd_tx.send(Command::RestorePrevious { adapter });
    }

    /// Re-issues whichever action triggered the elevation prompt. Reserved
    /// for platforms/flows where `relaunch_elevated` returns instead of the
    /// new elevated process taking over; on Windows today the successful
    /// path always exits the current process first, so this is currently
    /// unreachable in practice.
    #[allow(dead_code)]
    fn retry_pending_action(&mut self) {
        match self.pending_elevated_action.take() {
            Some(PendingAction::Apply) => self.request_apply(),
            Some(PendingAction::RestoreDhcp) => self.request_restore_dhcp(),
            Some(PendingAction::RestorePrevious) => self.request_restore_previous(),
            None => {}
        }
    }

    fn state_badge(&self, ui: &mut egui::Ui) {
        let (text, color) = match &self.state {
            AppState::Loading => ("● Loading".to_string(), egui::Color32::GRAY),
            AppState::Ready => ("● Ready".to_string(), egui::Color32::from_rgb(46, 160, 67)),
            AppState::Benchmarking => (
                "● Benchmarking".to_string(),
                egui::Color32::from_rgb(230, 150, 0),
            ),
            AppState::Applying => (
                "● Applying".to_string(),
                egui::Color32::from_rgb(230, 150, 0),
            ),
            AppState::Offline => (
                "● Offline (cached data)".to_string(),
                egui::Color32::from_rgb(212, 170, 0),
            ),
            AppState::Error(_) => ("● Error".to_string(), egui::Color32::from_rgb(220, 60, 60)),
        };
        ui.colored_label(color, text);
    }

    fn server_table(&mut self, ui: &mut egui::Ui) {
        if self.servers.is_empty() {
            ui.label("No DNS servers loaded yet.");
            return;
        }
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                egui::Grid::new("server_grid")
                    .striped(true)
                    .num_columns(6)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("");
                        ui.strong("Name");
                        ui.strong("Preferred IP");
                        ui.strong("Alternate IP");
                        ui.strong("DNS latency");
                        ui.strong("Ping");
                        ui.end_row();

                        for (idx, server) in self.servers.iter().enumerate() {
                            let selected = self.selected_server == Some(idx);
                            if ui.radio(selected, "").clicked() {
                                self.selected_server = Some(idx);
                            }
                            ui.label(&server.name);
                            ui.label(&server.preferred_ip);
                            ui.label(if server.alternate_ip.trim().is_empty() {
                                "—"
                            } else {
                                &server.alternate_ip
                            });

                            match self
                                .bench_results
                                .iter()
                                .find(|r| r.ip == server.preferred_ip)
                            {
                                Some(result) => {
                                    ui.label(format_dns_status(result));
                                    ui.label(
                                        result
                                            .ping_latency_ms
                                            .map(|p| format!("{p:.0} ms"))
                                            .unwrap_or_else(|| "—".into()),
                                    );
                                }
                                None => {
                                    ui.label("—");
                                    ui.label("—");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn elevation_modal(&mut self, ctx: &egui::Context) {
        if self.pending_elevated_action.is_none() {
            return;
        }
        let mut confirmed = false;
        let mut cancelled = false;

        egui::Window::new("Administrator privileges required")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Changing DNS settings requires administrator privileges.");
                ui.label("NotroDNS will restart itself and Windows will show a UAC prompt.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Restart as Administrator").clicked() {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });

        if confirmed {
            match windows::relaunch_elevated() {
                // The elevated instance takes over; this process exits and
                // never reaches retry_pending_action.
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    self.status_message = format!("Elevation failed: {e}");
                    self.pending_elevated_action = None;
                }
            }
        } else if cancelled {
            self.pending_elevated_action = None;
        }
    }
}

impl eframe::App for NotroDnsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        // Keep polling the worker channel even with no user input, so
        // async results (fetch, benchmark) show up promptly.
        ctx.request_repaint_after(Duration::from_millis(200));

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("NotroDNS");
                ui.separator();
                self.state_badge(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⟳ Refresh").clicked() {
                        self.state = AppState::Loading;
                        self.status_message = "Refreshing DNS server list…".into();
                        let _ = self.cmd_tx.send(Command::RefreshServers);
                        let _ = self.cmd_tx.send(Command::ListAdapters);
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_message);
                if let Some(age) = self.cache_age_secs {
                    ui.weak(format!("(cache age: {})", format_age(age)));
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Network adapter:");
                egui::ComboBox::from_id_source("adapter_combo")
                    .selected_text(
                        self.selected_adapter
                            .clone()
                            .unwrap_or_else(|| "(none detected)".to_string()),
                    )
                    .show_ui(ui, |ui| {
                        for adapter in self.adapters.clone() {
                            let label = format!("{} [{}]", adapter.name, adapter.state);
                            ui.selectable_value(
                                &mut self.selected_adapter,
                                Some(adapter.name.clone()),
                                label,
                            );
                        }
                    });
                ui.separator();
                ui.checkbox(&mut self.use_ipv6, "Apply as IPv6");
                ui.checkbox(&mut self.with_ping, "Include ping (secondary metric)");
            });

            ui.separator();

            ui.horizontal(|ui| {
                let benchmarking = matches!(self.state, AppState::Benchmarking);
                let can_bench = !benchmarking && !self.servers.is_empty();
                if ui
                    .add_enabled(can_bench, egui::Button::new("▶ Benchmark all"))
                    .clicked()
                {
                    self.state = AppState::Benchmarking;
                    self.status_message = "Benchmarking DNS servers…".into();
                    let _ = self.cmd_tx.send(Command::Benchmark {
                        servers: self.servers.clone(),
                        with_ping: self.with_ping,
                    });
                }
                if benchmarking {
                    ui.spinner();
                    ui.label("Benchmarking…");
                }
            });

            ui.separator();
            self.server_table(ui);
            ui.separator();

            ui.horizontal(|ui| {
                let applying = matches!(self.state, AppState::Applying);
                let can_apply =
                    self.selected_adapter.is_some() && self.selected_server.is_some() && !applying;
                let can_restore = self.selected_adapter.is_some() && !applying;

                if ui
                    .add_enabled(can_apply, egui::Button::new("✔ Apply selected DNS"))
                    .clicked()
                {
                    self.request_apply();
                }
                if ui
                    .add_enabled(can_restore, egui::Button::new("↺ Restore automatic (DHCP)"))
                    .clicked()
                {
                    self.request_restore_dhcp();
                }
                if ui
                    .add_enabled(can_restore, egui::Button::new("⤺ Restore previous DNS"))
                    .clicked()
                {
                    self.request_restore_previous();
                }
                if applying {
                    ui.spinner();
                }
            });
        });

        self.elevation_modal(ctx);
    }
}

fn format_dns_status(r: &BenchResult) -> String {
    match (&r.status, r.dns_latency_ms) {
        (BenchStatus::Ok, Some(ms)) => format!("{ms:.0} ms"),
        (BenchStatus::Timeout, _) => "timeout".to_string(),
        (BenchStatus::Unresolved, _) => "invalid IP".to_string(),
        (BenchStatus::Error(e), _) => format!("error: {e}"),
        _ => "—".to_string(),
    }
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

fn spawn_worker(cmd_rx: Receiver<Command>, evt_tx: Sender<Event>) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = evt_tx.send(Event::ServersError(format!(
                    "failed to start async runtime: {e}"
                )));
                return;
            }
        };
        let client = reqwest::Client::new();

        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                Command::RefreshServers => {
                    let client = client.clone();
                    let evt_tx = evt_tx.clone();
                    rt.block_on(async move { handle_refresh_servers(&client, &evt_tx).await });
                }
                Command::ListAdapters => match windows::list_adapters() {
                    Ok(adapters) => {
                        let _ = evt_tx.send(Event::AdaptersLoaded(adapters));
                    }
                    Err(e) => {
                        let _ = evt_tx.send(Event::AdaptersError(e.to_string()));
                    }
                },
                Command::Benchmark { servers, with_ping } => {
                    let evt_tx = evt_tx.clone();
                    rt.block_on(async move {
                        let results = bench::benchmark_all(&servers, with_ping).await;
                        let _ = evt_tx.send(Event::BenchDone(results));
                    });
                }
                Command::ApplyDns {
                    adapter,
                    primary,
                    secondary,
                    ipv6,
                } => {
                    let result = apply_dns(&adapter, &primary, secondary.as_deref(), ipv6);
                    let _ = evt_tx.send(match result {
                        Ok(()) => Event::Applied { adapter },
                        Err(e) => Event::ApplyError(e.to_string()),
                    });
                }
                Command::RestoreDhcp { adapter } => {
                    let result = windows::restore_dhcp_ipv4(&adapter);
                    // IPv6 restore is best-effort and shouldn't mask an IPv4 success.
                    let _ = windows::restore_dhcp_ipv6(&adapter);
                    let _ = evt_tx.send(match result {
                        Ok(()) => Event::Restored { adapter },
                        Err(e) => Event::RestoreError(e.to_string()),
                    });
                }
                Command::RestorePrevious { adapter } => {
                    let result = restore_previous(&adapter);
                    let _ = evt_tx.send(match result {
                        Ok(()) => Event::Restored { adapter },
                        Err(e) => Event::RestoreError(e.to_string()),
                    });
                }
            }
        }
    });
}

async fn handle_refresh_servers(client: &reqwest::Client, evt_tx: &Sender<Event>) {
    match api::fetch_dns_servers(client).await {
        Ok(servers) => {
            let cache = DnsCache::new(servers.clone());
            if let Err(e) = cache.save() {
                log::warn!("failed to write DNS cache: {e}");
            }
            let _ = evt_tx.send(Event::ServersLoaded {
                servers,
                from_cache: false,
                cache_age_secs: None,
            });
        }
        Err(e) => {
            log::warn!("live DNS server list fetch failed ({e}); falling back to cache");
            match DnsCache::load() {
                Some(cache) => {
                    let age = cache.age_secs();
                    let _ = evt_tx.send(Event::ServersLoaded {
                        servers: cache.servers,
                        from_cache: true,
                        cache_age_secs: Some(age),
                    });
                }
                None => {
                    let _ = evt_tx.send(Event::ServersError(e.to_string()));
                }
            }
        }
    }
}

/// Snapshots the adapter's current DNS config (if none is recorded yet)
/// before applying a new one, then sets the requested static DNS servers.
fn apply_dns(adapter: &str, primary: &str, secondary: Option<&str>, ipv6: bool) -> AppResult<()> {
    let mut state = RestoreState::load();
    if !state.has(adapter) {
        if let Ok(current) = windows::get_current_dns(adapter) {
            state.record_if_absent(
                adapter,
                AdapterSnapshot {
                    dhcp: current.dhcp,
                    ipv4_servers: current.ipv4_servers,
                },
            );
            if let Err(e) = state.save() {
                log::warn!("failed to persist pre-change DNS snapshot for {adapter}: {e}");
            }
        }
    }

    if ipv6 {
        windows::set_ipv6_dns(adapter, primary, secondary)
    } else {
        windows::set_ipv4_dns(adapter, primary, secondary)
    }
}

/// Restores whatever was recorded for `adapter` before NotroDNS first
/// touched it; falls back to plain DHCP if nothing was recorded.
fn restore_previous(adapter: &str) -> AppResult<()> {
    let mut state = RestoreState::load();
    match state.take(adapter) {
        Some(snapshot) => {
            let result = if snapshot.dhcp {
                windows::restore_dhcp_ipv4(adapter)
            } else if let Some(primary) = snapshot.ipv4_servers.first() {
                windows::set_ipv4_dns(
                    adapter,
                    primary,
                    snapshot.ipv4_servers.get(1).map(String::as_str),
                )
            } else {
                windows::restore_dhcp_ipv4(adapter)
            };
            if let Err(e) = state.save() {
                log::warn!("failed to persist DNS restore state for {adapter}: {e}");
            }
            result
        }
        None => windows::restore_dhcp_ipv4(adapter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_age_buckets() {
        assert_eq!(format_age(5), "5s");
        assert_eq!(format_age(125), "2m");
        assert_eq!(format_age(7500), "2h");
    }

    #[test]
    fn formats_dns_status() {
        let ok = BenchResult {
            server_name: "A".into(),
            ip: "1.1.1.1".into(),
            dns_latency_ms: Some(12.3),
            ping_latency_ms: None,
            status: BenchStatus::Ok,
        };
        assert_eq!(format_dns_status(&ok), "12 ms");

        let timeout = BenchResult {
            server_name: "B".into(),
            ip: "9.9.9.9".into(),
            dns_latency_ms: None,
            ping_latency_ms: None,
            status: BenchStatus::Timeout,
        };
        assert_eq!(format_dns_status(&timeout), "timeout");
    }
}
