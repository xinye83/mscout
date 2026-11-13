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
mod preset;
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
                // Rows the reader knows were wrong are skipped everywhere, so the
                // chart and every rate calculation see clean data.
                if value.get("bad").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
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

/// The newest recorded row. The engine compares each reading against the last one
/// it recorded, so without seeding from this the first reading after a restart is
/// unchecked — which is how a two-EXP blip got into the history.
pub(crate) fn last_sample_record() -> Option<Value> {
    let dir = data_dir().ok()?.join("samples");
    let mut days: Vec<std::path::PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    days.sort();

    let file = fs::File::open(days.pop()?).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|value| !value.get("bad").and_then(Value::as_bool).unwrap_or(false))
        .last()
}

/// Indices of rows that fall below the running high-water mark and later climb back
/// over it. A genuine loss of EXP — dying costs EXP — never recovers, so it is left
/// alone; a misread always does, because the next good read overtakes it.
fn find_misreads(rows: &[(u32, i64)]) -> Vec<usize> {
    let mut flagged = Vec::new();
    let mut parked: Vec<usize> = Vec::new();
    let mut baseline: Option<(u32, i64)> = None;

    for (index, (level, exp)) in rows.iter().enumerate() {
        match baseline {
            Some((base_level, base_exp))
                if *level < base_level || (*level == base_level && *exp < base_exp) =>
            {
                parked.push(index);
            }
            _ => {
                if let Some((base_level, base_exp)) = baseline {
                    if !parked.is_empty() {
                        if *level > base_level || (*level == base_level && *exp >= base_exp) {
                            flagged.append(&mut parked);
                        } else {
                            parked.clear();
                        }
                    }
                }
                baseline = Some((*level, *exp));
            }
        }
    }

    flagged
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairSummary {
    pub checked: usize,
    pub flagged: usize,
    pub files: usize,
}

/// Flag the recorded rows that break the invariants the reader now enforces, so
/// the chart and the rate maths stop being dragged down by misreads.
///
/// Rows are flagged rather than deleted: the file stays as the evidence of how
/// often the reader failed, and a mistaken flag can be removed by hand. A row is
/// only flagged when it sits below the running high-water mark *and* the recording
/// later climbs back over it — a genuine loss of EXP never recovers, so it stays.
#[tauri::command]
fn repair_history() -> Result<RepairSummary, String> {
    let dir = data_dir()?.join("samples");
    if !dir.exists() {
        return Ok(RepairSummary {
            checked: 0,
            flagged: 0,
            files: 0,
        });
    }

    let mut files: Vec<std::path::PathBuf> = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();

    let mut lines_per_file: Vec<Vec<String>> = Vec::new();
    let mut rows: Vec<(usize, usize, Value)> = Vec::new();
    for (file_index, path) in files.iter().enumerate() {
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut lines: Vec<String> = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if value.get("t").and_then(Value::as_i64).is_some() {
                    rows.push((file_index, lines.len(), value));
                }
            }
            lines.push(line);
        }
        lines_per_file.push(lines);
    }

    // Every row of every day, in time order, so a dip across midnight is caught too.
    rows.sort_by_key(|(_, _, value)| value.get("t").and_then(Value::as_i64).unwrap_or(0));
    let checked = rows.len();

    let series: Vec<(u32, i64)> = rows
        .iter()
        .map(|(_, _, row)| {
            (
                row.get("lv").and_then(Value::as_u64).unwrap_or(0) as u32,
                row.get("exp").and_then(Value::as_i64).unwrap_or(0),
            )
        })
        .collect();

    let flagged: Vec<(usize, usize)> = find_misreads(&series)
        .into_iter()
        .filter_map(|index| {
            rows.get(index)
                .map(|(file_index, line_index, _)| (*file_index, *line_index))
        })
        .collect();

    let mut changed_files = 0usize;
    for (file_index, lines) in lines_per_file.iter_mut().enumerate() {
        let mut changed = false;
        for (flagged_file, line_index) in &flagged {
            if *flagged_file != file_index {
                continue;
            }
            let Ok(mut value) = serde_json::from_str::<Value>(&lines[*line_index]) else {
                continue;
            };
            let Some(object) = value.as_object_mut() else {
                continue;
            };
            if object.get("bad").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            object.insert("bad".into(), Value::Bool(true));
            lines[*line_index] = serde_json::to_string(&value).map_err(|e| e.to_string())?;
            changed = true;
        }

        if changed {
            let text = lines.join("\n");
            fs::write(&files[file_index], format!("{text}\n")).map_err(|e| e.to_string())?;
            changed_files += 1;
        }
    }

    // Report what the files now carry, so pressing the button twice is stable.
    let total = lines_per_file
        .iter()
        .flat_map(|lines| lines.iter())
        .filter(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| value.get("bad").and_then(Value::as_bool))
                .unwrap_or(false)
        })
        .count();

    Ok(RepairSummary {
        checked,
        flagged: total,
        files: changed_files,
    })
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
    // Only records the override; the hover thread applies click-through, so this
    // never pulls focus and never fights the hover state.
    app.state::<OverlayState>()
        .interactive
        .store(interactive, Ordering::Relaxed);
    Ok(())
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
        let _ = if visible { window.hide() } else { window.show() };
    }
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
            resize_overlay,
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
            calibration::map_label_crop,
            calibration::teach_identity,
            calibration::parse_probe,
            calibration::split_identity,
            calibration::exp_glyphs,
            calibration::label_glyph,
            calibration::clear_maps,
            calibration::apply_preset,
            calibration::review_frames,
            calibration::label_review,
            calibration::clear_review,
            calibration::open_calibration,
            repair_history,
            engine::get_profile,
            profile::list_profiles,
            profile::active_profile,
            profile::delete_profile,
            settings::ui_modes,
            settings::set_ui_mode
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
    fn flags_the_dips_the_reader_used_to_record() {
        // The shape of the recorded history: a two-EXP blip, then a truncated read
        // whose derived level falls away and later comes back.
        let rows = vec![
            (35, 21_478),
            (35, 21_476),
            (35, 21_476),
            (35, 21_500),
            (19, 2_106),
            (19, 2_106),
            (35, 21_704),
            (35, 21_710),
        ];
        assert_eq!(find_misreads(&rows), vec![1, 2, 4, 5]);
    }

    #[test]
    fn keeps_a_loss_that_never_recovers() {
        // Dying costs EXP and the recording stays lower, so nothing is flagged.
        let rows = vec![(35, 50_000), (35, 49_000), (35, 49_500), (35, 49_800)];
        assert_eq!(find_misreads(&rows), Vec::<usize>::new());
    }

    #[test]
    fn keeps_an_ordinary_climb() {
        let rows = vec![(35, 1_000), (35, 2_000), (36, 10), (36, 900)];
        assert_eq!(find_misreads(&rows), Vec::<usize>::new());
    }

    #[test]
    #[ignore = "tool: flags the misread rows in the recorded history"]
    fn repair_history_now() {
        match repair_history() {
            Ok(summary) => println!(
                "checked {} rows, {} now flagged, {} files rewritten",
                summary.checked, summary.flagged, summary.files
            ),
            Err(error) => println!("repair failed: {error}"),
        }
    }
}
