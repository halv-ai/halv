//! Navigation guard for the main Halv webview.
//!
//! Tauri sends every app event to the main webview, for example all terminal
//! output and chat streams. A remote page in that webview could read them, so
//! the main webview can only load the Halv interface. The guard accepts only
//! the app origin of the current platform, because http://tauri.localhost is
//! a real network address on macOS, Linux and iOS.
//!
//! The main window from tauri.conf.json has no new-window handler. Wry then
//! denies every new-window request from it on macOS, Windows and Linux.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};
use url::Url;

const MAIN_WEBVIEW_LABEL: &str = "main";

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("main-navigation-guard")
        .on_navigation(|webview, url| {
            if webview.label() != MAIN_WEBVIEW_LABEL {
                return true;
            }
            let dev_url = if cfg!(debug_assertions) {
                webview.config().build.dev_url.as_ref()
            } else {
                None
            };
            let allowed = main_navigation_allowed(url, dev_url);
            if !allowed {
                eprintln!(
                    "blocked main window navigation to {}://{}",
                    url.scheme(),
                    url.host_str().unwrap_or("")
                );
            }
            allowed
        })
        .build()
}

/// True on the platforms where Tauri serves the app from
/// http(s)://tauri.localhost. The other platforms use tauri://localhost.
/// This is the same rule as tauri_protocol_url() in Tauri.
const HTTP_APP_ORIGIN: bool = cfg!(windows) || cfg!(target_os = "android");

/// Returns true only for the app origin of this platform, or for the dev URL
/// when the caller gives one. Debug builds give the dev URL; release builds
/// give none.
pub(crate) fn main_navigation_allowed(url: &Url, dev_url: Option<&Url>) -> bool {
    is_app_origin(url, HTTP_APP_ORIGIN)
        || dev_url.is_some_and(|dev_url| {
            url.scheme() == dev_url.scheme()
                && url.host_str() == dev_url.host_str()
                && url.port_or_known_default() == dev_url.port_or_known_default()
        })
}

/// Returns true when the URL is on the app origin. Set `http_app_origin` to
/// true for the platforms that serve the app from http(s)://tauri.localhost.
/// On the other platforms, any server on port 80 can answer at
/// http://tauri.localhost, so this function rejects that origin there.
fn is_app_origin(url: &Url, http_app_origin: bool) -> bool {
    if url.port().is_some() {
        return false;
    }
    match (url.scheme(), http_app_origin) {
        ("tauri", false) => url.host_str() == Some("localhost"),
        ("http" | "https", true) => url.host_str() == Some("tauri.localhost"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_app_origin, main_navigation_allowed};
    use url::Url;

    const TAURI_ORIGIN: &[&str] = &[
        "tauri://localhost",
        "tauri://localhost/#user-content-fn-1",
        "tauri://localhost/index.html",
    ];
    const HTTP_ORIGIN: &[&str] = &[
        "http://tauri.localhost/",
        "https://tauri.localhost/",
        "https://tauri.localhost/index.html",
        "http://tauri.localhost/#user-content-fn-1",
    ];

    fn url(value: &str) -> Url {
        value.parse().unwrap()
    }

    /// Returns the app origin URLs of this platform, then the app origin URLs
    /// of the other platforms.
    fn platform_origins() -> (&'static [&'static str], &'static [&'static str]) {
        if cfg!(windows) || cfg!(target_os = "android") {
            (HTTP_ORIGIN, TAURI_ORIGIN)
        } else {
            (TAURI_ORIGIN, HTTP_ORIGIN)
        }
    }

    #[test]
    fn main_webview_loads_only_the_app_origin() {
        let (app_origin, other_platform_origin) = platform_origins();
        for allowed in app_origin {
            assert!(main_navigation_allowed(&url(allowed), None), "{allowed}");
        }
        // On macOS, Linux and iOS, http://tauri.localhost goes to a real
        // server on port 80. On Windows and Android, tauri://localhost is
        // not the app origin.
        for denied in other_platform_origin {
            assert!(!main_navigation_allowed(&url(denied), None), "{denied}");
        }
        for denied in [
            "https://attacker.example/x",
            "HTTPS://attacker.example/x",
            "https:attacker.example/x",
            "HTTP://127.0.0.1:47823/x",
            "http://localhost:1420/",
            "http://tauri.localhost:8080/",
            "https://tauri.localhost.attacker.example/",
            "tauri://localhost:8080/",
            "tauri://attacker.example/",
            "tauri://tauri.localhost/",
            "file:///etc/passwd",
            "about:blank",
            "data:text/html,hi",
            "javascript:alert(1)",
        ] {
            assert!(!main_navigation_allowed(&url(denied), None), "{denied}");
        }
    }

    #[test]
    fn each_platform_has_one_app_origin() {
        for value in TAURI_ORIGIN {
            assert!(is_app_origin(&url(value), false), "{value}");
            assert!(!is_app_origin(&url(value), true), "{value}");
        }
        for value in HTTP_ORIGIN {
            assert!(is_app_origin(&url(value), true), "{value}");
            assert!(!is_app_origin(&url(value), false), "{value}");
        }
    }

    #[test]
    fn debug_builds_also_load_the_dev_url() {
        let dev_url = url("http://localhost:1420");
        let (app_origin, _) = platform_origins();
        for allowed in [
            "http://localhost:1420/",
            "http://localhost:1420/src/main.tsx",
        ]
        .iter()
        .chain(app_origin)
        {
            assert!(
                main_navigation_allowed(&url(allowed), Some(&dev_url)),
                "{allowed}"
            );
        }
        for denied in [
            "http://localhost:1421/",
            "https://localhost:1420/",
            "http://127.0.0.1:1420/",
            "https://attacker.example/x",
        ] {
            assert!(
                !main_navigation_allowed(&url(denied), Some(&dev_url)),
                "{denied}"
            );
        }
    }
}
