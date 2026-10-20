use tauri::{LogicalPosition, LogicalSize, Manager, Webview, WebviewBuilder, WebviewUrl};

const DOT_LABEL: &str = "dot-remote";
const DOT_URL: &str = "https://chatgpt.com/dots";
static VIEW_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn check_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() != "main" {
        return Err("Only the Halv interface can control the Dot panel".into());
    }
    Ok(())
}

fn valid_bounds(x: f64, y: f64, width: f64, height: f64) -> bool {
    [x, y, width, height].iter().all(|n| n.is_finite())
        && x >= 0.0
        && y >= 0.0
        && width >= 1.0
        && height >= 1.0
        && [x, y, width, height].iter().all(|n| *n < 32768.0)
}

// Remote content has no Tauri capability. Never load app/local origins here.
fn remote_navigation(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.host_str() != Some("tauri.localhost")
        && url.host_str() != Some("localhost")
        && url.username().is_empty()
        && url.password().is_none()
}

#[tauri::command]
pub async fn dot_set_view(
    app: tauri::AppHandle,
    webview: Webview,
    visible: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    check_caller(&webview)?;
    let _guard = VIEW_LOCK.lock().await;
    if !visible {
        if let Some(view) = app.get_webview(DOT_LABEL) {
            view.hide().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if !valid_bounds(x, y, width, height) {
        return Err("Invalid Dot panel bounds".into());
    }
    let position = LogicalPosition::new(x, y);
    let size = LogicalSize::new(width, height);
    let view = if let Some(view) = app.get_webview(DOT_LABEL) {
        view
    } else {
        let window = app.get_window("main").ok_or("Halv window is unavailable")?;
        let popup_app = app.clone();
        let builder =
            WebviewBuilder::new(DOT_LABEL, WebviewUrl::External(DOT_URL.parse().unwrap()))
                .on_navigation(remote_navigation)
                .on_new_window(move |url, _| {
                    if remote_navigation(&url) {
                        if let Some(view) = popup_app.get_webview(DOT_LABEL) {
                            let _ = view.navigate(url);
                        }
                    }
                    tauri::webview::NewWindowResponse::Deny
                });
        window
            .add_child(builder, position, size)
            .map_err(|e| e.to_string())?
    };
    view.set_position(position).map_err(|e| e.to_string())?;
    view.set_size(size).map_err(|e| e.to_string())?;
    view.show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn dot_reload(app: tauri::AppHandle, webview: Webview) -> Result<(), String> {
    check_caller(&webview)?;
    let _guard = VIEW_LOCK.lock().await;
    app.get_webview(DOT_LABEL)
        .ok_or("Dot is not open")?
        .navigate(DOT_URL.parse().unwrap())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_panel_never_navigates_to_privileged_origins() {
        for url in [
            "file:///etc/passwd",
            "tauri://localhost",
            "http://localhost:1420",
            "https://tauri.localhost",
            "javascript:alert(1)",
            "https://u:p@chatgpt.com",
        ] {
            assert!(!remote_navigation(&url.parse().unwrap()), "{url}");
        }
        assert!(remote_navigation(&DOT_URL.parse().unwrap()));
        assert!(remote_navigation(
            &"https://auth.openai.com/".parse().unwrap()
        ));
    }
    #[test]
    fn bounds_reject_non_finite_negative_or_zero_rectangles() {
        assert!(valid_bounds(260.0, 140.0, 940.0, 600.0));
        assert!(!valid_bounds(f64::NAN, 0.0, 10.0, 10.0));
        assert!(!valid_bounds(0.0, -1.0, 10.0, 10.0));
        assert!(!valid_bounds(0.0, 0.0, 0.0, 10.0));
    }
}
