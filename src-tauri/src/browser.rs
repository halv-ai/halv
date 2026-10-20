use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(12);
const CDP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BROWSER_TEXT_BYTES: usize = 64 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 12 * 1024 * 1024;
const SETTINGS_FILENAME: &str = "settings.json";

type BrowserResult<T> = Result<T, String>;

#[derive(Debug, Clone)]
pub struct BrowserToolResult {
    pub text: String,
    pub screenshot_png: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTarget {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    r#type: String,
    web_socket_debugger_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSiteRule {
    pub hostname: String,
    pub decision: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSettings {
    pub site_access: String,
    #[serde(default)]
    pub site_rules: Vec<BrowserSiteRule>,
    #[serde(default)]
    pub full_cdp_access: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            site_access: "always_ask".to_string(),
            site_rules: Vec::new(),
            full_cdp_access: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserUiState {
    pub tabs: Vec<BrowserTab>,
    pub active_tab_id: String,
    pub title: String,
    pub url: String,
    pub screenshot_data_url: String,
    pub viewport_width: f64,
    pub viewport_height: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAnnotationTarget {
    pub annotation_id: String,
    pub url: String,
    pub selector: String,
    pub tag: String,
    pub text: String,
    pub rect: BrowserRect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStylePreview {
    pub font_size: Option<f64>,
    pub color: Option<String>,
    pub padding: Option<f64>,
    pub margin: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct BrowserPermissionRequest {
    pub hostname: Option<String>,
    pub blocked: bool,
    pub site_allowed: bool,
    pub sensitive: bool,
}

#[derive(Default)]
struct BrowserState {
    app_data_dir: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    settings: BrowserSettings,
    child: Option<Child>,
    port: Option<u16>,
    active_target_id: Option<String>,
    approved_sites: HashSet<String>,
}

impl Drop for BrowserState {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Clone, Default)]
pub struct BrowserManager {
    inner: Arc<Mutex<BrowserState>>,
    client: reqwest::Client,
}

impl BrowserManager {
    pub fn set_app_data_dir(&self, app_data_dir: PathBuf) -> BrowserResult<()> {
        let browser_dir = app_data_dir.join("browser");
        fs::create_dir_all(&browser_dir)
            .map_err(|error| format!("failed to create browser data directory: {error}"))?;
        let settings_path = browser_dir.join(SETTINGS_FILENAME);
        let settings = load_browser_settings(&settings_path)?;
        let mut state = self
            .inner
            .try_lock()
            .map_err(|_| "browser manager is busy during initialization".to_string())?;
        state.app_data_dir = Some(browser_dir);
        state.settings_path = Some(settings_path);
        state.settings = settings;
        Ok(())
    }

    pub async fn session_is_approved(&self, conversation_id: &str, hostname: Option<&str>) -> bool {
        let Some(hostname) = hostname else {
            return true;
        };
        self.inner
            .lock()
            .await
            .approved_sites
            .contains(&approved_site_key(conversation_id, hostname))
    }

    pub async fn approve_session(&self, conversation_id: &str, hostname: Option<&str>) {
        if let Some(hostname) = hostname {
            self.inner
                .lock()
                .await
                .approved_sites
                .insert(approved_site_key(conversation_id, hostname));
        }
    }

    pub async fn settings(&self) -> BrowserSettings {
        self.inner.lock().await.settings.clone()
    }

    pub async fn update_settings(
        &self,
        mut settings: BrowserSettings,
    ) -> BrowserResult<BrowserSettings> {
        validate_browser_settings(&mut settings)?;
        let mut state = self.inner.lock().await;
        let path = state
            .settings_path
            .as_ref()
            .ok_or_else(|| "browser manager is not initialized".to_string())?;
        save_browser_settings(path, &settings)?;
        state.settings = settings.clone();
        state.approved_sites.clear();
        Ok(settings)
    }

    pub async fn clear_data(&self) -> BrowserResult<()> {
        let mut state = self.inner.lock().await;
        if let Some(child) = state.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.child = None;
        state.port = None;
        state.active_target_id = None;
        state.approved_sites.clear();
        let browser_dir = state
            .app_data_dir
            .as_ref()
            .ok_or_else(|| "browser manager is not initialized".to_string())?;
        let profile = browser_dir.join("profile");
        if profile.exists() {
            fs::remove_dir_all(&profile)
                .map_err(|error| format!("failed to clear browser profile: {error}"))?;
        }
        fs::create_dir_all(&profile)
            .map_err(|error| format!("failed to recreate browser profile: {error}"))?;
        Ok(())
    }

    pub async fn permission_request(
        &self,
        name: &str,
        input: &str,
    ) -> BrowserResult<BrowserPermissionRequest> {
        let hostname = if name == "browser_open" {
            let args = parse_args::<BrowserOpenArgs>(input, name)?;
            validate_browser_url(&args.url)?
                .host_str()
                .map(normalize_hostname)
        } else if matches!(name, "browser_tabs") {
            None
        } else {
            self.active_hostname().await?
        };
        let sensitive = match name {
            "browser_type" => parse_args::<BrowserTypeArgs>(input, name)?.submit,
            "browser_click" => {
                let args = parse_args::<BrowserRefArgs>(input, name)?;
                self.reference_is_sensitive(&args.reference)
                    .await
                    .unwrap_or(true)
            }
            "browser_cdp" => true,
            _ => false,
        };
        let state = self.inner.lock().await;
        let rules = state
            .settings
            .site_rules
            .iter()
            .map(|rule| (rule.hostname.as_str(), rule.decision.as_str()))
            .collect::<HashMap<_, _>>();
        let explicit = hostname
            .as_deref()
            .and_then(|hostname| rules.get(hostname).copied());
        let blocked = explicit == Some("block");
        let site_allowed = hostname.is_none()
            || explicit == Some("allow")
            || (explicit.is_none()
                && matches!(
                    state.settings.site_access.as_str(),
                    "auto_approve" | "always_allow"
                ));
        Ok(BrowserPermissionRequest {
            hostname,
            blocked,
            site_allowed,
            sensitive,
        })
    }

    pub async fn ui_state(&self) -> BrowserResult<BrowserUiState> {
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let target = self.active_target(&mut state, port).await?;
        state.active_target_id = Some(target.id.clone());
        let socket = target
            .web_socket_debugger_url
            .clone()
            .ok_or_else(|| "browser tab did not expose a debugger socket".to_string())?;
        let targets = self.list_targets(port).await?;
        let tabs = targets
            .into_iter()
            .filter(|target| target.r#type == "page")
            .map(|target| BrowserTab {
                active: target.id == state.active_target_id.as_deref().unwrap_or_default(),
                id: target.id,
                title: target.title,
                url: target.url,
            })
            .collect::<Vec<_>>();
        let metrics = cdp_evaluate(
            &socket,
            "({ width: window.innerWidth, height: window.innerHeight })",
        )
        .await?;
        let width = metrics
            .get("width")
            .and_then(Value::as_f64)
            .filter(|value| *value > 0.0)
            .unwrap_or(1.0);
        let height = metrics
            .get("height")
            .and_then(Value::as_f64)
            .filter(|value| *value > 0.0)
            .unwrap_or(1.0);
        let screenshot = capture_screenshot_data(&socket).await?;
        Ok(BrowserUiState {
            tabs,
            active_tab_id: target.id,
            title: target.title,
            url: target.url,
            screenshot_data_url: format!("data:image/png;base64,{screenshot}"),
            viewport_width: width,
            viewport_height: height,
        })
    }

    pub async fn ui_navigate(&self, url: &str, new_tab: bool) -> BrowserResult<BrowserUiState> {
        self.open(url, new_tab).await?;
        self.ui_state().await
    }

    pub async fn ui_select_tab(&self, target_id: &str) -> BrowserResult<BrowserUiState> {
        self.select_tab(target_id).await?;
        self.ui_state().await
    }

    pub async fn ui_close_tab(&self, target_id: &str) -> BrowserResult<BrowserUiState> {
        self.close_tab(target_id).await?;
        self.ui_state().await
    }

    pub async fn ui_history(&self, direction: &str) -> BrowserResult<BrowserUiState> {
        let (_target_id, socket) = self.active_socket().await?;
        match direction {
            "back" => {
                cdp_evaluate(&socket, "history.back(); true").await?;
            }
            "forward" => {
                cdp_evaluate(&socket, "history.forward(); true").await?;
            }
            "reload" => {
                cdp_command(&socket, "Page.reload", json!({})).await?;
            }
            _ => return Err("browser history direction must be back, forward, or reload".into()),
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
        self.ui_state().await
    }

    pub async fn ui_click(
        &self,
        x: f64,
        y: f64,
        surface_width: f64,
        surface_height: f64,
    ) -> BrowserResult<BrowserUiState> {
        let (_target_id, socket) = self.active_socket().await?;
        let (x, y) = browser_surface_point(&socket, x, y, surface_width, surface_height).await?;
        let expression = format!(
            "(() => {{ const e = document.elementFromPoint({x}, {y}); if (!e) return false; e.focus(); e.click(); return true; }})()"
        );
        if cdp_evaluate(&socket, &expression).await? != Value::Bool(true) {
            return Err("no browser element was found at that point".to_string());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.ui_state().await
    }

    pub async fn ui_type_focused(&self, text: &str, submit: bool) -> BrowserResult<BrowserUiState> {
        if text.len() > MAX_BROWSER_TEXT_BYTES {
            return Err("browser input text is too large".to_string());
        }
        let (_target_id, socket) = self.active_socket().await?;
        let text = serde_json::to_string(text)
            .map_err(|error| format!("failed to encode browser input: {error}"))?;
        let submit_literal = if submit { "true" } else { "false" };
        let expression = format!(
            r#"(() => {{
              const element = document.activeElement;
              if (!element || element === document.body) return false;
              const text = {text};
              if (element.isContentEditable) element.textContent = text;
              else if ("value" in element) {{
                const prototype = element instanceof HTMLTextAreaElement
                  ? HTMLTextAreaElement.prototype
                  : HTMLInputElement.prototype;
                const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
                if (setter) setter.call(element, text); else element.value = text;
              }} else return false;
              element.dispatchEvent(new InputEvent("input", {{ bubbles: true, inputType: "insertText", data: text }}));
              element.dispatchEvent(new Event("change", {{ bubbles: true }}));
              if ({submit_literal}) {{
                const form = element.closest("form");
                if (form?.requestSubmit) form.requestSubmit();
                else element.dispatchEvent(new KeyboardEvent("keydown", {{ key: "Enter", code: "Enter", bubbles: true }}));
              }}
              return true;
            }})()"#
        );
        if cdp_evaluate(&socket, &expression).await? != Value::Bool(true) {
            return Err("click a text field in the browser view first".to_string());
        }
        if submit {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        self.ui_state().await
    }

    pub async fn annotation_target(
        &self,
        x: f64,
        y: f64,
        surface_width: f64,
        surface_height: f64,
    ) -> BrowserResult<BrowserAnnotationTarget> {
        let (_target_id, socket) = self.active_socket().await?;
        let (x, y) = browser_surface_point(&socket, x, y, surface_width, surface_height).await?;
        let annotation_id = format!("a{:016x}", rand::random::<u64>());
        let encoded_id = serde_json::to_string(&annotation_id)
            .map_err(|error| format!("failed to encode annotation id: {error}"))?;
        let expression = format!(
            r#"(() => {{
              const element = document.elementFromPoint({x}, {y});
              if (!element) return {{ ok: false }};
              const id = {encoded_id};
              element.setAttribute("data-halv-annotation-id", id);
              const rect = element.getBoundingClientRect();
              const selector = element.id
                ? `#${{CSS.escape(element.id)}}`
                : `[data-halv-annotation-id="${{id}}"]`;
              return {{
                ok: true,
                annotationId: id,
                url: location.href,
                selector,
                tag: element.tagName.toLowerCase(),
                text: String(element.innerText || element.getAttribute("aria-label") || "").replace(/\s+/g, " ").trim().slice(0, 500),
                rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }}
              }};
            }})()"#
        );
        let value = cdp_evaluate(&socket, &expression).await?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err("no browser element was found at that point".to_string());
        }
        serde_json::from_value(value)
            .map_err(|error| format!("failed to decode browser annotation target: {error}"))
    }

    pub async fn preview_style(
        &self,
        annotation_id: &str,
        style: BrowserStylePreview,
    ) -> BrowserResult<BrowserUiState> {
        validate_annotation_id(annotation_id)?;
        validate_style_preview(&style)?;
        let (_target_id, socket) = self.active_socket().await?;
        let selector =
            serde_json::to_string(&format!("[data-halv-annotation-id=\"{annotation_id}\"]"))
                .map_err(|error| format!("failed to encode annotation selector: {error}"))?;
        let font_size = style
            .font_size
            .map(|value| serde_json::to_string(&format!("{value}px")).unwrap())
            .unwrap_or_else(|| "null".to_string());
        let color = style
            .color
            .as_ref()
            .map(|value| serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()))
            .unwrap_or_else(|| "null".to_string());
        let padding = style
            .padding
            .map(|value| serde_json::to_string(&format!("{value}px")).unwrap())
            .unwrap_or_else(|| "null".to_string());
        let margin = style
            .margin
            .map(|value| serde_json::to_string(&format!("{value}px")).unwrap())
            .unwrap_or_else(|| "null".to_string());
        let expression = format!(
            r#"(() => {{
              const element = document.querySelector({selector});
              if (!element) return false;
              const fontSize = {font_size};
              const color = {color};
              const padding = {padding};
              const margin = {margin};
              if (fontSize) element.style.fontSize = fontSize;
              if (color) element.style.color = color;
              if (padding) element.style.padding = padding;
              if (margin) element.style.margin = margin;
              return true;
            }})()"#
        );
        if cdp_evaluate(&socket, &expression).await? != Value::Bool(true) {
            return Err("annotated element is no longer present".to_string());
        }
        self.ui_state().await
    }

    async fn active_hostname(&self) -> BrowserResult<Option<String>> {
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let target = self.active_target(&mut state, port).await?;
        Ok(Url::parse(&target.url)
            .ok()
            .and_then(|url| url.host_str().map(normalize_hostname)))
    }

    async fn reference_is_sensitive(&self, reference: &str) -> BrowserResult<bool> {
        validate_reference(reference)?;
        let (_target_id, socket) = self.active_socket().await?;
        let selector = serde_json::to_string(&format!("[data-halv-ref=\"{reference}\"]"))
            .map_err(|error| format!("failed to encode browser ref: {error}"))?;
        let expression = format!(
            r#"(() => {{
              const e = document.querySelector({selector});
              if (!e) return true;
              const text = String(e.innerText || e.value || e.getAttribute("aria-label") || "").toLowerCase();
              const dangerous = /(buy|purchase|pay|submit|confirm|delete|remove|send|publish|book|order|sign in|log in)/;
              return e.matches('button[type="submit"], input[type="submit"]') || dangerous.test(text);
            }})()"#
        );
        Ok(cdp_evaluate(&socket, &expression)
            .await?
            .as_bool()
            .unwrap_or(true))
    }

    pub async fn execute(&self, name: &str, input: &str) -> BrowserResult<BrowserToolResult> {
        match name {
            "browser_open" => {
                let args = parse_args::<BrowserOpenArgs>(input, name)?;
                self.open(&args.url, args.new_tab).await
            }
            "browser_snapshot" => {
                parse_args::<EmptyArgs>(input, name)?;
                self.snapshot().await
            }
            "browser_click" => {
                let args = parse_args::<BrowserRefArgs>(input, name)?;
                self.click(&args.reference).await
            }
            "browser_type" => {
                let args = parse_args::<BrowserTypeArgs>(input, name)?;
                self.type_text(&args.reference, &args.text, args.submit)
                    .await
            }
            "browser_screenshot" => {
                parse_args::<EmptyArgs>(input, name)?;
                self.screenshot().await
            }
            "browser_tabs" => {
                parse_args::<EmptyArgs>(input, name)?;
                self.tabs().await
            }
            "browser_select_tab" => {
                let args = parse_args::<BrowserTargetArgs>(input, name)?;
                self.select_tab(&args.target_id).await
            }
            "browser_close_tab" => {
                let args = parse_args::<BrowserTargetArgs>(input, name)?;
                self.close_tab(&args.target_id).await
            }
            "browser_cdp" => {
                let args = parse_args::<BrowserCdpArgs>(input, name)?;
                self.raw_cdp(&args.method, args.params).await
            }
            _ => Err(format!("unsupported browser tool `{name}`")),
        }
    }

    async fn raw_cdp(&self, method: &str, params: Value) -> BrowserResult<BrowserToolResult> {
        if !self.inner.lock().await.settings.full_cdp_access {
            return Err(
                "full CDP access is disabled; enable Developer mode in Browser settings"
                    .to_string(),
            );
        }
        if method.is_empty()
            || method.len() > 160
            || !method.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_')
            })
        {
            return Err("invalid CDP method".to_string());
        }
        let (_target_id, socket) = self.active_socket().await?;
        let result = cdp_command(&socket, method, params).await?;
        Ok(BrowserToolResult {
            text: truncate_utf8(
                &serde_json::to_string_pretty(&result)
                    .map_err(|error| format!("failed to encode CDP result: {error}"))?,
                MAX_BROWSER_TEXT_BYTES,
            ),
            screenshot_png: None,
        })
    }

    async fn open(&self, raw_url: &str, new_tab: bool) -> BrowserResult<BrowserToolResult> {
        let url = validate_browser_url(raw_url)?;
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let target = if new_tab {
            self.create_target(port, url.as_str()).await?
        } else {
            self.active_target(&mut state, port).await?
        };
        state.active_target_id = Some(target.id.clone());
        let socket = target
            .web_socket_debugger_url
            .as_deref()
            .ok_or_else(|| "browser tab did not expose a debugger socket".to_string())?;
        cdp_command(socket, "Page.navigate", json!({ "url": url.as_str() })).await?;
        drop(state);
        tokio::time::sleep(Duration::from_millis(400)).await;
        Ok(BrowserToolResult {
            text: format!("Opened {}. Run browser_snapshot to inspect the page.", url),
            screenshot_png: None,
        })
    }

    async fn snapshot(&self) -> BrowserResult<BrowserToolResult> {
        let (_target_id, socket) = self.active_socket().await?;
        let value = cdp_evaluate(&socket, SNAPSHOT_SCRIPT).await?;
        let text = serde_json::to_string_pretty(&value)
            .map_err(|error| format!("failed to encode browser snapshot: {error}"))?;
        Ok(BrowserToolResult {
            text: truncate_utf8(&text, MAX_BROWSER_TEXT_BYTES),
            screenshot_png: None,
        })
    }

    async fn click(&self, reference: &str) -> BrowserResult<BrowserToolResult> {
        validate_reference(reference)?;
        let (_target_id, socket) = self.active_socket().await?;
        let reference = serde_json::to_string(reference)
            .map_err(|error| format!("failed to encode browser ref: {error}"))?;
        let script = format!(
            r#"(() => {{
              const ref = {reference};
              const element = document.querySelector(`[data-halv-ref="${{ref}}"]`);
              if (!element) return {{ ok: false, error: `No element has ref ${{ref}}. Take a new snapshot.` }};
              element.scrollIntoView({{ block: "center", inline: "center" }});
              if (element instanceof HTMLAnchorElement && element.target === "_blank" && element.href) {{
                window.location.href = element.href;
              }} else {{
                element.click();
              }}
              return {{ ok: true, ref, tag: element.tagName.toLowerCase() }};
            }})()"#
        );
        let result = cdp_evaluate(&socket, &script).await?;
        ensure_script_ok(&result)?;
        tokio::time::sleep(Duration::from_millis(350)).await;
        Ok(BrowserToolResult {
            text: format!(
                "Clicked {reference}. Take a new browser_snapshot before using another ref."
            ),
            screenshot_png: None,
        })
    }

    async fn type_text(
        &self,
        reference: &str,
        text: &str,
        submit: bool,
    ) -> BrowserResult<BrowserToolResult> {
        validate_reference(reference)?;
        if text.len() > MAX_BROWSER_TEXT_BYTES {
            return Err("browser input text is too large".to_string());
        }
        let (_target_id, socket) = self.active_socket().await?;
        let reference = serde_json::to_string(reference)
            .map_err(|error| format!("failed to encode browser ref: {error}"))?;
        let text = serde_json::to_string(text)
            .map_err(|error| format!("failed to encode browser input: {error}"))?;
        let script = format!(
            r#"(() => {{
              const ref = {reference};
              const text = {text};
              const element = document.querySelector(`[data-halv-ref="${{ref}}"]`);
              if (!element) return {{ ok: false, error: `No element has ref ${{ref}}. Take a new snapshot.` }};
              element.scrollIntoView({{ block: "center", inline: "center" }});
              element.focus();
              if (element.isContentEditable) {{
                element.textContent = text;
              }} else if ("value" in element) {{
                const setter = Object.getOwnPropertyDescriptor(
                  element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,
                  "value"
                )?.set;
                if (setter) setter.call(element, text); else element.value = text;
              }} else {{
                return {{ ok: false, error: `Element ${{ref}} does not accept text.` }};
              }}
              element.dispatchEvent(new InputEvent("input", {{ bubbles: true, inputType: "insertText", data: text }}));
              element.dispatchEvent(new Event("change", {{ bubbles: true }}));
              if ({submit}) {{
                const form = element.closest("form");
                if (form?.requestSubmit) form.requestSubmit();
                else element.dispatchEvent(new KeyboardEvent("keydown", {{ key: "Enter", code: "Enter", bubbles: true }}));
              }}
              return {{ ok: true, ref, submitted: {submit} }};
            }})()"#,
            submit = if submit { "true" } else { "false" },
        );
        let result = cdp_evaluate(&socket, &script).await?;
        ensure_script_ok(&result)?;
        if submit {
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
        Ok(BrowserToolResult {
            text: format!(
                "Entered text in {reference}{}. Take a new browser_snapshot before using another ref.",
                if submit { " and submitted" } else { "" }
            ),
            screenshot_png: None,
        })
    }

    async fn screenshot(&self) -> BrowserResult<BrowserToolResult> {
        let (_target_id, socket) = self.active_socket().await?;
        let data = capture_screenshot_data(&socket).await?;
        Ok(BrowserToolResult {
            text: "Captured the active browser tab.".to_string(),
            screenshot_png: Some(data),
        })
    }

    async fn tabs(&self) -> BrowserResult<BrowserToolResult> {
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let targets = self.list_targets(port).await?;
        let tabs = targets
            .into_iter()
            .filter(|target| target.r#type == "page")
            .map(|target| BrowserTab {
                active: state.active_target_id.as_deref() == Some(target.id.as_str()),
                id: target.id,
                title: target.title,
                url: target.url,
            })
            .collect::<Vec<_>>();
        Ok(BrowserToolResult {
            text: serde_json::to_string_pretty(&tabs)
                .map_err(|error| format!("failed to encode browser tabs: {error}"))?,
            screenshot_png: None,
        })
    }

    async fn select_tab(&self, target_id: &str) -> BrowserResult<BrowserToolResult> {
        validate_target_id(target_id)?;
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let target = self
            .list_targets(port)
            .await?
            .into_iter()
            .find(|target| target.id == target_id && target.r#type == "page")
            .ok_or_else(|| "browser tab was not found".to_string())?;
        state.active_target_id = Some(target.id.clone());
        Ok(BrowserToolResult {
            text: format!("Selected tab {} ({})", target.title, target.url),
            screenshot_png: None,
        })
    }

    async fn close_tab(&self, target_id: &str) -> BrowserResult<BrowserToolResult> {
        validate_target_id(target_id)?;
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let url = format!("http://127.0.0.1:{port}/json/close/{target_id}");
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| format!("failed to close browser tab: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "failed to close browser tab: HTTP {}",
                response.status()
            ));
        }
        if state.active_target_id.as_deref() == Some(target_id) {
            state.active_target_id = None;
        }
        Ok(BrowserToolResult {
            text: format!("Closed browser tab {target_id}."),
            screenshot_png: None,
        })
    }

    async fn active_socket(&self) -> BrowserResult<(String, String)> {
        let mut state = self.inner.lock().await;
        let port = self.ensure_started(&mut state).await?;
        let target = self.active_target(&mut state, port).await?;
        state.active_target_id = Some(target.id.clone());
        let socket = target
            .web_socket_debugger_url
            .ok_or_else(|| "browser tab did not expose a debugger socket".to_string())?;
        Ok((target.id, socket))
    }

    async fn active_target(&self, state: &mut BrowserState, port: u16) -> BrowserResult<CdpTarget> {
        let targets = self.list_targets(port).await?;
        if let Some(active) = state.active_target_id.as_deref() {
            if let Some(target) = targets
                .iter()
                .find(|target| target.id == active && target.r#type == "page")
            {
                return Ok(target.clone());
            }
        }
        if let Some(target) = targets
            .into_iter()
            .find(|target| target.r#type == "page" && target.web_socket_debugger_url.is_some())
        {
            state.active_target_id = Some(target.id.clone());
            return Ok(target);
        }
        let target = self.create_target(port, "about:blank").await?;
        state.active_target_id = Some(target.id.clone());
        Ok(target)
    }

    async fn list_targets(&self, port: u16) -> BrowserResult<Vec<CdpTarget>> {
        self.client
            .get(format!("http://127.0.0.1:{port}/json/list"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| format!("failed to list browser tabs: {error}"))?
            .error_for_status()
            .map_err(|error| format!("failed to list browser tabs: {error}"))?
            .json::<Vec<CdpTarget>>()
            .await
            .map_err(|error| format!("invalid browser tab response: {error}"))
    }

    async fn create_target(&self, port: u16, target_url: &str) -> BrowserResult<CdpTarget> {
        let mut url = Url::parse(&format!("http://127.0.0.1:{port}/json/new"))
            .map_err(|error| format!("failed to build browser endpoint: {error}"))?;
        url.query_pairs_mut().append_pair("url", target_url);
        self.client
            .put(url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| format!("failed to create browser tab: {error}"))?
            .error_for_status()
            .map_err(|error| format!("failed to create browser tab: {error}"))?
            .json::<CdpTarget>()
            .await
            .map_err(|error| format!("invalid browser tab response: {error}"))
    }

    async fn ensure_started(&self, state: &mut BrowserState) -> BrowserResult<u16> {
        if let Some(port) = state.port {
            if probe_debug_port(&self.client, port).await {
                return Ok(port);
            }
            state.port = None;
        }
        let browser_dir = state
            .app_data_dir
            .clone()
            .ok_or_else(|| "browser manager is not initialized".to_string())?;
        let profile = browser_dir.join("profile");
        fs::create_dir_all(&profile)
            .map_err(|error| format!("failed to create browser profile: {error}"))?;
        if let Some(port) = read_devtools_port(&profile) {
            if probe_debug_port(&self.client, port).await {
                state.port = Some(port);
                return Ok(port);
            }
        }
        let _ = fs::remove_file(profile.join(DEVTOOLS_PORT_FILE));
        if let Some(child) = state.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let binary = find_browser_binary().ok_or_else(|| {
            "No compatible Chromium browser was found. Install Chrome, Chromium, Brave, or Edge, then retry."
                .to_string()
        })?;
        let mut child = Command::new(&binary)
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.to_string_lossy()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to launch browser `{}`: {error}", binary.display()))?;
        let started = Instant::now();
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("failed to inspect browser process: {error}"))?
            {
                return Err(format!("browser exited during startup with {status}"));
            }
            if let Some(port) = read_devtools_port(&profile) {
                if probe_debug_port(&self.client, port).await {
                    state.child = Some(child);
                    state.port = Some(port);
                    return Ok(port);
                }
            }
            if started.elapsed() >= STARTUP_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err("browser did not expose DevTools within 12 seconds".to_string());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[tauri::command]
pub async fn browser_ui_state(
    browser: tauri::State<'_, BrowserManager>,
) -> BrowserResult<BrowserUiState> {
    browser.ui_state().await
}

#[tauri::command]
pub async fn browser_ui_navigate(
    browser: tauri::State<'_, BrowserManager>,
    url: String,
    new_tab: bool,
) -> BrowserResult<BrowserUiState> {
    browser.ui_navigate(&url, new_tab).await
}

#[tauri::command]
pub async fn browser_ui_select_tab(
    browser: tauri::State<'_, BrowserManager>,
    target_id: String,
) -> BrowserResult<BrowserUiState> {
    browser.ui_select_tab(&target_id).await
}

#[tauri::command]
pub async fn browser_ui_close_tab(
    browser: tauri::State<'_, BrowserManager>,
    target_id: String,
) -> BrowserResult<BrowserUiState> {
    browser.ui_close_tab(&target_id).await
}

#[tauri::command]
pub async fn browser_ui_history(
    browser: tauri::State<'_, BrowserManager>,
    direction: String,
) -> BrowserResult<BrowserUiState> {
    browser.ui_history(&direction).await
}

#[tauri::command]
pub async fn browser_ui_click(
    browser: tauri::State<'_, BrowserManager>,
    x: f64,
    y: f64,
    surface_width: f64,
    surface_height: f64,
) -> BrowserResult<BrowserUiState> {
    browser.ui_click(x, y, surface_width, surface_height).await
}

#[tauri::command]
pub async fn browser_ui_type_focused(
    browser: tauri::State<'_, BrowserManager>,
    text: String,
    submit: bool,
) -> BrowserResult<BrowserUiState> {
    browser.ui_type_focused(&text, submit).await
}

#[tauri::command]
pub async fn browser_ui_annotation_target(
    browser: tauri::State<'_, BrowserManager>,
    x: f64,
    y: f64,
    surface_width: f64,
    surface_height: f64,
) -> BrowserResult<BrowserAnnotationTarget> {
    browser
        .annotation_target(x, y, surface_width, surface_height)
        .await
}

#[tauri::command]
pub async fn browser_ui_preview_style(
    browser: tauri::State<'_, BrowserManager>,
    annotation_id: String,
    style: BrowserStylePreview,
) -> BrowserResult<BrowserUiState> {
    browser.preview_style(&annotation_id, style).await
}

#[tauri::command]
pub async fn browser_settings_get(
    browser: tauri::State<'_, BrowserManager>,
) -> BrowserResult<BrowserSettings> {
    Ok(browser.settings().await)
}

#[tauri::command]
pub async fn browser_settings_set(
    browser: tauri::State<'_, BrowserManager>,
    settings: BrowserSettings,
) -> BrowserResult<BrowserSettings> {
    browser.update_settings(settings).await
}

#[tauri::command]
pub async fn browser_clear_data(browser: tauri::State<'_, BrowserManager>) -> BrowserResult<()> {
    browser.clear_data().await
}

fn load_browser_settings(path: &Path) -> BrowserResult<BrowserSettings> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BrowserSettings::default());
        }
        Err(error) => return Err(format!("failed to read browser settings: {error}")),
    };
    if raw.len() > 1024 * 1024 {
        return Err("browser settings exceed the 1 MB safety limit".to_string());
    }
    let mut settings = serde_json::from_slice::<BrowserSettings>(&raw)
        .map_err(|error| format!("invalid browser settings: {error}"))?;
    validate_browser_settings(&mut settings)?;
    Ok(settings)
}

