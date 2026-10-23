//! OS notifications for terminal panes, with click to focus.
//!
//! The notification plugin shows a banner but drops it, so the app never
//! learns about a click. This command shows the banner itself and keeps a
//! thread that waits for the click. A click brings the window forward and
//! emits `terminal-notification-clicked` with the session id. Each waiting
//! banner holds one thread until the user clicks or clears it, and a banner
//! stays in Notification Center after it hides. A cap limits the waiting
//! threads. On macOS, a full cap removes the oldest delivered banners of the
//! app, so their waits end and new banners keep the click action. Elsewhere
//! a banner above the cap shows without a click action.
//!
//! When macOS denies Halv, nobody can click a banner, so a denied banner
//! takes no waiting thread. Each banner adds one line to `notifications.log`.

use crate::notification_permission::NotificationPermissionState;
use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::AppHandle;

pub(crate) const NOTIFICATION_CLICKED_EVENT: &str = "terminal-notification-clicked";
const MAX_WAITING_NOTIFICATIONS: usize = 16;
/// A trim keeps this many delivered banners, so a few slots come free.
#[cfg(target_os = "macos")]
const BANNERS_AFTER_TRIM: usize = MAX_WAITING_NOTIFICATIONS - 4;
/// A new banner waits this long for a slot after a trim. The notification
/// library sees a removed banner within 0.5 s.
#[cfg(target_os = "macos")]
const SLOT_WAIT: std::time::Duration = std::time::Duration::from_millis(1_500);
const MAX_TITLE_CHARS: usize = 120;
const MAX_BODY_CHARS: usize = 360;

#[cfg_attr(not(desktop), allow(dead_code))]
static WAITERS: WaiterSlots = WaiterSlots::new(MAX_WAITING_NOTIFICATIONS);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationClicked {
    session_id: String,
}

/// Counts the threads that wait for a click.
pub(crate) struct WaiterSlots {
    used: AtomicUsize,
    capacity: usize,
}

/// One waiting thread. Drop releases the slot.
pub(crate) struct WaiterSlot<'a> {
    slots: &'a WaiterSlots,
}

impl WaiterSlots {
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            capacity,
        }
    }

    pub(crate) fn try_acquire(&self) -> Option<WaiterSlot<'_>> {
        let mut used = self.used.load(Ordering::SeqCst);
        loop {
            if used >= self.capacity {
                return None;
            }
            // Keep the declared MSRV without using the deprecated fetch_update API.
            match self.used.compare_exchange_weak(
                used,
                used + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(WaiterSlot { slots: self }),
                Err(current) => used = current,
            }
        }
    }
}

impl Drop for WaiterSlot<'_> {
    fn drop(&mut self) {
        self.slots.used.fetch_sub(1, Ordering::SeqCst);
    }
}

fn clip(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut clipped = text
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    clipped.push('…');
    clipped
}

/// What a click on a banner announces. A banner without a pane, such as the
/// test banner of Settings, only brings the window forward.
#[cfg_attr(not(desktop), allow(dead_code))]
fn click_announcement(session_id: String) -> Option<NotificationClicked> {
    (!session_id.is_empty()).then_some(NotificationClicked { session_id })
}

/// True when a click on the banner can reach Halv. A denied banner stays
/// hidden, so a thread that waits for its click would wait for ever.
#[cfg_attr(any(windows, not(desktop)), allow(dead_code))]
fn waits_for_click(state: NotificationPermissionState) -> bool {
    state != NotificationPermissionState::Denied
}

/// Writes the line of one banner to the log. A failed write only costs a line.
#[cfg(desktop)]
fn record_banner(
    app: &AppHandle,
    session_id: &str,
    kind: Option<&str>,
    trace: Option<&str>,
    state: NotificationPermissionState,
) {
    let state = serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let line = crate::notification_log::banner_line(
        &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        session_id,
        kind,
        trace,
        &state,
    );
    let written = crate::app_data::resolved_app_data_dir(app).and_then(|dir| {
        crate::notification_log::append(&dir.join("logs"), &line).map_err(|error| error.to_string())
    });
    if let Err(error) = written {
        eprintln!("agent status: could not log a notification: {error}");
    }
}

