use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{
    menu::{MenuBuilder, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

pub const USAGE_EVENT: &str = "halv://tray/usage";
pub const NEW_CHAT_EVENT: &str = "halv://tray/new-chat";

const TRAY_ID: &str = "halv-tray";
const PANEL_ID: &str = "tray";
const SNAPSHOT_EVENT: &str = "halv://tray/snapshot";
const PANEL_WIDTH: f64 = 376.0;

#[derive(Default)]
struct PanelState {
    anchor: Mutex<Option<tauri::Rect>>,
    hidden_at: Mutex<Option<Instant>>,
}
const NEW_CHAT_MENU_ID: &str = "tray.new-chat";
const SHOW_MENU_ID: &str = "tray.show";
const QUIT_MENU_ID: &str = "tray.quit";
const QUIT_STOP_AGENTS_MENU_ID: &str = "tray.quit-stop-agents";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    NewChat,
    Usage,
    Show,
    Quit,
    QuitAndStopAgents,
}

const METRIC_IDS: [&str; 7] = [
    "tray.today",
    "tray.total",
    "tray.claude.short",
    "tray.claude.week",
    "tray.codex.short",
    "tray.codex.week",
    "tray.checked",
];

struct TrayState {
    items: Vec<MenuItem<tauri::Wry>>,
    labels: Mutex<BTreeMap<String, String>>,
    snapshot: Mutex<TraySnapshot>,
}

#[derive(Clone, Default, Serialize)]
pub struct TraySnapshot {
    today: Option<i64>,
    total: Option<i64>,
    /// Dollars saved with the methods of ADR 0015.
    today_usd: Option<f64>,
    total_usd: Option<f64>,
    windows: Vec<QuotaWindow>,
    checked_at: Option<chrono::DateTime<chrono::Local>>,
}

#[derive(Clone, Serialize)]
struct QuotaWindow {
    provider: &'static str,
    label: String,
    used: Option<f64>,
    reset: Option<String>,
}

fn default_labels() -> BTreeMap<String, String> {
    [
        ("today", "Saved today: {amount}"),
        ("total", "Saved all time: {amount}"),
        (
            "quota",
            "{provider} {window}: {percent}% left · resets in {reset}",
        ),
        ("unavailable", "{provider} {window}: unavailable"),
        ("checked", "Last checked: {time}"),
        ("loading", "Loading usage…"),
        ("newChat", "New chat"),
        ("show", "Show Halv"),
        ("quit", "Quit Halv"),
        ("stop", "Quit and Stop Agents"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

pub fn install(app: &mut App) -> tauri::Result<()> {
    app.manage(PanelState::default());
    install_panel(app)?;
    let mut items = Vec::new();
    for id in METRIC_IDS {
        items.push(MenuItem::with_id(
            app,
            id,
            "Loading usage…",
            id != "tray.checked",
            None::<&str>,
        )?);
    }
    for (id, label) in [
        (NEW_CHAT_MENU_ID, "New chat"),
        (SHOW_MENU_ID, "Show Halv"),
        (QUIT_MENU_ID, "Quit Halv"),
        (QUIT_STOP_AGENTS_MENU_ID, "Quit and Stop Agents"),
    ] {
        items.push(MenuItem::with_id(app, id, label, true, None::<&str>)?);
    }
    let menu = MenuBuilder::new(app)
        .item(&items[0])
        .item(&items[1])
        .separator()
        .item(&items[2])
        .item(&items[3])
        .item(&items[4])
        .item(&items[5])
        .item(&items[6])
        .separator()
        .item(&items[7])
        .item(&items[8])
        .separator()
        .item(&items[9])
        .item(&items[10])
        .build()?;
    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "linux"))
        .icon(tauri::include_image!("./icons/tray/32x32.png"))
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Halv")
        .on_menu_event(|app, event| handle_menu_action(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                if let Err(error) = toggle_panel(tray.app_handle(), rect) {
                    eprintln!("failed to open tray panel: {error}");
                }
            }
        })
        .build(app)?;
    app.manage(TrayState {
        items,
        labels: Mutex::new(default_labels()),
        snapshot: Mutex::new(TraySnapshot::default()),
    });
    render(app.handle());
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        loop {
            // Native polling continues with the main window hidden. Existing provider
            // readers share their 60-second cache with the in-app usage meter.
            let (claude, codex) = tokio::join!(
                crate::agent_sessions::agent_claude_usage_windows(),
                crate::agent_sessions::agent_codex_usage_windows()
            );
            let db_handle = handle.clone();
            let savings = tauri::async_runtime::spawn_blocking(move || {
                let storage = db_handle.state::<crate::storage::Storage>();
                let today = storage.savings_today().ok();
                let total = storage.total_savings().ok();
                (
                    today.as_ref().map(|v| v.tokens_saved),
                    total.as_ref().map(|v| v.tokens_saved),
                    today.map(|v| v.dollars_usd),
                    total.map(|v| v.dollars_usd),
                )
            })
            .await
            .unwrap_or((None, None, None, None));
            let mut windows = Vec::new();
            if claude.unavailable_reason.is_none() {
                windows.extend(claude.windows.into_iter().map(|v| QuotaWindow {
                    provider: "Claude",
                    label: v.label,
                    used: v.used_percent,
                    reset: v.resets_at,
                }));
            }
            windows.extend(codex.windows.into_iter().take(2).map(|v| QuotaWindow {
                provider: "Codex",
                label: v.label,
                used: v.used_percent,
                reset: v.resets_at,
            }));
            *handle
                .state::<TrayState>()
                .snapshot
                .lock()
                .expect("tray snapshot lock poisoned") = TraySnapshot {
                today: savings.0,
                total: savings.1,
                today_usd: savings.2,
                total_usd: savings.3,
                windows,
                checked_at: Some(chrono::Local::now()),
            };
            render(&handle);
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    Ok(())
}

#[tauri::command]
pub fn tray_set_labels(app: AppHandle, labels: BTreeMap<String, String>) -> Result<(), String> {
    let defaults = default_labels();
    if labels.len() != defaults.len()
        || labels.iter().any(|(key, value)| {
            !defaults.contains_key(key) || value.is_empty() || value.len() > 1000
        })
    {
        return Err("invalid tray labels".into());
    }
    *app.state::<TrayState>()
        .labels
        .lock()
        .map_err(|e| e.to_string())? = labels;
    render(&app);
    Ok(())
}

/// The compact dollar form of the app (src/format.ts `formatUsdCompact`),
/// in the US English number format: $0, <$0.01, $0.42, $12.3, $123, $1.2K.
pub(crate) fn format_usd_compact(value: Option<f64>) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "—".into();
    };
    let sign = if value < 0.0 { "-" } else { "" };
    let amount = value.abs();
    let trim = |text: String| text.strip_suffix(".0").map(str::to_string).unwrap_or(text);
    let body = if amount == 0.0 {
        "$0".to_string()
    } else if amount < 0.01 {
        "<$0.01".to_string()
    } else if amount < 9.995 {
        format!("${amount:.2}")
    } else if amount < 99.95 {
        trim(format!("${amount:.1}"))
    } else if amount < 999.5 {
        format!("${amount:.0}")
    } else if amount < 999_950.0 {
        format!("{}K", trim(format!("${:.1}", amount / 1_000.0)))
    } else if amount < 999_950_000.0 {
        format!("{}M", trim(format!("${:.1}", amount / 1_000_000.0)))
    } else {
        format!("{}B", trim(format!("${:.1}", amount / 1_000_000_000.0)))
    };
    format!("{sign}{body}")
}

