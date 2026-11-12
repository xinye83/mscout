use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

pub use crate::settings::WindowSelector;

const MAX_PROCESS_PATH: usize = 1024;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub hwnd: i64,
    pub title: String,
    pub process: String,
    pub pid: u32,
    pub minimized: bool,
    pub client: Rect,
    pub screen: Rect,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetStatus {
    pub selector: Option<WindowSelector>,
    pub configured: bool,
    pub found: bool,
    pub title: Option<String>,
    pub process: Option<String>,
    pub pid: Option<u32>,
    pub hwnd: Option<i64>,
    pub minimized: bool,
    pub client: Option<Rect>,
    pub screen: Option<Rect>,
}

pub struct TargetState {
    selector: Mutex<Option<WindowSelector>>,
    published: Mutex<Option<TargetStatus>>,
}

impl TargetState {
    pub fn new(selector: Option<WindowSelector>) -> Self {
        Self {
            selector: Mutex::new(selector),
            published: Mutex::new(None),
        }
    }

    pub fn selector(&self) -> Option<WindowSelector> {
        lock(&self.selector).clone()
    }
}

fn lock<'a, T>(mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }

        let mut buffer = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied <= 0 {
            return String::new();
        }

        String::from_utf16_lossy(&buffer[..copied as usize])
    }
}

fn process_name(pid: u32) -> String {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return String::new();
        }

        let mut buffer = vec![0u16; MAX_PROCESS_PATH];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size);
        let _ = CloseHandle(handle);

        if ok == 0 || size == 0 {
            return String::new();
        }

        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        path.rsplit(['\\', '/'])
            .next()
            .unwrap_or(path.as_str())
            .to_string()
    }
}

fn inspect(hwnd: HWND, own_pid: u32) -> Option<WindowInfo> {
    if hwnd.is_null() {
        return None;
    }

    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return None;
        }

        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW != 0 {
            return None;
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == own_pid {
            return None;
        }

        let title = window_title(hwnd);
        if title.trim().is_empty() {
            return None;
        }

        let mut client = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut client) == 0 {
            return None;
        }

        let w = client.right - client.left;
        let h = client.bottom - client.top;
        if w <= 0 || h <= 0 {
            return None;
        }

        let mut origin = POINT { x: 0, y: 0 };
        if ClientToScreen(hwnd, &mut origin) == 0 {
            origin.x = 0;
            origin.y = 0;
        }

        Some(WindowInfo {
            hwnd: hwnd as usize as i64,
            title,
            process: process_name(pid),
            pid,
            minimized: IsIconic(hwnd) != 0,
            client: Rect { x: 0, y: 0, w, h },
            screen: Rect {
                x: origin.x,
                y: origin.y,
                w,
                h,
            },
        })
    }
}

struct EnumContext {
    own_pid: u32,
    windows: Vec<WindowInfo>,
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = &mut *(lparam as *mut EnumContext);
    if let Some(info) = inspect(hwnd, context.own_pid) {
        context.windows.push(info);
    }
    1
}

fn enumerate_windows() -> Vec<WindowInfo> {
    let mut context = EnumContext {
        own_pid: std::process::id(),
        windows: Vec::new(),
    };

    unsafe {
        EnumWindows(Some(enum_window), &mut context as *mut EnumContext as LPARAM);
    }

    context
        .windows
        .sort_by_key(|window| window.title.to_lowercase());
    context.windows
}

pub(crate) fn resolve_selector(selector: &WindowSelector) -> Option<WindowInfo> {
    let wanted_title = selector.title.to_lowercase();
    let wanted_process = selector.process.to_lowercase();
    if wanted_title.is_empty() && wanted_process.is_empty() {
        return None;
    }

    let mut best: Option<(u8, WindowInfo)> = None;
    for window in enumerate_windows() {
        let title_match = !wanted_title.is_empty() && window.title.to_lowercase() == wanted_title;
        let process_match =
            !wanted_process.is_empty() && window.process.to_lowercase() == wanted_process;

        let score = match (title_match, process_match) {
            (true, true) => 3,
            (false, true) => 2,
            (true, false) => 1,
            (false, false) => 0,
        };
        if score == 0 {
            continue;
        }

        let better = match &best {
            None => true,
            Some((best_score, best_window)) => {
                score > *best_score
                    || (score == *best_score
                        && window.client.w * window.client.h
                            > best_window.client.w * best_window.client.h)
            }
        };
        if better {
            best = Some((score, window));
        }
    }

    best.map(|(_, window)| window)
}