fn save_browser_settings(path: &Path, settings: &BrowserSettings) -> BrowserResult<()> {
    let raw = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("failed to encode browser settings: {error}"))?;
    let temporary = path.with_extension(format!("tmp-{:016x}", rand::random::<u64>()));
    fs::write(&temporary, raw)
        .map_err(|error| format!("failed to write browser settings: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("failed to install browser settings: {error}")
    })
}

fn validate_browser_settings(settings: &mut BrowserSettings) -> BrowserResult<()> {
    if !matches!(
        settings.site_access.as_str(),
        "always_ask" | "auto_approve" | "always_allow"
    ) {
        return Err("site access must be always_ask, auto_approve, or always_allow".to_string());
    }
    if settings.site_rules.len() > 512 {
        return Err("browser settings support at most 512 site rules".to_string());
    }
    let mut rules = HashMap::new();
    for rule in &settings.site_rules {
        let hostname = normalize_hostname(&rule.hostname);
        if hostname.is_empty()
            || hostname.len() > 253
            || hostname.contains('/')
            || hostname.contains(':')
            || hostname.chars().any(char::is_whitespace)
        {
            return Err(format!("invalid browser hostname `{}`", rule.hostname));
        }
        if !matches!(rule.decision.as_str(), "allow" | "block") {
            return Err("browser site rules must allow or block".to_string());
        }
        rules.insert(hostname, rule.decision.clone());
    }
    settings.site_rules = rules
        .into_iter()
        .map(|(hostname, decision)| BrowserSiteRule { hostname, decision })
        .collect();
    settings
        .site_rules
        .sort_by(|left, right| left.hostname.cmp(&right.hostname));
    Ok(())
}

