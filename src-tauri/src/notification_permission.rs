//! What macOS lets Halv show.
//!
//! The notification plugin reports "granted" on every desktop system, even
//! when macOS blocks the app, and it drops every send error. A user who
//! denied Halv once, or who switched "Allow Notifications" off, then sees no
//! banner and no warning. This module reads the real state from
//! `UNUserNotificationCenter`. Banners still travel through the legacy
//! notification center, but both read one record of settings.
//!
//! A dev build has no app bundle, and `UNUserNotificationCenter` raises an
//! exception without one. A process outside an `.app` bundle reports
//! `unknown` and never calls the framework.

#![cfg_attr(not(desktop), allow(dead_code))]

use serde::Serialize;
use std::path::Path;

/// The macOS state of Halv notifications. The names reach the interface.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NotificationPermissionState {
    /// macOS lets Halv show banners.
    Granted,
    /// The user denied Halv. No banner, no sound, no history entry.
    Denied,
    /// macOS has not asked yet. The first request shows the prompt.
    NotDetermined,
    /// Halv may notify, but the alert style is "None". No banner shows.
    BannersOff,
    /// Halv cannot read the state on this system.
    Unknown,
}

const SETTINGS_PANE: &str = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";

/// Maps `UNAuthorizationStatus` and `UNAlertStyle` to a state.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn state_from_settings(authorization: isize, alert_style: isize) -> NotificationPermissionState {
    match authorization {
        0 => NotificationPermissionState::NotDetermined,
        1 => NotificationPermissionState::Denied,
        // Authorized, provisional, and ephemeral all deliver.
        2..=4 if alert_style == 0 => NotificationPermissionState::BannersOff,
        2..=4 => NotificationPermissionState::Granted,
        _ => NotificationPermissionState::Unknown,
    }
}

/// True when the executable sits inside a `.app` bundle.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn runs_from_app_bundle(executable: &Path) -> bool {
    executable
        .ancestors()
        .skip(1)
        .any(|ancestor| ancestor.extension().is_some_and(|ext| ext == "app"))
}

/// The link that opens the notification settings of an app. An identifier
/// with an unexpected character opens the list of apps instead.
fn settings_url(identifier: &str) -> String {
    let safe = !identifier.is_empty()
        && identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if safe {
        format!("{SETTINGS_PANE}?id={identifier}")
    } else {
        SETTINGS_PANE.to_string()
    }
}

#[cfg(target_os = "macos")]
mod native {
    use block2::RcBlock;
    use objc2::{
        msg_send,
        runtime::{AnyClass, AnyObject, Bool},
    };
    use std::{sync::mpsc, time::Duration};

    #[link(name = "UserNotifications", kind = "framework")]
    extern "C" {}

    /// The framework answers within milliseconds. This bound keeps a stuck
    /// answer from holding a thread.
    const READ_WAIT: Duration = Duration::from_secs(3);
    /// The prompt waits for the user. This bound ends a prompt that nobody
    /// answers.
    const PROMPT_WAIT: Duration = Duration::from_secs(120);
    /// `UNAuthorizationOptionSound | UNAuthorizationOptionAlert`.
    const OPTIONS: usize = (1 << 1) | (1 << 2);

    /// Returns `(authorizationStatus, alertStyle)`, or `None` without an answer.
    pub(super) fn read_settings() -> Option<(isize, isize)> {
        let class = AnyClass::get(c"UNUserNotificationCenter")?;
        let (sender, receiver) = mpsc::channel();
        let handler = RcBlock::new(move |settings: *mut AnyObject| {
            let values = if settings.is_null() {
                None
            } else {
                // SAFETY: `settings` is a live UNNotificationSettings for the
                // length of this call. Both messages exist on it and return
                // an NSInteger.
                unsafe {
                    let authorization: isize = msg_send![settings, authorizationStatus];
                    let alert_style: isize = msg_send![settings, alertStyle];
                    Some((authorization, alert_style))
                }
            };
            let _ = sender.send(values);
        });
        // SAFETY: The caller checked that the process runs from an app
        // bundle, so the shared center exists. The message takes one block.
        unsafe {
            let center: *mut AnyObject = msg_send![class, currentNotificationCenter];
            if center.is_null() {
                return None;
            }
            let _: () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*handler];
        }
        receiver.recv_timeout(READ_WAIT).ok().flatten()
    }

    /// Shows the macOS prompt and waits for the answer.
    pub(super) fn request_authorization() {
        let Some(class) = AnyClass::get(c"UNUserNotificationCenter") else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let handler = RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
            let _ = sender.send(granted.as_bool());
        });
        // SAFETY: The caller checked that the process runs from an app
        // bundle. The message takes an NSUInteger and one block.
        unsafe {
            let center: *mut AnyObject = msg_send![class, currentNotificationCenter];
            if center.is_null() {
                return;
            }
            let _: () = msg_send![
                center,
                requestAuthorizationWithOptions: OPTIONS,
                completionHandler: &*handler
            ];
        }
        let _ = receiver.recv_timeout(PROMPT_WAIT);
    }
}