/// Shows an OS notification for a terminal pane. A click on it focuses the
/// app and names the pane in `terminal-notification-clicked`. `kind` and
/// `trace` name the reason of the banner for the log.
#[tauri::command]
pub fn terminal_notify(
    app: AppHandle,
    session_id: String,
    title: String,
    body: String,
    kind: Option<String>,
    trace: Option<String>,
) -> Result<(), String> {
    let title = clip(&title, MAX_TITLE_CHARS);
    if title.is_empty() {
        return Err("the notification needs a title".to_string());
    }
    deliver(
        app,
        session_id,
        title,
        clip(&body, MAX_BODY_CHARS),
        kind,
        trace,
    );
    Ok(())
}

#[cfg(desktop)]
fn clicked(app: &AppHandle, session_id: String) {
    use tauri::Emitter;
    if let Err(error) = crate::tray::show_main_window(app) {
        eprintln!("agent status: could not show the window: {error}");
    }
    let Some(announcement) = click_announcement(session_id) else {
        return;
    };
    if let Err(error) = app.emit(NOTIFICATION_CLICKED_EVENT, announcement) {
        eprintln!("agent status: could not announce a notification click: {error}");
    }
}

#[cfg(all(desktop, not(windows)))]
fn deliver(
    app: AppHandle,
    session_id: String,
    title: String,
    body: String,
    kind: Option<String>,
    trace: Option<String>,
) {
    #[cfg(target_os = "macos")]
    {
        // The plugin does the same. A dev build has no bundle, so it posts
        // as Terminal, and a click there opens Terminal, not Halv.
        let identifier = app.config().identifier.clone();
        let _ = notify_rust::set_application(if tauri::is_dev() {
            "com.apple.Terminal"
        } else {
            &identifier
        });
    }
    let mut notification = notify_rust::Notification::new();
    notification.summary(&title).body(&body);
    #[cfg(not(target_os = "macos"))]
    notification.action("default", "default");
    std::thread::spawn(move || {
        // The read waits a few milliseconds, so it runs on this thread.
        let state = crate::notification_permission::current_state();
        record_banner(&app, &session_id, kind.as_deref(), trace.as_deref(), state);
        let slot = if waits_for_click(state) {
            WAITERS.try_acquire().or_else(|| free_a_slot(&app))
        } else {
            None
        };
        let handle = match notification.show() {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("agent status: could not show a notification: {error}");
                return;
            }
        };
        if slot.is_none() {
            // Dropping the handle sends the banner without a click wait.
            return;
        }
        handle.wait_for_action(|action| {
            if action == "default" {
                clicked(&app, session_id);
            }
        });
        drop(slot);
    });
}

/// Removes the oldest delivered banners and waits for a free slot.
#[cfg(target_os = "macos")]
fn free_a_slot(app: &AppHandle) -> Option<WaiterSlot<'static>> {
    // The notification center belongs to the main thread.
    if app
        .run_on_main_thread(|| trim_delivered_banners(BANNERS_AFTER_TRIM))
        .is_err()
    {
        return None;
    }
    let deadline = std::time::Instant::now() + SLOT_WAIT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(slot) = WAITERS.try_acquire() {
            return Some(slot);
        }
    }
    None
}

#[cfg(all(desktop, not(windows), not(target_os = "macos")))]
fn free_a_slot(_app: &AppHandle) -> Option<WaiterSlot<'static>> {
    None
}

