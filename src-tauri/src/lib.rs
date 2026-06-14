pub mod api;
#[cfg(feature = "desktop")]
pub mod commands;
pub mod db;
pub mod drift;
pub mod event;
pub mod parser;
pub mod scoring;
pub mod season;
#[cfg(feature = "server")]
pub mod server;
pub mod session;
pub mod settings;
pub mod udp;

use drift::DriftRunManager;
use session::SessionManager;
use std::sync::Mutex;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub session_manager: Mutex<SessionManager>,
    pub drift_manager: Mutex<DriftRunManager>,
    pub settings: Mutex<settings::Settings>,
}

/// Shared owner so the UDP writer and the request handlers share one DB mutex.
pub type Shared = std::sync::Arc<AppState>;

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use std::sync::Arc;
    use tauri::Emitter;
    use tauri_plugin_global_shortcut::{
        Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
    };

    let loaded_settings = settings::load();
    let port = loaded_settings.port;
    let auto_record = loaded_settings.auto_record;

    let state: Shared = Arc::new(AppState {
        db: Mutex::new(db::open().expect("failed to open database")),
        session_manager: Mutex::new(SessionManager::new(auto_record)),
        drift_manager: Mutex::new(DriftRunManager::new()),
        settings: Mutex::new(loaded_settings),
    });

    // The initial receiver is dropped; the forwarder task below calls tx.subscribe()
    // before the ingest loop starts sending, so no ticks are missed.
    let (tx, _rx) = tokio::sync::broadcast::channel::<event::ServerEvent>(256);
    let capture_active_shortcut =
        Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyZ);
    let capture_left_shortcut =
        Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyL);
    let capture_right_shortcut =
        Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyR);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler({
                    let capture_active_shortcut = capture_active_shortcut.clone();
                    let capture_left_shortcut = capture_left_shortcut.clone();
                    let capture_right_shortcut = capture_right_shortcut.clone();
                    move |app, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        let side = if shortcut == &capture_left_shortcut {
                            Some("left")
                        } else if shortcut == &capture_right_shortcut {
                            Some("right")
                        } else if shortcut == &capture_active_shortcut {
                            None
                        } else {
                            return;
                        };
                        let _ = app.emit(
                            "drift_zone_capture",
                            serde_json::json!({
                                "side": side,
                            }),
                        );
                    }
                })
                .build(),
        )
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_sessions,
            commands::get_session_packets,
            commands::get_session_laps,
            commands::delete_session,
            commands::clear_all_sessions,
            commands::rename_session,
            commands::set_session_bookmark,
            commands::get_drift_runs,
            commands::get_drift_run_packets,
            commands::recompute_drift_scores,
            commands::set_drift_run_manual_score,
            commands::delete_drift_run,
            commands::delete_invalid_drift_runs,
            commands::get_drift_run_status,
            commands::get_drift_zones,
            commands::save_drift_zone,
            commands::delete_drift_zone,
            commands::get_settings,
            commands::save_settings,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            // Forward broadcast events to the webview via the original event names.
            let mut rx = tx.subscribe();
            tauri::async_runtime::spawn(async move {
                while let Ok(ev) = rx.recv().await {
                    match ev {
                        event::ServerEvent::Tick(pkt) => {
                            let _ = handle.emit("telemetry_tick", &pkt);
                        }
                        event::ServerEvent::BindFailed(msg) => {
                            let _ = handle.emit("udp_bind_failed", msg);
                        }
                        event::ServerEvent::SessionError(msg) => {
                            let _ = handle.emit("session_error", msg);
                        }
                        event::ServerEvent::DriftRunStatus(status) => {
                            let _ = handle.emit("drift_run_status", &status);
                        }
                    }
                }
            });

            // Ingest loop.
            let udp_state = state.clone();
            let udp_tx = tx.clone();
            tauri::async_runtime::spawn(async move {
                udp::run(udp_state, port, udp_tx).await;
            });
            if let Err(e) = app
                .global_shortcut()
                .register(capture_active_shortcut.clone())
            {
                eprintln!("[shortcut] failed to register Ctrl+Alt+Z: {e}");
            }
            if let Err(e) = app
                .global_shortcut()
                .register(capture_left_shortcut.clone())
            {
                eprintln!("[shortcut] failed to register Ctrl+Alt+L: {e}");
            }
            if let Err(e) = app
                .global_shortcut()
                .register(capture_right_shortcut.clone())
            {
                eprintln!("[shortcut] failed to register Ctrl+Alt+R: {e}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