#[cfg_attr(not(test), allow(dead_code))]
fn format_tokens(value: Option<i64>) -> String {
    match value {
        None => "—".into(),
        Some(v) if v.unsigned_abs() >= 1_000_000 => format!("{:.1}M", v as f64 / 1_000_000.0),
        Some(v) if v.unsigned_abs() >= 1_000 => format!("{:.1}k", v as f64 / 1_000.0),
        Some(v) => v.to_string(),
    }
}

fn quota_text(window: &QuotaWindow, labels: &BTreeMap<String, String>, now: i64) -> String {
    let reset = window
        .reset
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.timestamp());
    let valid = window
        .used
        .filter(|v| v.is_finite())
        .zip(reset)
        .filter(|(_, reset)| *reset > now);
    let text = if let Some((used, reset)) = valid {
        // Keep nonzero remainders below 1% visible without rounding up to 100%.
        let remaining = (100.0 - used).clamp(0.0, 100.0);
        let percent = if remaining > 0.0 && remaining < 1.0 {
            "<1".into()
        } else {
            format!("{:.0}", remaining.floor())
        };
        let minutes = ((reset - now) as u64).div_ceil(60);
        let duration = if minutes >= 1440 {
            format!("{}d {}h", minutes / 1440, minutes % 1440 / 60)
        } else if minutes >= 60 {
            format!("{}h {}m", minutes / 60, minutes % 60)
        } else {
            format!("{minutes}m")
        };
        labels["quota"]
            .replace("{percent}", &percent)
            .replace("{reset}", &duration)
    } else {
        labels["unavailable"].clone()
    };
    text.replace("{provider}", window.provider)
        .replace("{window}", &window.label)
}