/// Removes the oldest delivered banners of the app until `keep` remain. A
/// removed banner ends the click wait of its thread.
#[cfg(target_os = "macos")]
fn trim_delivered_banners(keep: usize) {
    use objc2::{
        msg_send,
        rc::autoreleasepool,
        runtime::{AnyClass, AnyObject},
    };
    let Some(class) = AnyClass::get(c"NSUserNotificationCenter") else {
        return;
    };
    autoreleasepool(|_| {
        // SAFETY: Each message exists on these AppKit classes, with these
        // argument and return types. The array and its banners stay alive
        // in this autorelease pool.
        unsafe {
            let center: *mut AnyObject = msg_send![class, defaultUserNotificationCenter];
            if center.is_null() {
                return;
            }
            let delivered: *mut AnyObject = msg_send![center, deliveredNotifications];
            if delivered.is_null() {
                return;
            }
            let count: usize = msg_send![delivered, count];
            if count <= keep {
                return;
            }
            let mut banners = (0..count)
                .map(|index| {
                    let banner: *mut AnyObject = msg_send![delivered, objectAtIndex: index];
                    let date: *mut AnyObject = msg_send![banner, actualDeliveryDate];
                    let at: f64 = if date.is_null() {
                        0.0
                    } else {
                        msg_send![date, timeIntervalSince1970]
                    };
                    (at, banner)
                })
                .collect::<Vec<_>>();
            banners.sort_by(|left, right| left.0.total_cmp(&right.0));
            for (_, banner) in banners.into_iter().take(count - keep) {
                let _: () = msg_send![center, removeDeliveredNotification: banner];
            }
        }
    });
}

#[cfg(all(desktop, windows))]
fn deliver(
    app: AppHandle,
    session_id: String,
    title: String,
    body: String,
    kind: Option<String>,
    trace: Option<String>,
) {
    std::thread::spawn(move || {
        record_banner(
            &app,
            &session_id,
            kind.as_deref(),
            trace.as_deref(),
            NotificationPermissionState::Unknown,
        );
        if let Err(error) = notify_rust::Notification::new()
            .summary(&title)
            .body(&body)
            .show()
        {
            eprintln!("agent status: could not show a notification: {error}");
        }
    });
}

#[cfg(not(desktop))]
fn deliver(
    _app: AppHandle,
    _session_id: String,
    _title: String,
    _body: String,
    _kind: Option<String>,
    _trace: Option<String>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_waiter_cap_sends_extra_banners_without_a_wait() {
        let slots = WaiterSlots::new(2);
        let first = slots.try_acquire();
        let second = slots.try_acquire();
        assert!(first.is_some());
        assert!(second.is_some());
        assert!(slots.try_acquire().is_none());
        drop(first);
        let third = slots.try_acquire();
        assert!(third.is_some());
        assert!(slots.try_acquire().is_none());
        drop(second);
        drop(third);
        assert_eq!(slots.used.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn long_text_is_clipped_on_a_character_boundary() {
        assert_eq!(clip("  short  ", 10), "short");
        assert_eq!(clip("ééééé", 3), "éé…");
        assert_eq!(
            clip(&"x".repeat(500), MAX_BODY_CHARS).chars().count(),
            MAX_BODY_CHARS
        );
    }

    #[test]
    fn a_denied_banner_takes_no_waiting_thread() {
        assert!(!waits_for_click(NotificationPermissionState::Denied));
        for state in [
            NotificationPermissionState::Granted,
            NotificationPermissionState::NotDetermined,
            NotificationPermissionState::BannersOff,
            NotificationPermissionState::Unknown,
        ] {
            assert!(waits_for_click(state));
        }
    }

    #[test]
    fn a_banner_without_a_pane_announces_no_click() {
        assert!(click_announcement(String::new()).is_none());
        let announcement = click_announcement("terminal-1".to_string()).unwrap();
        assert_eq!(announcement.session_id, "terminal-1");
    }

    #[test]
    fn the_click_event_names_the_session() {
        let payload = serde_json::to_value(NotificationClicked {
            session_id: "terminal-1".to_string(),
        })
        .unwrap();
        assert_eq!(payload, serde_json::json!({ "sessionId": "terminal-1" }));
    }
}
