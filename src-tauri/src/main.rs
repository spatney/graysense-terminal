#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod canvas;
mod commands;
mod engine;
mod memory;
mod missions;
mod search;
mod sessions;
mod settings;
mod sysmon;
mod terminal;
mod update;
mod util;
mod win;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tauri::Manager;

use commands::CoreState;

fn main() {
    // Installer hooks (install, update, uninstall) run and exit here, and an update that was
    // downloaded but not installed yet is applied before the app starts.
    velopack::VelopackApp::build().run();
    init_tracing();

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("graysense-rt")
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime"),
    );
    let cwd = std::env::current_dir().ok();
    let startup = engine::Startup::parse(std::env::args().skip(1), cwd.as_deref());
    let terminals = terminal::Terminals::default();

    let builder = tauri::Builder::default();
    // One Graysense at a time: a second launch hands its arguments to the running one. Release
    // builds only, so a development build runs beside the installed app.
    #[cfg(all(windows, not(debug_assertions)))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
        let startup = engine::Startup::parse(args.into_iter().skip(1), Some(std::path::Path::new(&cwd)));
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        if let Some(state) = app.try_state::<CoreState>() {
            let core = state.0.clone();
            std::thread::spawn(move || {
                let mut c = core.lock();
                c.open_startup(startup);
                drop(c.send_out());
            });
        }
    }));

    let page_terminals = terminals.clone();
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(terminals.clone())
        // Terminals live on the page: a page that loads again (a reload) starts its shells anew.
        .on_page_load(move |webview, payload| {
            if webview.label() == "main" && matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
                let terminals = page_terminals.clone();
                std::thread::spawn(move || terminals.kill_all());
            }
        })
        .setup(move |app| {
            let courier = engine::Courier::start(app.handle().clone());
            let (core, rx) = engine::Core::new(runtime, startup, terminals, courier);
            let focused = core.window_focused.clone();
            let core = Arc::new(Mutex::new(core));
            app.manage(CoreState(core.clone()));
            engine::run_loop(core, rx);
            open_main_window(app.handle(), focused)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::act,
            commands::search,
            commands::app_view,
            commands::list_dir,
            commands::complete_path,
            commands::save_pasted_image,
            commands::read_text,
            commands::terminal_shells,
            commands::terminal_spawn,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_pause,
            commands::terminal_kill,
            commands::window_reveal,
            commands::window_theme,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Graysense")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(state) = app.try_state::<CoreState>()
            {
                state.0.lock().on_exit();
            }
        });
}

/// Graysense's window (`app > windows` in tauri.conf.json, which doesn't create it). It starts
/// hidden; the UI shows it once its first screen is painted, or after a few seconds regardless.
fn open_main_window(app: &tauri::AppHandle, focused: Arc<AtomicBool>) -> tauri::Result<()> {
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main").cloned() else {
        return Err(tauri::Error::WindowNotFound);
    };
    // Clipboard access lets the terminal paste without a prompt. The window only shows Graysense's page.
    let builder = tauri::WebviewWindowBuilder::from_config(app, &config)?.enable_clipboard_access();
    // Development builds can expose the page to DevTools automation (never release builds).
    #[cfg(debug_assertions)]
    let builder = match std::env::var("GRAYSENSE_DEVTOOLS_PORT") {
        Ok(port) if port.parse::<u16>().is_ok() => builder.additional_browser_args(&format!("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={port}")),
        _ => builder,
    };
    let window = builder.build()?;
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(f) = event {
            focused.store(*f, Ordering::Relaxed);
        }
    });
    let shown = window.as_ref().window();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(4));
        commands::reveal(&shown);
    });
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("GRAYSENSE_LOG").unwrap_or_else(|_| EnvFilter::new("info,tao=warn,wry=warn"));
    let log_path = settings::config_dir().join("graysense.log");
    match std::fs::File::create(&log_path) {
        Ok(file) => {
            let _ = fmt().with_env_filter(filter).with_ansi(false).with_writer(std::sync::Mutex::new(file)).try_init();
        }
        Err(_) => {
            let _ = fmt().with_env_filter(filter).try_init();
        }
    }
}
