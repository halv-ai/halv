//! Keeps the app command list in build.rs, the invoke handler and the
//! capability files the same. Tauri rejects an app command in a webview when
//! no capability of that webview grants it, so a gap breaks a feature only at
//! runtime.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The tray panel invokes these commands. tray.rs also rejects them from every
/// window other than the tray panel (require_panel).
const PANEL_COMMANDS: &[&str] = &["tray_get_snapshot", "tray_action", "tray_resize"];

/// The windows and webviews that load the local Halv interface. The other
/// webviews (dot-remote, halv-checkout, halv-email-challenge) load remote pages
/// and get no capability.
const LOCAL_LABELS: &[&str] = &["main", "tray"];

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start:?}"))
        + start.len();
    let to = source[from..]
        .find(end)
        .unwrap_or_else(|| panic!("missing {end:?} after {start:?}"));
    &source[from..from + to]
}

fn unique(names: Vec<String>, origin: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for name in names {
        assert!(set.insert(name.clone()), "{origin} lists {name} twice");
    }
    set
}

fn handler_commands() -> BTreeSet<String> {
    let source = read("src/lib.rs");
    let list = between(&source, "tauri::generate_handler![", "]");
    let names = list
        .lines()
        .map(|line| line.split("//").next().unwrap_or("").trim())
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.rsplit("::").next().unwrap_or(entry).to_string())
        .collect();
    unique(names, "generate_handler!")
}

fn build_script_commands() -> BTreeSet<String> {
    let source = read("build.rs");
    let list = between(&source, "const APP_COMMANDS: &[&str] = &[", "];");
    let names = list
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .strip_prefix('"')
                .and_then(|entry| entry.strip_suffix('"'))
                .unwrap_or_else(|| panic!("build.rs APP_COMMANDS has a bad entry: {entry}"))
                .to_string()
        })
        .collect();
    unique(names, "build.rs APP_COMMANDS")
}

fn capability(name: &str) -> serde_json::Value {
    serde_json::from_str(&read(&format!("capabilities/{name}")))
        .unwrap_or_else(|error| panic!("parse capabilities/{name}: {error}"))
}

/// Returns the app commands that the capability allows. Plugin and core
/// permissions have a "name:" prefix, app permissions have no prefix.
fn granted_commands(capability: &serde_json::Value) -> BTreeSet<String> {
    let names = capability["permissions"]
        .as_array()
        .expect("capability permissions")
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .or_else(|| entry["identifier"].as_str())
                .expect("permission identifier")
        })
        .filter(|identifier| !identifier.contains(':'))
        .map(|identifier| {
            identifier
                .strip_prefix("allow-")
                .unwrap_or_else(|| panic!("app permission {identifier} is not an allow-* grant"))
                .replace('-', "_")
        })
        .collect();
    unique(names, "capability")
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| name.to_string()).collect()
}

#[test]
fn build_script_lists_every_invoke_handler_command() {
    let handler = handler_commands();
    let build_script = build_script_commands();
    let missing: Vec<_> = handler.difference(&build_script).collect();
    let extra: Vec<_> = build_script.difference(&handler).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "build.rs APP_COMMANDS must match generate_handler!: missing {missing:?}, extra {extra:?}"
    );
}

#[test]
fn main_window_gets_every_command_except_the_panel_commands() {
    let main = capability("default.json");
    assert_eq!(main["webviews"], serde_json::json!(["main"]));
    let expected: BTreeSet<_> = handler_commands()
        .difference(&names(PANEL_COMMANDS))
        .cloned()
        .collect();
    let granted = granted_commands(&main);
    let missing: Vec<_> = expected.difference(&granted).collect();
    let extra: Vec<_> = granted.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "capabilities/default.json: missing {missing:?}, extra {extra:?}"
    );
}

#[test]
fn tray_panel_gets_only_the_panel_commands() {
    let tray = capability("tray.json");
    assert_eq!(tray["windows"], serde_json::json!(["tray"]));
    assert_eq!(granted_commands(&tray), names(PANEL_COMMANDS));
    assert!(names(PANEL_COMMANDS).is_subset(&handler_commands()));
}

#[test]
fn only_local_webviews_have_a_capability() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    for entry in fs::read_dir(&directory).expect("read capabilities") {
        let name = entry.expect("capability entry").file_name();
        let name = name.to_string_lossy();
        let capability = capability(&name);
        assert!(
            capability.get("remote").is_none(),
            "{name} must not grant remote pages"
        );
        for key in ["windows", "webviews"] {
            for label in capability[key].as_array().into_iter().flatten() {
                let label = label.as_str().expect("label");
                assert!(
                    LOCAL_LABELS.contains(&label),
                    "{name} targets {label}, which is not a local Halv webview"
                );
            }
        }
    }
}
