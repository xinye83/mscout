use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Runtime};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

mod calibration;
mod capture;
#[cfg(test)]
mod corpus;
mod engine;
mod exp_table;
mod font_seed;
mod fsutil;
mod game_font;
mod guard;
mod jobs;
mod map_match;
mod map_names;
mod model;
mod native;
mod notify;
mod ocr;
mod preset;
mod profile;
#[cfg(test)]
mod regression;
mod samples;
mod settings;
mod window_target;
#[cfg(windows)]
mod winocr;

struct OverlayState {
    interactive: AtomicBool,
}

/// Set when the player asks the overlay to stop using a dragged position.
///
/// The engine loop owns the anchoring and cannot be reached from the tray callback,
/// so the reset is a flag it reads and clears rather than a direct mutation.
static POSITION_RESET: AtomicBool = AtomicBool::new(false);

/// Read and clear the "forget the dragged position" request.
pub(crate) fn take_position_reset() -> bool {
    POSITION_RESET.swap(false, Ordering::Relaxed)
}

pub(crate) fn data_dir() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("MSCOUT_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    // A test never writes into the developer's real `data/` (debug.log, history,
    // profiles). Without MSCOUT_DATA_DIR it gets a per-process temp folder.
    #[cfg(test)]
    return Ok(test_data_dir());

    #[allow(unreachable_code)]
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

/// The data folder for a test run without MSCOUT_DATA_DIR: a temp folder per
/// process. The real folder's MSCW install path is copied in (read only), so the
/// tests that need the client's WZ files still find them.
#[cfg(test)]
fn test_data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mscout-test-data-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let real = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|root| root.join("data").join("settings.json"));
        let mscw = real
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| {
                serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok()
            })
            .and_then(|value| value.get("mscwDir").cloned());
        let settings = match mscw {
            Some(dir) => serde_json::json!({ "mscwDir": dir }),
            None => serde_json::json!({}),
        };
        let _ = fs::write(dir.join("settings.json"), settings.to_string());
        dir
    })
    .clone()
}

/// Recorded rows in `[from, to]`. `bucket` (seconds) thins long ranges on the
/// Rust side. Async, like the repair: a sync command runs on the main thread,
/// and reading weeks of history there froze every window, the overlay included.
#[tauri::command(async)]
fn read_samples(from: i64, to: i64, bucket: Option<i64>) -> Result<Vec<Value>, String> {
    samples::read(from, to, bucket)
}

#[tauri::command(async)]
fn repair_history() -> Result<samples::RepairSummary, String> {
    samples::repair()
}

fn cursor_position() -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } != 0 {
        (point.x, point.y)
    } else {
        (-1, -1)
    }
}

/// Keep the overlay clickable while the cursor is over it, and click-through
/// otherwise, so the panel's buttons work without the interact hotkey while
/// clicks anywhere else still reach the game. The hotkey/tray toggle forces it
/// clickable regardless of hover.
fn start_hover_interactivity<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let mut applied = false;

        loop {
            std::thread::sleep(std::time::Duration::from_millis(80));

            let Some(window) = app.get_webview_window("main") else {
                continue;
            };
            let forced = app
                .state::<OverlayState>()
                .interactive
                .load(Ordering::Relaxed);

            let hovered = match (window.outer_position(), window.outer_size()) {
                (Ok(position), Ok(size)) => {
                    let (x, y) = cursor_position();
                    x >= position.x
                        && x < position.x + size.width as i32
                        && y >= position.y
                        && y < position.y + size.height as i32
                }
                _ => false,
            };

            let effective = forced || hovered;
            if effective != applied {
                applied = effective;
                let _ = window.set_ignore_cursor_events(!effective);
                let _ = app.emit("interactive", effective);
            }
        }
    });
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

/// Resize the overlay to the height its content actually needs. The fixed sizes in
/// `set_mode` are only a starting point: a warning that wraps to several lines used
/// to overlap the rows below it, because the window never grew to match.
#[tauri::command]
fn resize_overlay<R: Runtime>(app: AppHandle<R>, height: f64) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("overlay window missing")?;

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let current = window.inner_size().map_err(|e| e.to_string())?;
    let width = current.width as f64 / scale;
    let height = height.clamp(60.0, 900.0);

    if (current.height as f64 / scale - height).abs() < 1.0 {
        return Ok(());
    }

    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|e| e.to_string())
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
        let _ = if visible {
            window.hide()
        } else {
            window.show()
        };
    }
}

/// Forget a dragged overlay position, so the panel follows the game window again.
///
/// A drag and the automatic anchor are both reasonable places to sit; without a way
/// back, a single accidental drag would permanently pin the panel away from the
/// corner it is designed to hug.
fn reset_overlay_position<R: Runtime>(app: &AppHandle<R>) {
    if let Err(error) = settings::clear_overlay_position() {
        crate::calibration::log(&format!("could not clear the overlay position: {error}"));
    }
    POSITION_RESET.store(true, Ordering::Relaxed);
    let _ = app.emit("overlay-position-reset", ());
}