fn find_quota_window<'a>(
    windows: &'a [QuotaWindow],
    provider: &str,
    index: usize,
    label: &str,
) -> Option<&'a QuotaWindow> {
    let canonical = provider == "Claude"
        || windows
            .iter()
            .filter(|window| window.provider == provider)
            .all(|window| window.label == "5h" || window.label == "7d");
    if canonical {
        windows
            .iter()
            .find(|window| window.provider == provider && window.label == label)
    } else {
        windows
            .iter()
            .filter(|window| window.provider == provider)
            .nth(index)
    }
}

fn render(app: &AppHandle) {
    let state = app.state::<TrayState>();
    let labels = state
        .labels
        .lock()
        .expect("tray labels lock poisoned")
        .clone();
    let snapshot = state
        .snapshot
        .lock()
        .expect("tray snapshot lock poisoned")
        .clone();
    let now = chrono::Utc::now().timestamp();
    let fresh = snapshot
        .checked_at
        .is_some_and(|checked| now - checked.timestamp() < 120);
    let mut lines = vec![
        labels["today"].replace(
            "{amount}",
            &format_usd_compact(if fresh { snapshot.today_usd } else { None }),
        ),
        labels["total"].replace(
            "{amount}",
            &format_usd_compact(if fresh { snapshot.total_usd } else { None }),
        ),
    ];
    for (provider, index, label) in [
        ("Claude", 0, "5h"),
        ("Claude", 1, "7d"),
        ("Codex", 0, "5h"),
        ("Codex", 1, "7d"),
    ] {
        let placeholder = QuotaWindow {
            provider,
            label: label.into(),
            used: None,
            reset: None,
        };
        let window = if fresh {
            find_quota_window(&snapshot.windows, provider, index, label).unwrap_or(&placeholder)
        } else {
            &placeholder
        };
        lines.push(quota_text(window, &labels, now));
    }
    lines.push(
        snapshot
            .checked_at
            .map(|time| labels["checked"].replace("{time}", &time.format("%H:%M").to_string()))
            .unwrap_or_else(|| labels["loading"].clone()),
    );
    for key in ["newChat", "show", "quit", "stop"] {
        lines.push(labels[key].clone());
    }
    let _ = app.emit_to(PANEL_ID, SNAPSHOT_EVENT, &snapshot);
    for (item, text) in state.items.iter().zip(&lines) {
        if let Err(error) = item.set_text(text) {
            eprintln!("failed to update tray: {error}");
        }
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(format!("Halv\n{}\n{}", lines[0], lines[1])));
    }
}