fn approved_site_key(conversation_id: &str, hostname: &str) -> String {
    format!("{conversation_id}\n{}", normalize_hostname(hostname))
}

fn normalize_hostname(hostname: &str) -> String {
    hostname.trim().trim_end_matches('.').to_ascii_lowercase()
}

async fn capture_screenshot_data(socket: &str) -> BrowserResult<String> {
    let result = cdp_command(
        socket,
        "Page.captureScreenshot",
        json!({ "format": "png", "fromSurface": true, "captureBeyondViewport": false }),
    )
    .await?;
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "browser screenshot did not contain image data".to_string())?;
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| "browser screenshot returned invalid base64".to_string())?;
    if decoded.len() > MAX_SCREENSHOT_BYTES || !decoded.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("browser screenshot was invalid or exceeded 12 MiB".to_string());
    }
    Ok(data.to_string())
}

async fn browser_surface_point(
    socket: &str,
    x: f64,
    y: f64,
    surface_width: f64,
    surface_height: f64,
) -> BrowserResult<(f64, f64)> {
    if ![x, y, surface_width, surface_height]
        .iter()
        .all(|value| value.is_finite())
        || surface_width <= 0.0
        || surface_height <= 0.0
        || x < 0.0
        || y < 0.0
        || x > surface_width
        || y > surface_height
    {
        return Err("invalid browser surface coordinates".to_string());
    }
    let metrics = cdp_evaluate(
        socket,
        "({ width: window.innerWidth, height: window.innerHeight })",
    )
    .await?;
    let viewport_width = metrics
        .get("width")
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
        .ok_or_else(|| "browser viewport width is unavailable".to_string())?;
    let viewport_height = metrics
        .get("height")
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
        .ok_or_else(|| "browser viewport height is unavailable".to_string())?;
    Ok((
        (x / surface_width * viewport_width).clamp(0.0, viewport_width - 1.0),
        (y / surface_height * viewport_height).clamp(0.0, viewport_height - 1.0),
    ))
}