/// Reads the state. It blocks for a few milliseconds, so call it off the
/// main thread.
pub(crate) fn current_state() -> NotificationPermissionState {
    #[cfg(target_os = "macos")]
    {
        let bundled = std::env::current_exe()
            .map(|executable| runs_from_app_bundle(&executable))
            .unwrap_or(false);
        if bundled {
            return native::read_settings()
                .map(|(authorization, alert_style)| state_from_settings(authorization, alert_style))
                .unwrap_or(NotificationPermissionState::Unknown);
        }
    }
    NotificationPermissionState::Unknown
}

/// Shows the macOS prompt when macOS has not decided yet. Any other state
/// stays as it is, because macOS shows the prompt only once.
fn request_state() -> NotificationPermissionState {
    let state = current_state();
    #[cfg(target_os = "macos")]
    if state == NotificationPermissionState::NotDetermined {
        native::request_authorization();
        return current_state();
    }
    state
}

#[tauri::command]
pub async fn notification_permission_state() -> NotificationPermissionState {
    tauri::async_runtime::spawn_blocking(current_state)
        .await
        .unwrap_or(NotificationPermissionState::Unknown)
}

#[tauri::command]
pub async fn notification_request_permission() -> NotificationPermissionState {
    tauri::async_runtime::spawn_blocking(request_state)
        .await
        .unwrap_or(NotificationPermissionState::Unknown)
}

/// Opens the notification settings of Halv in System Settings. A dev build
/// posts as Terminal, so its link names Terminal.
#[tauri::command]
pub fn notification_open_system_settings(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let identifier = if tauri::is_dev() {
            "com.apple.Terminal".to_string()
        } else {
            app.config().identifier.clone()
        };
        let status = std::process::Command::new("/usr/bin/open")
            .arg(settings_url(&identifier))
            .status()
            .map_err(|error| format!("could not open System Settings: {error}"))?;
        if status.success() {
            return Ok(());
        }
        Err(format!("System Settings did not open ({status})"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&app, settings_url);
        Err("System notification settings open only on macOS.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_denied_authorization_reads_as_denied_whatever_the_alert_style() {
        for style in 0..=2 {
            assert_eq!(
                state_from_settings(1, style),
                NotificationPermissionState::Denied
            );
        }
    }

    #[test]
    fn an_authorized_app_with_alert_style_none_shows_no_banner() {
        assert_eq!(
            state_from_settings(2, 0),
            NotificationPermissionState::BannersOff
        );
        assert_eq!(
            state_from_settings(2, 1),
            NotificationPermissionState::Granted
        );
        assert_eq!(
            state_from_settings(2, 2),
            NotificationPermissionState::Granted
        );
    }

    #[test]
    fn an_undecided_or_new_status_never_reads_as_granted() {
        assert_eq!(
            state_from_settings(0, 1),
            NotificationPermissionState::NotDetermined
        );
        assert_eq!(
            state_from_settings(99, 1),
            NotificationPermissionState::Unknown
        );
        assert_eq!(
            state_from_settings(-1, 1),
            NotificationPermissionState::Unknown
        );
    }

    #[test]
    fn provisional_and_ephemeral_authorization_deliver() {
        assert_eq!(
            state_from_settings(3, 1),
            NotificationPermissionState::Granted
        );
        assert_eq!(
            state_from_settings(4, 1),
            NotificationPermissionState::Granted
        );
    }

    #[test]
    fn the_state_names_match_the_interface() {
        let names = [
            (NotificationPermissionState::Granted, "granted"),
            (NotificationPermissionState::Denied, "denied"),
            (NotificationPermissionState::NotDetermined, "notDetermined"),
            (NotificationPermissionState::BannersOff, "bannersOff"),
            (NotificationPermissionState::Unknown, "unknown"),
        ];
        for (state, name) in names {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
        }
    }

    #[test]
    fn only_a_process_inside_an_app_bundle_may_call_the_framework() {
        let bundled = PathBuf::from("/Applications/Halv.app/Contents/MacOS/halv");
        let dev = PathBuf::from("/Users/dev/halv/src-tauri/target/debug/halv");
        let app_named_file = PathBuf::from("/Users/dev/tools/halv.app");
        assert!(runs_from_app_bundle(&bundled));
        assert!(!runs_from_app_bundle(&dev));
        // A binary that is itself named x.app is not inside a bundle.
        assert!(!runs_from_app_bundle(&app_named_file));
    }

    #[test]
    fn a_process_outside_a_bundle_reports_unknown() {
        // The test binary runs from target/, so no framework call happens.
        assert_eq!(current_state(), NotificationPermissionState::Unknown);
    }

    #[test]
    fn the_settings_link_names_the_app() {
        assert_eq!(
            settings_url("ai.halv.app"),
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=ai.halv.app"
        );
        assert_eq!(settings_url(""), SETTINGS_PANE);
        assert_eq!(settings_url("bad id&x=1"), SETTINGS_PANE);
    }
}
