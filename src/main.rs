//! NotroDNS — a lightweight Windows DNS manager.
//!
//! Fetches a curated DNS server list, benchmarks it, and lets the user
//! apply, or restore, DNS settings on a chosen network adapter. See
//! `README.md` for the full feature list and architecture notes.
//!
//! This binary does not request elevation at launch: it runs, browses, and
//! benchmarks fully as a standard user, and only prompts for a UAC restart
//! right before an action that genuinely needs administrator rights
//! (setting or restoring DNS). See `src/windows/elevation.rs`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 580.0])
            .with_min_inner_size([620.0, 440.0])
            .with_title("NotroDNS"),
        ..Default::default()
    };

    eframe::run_native(
        "NotroDNS",
        native_options,
        Box::new(|cc| Ok(Box::new(notrodns::app::NotroDnsApp::new(cc)))),
    )
}