fn validate_annotation_id(annotation_id: &str) -> BrowserResult<()> {
    if annotation_id.len() < 2
        || annotation_id.len() > 40
        || !annotation_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("invalid browser annotation id".to_string());
    }
    Ok(())
}

fn validate_style_preview(style: &BrowserStylePreview) -> BrowserResult<()> {
    for (label, value, minimum, maximum) in [
        ("font size", style.font_size, 8.0, 96.0),
        ("padding", style.padding, 0.0, 128.0),
        ("margin", style.margin, 0.0, 128.0),
    ] {
        if value.is_some_and(|value| !value.is_finite() || value < minimum || value > maximum) {
            return Err(format!("browser annotation {label} is out of range"));
        }
    }
    if style.color.as_deref().is_some_and(|color| {
        let color = color.trim();
        !matches!(color.len(), 4 | 7 | 9)
            || !color.starts_with('#')
            || !color[1..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
    }) {
        return Err("browser annotation color must be a hex color".to_string());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct EmptyArgs {}

#[derive(Debug, Deserialize)]
struct BrowserOpenArgs {
    url: String,
    #[serde(default)]
    new_tab: bool,
}

#[derive(Debug, Deserialize)]
struct BrowserRefArgs {
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Debug, Deserialize)]
struct BrowserTypeArgs {
    #[serde(rename = "ref")]
    reference: String,
    text: String,
    #[serde(default)]
    submit: bool,
}

#[derive(Debug, Deserialize)]
struct BrowserTargetArgs {
    target_id: String,
}

#[derive(Debug, Deserialize)]
struct BrowserCdpArgs {
    method: String,
    #[serde(default)]
    params: Value,
}

fn parse_args<T: for<'de> Deserialize<'de>>(input: &str, name: &str) -> BrowserResult<T> {
    serde_json::from_str(input).map_err(|error| format!("invalid {name} arguments: {error}"))
}

fn validate_browser_url(raw: &str) -> BrowserResult<Url> {
    if raw.len() > 4_096 {
        return Err("browser URL is too large".to_string());
    }
    let url = Url::parse(raw.trim()).map_err(|error| format!("invalid browser URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("browser URLs must use HTTP or HTTPS".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("browser URLs cannot contain embedded credentials".to_string());
    }
    Ok(url)
}

fn validate_reference(reference: &str) -> BrowserResult<()> {
    if reference.len() < 2
        || reference.len() > 12
        || !reference.starts_with('r')
        || !reference[1..]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return Err("browser ref must look like r0, r1, or r2".to_string());
    }
    Ok(())
}

fn validate_target_id(target_id: &str) -> BrowserResult<()> {
    if target_id.is_empty()
        || target_id.len() > 128
        || !target_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("invalid browser target id".to_string());
    }
    Ok(())
}

fn ensure_script_ok(value: &Value) -> BrowserResult<()> {
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("browser action failed")
            .to_string());
    }
    Ok(())
}

