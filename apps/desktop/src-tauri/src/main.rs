//! OpenDictate desktop shell (M3).
//!
//! Hosts the session controller, registers global hotkeys, and projects
//! pipeline events onto the tray icon and the overlay pill. Rust owns all
//! state (ADR-11); the webview is a passive projection fed by `app-event`
//! emissions plus a polled `get_level` command.
//!
//! Default hotkeys (configurable in M7):
//! - `Ctrl+Shift+Space` — toggle dictation
//! - `Ctrl+Shift+D` — push-to-talk (hold)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::Instant;

use od_audio::CaptureConfig;
use od_core_types::{AppEvent, LanguageHint, PipelineCtx, SessionState};
use od_pipeline::{SessionCommand, SessionHandle, TranscriberConfig};
use od_stt::{WhisperConfig, WhisperEngine};
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

struct AppState {
    session: SessionHandle,
}

/// Input level for the overlay meter (polled ~15 Hz while listening).
#[tauri::command]
fn get_level(state: tauri::State<'_, AppState>) -> f32 {
    state.session.level()
}

fn main() {
    let t0 = Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Fail fast with a clear message when the STT model hasn't been fetched:
    // a dictation app without its model is not degraded, it is inoperative.
    let model_path = od_stt::default_model_path();
    if !model_path.is_file() {
        eprintln!(
            "STT model missing at {}.\nRun scripts/fetch-models.ps1 first.",
            model_path.display()
        );
        std::process::exit(2);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            // Pipeline first: the hotkey must be live even if the webview is
            // still warming up (cold-start contract, docs/02).
            let session = od_pipeline::spawn(
                CaptureConfig::default(),
                TranscriberConfig::default(),
                PipelineCtx {
                    language: LanguageHint::Fixed("en".into()),
                    ..PipelineCtx::default()
                },
                || WhisperEngine::new(WhisperConfig::default()),
                || match od_insertion::WindowsInserter::new() {
                    Ok(inserter) => Some(inserter),
                    Err(e) => {
                        tracing::error!("inserter init failed ({e}); display-only mode");
                        None
                    }
                },
            );
            let events = session.subscribe();
            app.manage(AppState { session });

            register_hotkeys(app.handle())?;
            build_tray(app.handle())?;
            spawn_event_bridge(app.handle().clone(), events);
            position_overlay(app.handle());

            tracing::info!(
                cold_start_ms = t0.elapsed().as_millis() as u64,
                "hotkeys live; app ready"
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_level])
        .build(tauri::generate_context!())
        .expect("failed to build tauri app")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Best-effort orderly stop so the mic is closed by us, not
                // by process teardown.
                if let Some(state) = app.try_state::<AppState>() {
                    state.session.send(SessionCommand::Shutdown);
                }
            }
        });
}

fn register_hotkeys(app: &tauri::AppHandle) -> tauri::Result<()> {
    let toggle = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
    let ptt = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyD);

    let gs = app.global_shortcut();
    gs.on_shortcut(toggle, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            let state = app.state::<AppState>();
            state.session.send(SessionCommand::Toggle);
        }
    })
    .map_err(|e| tauri::Error::Anyhow(e.into()))?;

    gs.on_shortcut(ptt, |app, _shortcut, event| {
        let state = app.state::<AppState>();
        state.session.send(match event.state {
            ShortcutState::Pressed => SessionCommand::PttPressed,
            ShortcutState::Released => SessionCommand::PttReleased,
        });
    })
    .map_err(|e| tauri::Error::Anyhow(e.into()))?;

    tracing::info!("global shortcuts registered (Ctrl+Shift+Space toggle, Ctrl+Shift+D ptt)");
    Ok(())
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let quit = MenuItem::with_id(app, "quit", "Quit OpenDictate", true, None::<&str>)?;
    let menu = MenuBuilder::new(app).item(&quit).build()?;

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .expect("bundle icon configured")
                .clone(),
        )
        .tooltip("OpenDictate — idle")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            if event.id.as_ref() == "quit" {
                app.exit(0);
            }
        })
        .build(app)?;
    Ok(())
}

/// Forwards bus events to the webview and drives tray/overlay visibility.
/// The mic-truth rule (docs/06 T1): tray tooltip and overlay visibility are
/// derived from the same `StateChanged` events as everything else.
fn spawn_event_bridge(app: tauri::AppHandle, events: std::sync::mpsc::Receiver<AppEvent>) {
    std::thread::Builder::new()
        .name("od-event-bridge".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                if let AppEvent::StateChanged { state } = &event {
                    apply_state(&app, *state);
                }
                if let Err(e) = app.emit("app-event", &event) {
                    tracing::debug!("emit failed (shutdown?): {e}");
                }
            }
            tracing::info!("event bridge exiting");
        })
        .expect("spawn event bridge");
}

fn apply_state(app: &tauri::AppHandle, state: SessionState) {
    if let Some(tray) = app.tray_by_id("main") {
        let tip = match state {
            SessionState::Idle => "OpenDictate — idle",
            SessionState::Listening => "OpenDictate — LISTENING",
            SessionState::Finalizing => "OpenDictate — finishing…",
        };
        let _ = tray.set_tooltip(Some(tip));
    }
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = match state {
            SessionState::Listening => overlay.show(),
            // Keep the pill up through Finalizing so the user sees the tail
            // text land; hide on Idle.
            SessionState::Finalizing => Ok(()),
            SessionState::Idle => overlay.hide(),
        };
    }
}

/// Bottom-center of the primary monitor, above the taskbar.
fn position_overlay(app: &tauri::AppHandle) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let (Ok(Some(monitor)), Ok(size)) = (overlay.primary_monitor(), overlay.outer_size()) else {
        return;
    };
    let mon = monitor.size();
    let x = (mon.width.saturating_sub(size.width)) / 2;
    let y = mon.height.saturating_sub(size.height + 96);
    let _ = overlay.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
}
