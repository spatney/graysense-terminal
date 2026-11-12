#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod app;
mod canvas;
mod control;
mod kit;
mod layout;
mod mcp_apps;
mod memory;
mod missions;
mod palette;
mod search;
mod search_ui;
mod sessions;
mod settings;
mod sysmon;
mod term;
mod theme;
mod win;

use std::sync::Arc;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let startup = app::Startup::from_args();
    init_tracing();

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("graysense-rt")
            .enable_all()
            .build()
            .expect("failed to start tokio runtime"),
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Graysense")
            .with_app_id("graysense")
            .with_inner_size([1640.0, 980.0])
            .with_min_inner_size([980.0, 620.0])
            .with_decorations(false)
            .with_icon(Arc::new(kit::app_icon())),
        ..Default::default()
    };

    eframe::run_native("Graysense",
        options,
        Box::new(move |cc| Ok(Box::new(app::GraysenseApp::new(cc, runtime, startup)))),
    )
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("GRAYSENSE_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,naga=warn,wgpu_core=warn,wgpu_hal=error"));
    let log_path = settings::config_dir().join("graysense.log");
    match std::fs::File::create(&log_path) {
        Ok(file) => {
            let _ = fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
        }
        Err(_) => {
            let _ = fmt().with_env_filter(filter).try_init();
        }
    }
}