async fn probe_debug_port(client: &reqwest::Client, port: u16) -> bool {
    client
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn read_devtools_port(profile: &Path) -> Option<u16> {
    fs::read_to_string(profile.join(DEVTOOLS_PORT_FILE))
        .ok()?
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn find_browser_binary() -> Option<PathBuf> {
    if let Some(binary) = env::var_os("HALV_BROWSER_BINARY") {
        let binary = PathBuf::from(binary);
        if binary.is_file() {
            return Some(binary);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let mut candidates = Vec::<PathBuf>::new();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates = Vec::<PathBuf>::new();
    #[cfg(target_os = "macos")]
    {
        candidates.extend(
            [
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            ]
            .into_iter()
            .map(PathBuf::from),
        );
        if let Some(home) = env::var_os("HOME") {
            let applications = PathBuf::from(home).join("Applications");
            candidates.extend([
                applications.join("Google Chrome.app/Contents/MacOS/Google Chrome"),
                applications.join("Chromium.app/Contents/MacOS/Chromium"),
                applications.join("Brave Browser.app/Contents/MacOS/Brave Browser"),
                applications.join("Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            ]);
        }
    }
    #[cfg(target_os = "windows")]
    {
        for root in [
            env::var_os("PROGRAMFILES"),
            env::var_os("PROGRAMFILES(X86)"),
            env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            let root = PathBuf::from(root);
            candidates.extend([
                root.join("Google/Chrome/Application/chrome.exe"),
                root.join("Microsoft/Edge/Application/msedge.exe"),
                root.join("BraveSoftware/Brave-Browser/Application/brave.exe"),
            ]);
        }
    }
    if let Some(path) = candidates.into_iter().find(|candidate| candidate.is_file()) {
        return Some(path);
    }
    find_on_path(&[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "brave-browser",
        "microsoft-edge",
        "chrome",
        "msedge",
    ])
}

fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
}

async fn cdp_evaluate(socket_url: &str, expression: &str) -> BrowserResult<Value> {
    let response = cdp_command(
        socket_url,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "awaitPromise": true,
            "returnByValue": true,
            "userGesture": true
        }),
    )
    .await?;
    if let Some(exception) = response.get("exceptionDetails") {
        return Err(format!("browser script failed: {exception}"));
    }
    response
        .pointer("/result/value")
        .cloned()
        .ok_or_else(|| "browser script returned no serializable value".to_string())
}

async fn cdp_command(socket_url: &str, method: &str, params: Value) -> BrowserResult<Value> {
    let future = async {
        let (mut socket, _) = connect_async(socket_url)
            .await
            .map_err(|error| format!("failed to connect to browser: {error}"))?;
        socket
            .send(Message::Text(
                json!({ "id": 1, "method": method, "params": params })
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|error| format!("failed to send browser command: {error}"))?;
        while let Some(message) = socket.next().await {
            let message = message.map_err(|error| format!("browser connection failed: {error}"))?;
            let text = match message {
                Message::Text(text) => text,
                Message::Ping(data) => {
                    socket
                        .send(Message::Pong(data))
                        .await
                        .map_err(|error| format!("browser connection failed: {error}"))?;
                    continue;
                }
                Message::Close(_) => {
                    return Err("browser closed the debugger connection".to_string())
                }
                _ => continue,
            };
            let value = serde_json::from_str::<Value>(&text)
                .map_err(|error| format!("browser returned invalid JSON: {error}"))?;
            if value.get("id").and_then(Value::as_u64) != Some(1) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("browser command `{method}` failed: {error}"));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| format!("browser command `{method}` returned no result"));
        }
        Err("browser debugger connection ended unexpectedly".to_string())
    };
    tokio::time::timeout(CDP_TIMEOUT, future)
        .await
        .map_err(|_| format!("browser command `{method}` timed out"))?
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}\n… browser snapshot truncated", &value[..boundary])
}

const SNAPSHOT_SCRIPT: &str = r#"(() => {
  const visible = (element) => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
  };
  document.querySelectorAll("[data-halv-ref]").forEach((element) => element.removeAttribute("data-halv-ref"));
  const selector = [
    "a[href]", "button", "input", "textarea", "select", "summary",
    "[contenteditable='true']", "[role='button']", "[role='link']", "[role='textbox']",
    "[role='checkbox']", "[role='radio']", "[role='tab']", "[tabindex]"
  ].join(",");
  const elements = Array.from(document.querySelectorAll(selector))
    .filter((element) => visible(element) && !element.disabled)
    .slice(0, 400)
    .map((element, index) => {
      const ref = `r${index}`;
      element.setAttribute("data-halv-ref", ref);
      const labelledBy = element.getAttribute("aria-labelledby");
      const labelledText = labelledBy
        ? labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.innerText || "").join(" ")
        : "";
      const name = element.getAttribute("aria-label") || labelledText || element.getAttribute("title") || element.getAttribute("placeholder") || element.innerText || element.value || "";
      return {
        ref,
        tag: element.tagName.toLowerCase(),
        role: element.getAttribute("role") || null,
        name: String(name).replace(/\s+/g, " ").trim().slice(0, 300),
        value: "value" in element ? String(element.value).slice(0, 500) : null,
        href: element.href || null,
        checked: "checked" in element ? Boolean(element.checked) : null
      };
    });
  return {
    url: location.href,
    title: document.title,
    readyState: document.readyState,
    text: (document.body?.innerText || "").replace(/\n{3,}/g, "\n\n").slice(0, 30000),
    elements,
    hint: "Use the current refs with browser_click or browser_type. Take a new snapshot after navigation or DOM changes."
  };
})()"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener, thread};

    #[test]
    fn browser_urls_reject_credentials_and_non_http_schemes() {
        assert!(validate_browser_url("https://example.com/path").is_ok());
        assert!(validate_browser_url("file:///etc/passwd").is_err());
        assert!(validate_browser_url("https://user:secret@example.com").is_err());
    }

    #[test]
    fn browser_refs_are_intentionally_narrow() {
        assert!(validate_reference("r0").is_ok());
        assert!(validate_reference("r1234").is_ok());
        assert!(validate_reference("button-1").is_err());
        assert!(validate_reference("r-1").is_err());
    }

    #[test]
    fn rrule_sized_snapshots_truncate_at_utf8_boundaries() {
        let value = format!("{}🟢", "a".repeat(10));
        let truncated = truncate_utf8(&value, 11);
        assert!(truncated.starts_with("aaaaaaaaaa"));
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn browser_settings_normalize_site_rules_and_validate_style_adjustments() {
        let mut settings = BrowserSettings {
            site_access: "always_ask".to_string(),
            site_rules: vec![
                BrowserSiteRule {
                    hostname: "Example.COM.".to_string(),
                    decision: "allow".to_string(),
                },
                BrowserSiteRule {
                    hostname: "example.com".to_string(),
                    decision: "block".to_string(),
                },
            ],
            full_cdp_access: false,
        };
        validate_browser_settings(&mut settings).expect("settings should validate");
        assert_eq!(settings.site_rules.len(), 1);
        assert_eq!(settings.site_rules[0].hostname, "example.com");
        assert_eq!(settings.site_rules[0].decision, "block");
        assert!(validate_style_preview(&BrowserStylePreview {
            font_size: Some(16.0),
            color: Some("#69b596".to_string()),
            padding: Some(8.0),
            margin: None,
        })
        .is_ok());
        assert!(validate_style_preview(&BrowserStylePreview {
            font_size: Some(500.0),
            color: None,
            padding: None,
            margin: None,
        })
        .is_err());
    }

    #[tokio::test]
    async fn browser_permissions_are_scoped_to_host_and_persisted_rules() {
        let data_dir = env::temp_dir().join(format!(
            "halv-browser-permissions-{}",
            crate::storage::new_uuid_v4()
        ));
        let manager = BrowserManager::default();
        manager
            .set_app_data_dir(data_dir.clone())
            .expect("browser should initialize");
        let request = manager
            .permission_request(
                "browser_open",
                &json!({ "url": "https://example.com" }).to_string(),
            )
            .await
            .expect("permission should resolve");
        assert_eq!(request.hostname.as_deref(), Some("example.com"));
        assert!(!request.site_allowed);
        manager
            .approve_session("conversation-a", request.hostname.as_deref())
            .await;
        assert!(
            manager
                .session_is_approved("conversation-a", Some("example.com"))
                .await
        );
        assert!(
            !manager
                .session_is_approved("conversation-b", Some("example.com"))
                .await
        );

        manager
            .update_settings(BrowserSettings {
                site_access: "always_ask".to_string(),
                site_rules: vec![BrowserSiteRule {
                    hostname: "example.com".to_string(),
                    decision: "block".to_string(),
                }],
                full_cdp_access: false,
            })
            .await
            .expect("settings should save");
        let blocked = manager
            .permission_request(
                "browser_open",
                &json!({ "url": "https://example.com" }).to_string(),
            )
            .await
            .expect("permission should resolve");
        assert!(blocked.blocked);
        drop(manager);

        let reopened = BrowserManager::default();
        reopened
            .set_app_data_dir(data_dir.clone())
            .expect("saved browser settings should load");
        assert_eq!(reopened.settings().await.site_rules[0].decision, "block");
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    #[ignore = "requires an installed Chromium browser"]
    async fn managed_browser_can_open_snapshot_type_and_capture() {
        let data_dir = env::temp_dir().join(format!(
            "halv-browser-live-{}",
            crate::storage::new_uuid_v4()
        ));
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let body = r#"<!doctype html><title>Halv Browser Test</title><main><label>Name <input placeholder="Your name"></label><button>Save</button><p>Browser bridge ready.</p></main>"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let manager = BrowserManager::default();
        manager
            .set_app_data_dir(data_dir.clone())
            .expect("browser should initialize");
        manager
            .execute(
                "browser_open",
                &json!({ "url": format!("http://{address}") }).to_string(),
            )
            .await
            .expect("page should open");
        let snapshot = manager
            .execute("browser_snapshot", "{}")
            .await
            .expect("snapshot should complete");
        assert!(snapshot.text.contains("Browser bridge ready"));
        assert!(snapshot.text.contains("\"ref\": \"r0\""));
        manager
            .execute(
                "browser_type",
                &json!({ "ref": "r0", "text": "Halv" }).to_string(),
            )
            .await
            .expect("typing should complete");
        let screenshot = manager
            .execute("browser_screenshot", "{}")
            .await
            .expect("screenshot should complete");
        assert!(screenshot.screenshot_png.is_some());
        drop(manager);
        let _ = fs::remove_dir_all(data_dir);
    }
}
