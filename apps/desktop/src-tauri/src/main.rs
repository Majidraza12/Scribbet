//! OpenDictate desktop shell (M3, extended in M7).
//!
//! Hosts the session controller, registers global hotkeys, and projects
//! pipeline events onto the tray icon and the overlay pill. Rust owns all
//! state (ADR-11); the webview is a passive projection fed by `app-event`
//! emissions plus a polled `get_level` command. M7 adds the settings window
//! (profiles, dictionary, hotkeys, mic, history, latency HUD) backed by the
//! commands below — the webview still never touches fs/network directly
//! (docs/06 TB2).
//!
//! Default hotkeys (configurable in Settings):
//! - `Ctrl+Shift+Space` — toggle dictation
//! - `Ctrl+Shift+D` — push-to-talk (hold)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use od_audio::{CaptureConfig, DeviceSelector};
use od_core_types::{AppEvent, PipelineCtx, SessionState};
use od_pipeline::{SessionCommand, SessionHandle, TranscriberConfig};
use od_storage::{
    DictionaryRepo, HistoryRepo, ProfileStore, Settings, SqliteDictionaryRepo, SqliteHistoryRepo,
    load_settings, resolve_profile, save_settings,
};
use od_stt::{WhisperConfig, WhisperEngine};
use serde::{Deserialize, Serialize};
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Rolling pipeline metrics for the settings-window latency HUD. Updated by
/// the event bridge; read by `get_perf`.
#[derive(Clone, Debug, Default, Serialize)]
struct Perf {
    cold_start_ms: u64,
    /// Most recent speech-end → final latency (P1-1 headline metric).
    finalize_ms_last: Option<u64>,
    finalize_ms_best: Option<u64>,
    insert_ms_last: Option<u64>,
    insert_tier_last: Option<String>,
    utterances: u64,
    inserts: u64,
    insert_failures: u64,
}

struct AppState {
    session: SessionHandle,
    config_dir: PathBuf,
    settings: Mutex<Settings>,
    dict: Mutex<Option<SqliteDictionaryRepo>>,
    history: Mutex<Option<SqliteHistoryRepo>>,
    perf: Mutex<Perf>,
    /// Display name of the active profile (history rows tag it).
    profile_name: Mutex<String>,
}

/// Input level for the overlay meter (polled ~15 Hz while listening).
#[tauri::command]
fn get_level(state: tauri::State<'_, AppState>) -> f32 {
    state.session.level()
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Settings {
    state.settings.lock().expect("settings lock").clone()
}

/// Persists new settings and applies every live-appliable change: hotkeys
/// re-register, device swaps at next session, profile re-resolves into a new
/// pipeline context.
#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    new: Settings,
) -> Result<(), String> {
    let old = state.settings.lock().expect("settings lock").clone();

    if new.hotkey_toggle != old.hotkey_toggle || new.hotkey_ptt != old.hotkey_ptt {
        register_hotkeys(&app, &new.hotkey_toggle, &new.hotkey_ptt)?;
    }
    if new.input_device != old.input_device {
        let dev = match &new.input_device {
            Some(name) => DeviceSelector::ByName(name.clone()),
            None => DeviceSelector::SystemDefault,
        };
        state.session.send(SessionCommand::SetDevice(dev));
    }
    if new.active_profile != old.active_profile {
        let ctx = build_ctx(&state.config_dir, &new)?;
        *state.profile_name.lock().expect("profile lock") = ctx.profile.name.clone();
        state.session.send(SessionCommand::UpdateCtx(ctx));
    }

    save_settings(&state.config_dir.join("settings.json"), &new).map_err(|e| e.to_string())?;
    *state.settings.lock().expect("settings lock") = new;
    Ok(())
}

#[derive(Serialize)]
struct ProfileInfo {
    id: String,
    name: String,
}