fn open_history<R: Runtime>(app: &AppHandle<R>) {
    // The window must be built from the event loop, never synchronously inside
    // the IPC message handler: a webview created there comes up blank. The
    // thread hop guarantees the build is queued to the loop.
    let handle = app.clone();
    std::thread::spawn(move || {
        let inner = handle.clone();
        let _ = handle.run_on_main_thread(move || open_history_on_main(&inner));
    });
}

fn open_history_on_main<R: Runtime>(app: &AppHandle<R>) {
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

/// The global hotkeys, by the key pressed with Ctrl+Alt.
const SHORTCUT_KEYS: [(Code, &str); 4] = [
    (Code::KeyM, "M"),
    (Code::KeyV, "V"),
    (Code::KeyH, "H"),
    (Code::KeyT, "T"),
];

/// Register every hotkey that is free. A hotkey another app already holds is
/// skipped, not fatal: it used to abort startup, so one taken Ctrl+Alt
/// combination stopped MScout from running at all. Returns the ones that failed.
fn register_shortcuts<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let modifiers = Modifiers::CONTROL | Modifiers::ALT;
    let mut failed = Vec::new();
    for (code, key) in SHORTCUT_KEYS {
        if let Err(error) = app
            .global_shortcut()
            .register(Shortcut::new(Some(modifiers), code))
        {
            failed.push(format!("Ctrl+Alt+{key} ({error})"));
        }
    }
    failed
}

/// The notice shown when some hotkeys could not be registered.
fn shortcut_failure_notice(failed: &[String]) -> Option<String> {
    (!failed.is_empty()).then(|| {
        format!(
            "Hotkeys in use by another app, so they are off: {}",
            failed.join(", ")
        )
    })
}

fn build_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / hide", true, None::<&str>)?;
    let interactive =
        MenuItem::with_id(app, "interactive", "Toggle interactive", true, None::<&str>)?;
    let target = MenuItem::with_id(app, "target", "Choose game window…", true, None::<&str>)?;
    let calibrate = MenuItem::with_id(app, "calibrate", "Calibrate boxes…", true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", "History", true, None::<&str>)?;
    let reposition = MenuItem::with_id(
        app,
        "reset-position",
        "Snap back to the game corner",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &interactive,
            &target,
            &calibrate,
            &history,
            &reposition,
            &quit,
        ],
    )?;

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
            "reset-position" => reset_overlay_position(app),
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
        .manage(window_target::TargetState::new(settings::load_target()))
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
            read_samples,
            set_mode,
            resize_overlay,
            window_target::list_windows,
            window_target::get_target_status,
            window_target::set_target,
            window_target::clear_target,
            window_target::open_target_window,
            calibration::capture_preview,
            calibration::save_diagnostic,
            calibration::get_calibration,
            calibration::set_calibration,
            calibration::get_learning,
            calibration::learn_atlas,
            calibration::learn_level_atlas,
            calibration::seed_atlas,
            calibration::teach_map,
            calibration::teach_street,
            calibration::box_crop,
            calibration::map_region,
            calibration::map_line_regions,
            calibration::preset_calibration,
            calibration::teach_identity,
            calibration::parse_probe,
            calibration::split_identity,
            calibration::box_glyphs,
            calibration::label_glyph,
            calibration::apply_preset,
            calibration::open_calibration,
            repair_history,
            engine::get_profile,
            profile::list_profiles,
            profile::active_profile,
            profile::delete_profile,
            settings::ui_modes,
            settings::set_ui_mode,
            game_font::get_mscw_status,
            game_font::set_mscw_dir,
            game_font::pick_mscw_dir
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

            let failed = register_shortcuts(&handle);
            if let Some(notice) = shortcut_failure_notice(&failed) {
                calibration::log(&notice);
                notify::notify(notify::Notice::Degraded(&notice));
            }
            build_tray(&handle)?;
            engine::start(handle.clone());
            start_hover_interactivity(handle.clone());

            if std::env::var_os("MSCOUT_OPEN_HISTORY").is_some() {
                open_history(&handle);
            }

            if std::env::var_os("MSCOUT_OPEN_CALIBRATION").is_some() {
                // Deferring matters here too: a webview built inside setup, before
                // the event loop starts, is created but never shown.
                calibration::open_calibration_window(&handle);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_never_use_the_real_data_folder() {
        if std::env::var("MSCOUT_DATA_DIR").is_ok() {
            return;
        }
        let dir = data_dir().expect("data dir");
        assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
    }

    #[test]
    fn a_taken_hotkey_is_reported_not_fatal() {
        assert_eq!(shortcut_failure_notice(&[]), None);
        let notice = shortcut_failure_notice(&["Ctrl+Alt+M (taken)".into()]).expect("notice");
        assert!(notice.contains("Ctrl+Alt+M"), "{notice}");
    }
}
