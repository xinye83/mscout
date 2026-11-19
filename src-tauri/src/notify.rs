//! Desktop notifications for capture failures.
//!
//! The overlay is only visible when the player looks at it, but a broken capture
//! matters while they are playing. `docs/SPEC.md` therefore requires a desktop
//! notification when the capture is lost, so the failure is discovered at the time
//! it happens instead of three hours later in the history.
//!
//! This is a **WinRT toast**, not a tray balloon. A balloon is the obvious older
//! API, but Windows 11 no longer draws one: `Shell_NotifyIconW` accepts the call
//! and reports success while nothing appears. That silent success is why this
//! module uses the toast API and why the outcome is logged.
//!
//! A toast from an *unpackaged* executable needs an AppUserModelID registered
//! under `HKCU\Software\Classes\AppUserModelId\<id>`; without it
//! `CreateToastNotifier` fails. Registration is per-user and needs no elevation,
//! so it works for a portable folder copy as well as an installed one.

/// The AUMID this app registers and posts toasts as.
///
/// Deliberately stable: renaming it orphans the registration and silently stops
/// notifications until the next run re-registers under the new id.
pub const APP_ID: &str = "MScout.Notifications";

/// What a notification is about, so callers do not pass raw UI text around.
pub enum Notice<'a> {
    /// The game window is gone, minimised, or could not be captured.
    CaptureLost(&'a str),
}

impl Notice<'_> {
    fn title(&self) -> &'static str {
        match self {
            Notice::CaptureLost(_) => "MScout — not reading",
        }
    }

    fn body(&self) -> &str {
        match self {
            Notice::CaptureLost(reason) => reason,
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    const DISPLAY_NAME: &str = "MScout";

    /// Make sure the AUMID is registered, so `CreateToastNotifier` can resolve it.
    ///
    /// Idempotent: rewriting the same values is cheaper than a read-modify-write and
    /// leaves no partial state if the app is killed part-way.
    fn register(app_id: &str) -> bool {
        let subkey: Vec<u16> = format!("Software\\Classes\\AppUserModelId\\{app_id}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut key = HKEY::default();
        let created = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        };
        if created != ERROR_SUCCESS {
            return false;
        }

        let name: Vec<u16> = "DisplayName"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let value: Vec<u16> = DISPLAY_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let length = std::mem::size_of_val(value.as_slice()) as u32;

        let written = unsafe {
            RegSetValueExW(
                key,
                PCWSTR(name.as_ptr()),
                None,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    value.as_ptr() as *const u8,
                    length as usize,
                )),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        }

        written == ERROR_SUCCESS
    }

    /// Escape the characters that would break the toast's XML payload.
    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// Post a toast. Returns whether Windows accepted it.
    pub fn show(app_id: &str, title: &str, body: &str) -> bool {
        if !register(app_id) {
            return false;
        }

        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\">\
             <text>{}</text><text>{}</text>\
             </binding></visual></toast>",
            escape(title),
            escape(body),
        );

        let Ok(document) = XmlDocument::new() else {
            return false;
        };
        if document.LoadXml(&HSTRING::from(xml.as_str())).is_err() {
            return false;
        }

        let Ok(toast) = ToastNotification::CreateToastNotification(&document) else {
            return false;
        };

        let Ok(notifier) =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))
        else {
            return false;
        };

        notifier.Show(&toast).is_ok()
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn show(_app_id: &str, _title: &str, _body: &str) -> bool {
        false
    }
}

/// Show a notification, logging the outcome either way.
///
/// A failed notification is not fatal — the overlay banner is still there — but
/// "no notification" is exactly the failure this feature exists to catch, so it
/// must never be silent. Success is recorded too: a toast leaves no trace once
/// Windows dismisses it, so the log is the only evidence it fired.
pub fn notify(notice: Notice<'_>) {
    if platform::show(APP_ID, notice.title(), notice.body()) {
        crate::calibration::log(&format!("notification shown: {}", notice.title()));
    } else {
        crate::calibration::log(&format!(
            "could not show the notification {:?}: {}",
            notice.title(),
            notice.body()
        ));
    }
}

/// Tell the player the capture stopped.
pub fn capture_lost(reason: &str) {
    notify(Notice::CaptureLost(reason));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_carries_the_reason_as_its_body() {
        let notice = Notice::CaptureLost("game window not found");
        assert_eq!(notice.title(), "MScout — not reading");
        assert_eq!(notice.body(), "game window not found");
    }

    #[test]
    fn a_notification_failure_does_not_panic() {
        // The real call is a WinRT activation; what matters is that an unavailable
        // shell is a reported false, never a panic.
        notify(Notice::CaptureLost("test reason"));
    }

    #[test]
    fn the_app_id_is_stable_and_namespaced() {
        // Changing this orphans every existing toast registration.
        assert_eq!(APP_ID, "MScout.Notifications");
        assert!(APP_ID.contains('.'));
    }
}