#[tauri::command]
fn list_profiles(state: tauri::State<'_, AppState>) -> Vec<ProfileInfo> {
    let store = ProfileStore::new(state.config_dir.join("profiles"));
    store
        .list()
        .into_iter()
        .filter_map(|id| {
            let p = store.load(&id).ok()?;
            Some(ProfileInfo {
                id,
                name: p.profile.name,
            })
        })
        .collect()
}

/// The active profile's full TOML document (JSON-shaped for the editor).
#[tauri::command]
fn get_active_profile(
    state: tauri::State<'_, AppState>,
) -> Result<(String, od_storage::ProfileToml), String> {
    let id = state
        .settings
        .lock()
        .expect("settings lock")
        .active_profile
        .clone();
    let store = ProfileStore::new(state.config_dir.join("profiles"));
    let p = store.load(&id).map_err(|e| e.to_string())?;
    Ok((id, p))
}

/// Saves edited profile config as the user shadow file and hot-swaps the
/// pipeline context if it is the active profile.
#[tauri::command]
fn save_profile(
    state: tauri::State<'_, AppState>,
    id: String,
    profile: od_storage::ProfileToml,
) -> Result<(), String> {
    let store = ProfileStore::new(state.config_dir.join("profiles"));
    store.save_user(&id, &profile).map_err(|e| e.to_string())?;

    let settings = state.settings.lock().expect("settings lock").clone();
    if settings.active_profile == id {
        let ctx = build_ctx(&state.config_dir, &settings)?;
        *state.profile_name.lock().expect("profile lock") = ctx.profile.name.clone();
        state.session.send(SessionCommand::UpdateCtx(ctx));
    }
    Ok(())
}

#[derive(Serialize)]
struct DeviceInfo {
    name: String,
    is_default: bool,
}

#[tauri::command]
fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    od_audio::list_input_devices()
        .map(|list| {
            list.into_iter()
                .map(|d| DeviceInfo {
                    name: d.name,
                    is_default: d.is_default,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize)]
struct DictEntryView {
    spoken: String,
    written: String,
    case_sensitive: bool,
}

#[tauri::command]
fn dict_list(state: tauri::State<'_, AppState>) -> Result<Vec<DictEntryView>, String> {
    let guard = state.dict.lock().expect("dict lock");
    let repo = guard.as_ref().ok_or("dictionary database unavailable")?;
    let entries = repo
        .entries(&["user".to_owned()])
        .map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|e| DictEntryView {
            spoken: e.spoken,
            written: e.written,
            case_sensitive: e.case_sensitive,
        })
        .collect())
}