fn found_status(selector: WindowSelector, window: WindowInfo) -> TargetStatus {
    TargetStatus {
        selector: Some(selector),
        configured: true,
        found: true,
        title: Some(window.title),
        process: Some(window.process),
        pid: Some(window.pid),
        hwnd: Some(window.hwnd),
        minimized: window.minimized,
        client: Some(window.client),
        screen: Some(window.screen),
    }
}

pub(crate) fn status_for(selector: &Option<WindowSelector>) -> TargetStatus {
    match selector {
        Some(selector) => match resolve_selector(selector) {
            Some(window) => found_status(selector.clone(), window),
            None => TargetStatus {
                selector: Some(selector.clone()),
                configured: true,
                found: false,
                title: None,
                process: None,
                pid: None,
                hwnd: None,
                minimized: false,
                client: None,
                screen: None,
            },
        },
        None => TargetStatus {
            selector: None,
            configured: false,
            found: false,
            title: None,
            process: None,
            pid: None,
            hwnd: None,
            minimized: false,
            client: None,
            screen: None,
        },
    }
}

pub(crate) fn publish_status<R: Runtime>(
    app: &AppHandle<R>,
    state: &TargetState,
    status: TargetStatus,
) {
    let changed = {
        let mut published = lock(&state.published);
        if published.as_ref() == Some(&status) {
            false
        } else {
            *published = Some(status.clone());
            true
        }
    };

    if changed {
        let _ = app.emit("target-status", status);
    }
}

#[tauri::command]
pub fn list_windows() -> Vec<WindowInfo> {
    enumerate_windows()
}

#[tauri::command]
pub fn get_target_status(state: State<'_, TargetState>) -> TargetStatus {
    let selector = lock(&state.selector).clone();
    status_for(&selector)
}

#[tauri::command]
pub fn set_target<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TargetState>,
    title: String,
    process: String,
) -> Result<TargetStatus, String> {
    let selector = WindowSelector {
        title: title.trim().to_string(),
        process: process.trim().to_string(),
    };

    if selector.title.is_empty() && selector.process.is_empty() {
        return Err("a window title or process name is required".into());
    }

    let window = resolve_selector(&selector).ok_or_else(|| "that window is not open".to_string())?;
    crate::settings::save_target(Some(&selector))?;

    *lock(&state.selector) = Some(selector.clone());

    let status = found_status(selector, window);
    publish_status(&app, &state, status.clone());
    Ok(status)
}

#[tauri::command]
pub fn clear_target<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TargetState>,
) -> Result<TargetStatus, String> {
    crate::settings::save_target(None)?;
    *lock(&state.selector) = None;

    let status = status_for(&None);
    publish_status(&app, &state, status.clone());
    Ok(status)
}

#[tauri::command]
pub fn open_target_window<R: Runtime>(app: AppHandle<R>) {
    open_target_picker(&app);
}

pub fn open_target_picker<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("target") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "target",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("MScout — Target window")
    .inner_size(640.0, 560.0)
    .min_inner_size(480.0, 360.0)
    .build();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_visible_windows() {
        let windows = enumerate_windows();
        println!("enumerated {} windows", windows.len());
        for window in windows.iter().take(5) {
            println!("  {} [{}] {}x{}", window.title, window.process, window.client.w, window.client.h);
        }
        assert!(!windows.is_empty(), "expected at least one titled window");
    }

    #[test]
    fn resolves_a_listed_window() {
        let windows = enumerate_windows();
        let first = windows.first().expect("no windows to resolve");
        let selector = WindowSelector {
            title: first.title.clone(),
            process: first.process.clone(),
        };

        let found = resolve_selector(&selector).expect("listed window did not resolve");
        assert_eq!(found.hwnd, first.hwnd);
    }
}