fn action_for_menu_id(menu_id: &str) -> Option<TrayAction> {
    if menu_id == "tray.usage" || METRIC_IDS[..6].contains(&menu_id) {
        return Some(TrayAction::Usage);
    }
    match menu_id {
        NEW_CHAT_MENU_ID => Some(TrayAction::NewChat),
        SHOW_MENU_ID => Some(TrayAction::Show),
        QUIT_MENU_ID => Some(TrayAction::Quit),
        QUIT_STOP_AGENTS_MENU_ID => Some(TrayAction::QuitAndStopAgents),
        _ => None,
    }
}

fn handle_menu_action<R: Runtime>(app: &AppHandle<R>, menu_id: &str) {
    if let Err(error) = perform_action(app, menu_id) {
        eprintln!("failed to run tray action: {error}");
    }
}

fn perform_action<R: Runtime>(app: &AppHandle<R>, menu_id: &str) -> Result<(), String> {
    match action_for_menu_id(menu_id) {
        Some(TrayAction::Usage) => {
            show_main_window(app)?;
            app.emit_to("main", USAGE_EVENT, ())
                .map_err(|e| e.to_string())?;
        }
        Some(TrayAction::NewChat) => {
            show_main_window(app)?;
            app.emit_to("main", NEW_CHAT_EVENT, ())
                .map_err(|e| e.to_string())?;
        }
        Some(TrayAction::Show) => show_main_window(app)?,
        Some(TrayAction::Quit) => app.exit(0),
        Some(TrayAction::QuitAndStopAgents) => {
            app.state::<crate::terminal::TerminalManager>()
                .stop_all_supervised()?;
            app.exit(0);
        }
        None => return Err("unknown tray action".into()),
    }
    Ok(())
}

fn install_panel(app: &App) -> tauri::Result<()> {
    let window = match app.get_webview_window(PANEL_ID) {
        Some(window) => window,
        None => WebviewWindowBuilder::new(app, PANEL_ID, WebviewUrl::App("tray.html".into()))
            .title("Halv · Usage")
            .inner_size(PANEL_WIDTH, 560.0)
            .visible(false)
            .focused(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .build()?,
    };
    let handle = app.handle().clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Focused(false) => {
            if let Some(window) = handle.get_webview_window(PANEL_ID) {
                if window.is_visible().unwrap_or(false) {
                    let _ = window.hide();
                    *handle.state::<PanelState>().hidden_at.lock().unwrap() = Some(Instant::now());
                }
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            if let Some(window) = handle.get_webview_window(PANEL_ID) {
                let _ = window.hide();
            }
        }
        _ => {}
    });
    Ok(())
}

