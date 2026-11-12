use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Runtime};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

mod calibration;
mod capture;
mod engine;
mod exp_table;
mod ocr;
mod profile;
mod settings;
mod window_target;

struct OverlayState {
    interactive: AtomicBool,
}

pub(crate) fn data_dir() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("MSCOUT_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    let base = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or("cannot resolve project root")?
    } else {
        std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or("cannot resolve executable directory")?
    };

    Ok(base.join("data"))
}

pub(crate) fn append_sample_record(record: &Value) -> Result<(), String> {
    let t = record
        .get("t")
        .and_then(Value::as_i64)
        .ok_or("sample is missing t")?;

    let day = chrono::DateTime::from_timestamp(t, 0)
        .ok_or("invalid timestamp")?
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string();

    let dir = data_dir()?.join("samples");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!("{day}.jsonl"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;

    let line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

#[tauri::command]
fn append_sample(record: Value) -> Result<(), String> {
    append_sample_record(&record)
}

#[tauri::command]
fn read_samples(from: i64, to: i64) -> Result<Vec<Value>, String> {
    let dir = data_dir()?.join("samples");
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut samples = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let file = fs::File::open(&path).map_err(|e| e.to_string())?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if let Some(t) = value.get("t").and_then(Value::as_i64) {
                    if t >= from && t <= to {
                        samples.push(value);
                    }
                }
            }
        }
    }

    samples.sort_by_key(|v| v.get("t").and_then(Value::as_i64).unwrap_or(0));
    Ok(samples)
}

#[tauri::command]
fn get_settings() -> Result<Value, String> {
    let path = data_dir()?.join("settings.json");
    if !path.exists() {
        return Ok(Value::Null);
    }
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_settings(value: Value) -> Result<(), String> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    fs::write(dir.join("settings.json"), raw).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_interactive<R: Runtime>(app: AppHandle<R>, interactive: bool) -> Result<(), String> {
    app.state::<OverlayState>()
        .interactive
        .store(interactive, Ordering::Relaxed);

    let window = app
        .get_webview_window("main")
        .ok_or("overlay window missing")?;

    window
        .set_ignore_cursor_events(!interactive)
        .map_err(|e| e.to_string())?;

    if interactive {
        let _ = window.set_focus();
    }

    let _ = app.emit("interactive", interactive);
    Ok(())
}

#[tauri::command]
fn set_mode<R: Runtime>(app: AppHandle<R>, expanded: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("overlay window missing")?;

    let size = if expanded {
        LogicalSize::new(380.0, 350.0)
    } else {
        LogicalSize::new(300.0, 140.0)
    };

    window.set_size(size).map_err(|e| e.to_string())
}

fn toggle_interactive<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<OverlayState>();
    let next = !state.interactive.load(Ordering::Relaxed);
    state.interactive.store(next, Ordering::Relaxed);

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_ignore_cursor_events(!next);
        if next {
            let _ = window.set_focus();
        }
    }

    let _ = app.emit("interactive", next);
}

fn toggle_visible<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(true);
        let _ = if visible { window.hide() } else { window.show() };
    }
}

fn open_history<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("history") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let result = tauri::WebviewWindowBuilder::new(
        app,
        "history",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("MScout — History")
    .inner_size(900.0, 540.0)
    .min_inner_size(560.0, 320.0)
    .build();

    if let Err(error) = result {
        if let Ok(dir) = data_dir() {
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(dir.join("window-error.log"), format!("{error}"));
        }
    }
}

fn register_shortcuts<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let modifiers = Modifiers::CONTROL | Modifiers::ALT;
    for code in [Code::KeyM, Code::KeyV, Code::KeyH, Code::KeyT] {
        app.global_shortcut()
            .register(Shortcut::new(Some(modifiers), code))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn build_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / hide", true, None::<&str>)?;
    let interactive = MenuItem::with_id(app, "interactive", "Toggle interactive", true, None::<&str>)?;
    let target = MenuItem::with_id(app, "target", "Choose game window…", true, None::<&str>)?;
    let calibrate = MenuItem::with_id(app, "calibrate", "Calibrate boxes…", true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", "History", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &interactive, &target, &calibrate, &history, &quit])?;

    let mut builder = TrayIconBuilder::with_id("tray")
        .menu(&menu)
        .show_menu_on_left_click(false);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_visible(app),
            "interactive" => toggle_interactive(app),
            "target" => window_target::open_target_picker(app),
            "calibrate" => calibration::open_calibration_window(app),
            "history" => open_history(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(OverlayState {
            interactive: AtomicBool::new(false),
        })
        .manage(window_target::TargetState::new(
            settings::load_target(),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }

                    let modifiers = Modifiers::CONTROL | Modifiers::ALT;
                    if shortcut.matches(modifiers, Code::KeyM) {
                        toggle_interactive(app);
                    } else if shortcut.matches(modifiers, Code::KeyV) {
                        toggle_visible(app);
                    } else if shortcut.matches(modifiers, Code::KeyH) {
                        open_history(app);
                    } else if shortcut.matches(modifiers, Code::KeyT) {
                        window_target::open_target_picker(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            append_sample,
            read_samples,
            get_settings,
            set_settings,
            set_interactive,
            set_mode,
            window_target::list_windows,
            window_target::get_target_status,
            window_target::set_target,
            window_target::clear_target,
            window_target::open_target_window,
            calibration::capture_preview,
            calibration::get_calibration,
            calibration::calibration_for,
            calibration::set_calibration,
            calibration::get_learning,
            calibration::learn_atlas,
            calibration::teach_map,
            calibration::teach_identity,
            calibration::parse_probe,
            calibration::open_calibration,
            engine::get_profile,
            profile::list_profiles,
            profile::active_profile
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_ignore_cursor_events(true);
                let _ = window.set_always_on_top(true);
            }

            // Bring an existing single-file calibration forward into a
            // size-scoped profile before the engine starts reading.
            if let Some(key) = profile::migrate_legacy() {
                let _ = handle.emit("profile-migrated", key);
            }

            register_shortcuts(&handle)?;
            build_tray(&handle)?;
            engine::start(handle.clone());

            if std::env::var_os("MSCOUT_OPEN_HISTORY").is_some() {
                open_history(&handle);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