#[tauri::command]
fn dict_add(state: tauri::State<'_, AppState>, entry: DictEntryView) -> Result<(), String> {
    if entry.spoken.trim().is_empty() || entry.written.trim().is_empty() {
        return Err("spoken and written forms are both required".into());
    }
    {
        let mut guard = state.dict.lock().expect("dict lock");
        let repo = guard.as_mut().ok_or("dictionary database unavailable")?;
        repo.add(
            "user",
            &od_core_types::DictEntry {
                spoken: entry.spoken.trim().to_owned(),
                written: entry.written.trim().to_owned(),
                case_sensitive: entry.case_sensitive,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    refresh_ctx(&state)
}

#[tauri::command]
fn dict_remove(state: tauri::State<'_, AppState>, spoken: String) -> Result<bool, String> {
    let removed = {
        let mut guard = state.dict.lock().expect("dict lock");
        let repo = guard.as_mut().ok_or("dictionary database unavailable")?;
        repo.remove("user", &spoken).map_err(|e| e.to_string())?
    };
    refresh_ctx(&state)?;
    Ok(removed)
}

#[tauri::command]
fn history_list(
    state: tauri::State<'_, AppState>,
    limit: u32,
) -> Result<Vec<od_storage::HistoryEntry>, String> {
    let guard = state.history.lock().expect("history lock");
    let repo = guard.as_ref().ok_or("history database unavailable")?;
    repo.recent(limit.min(1000)).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_purge(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let mut guard = state.history.lock().expect("history lock");
    let repo = guard.as_mut().ok_or("history database unavailable")?;
    repo.purge().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_perf(state: tauri::State<'_, AppState>) -> Perf {
    state.perf.lock().expect("perf lock").clone()
}

#[tauri::command]
fn model_status() -> model::ModelStatus {
    model::status()
}

/// Kicks off the model download (onboarding). Progress arrives as
/// `model-progress` events.
#[tauri::command]
fn model_download(app: AppHandle) {
    model::spawn_download(app);
}

/// Relaunches the app after onboarding put the model in place.
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

/// Re-resolves the active profile against the (possibly changed) dictionary
/// and hot-swaps the pipeline context.
fn refresh_ctx(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().expect("settings lock").clone();
    let ctx = build_ctx(&state.config_dir, &settings)?;
    *state.profile_name.lock().expect("profile lock") = ctx.profile.name.clone();
    state.session.send(SessionCommand::UpdateCtx(ctx));
    Ok(())
}

fn main() {
    let t0 = Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Without the STT model the pipeline can't come up; instead of dying
    // (pre-M8 behavior) the app opens the onboarding window, which downloads
    // the model with a pinned checksum and restarts.
    let model_present = od_stt::default_model_path().is_file();
    if !model_present {
        tracing::warn!(
            "STT model missing at {}; starting onboarding",
            od_stt::default_model_path().display()
        );
    }

    let dir = config_dir();
    let settings = load_settings(&dir.join("settings.json")).unwrap_or_else(|e| {
        tracing::error!("settings load failed ({e}); using defaults");
        Settings::default()
    });
    let ctx = load_pipeline_ctx(&dir, &settings);
    let profile_name = ctx.profile.name.clone();
    let capture = CaptureConfig {
        device: match &settings.input_device {
            Some(name) => DeviceSelector::ByName(name.clone()),
            None => DeviceSelector::SystemDefault,
        },
        ..CaptureConfig::default()
    };
    let (hotkey_toggle, hotkey_ptt) = (settings.hotkey_toggle.clone(), settings.hotkey_ptt.clone());

    tauri::Builder::default()
        // Must be the first plugin: a second launch hands off to the running
        // instance (its settings window) instead of panicking on the
        // already-registered global hotkey.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_settings(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            // Pipeline first: the hotkey must be live even if the webview is
            // still warming up (cold-start contract, docs/02).
            let session = od_pipeline::spawn(
                capture,
                TranscriberConfig::default(),
                ctx,
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

            let dict = SqliteDictionaryRepo::open(&dir.join("dictionary.db"))
                .map_err(|e| tracing::error!("dictionary db open failed: {e}"))
                .ok();
            let history = SqliteHistoryRepo::open(&dir.join("dictionary.db"))
                .map_err(|e| tracing::error!("history open failed: {e}"))
                .ok();

            app.manage(AppState {
                session,
                config_dir: dir.clone(),
                settings: Mutex::new(settings.clone()),
                dict: Mutex::new(dict),
                history: Mutex::new(history),
                perf: Mutex::new(Perf::default()),
                profile_name: Mutex::new(profile_name.clone()),
            });

            if let Err(e) = register_hotkeys(app.handle(), &hotkey_toggle, &hotkey_ptt) {
                // Fall back to defaults rather than starting hotkey-less.
                tracing::error!("hotkey registration failed ({e}); using defaults");
                let d = Settings::default();
                register_hotkeys(app.handle(), &d.hotkey_toggle, &d.hotkey_ptt)
                    .map_err(|e| format!("default hotkeys also failed: {e}"))?;
            }
            build_tray(app.handle())?;
            spawn_event_bridge(app.handle().clone(), events);
            position_overlay(app.handle());

            if !model_present {
                show_onboarding(app.handle());
            }

            let cold = t0.elapsed().as_millis() as u64;
            app.state::<AppState>()
                .perf
                .lock()
                .expect("perf lock")
                .cold_start_ms = cold;
            tracing::info!(cold_start_ms = cold, "hotkeys live; app ready");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_level,
            get_settings,
            update_settings,
            list_profiles,
            get_active_profile,
            save_profile,
            list_devices,
            dict_list,
            dict_add,
            dict_remove,
            history_list,
            history_purge,
            get_perf,
            model_status,
            model_download,
            restart_app,
        ])
        .on_window_event(|window, event| {
            // The settings window hides instead of closing: the app lives in
            // the tray, and re-creating the webview costs more than hiding.
            if window.label() == "settings" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
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

/// `%APPDATA%\OpenDictate` — settings, profiles, and the dictionary DB
/// (ADR-10). The STT model lives separately under `%LOCALAPPDATA%`.
fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OpenDictate")
}

/// Resolves the active profile into a pipeline context; errors are strings
/// for the command boundary.
fn build_ctx(dir: &std::path::Path, settings: &Settings) -> Result<PipelineCtx, String> {
    let store = ProfileStore::new(dir.join("profiles"));
    let profile = store
        .load(&settings.active_profile)
        .map_err(|e| e.to_string())?;
    let repo = SqliteDictionaryRepo::open(&dir.join("dictionary.db"))
        .or_else(|_| SqliteDictionaryRepo::open_in_memory())
        .map_err(|e| e.to_string())?;
    resolve_profile(&profile, &repo).map_err(|e| e.to_string())
}

/// Loads settings → active profile → resolved pipeline context (M5).
/// Any failure degrades to the shipped "general" profile with defaults —
/// dictation must come up even if config files are broken; the causes are
/// logged, never fatal.
fn load_pipeline_ctx(dir: &std::path::Path, settings: &Settings) -> PipelineCtx {
    match build_ctx(dir, settings) {
        Ok(ctx) => {
            tracing::info!(
                profile = %ctx.profile.name,
                dict_entries = ctx.profile.entries.len(),
                vocab_terms = ctx.vocab.terms.len(),
                "profile resolved"
            );
            ctx
        }
        Err(e) => {
            tracing::error!("profile resolve failed ({e}); trying shipped general");
            let general = Settings::default();
            build_ctx(dir, &general).unwrap_or_else(|e| {
                tracing::error!("general profile failed too ({e}); built-in defaults");
                PipelineCtx {
                    language: od_core_types::LanguageHint::Fixed("en".into()),
                    ..PipelineCtx::default()
                }
            })
        }
    }
}

/// (Re-)registers the two global shortcuts. Parses both before touching the
/// existing registration so a bad string can never leave the app hotkey-less.
fn register_hotkeys(app: &AppHandle, toggle: &str, ptt: &str) -> Result<(), String> {
    let toggle_sc: Shortcut = toggle
        .parse()
        .map_err(|e| format!("toggle hotkey {toggle:?}: {e}"))?;
    let ptt_sc: Shortcut = ptt
        .parse()
        .map_err(|e| format!("ptt hotkey {ptt:?}: {e}"))?;
    if toggle_sc == ptt_sc {
        return Err("toggle and push-to-talk hotkeys must differ".into());
    }

    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;

    gs.on_shortcut(toggle_sc, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            let state = app.state::<AppState>();
            state.session.send(SessionCommand::Toggle);
        }
    })
    .map_err(|e| e.to_string())?;

    gs.on_shortcut(ptt_sc, |app, _shortcut, event| {
        let state = app.state::<AppState>();
        state.session.send(match event.state {
            ShortcutState::Pressed => SessionCommand::PttPressed,
            ShortcutState::Released => SessionCommand::PttReleased,
        });
    })
    .map_err(|e| e.to_string())?;

    tracing::info!(toggle, ptt, "global shortcuts registered");
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit OpenDictate", true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .item(&settings_item)
        .item(&quit)
        .build()?;

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .expect("bundle icon configured")
                .clone(),
        )
        .tooltip("OpenDictate — idle")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => show_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Settings and onboarding webviews are created on demand, not in
/// tauri.conf: two idle WebView2 instances cost ~10 MB working set each,
/// which is what stood between the release build and the ≤120 MB idle
/// target (M9 soak).
fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let built = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("OpenDictate Settings")
    .inner_size(900.0, 640.0)
    .min_inner_size(720.0, 480.0)
    .center()
    .build();
    if let Err(e) = built {
        tracing::error!("settings window creation failed: {e}");
    }
}

fn show_onboarding(app: &AppHandle) {
    let built = tauri::WebviewWindowBuilder::new(
        app,
        "onboarding",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Welcome to OpenDictate")
    .inner_size(560.0, 560.0)
    .resizable(false)
    .maximizable(false)
    .center()
    .build();
    match built {
        Ok(w) => {
            let _ = w.set_focus();
        }
        Err(e) => tracing::error!("onboarding window creation failed: {e}"),
    }
}

/// Forwards bus events to the webview and drives tray/overlay visibility.
/// The mic-truth rule (docs/06 T1): tray tooltip and overlay visibility are
/// derived from the same `StateChanged` events as everything else. M7: this
/// thread is also the single history writer and perf aggregator.
fn spawn_event_bridge(app: AppHandle, events: std::sync::mpsc::Receiver<AppEvent>) {
    std::thread::Builder::new()
        .name("od-event-bridge".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                match &event {
                    AppEvent::StateChanged { state } => apply_state(&app, *state),
                    AppEvent::FinalReady { raw, cleaned, .. } => {
                        record_history(&app, raw, cleaned);
                    }
                    AppEvent::UtteranceFinalized { finalize_ms } => {
                        if let Some(state) = app.try_state::<AppState>() {
                            let mut perf = state.perf.lock().expect("perf lock");
                            perf.utterances += 1;
                            perf.finalize_ms_last = Some(*finalize_ms);
                            perf.finalize_ms_best = Some(
                                perf.finalize_ms_best
                                    .map_or(*finalize_ms, |b| b.min(*finalize_ms)),
                            );
                        }
                    }
                    AppEvent::Inserted {
                        tier, latency_ms, ..
                    } => {
                        if let Some(state) = app.try_state::<AppState>() {
                            let mut perf = state.perf.lock().expect("perf lock");
                            perf.inserts += 1;
                            perf.insert_ms_last = Some(*latency_ms);
                            perf.insert_tier_last = Some(tier.clone());
                        }
                    }
                    AppEvent::InsertFailed { .. } => {
                        if let Some(state) = app.try_state::<AppState>() {
                            state.perf.lock().expect("perf lock").insert_failures += 1;
                        }
                    }
                    AppEvent::ProfileChanged { name } => {
                        if let Some(state) = app.try_state::<AppState>() {
                            *state.profile_name.lock().expect("profile lock") = name.clone();
                        }
                    }
                    _ => {}
                }
                if let Err(e) = app.emit("app-event", &event) {
                    tracing::debug!("emit failed (shutdown?): {e}");
                }
            }
            tracing::info!("event bridge exiting");
        })
        .expect("spawn event bridge");
}

/// Appends one final segment to local history, honoring the settings toggle
/// and cap. Never blocks the pipeline — this runs on the bridge thread.
fn record_history(app: &AppHandle, raw: &str, cleaned: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let (enabled, cap) = {
        let s = state.settings.lock().expect("settings lock");
        (s.history_enabled, s.history_cap)
    };
    if !enabled || cleaned.trim().is_empty() {
        return;
    }
    let profile = state.profile_name.lock().expect("profile lock").clone();
    let mut guard = state.history.lock().expect("history lock");
    if let Some(repo) = guard.as_mut() {
        if let Err(e) = repo.add(raw, cleaned, &profile, cap) {
            tracing::warn!("history write failed: {e}");
        }
    }
}

fn apply_state(app: &AppHandle, state: SessionState) {
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
fn position_overlay(app: &AppHandle) {
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