fn toggle_panel(app: &AppHandle, anchor: tauri::Rect) -> Result<(), String> {
    let window = app
        .get_webview_window(PANEL_ID)
        .ok_or("tray panel unavailable")?;
    if window.is_visible().map_err(|e| e.to_string())? {
        return window.hide().map_err(|e| e.to_string());
    }
    // The status item can remove focus before it delivers the click.
    if app
        .state::<PanelState>()
        .hidden_at
        .lock()
        .unwrap()
        .is_some_and(|time| time.elapsed() < Duration::from_millis(200))
    {
        return Ok(());
    }
    *app.state::<PanelState>().anchor.lock().unwrap() = Some(anchor);
    position_panel(app, &window)?;
    render(app);
    app.emit_to(PANEL_ID, "halv://tray/opened", ())
        .map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelPlacement {
    x: f64,
    y: f64,
    arrow_x: f64,
    below: bool,
}

fn panel_placement(anchor: [f64; 4], area: [f64; 4], size: [f64; 2]) -> PanelPlacement {
    let [ax, ay, aw, ah] = anchor;
    let [left, top, width, height] = area;
    let [pw, ph] = size;
    let center = ax + aw / 2.0;
    let x = (center - pw / 2.0).clamp(left, (left + width - pw).max(left));
    let below = ay + ah / 2.0 < top + height / 2.0;
    let y = if below { ay + ah } else { ay - ph };
    PanelPlacement {
        x,
        y: y.clamp(top, (top + height - ph).max(top)),
        arrow_x: (center - x).clamp(28.0, pw - 28.0),
        below,
    }
}

fn position_panel(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let Some(anchor) = *app.state::<PanelState>().anchor.lock().unwrap() else {
        return Ok(());
    };
    let physical = anchor
        .position
        .to_physical::<f64>(window.scale_factor().map_err(|e| e.to_string())?);
    let monitor = app
        .monitor_from_point(physical.x, physical.y)
        .map_err(|e| e.to_string())?
        .or(window.current_monitor().map_err(|e| e.to_string())?)
        .ok_or("tray monitor unavailable")?;
    let scale = monitor.scale_factor();
    let position = anchor.position.to_logical::<f64>(scale);
    let size = anchor.size.to_logical::<f64>(scale);
    let area = monitor.work_area();
    let origin = area.position.to_logical::<f64>(scale);
    let available = area.size.to_logical::<f64>(scale);
    let window_size = window
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(window.scale_factor().map_err(|e| e.to_string())?);
    let placement = panel_placement(
        [position.x, position.y, size.width, size.height],
        [origin.x, origin.y, available.width, available.height],
        [window_size.width, window_size.height],
    );
    window
        .set_position(tauri::PhysicalPosition::new(
            (placement.x * scale).round() as i32,
            (placement.y * scale).round() as i32,
        ))
        .map_err(|e| e.to_string())?;
    app.emit_to(PANEL_ID, "halv://tray/position", placement)
        .map_err(|e| e.to_string())
}

fn require_panel(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == PANEL_ID {
        Ok(())
    } else {
        Err("invalid tray window".into())
    }
}

#[tauri::command]
pub fn tray_get_snapshot(app: AppHandle, window: WebviewWindow) -> Result<TraySnapshot, String> {
    require_panel(&window)?;
    let state = app.try_state::<TrayState>().ok_or("tray is starting")?;
    let snapshot = state.snapshot.lock().map_err(|e| e.to_string())?.clone();
    Ok(snapshot)
}

#[tauri::command]
pub fn tray_action(app: AppHandle, window: WebviewWindow, action: String) -> Result<(), String> {
    require_panel(&window)?;
    if action == "dismiss" {
        return window.hide().map_err(|e| e.to_string());
    }
    perform_action(&app, &action)?;
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn tray_resize(app: AppHandle, window: WebviewWindow, height: f64) -> Result<(), String> {
    require_panel(&window)?;
    if !height.is_finite() {
        return Err("invalid tray height".into());
    }
    let max_height = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .map(|m| m.work_area().size.height as f64 / m.scale_factor())
        .unwrap_or(720.0);
    window
        .set_size(tauri::LogicalSize::new(
            PANEL_WIDTH,
            height.clamp(320.0, 720.0).min(max_height),
        ))
        .map_err(|e| e.to_string())?;
    position_panel(&app, &window)
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    app.show().map_err(|error| error.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_never_invents_capacity_for_missing_expired_or_invalid_windows() {
        let labels = default_labels();
        let now = 1_700_000_000;
        let mut window = QuotaWindow {
            provider: "Codex",
            label: "5h".into(),
            used: None,
            reset: None,
        };
        assert!(quota_text(&window, &labels, now).contains("unavailable"));
        window.used = Some(40.0);
        window.reset = Some("2023-11-14T23:13:20Z".into());
        assert!(quota_text(&window, &labels, now).contains("60% left"));
        assert!(quota_text(&window, &labels, now + 3600).contains("unavailable"));
        window.used = Some(f64::NAN);
        assert!(quota_text(&window, &labels, now).contains("unavailable"));
        window.used = Some(99.5);
        assert!(quota_text(&window, &labels, now).contains("<1% left"));
        window.used = Some(110.0);
        assert!(quota_text(&window, &labels, now).contains("0% left"));
    }

    #[test]
    fn savings_distinguishes_unavailable_from_measured_zero() {
        assert_eq!(format_usd_compact(None), "—");
        assert_eq!(format_usd_compact(Some(0.0)), "$0");
        assert_eq!(format_usd_compact(Some(0.004)), "<$0.01");
        assert_eq!(format_usd_compact(Some(0.42)), "$0.42");
        assert_eq!(format_usd_compact(Some(9.996)), "$10");
        assert_eq!(format_usd_compact(Some(12.34)), "$12.3");
        assert_eq!(format_usd_compact(Some(123.4)), "$123");
        assert_eq!(format_usd_compact(Some(1_234.0)), "$1.2K");
        assert_eq!(format_usd_compact(Some(12_000.0)), "$12K");
        assert_eq!(format_usd_compact(Some(1_260_000.0)), "$1.3M");
        assert_eq!(format_usd_compact(Some(-0.42)), "-$0.42");
        assert_eq!(format_tokens(None), "—");
        assert_eq!(format_tokens(Some(0)), "0");
        assert_eq!(format_tokens(Some(1_250_000)), "1.2M");
        assert_eq!(format_tokens(Some(-1200)), "-1.2k");
        assert!(!format_tokens(Some(i64::MIN)).is_empty());
        assert_eq!(action_for_menu_id("tray.today"), Some(TrayAction::Usage));
        assert_eq!(action_for_menu_id("tray.checked"), None);
    }

    #[test]
    fn weekly_only_quota_does_not_replace_the_session_quota() {
        let windows = vec![QuotaWindow {
            provider: "Codex",
            label: "7d".into(),
            used: Some(59.0),
            reset: None,
        }];
        assert!(find_quota_window(&windows, "Codex", 0, "5h").is_none());
        assert_eq!(
            find_quota_window(&windows, "Codex", 1, "7d").unwrap().used,
            Some(59.0)
        );
        assert!(find_quota_window(&windows, "Claude", 1, "7d").is_none());
    }

    #[test]
    fn panel_stays_inside_each_monitor_and_tracks_the_tray() {
        let panel = panel_placement(
            [1390.0, 0.0, 24.0, 24.0],
            [0.0, 24.0, 1440.0, 876.0],
            [376.0, 536.0],
        );
        assert_eq!(panel.x, 1064.0);
        assert_eq!(panel.y, 24.0);
        assert!(panel.below);
        assert!(panel.arrow_x <= 348.0);
        let left = panel_placement(
            [-1900.0, 0.0, 24.0, 24.0],
            [-1920.0, 24.0, 1920.0, 1056.0],
            [376.0, 536.0],
        );
        assert_eq!(left.x, -1920.0);
        let bottom = panel_placement(
            [900.0, 1040.0, 24.0, 40.0],
            [0.0, 0.0, 1920.0, 1040.0],
            [376.0, 536.0],
        );
        assert!(!bottom.below);
        assert_eq!(bottom.y, 504.0);
    }

    #[test]
    fn menu_ids_map_only_to_supported_tray_actions() {
        assert_eq!(
            action_for_menu_id(NEW_CHAT_MENU_ID),
            Some(TrayAction::NewChat)
        );
        assert_eq!(action_for_menu_id(SHOW_MENU_ID), Some(TrayAction::Show));
        assert_eq!(action_for_menu_id(QUIT_MENU_ID), Some(TrayAction::Quit));
        assert_eq!(
            action_for_menu_id(QUIT_STOP_AGENTS_MENU_ID),
            Some(TrayAction::QuitAndStopAgents)
        );
        assert_eq!(action_for_menu_id("tray.unknown"), None);
    }
}
