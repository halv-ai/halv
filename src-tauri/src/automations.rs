use crate::storage::Storage;
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};

const CONFIG_FILE: &str = "automations.json";
const AUTOMATION_DUE_EVENT: &str = "automation_due";
const SCHEDULER_INTERVAL: Duration = Duration::from_secs(15);
const MAX_AUTOMATIONS: usize = 100;
const MAX_RUN_HISTORY: usize = 50;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_RRULE_BYTES: usize = 512;
type AutomationResult<T> = Result<T, String>;

fn default_timezone() -> String {
    "UTC".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTarget {
    Local,
    Cloud,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationWorkspaceStrategy {
    #[default]
    Current,
    NewWorktree,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationConversationMode {
    #[default]
    New,
    Existing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRun {
    pub id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub conversation_id: Option<String>,
    pub cloud_task_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub unread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub project_id: i64,
    pub workspace_id: Option<String>,
    pub model: String,
    pub effort: String,
    pub target: AutomationTarget,
    pub repository_url: Option<String>,
    pub base_ref: Option<String>,
    pub cadence_minutes: u64,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub workspace_strategy: AutomationWorkspaceStrategy,
    #[serde(default)]
    pub conversation_mode: AutomationConversationMode,
    #[serde(default)]
    pub conversation_id: Option<String>,
    pub enabled: bool,
    pub next_run_at: String,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub cloud_task_id: Option<String>,
    #[serde(default)]
    pub runs: Vec<AutomationRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationDraft {
    pub id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub project_id: i64,
    pub workspace_id: Option<String>,
    pub model: String,
    pub effort: String,
    pub target: AutomationTarget,
    pub repository_url: Option<String>,
    pub base_ref: Option<String>,
    pub cadence_minutes: u64,
    pub rrule: Option<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub workspace_strategy: AutomationWorkspaceStrategy,
    #[serde(default)]
    pub conversation_mode: AutomationConversationMode,
    pub conversation_id: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudTaskRequest {
    automation_id: String,
    prompt: String,
    repository_url: String,
    base_ref: Option<String>,
    model: String,
    effort: String,
}

#[derive(Debug, Deserialize)]
struct CloudTaskResponse {
    id: String,
}

#[derive(Clone, Default)]
pub struct AutomationManager {
    inner: Arc<Mutex<AutomationManagerInner>>,
}

#[derive(Default)]
struct AutomationManagerInner {
    app_data_dir: Option<PathBuf>,
    automations: Vec<Automation>,
}

impl AutomationManager {
    pub fn set_app_data_dir(&self, path: PathBuf) -> AutomationResult<()> {
        fs::create_dir_all(&path)
            .map_err(|error| format!("failed to create automation data directory: {error}"))?;
        let automations = load_config(&path)?;
        let mut inner = self.inner.lock().expect("automation manager lock poisoned");
        inner.app_data_dir = Some(path);
        inner.automations = automations;
        Ok(())
    }

    fn read<T>(&self, read: impl FnOnce(&[Automation]) -> T) -> T {
        read(
            &self
                .inner
                .lock()
                .expect("automation manager lock poisoned")
                .automations,
        )
    }

    fn update<T>(
        &self,
        update: impl FnOnce(&mut Vec<Automation>) -> AutomationResult<T>,
    ) -> AutomationResult<T> {
        let mut inner = self.inner.lock().expect("automation manager lock poisoned");
        let data_dir = inner
            .app_data_dir
            .clone()
            .ok_or_else(|| "automation manager is not initialized".to_string())?;
        let value = update(&mut inner.automations)?;
        save_config(&data_dir, &inner.automations)?;
        Ok(value)
    }

    fn claim_due(&self, now: DateTime<Utc>) -> AutomationResult<Vec<Automation>> {
        self.update(|automations| {
            let mut due = Vec::new();
            for automation in automations.iter_mut().filter(|item| item.enabled) {
                let next = DateTime::parse_from_rfc3339(&automation.next_run_at)
                    .map_err(|_| {
                        format!("automation `{}` has an invalid schedule", automation.name)
                    })?
                    .with_timezone(&Utc);
                if next > now {
                    continue;
                }
                automation.last_run_at = Some(now.to_rfc3339());
                automation.last_status = Some("running".to_string());
                automation.last_error = None;
                automation.runs.push(AutomationRun {
                    id: crate::storage::new_uuid_v4(),
                    started_at: now.to_rfc3339(),
                    finished_at: None,
                    status: "running".to_string(),
                    error: None,
                    conversation_id: None,
                    cloud_task_id: None,
                    workspace_id: None,
                    unread: false,
                });
                trim_run_history(&mut automation.runs);
                automation.updated_at = now.to_rfc3339();
                automation.next_run_at = next_run_for_automation(automation, now)?;
                due.push(automation.clone());
            }
            Ok(due)
        })
    }

    fn report(
        &self,
        id: &str,
        status: &str,
        error: Option<String>,
        cloud_task_id: Option<String>,
        conversation_id: Option<String>,
        workspace_id: Option<String>,
    ) -> AutomationResult<Automation> {
        self.update(|automations| {
            let automation = automations
                .iter_mut()
                .find(|automation| automation.id == id)
                .ok_or_else(|| "automation was not found".to_string())?;
            automation.last_status = Some(status.to_string());
            automation.last_error = error.clone();
            if cloud_task_id.is_some() {
                automation.cloud_task_id = cloud_task_id.clone();
            }
            let now = Utc::now().to_rfc3339();
            if let Some(run) = automation
                .runs
                .iter_mut()
                .rev()
                .find(|run| run.status == "running")
            {
                run.status = status.to_string();
                run.error = error;
                run.cloud_task_id = cloud_task_id;
                run.conversation_id = conversation_id;
                run.workspace_id = workspace_id;
                run.finished_at = Some(now.clone());
                run.unread = true;
            } else {
                automation.runs.push(AutomationRun {
                    id: crate::storage::new_uuid_v4(),
                    started_at: automation
                        .last_run_at
                        .clone()
                        .unwrap_or_else(|| now.clone()),
                    finished_at: Some(now.clone()),
                    status: status.to_string(),
                    error,
                    conversation_id,
                    cloud_task_id,
                    workspace_id,
                    unread: true,
                });
                trim_run_history(&mut automation.runs);
            }
            automation.updated_at = now;
            Ok(automation.clone())
        })
    }
}

pub fn spawn_scheduler(app: AppHandle, manager: AutomationManager) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(SCHEDULER_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let due = match manager.claim_due(Utc::now()) {
                Ok(due) => due,
                Err(error) => {
                    eprintln!("automation scheduler failed: {error}");
                    continue;
                }
            };
            for automation in due {
                match automation.target {
                    AutomationTarget::Local => {
                        if let Err(error) = app.emit(AUTOMATION_DUE_EVENT, &automation) {
                            let _ = manager.report(
                                &automation.id,
                                "failed",
                                Some(format!("failed to dispatch local automation: {error}")),
                                None,
                                None,
                                None,
                            );
                        }
                    }
                    AutomationTarget::Cloud => {
                        let result = enqueue_cloud_task(&app, &automation).await;
                        let _ = match result {
                            Ok(task_id) => manager.report(
                                &automation.id,
                                "queued_cloud",
                                None,
                                Some(task_id),
                                None,
                                None,
                            ),
                            Err(error) => manager.report(
                                &automation.id,
                                "failed",
                                Some(error),
                                None,
                                None,
                                None,
                            ),
                        };
                    }
                }
            }
        }
    });
}

#[tauri::command]
pub async fn automation_list(
    manager: tauri::State<'_, AutomationManager>,
) -> AutomationResult<Vec<Automation>> {
    Ok(manager.read(|automations| {
        let mut rows = automations.to_vec();
        rows.sort_by(|a, b| a.next_run_at.cmp(&b.next_run_at));
        rows
    }))
}

#[tauri::command]
pub async fn automation_upsert(
    manager: tauri::State<'_, AutomationManager>,
    draft: AutomationDraft,
) -> AutomationResult<Automation> {
    validate_draft(&draft)?;
    manager.update(|automations| {
        let now = Utc::now();
        let existing = draft
            .id
            .as_deref()
            .and_then(|id| automations.iter().find(|automation| automation.id == id))
            .cloned();
        let rrule = normalize_rrule(draft.rrule.as_deref())?;
        let created_at = existing
            .as_ref()
            .map(|automation| automation.created_at.clone())
            .unwrap_or_else(|| now.to_rfc3339());
        let timezone = normalize_timezone(&draft.timezone)?;
        let schedule_changed = existing.as_ref().is_some_and(|automation| {
            automation.cadence_minutes != draft.cadence_minutes
                || automation.rrule != rrule
                || automation.timezone != timezone
        });
        let next_run_at = if let Some(automation) = existing.as_ref().filter(|_| !schedule_changed)
        {
            automation.next_run_at.clone()
        } else {
            next_run_at(
                rrule.as_deref(),
                draft.cadence_minutes,
                &timezone,
                parse_rfc3339_utc(&created_at).unwrap_or(now),
                now,
            )?
        };
        let automation = Automation {
            id: existing
                .as_ref()
                .map(|automation| automation.id.clone())
                .unwrap_or_else(crate::storage::new_uuid_v4),
            name: draft.name.trim().to_string(),
            prompt: draft.prompt.trim().to_string(),
            project_id: draft.project_id,
            workspace_id: draft.workspace_id.filter(|value| !value.trim().is_empty()),
            model: draft.model.trim().to_string(),
            effort: draft.effort.trim().to_string(),
            target: draft.target,
            repository_url: draft
                .repository_url
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            base_ref: draft
                .base_ref
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            cadence_minutes: draft.cadence_minutes,
            rrule,
            timezone,
            workspace_strategy: draft.workspace_strategy,
            conversation_mode: draft.conversation_mode,
            conversation_id: draft
                .conversation_id
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            enabled: draft.enabled,
            next_run_at,
            last_run_at: existing
                .as_ref()
                .and_then(|automation| automation.last_run_at.clone()),
            last_status: existing
                .as_ref()
                .and_then(|automation| automation.last_status.clone()),
            last_error: existing
                .as_ref()
                .and_then(|automation| automation.last_error.clone()),
            cloud_task_id: existing
                .as_ref()
                .and_then(|automation| automation.cloud_task_id.clone()),
            runs: existing
                .as_ref()
                .map(|automation| automation.runs.clone())
                .unwrap_or_default(),
            created_at,
            updated_at: now.to_rfc3339(),
        };
        if existing.is_none() && automations.len() >= MAX_AUTOMATIONS {
            return Err("automation limit reached".to_string());
        }
        automations.retain(|item| item.id != automation.id);
        automations.push(automation.clone());
        Ok(automation)
    })
}

#[tauri::command]
pub async fn automation_set_enabled(
    manager: tauri::State<'_, AutomationManager>,
    automation_id: String,
    enabled: bool,
) -> AutomationResult<Automation> {
    manager.update(|automations| {
        let automation = automations
            .iter_mut()
            .find(|automation| automation.id == automation_id)
            .ok_or_else(|| "automation was not found".to_string())?;
        automation.enabled = enabled;
        automation.updated_at = Utc::now().to_rfc3339();
        Ok(automation.clone())
    })
}

#[tauri::command]
pub async fn automation_run_now(
    manager: tauri::State<'_, AutomationManager>,
    automation_id: String,
) -> AutomationResult<Automation> {
    manager.update(|automations| {
        let automation = automations
            .iter_mut()
            .find(|automation| automation.id == automation_id)
            .ok_or_else(|| "automation was not found".to_string())?;
        automation.enabled = true;
        automation.next_run_at = Utc::now().to_rfc3339();
        automation.updated_at = Utc::now().to_rfc3339();
        Ok(automation.clone())
    })
}

#[tauri::command]
pub async fn automation_delete(
    manager: tauri::State<'_, AutomationManager>,
    automation_id: String,
) -> AutomationResult<()> {
    manager.update(|automations| {
        let before = automations.len();
        automations.retain(|automation| automation.id != automation_id);
        (before != automations.len())
            .then_some(())
            .ok_or_else(|| "automation was not found".to_string())
    })
}

#[tauri::command]
pub async fn automation_report_run(
    manager: tauri::State<'_, AutomationManager>,
    automation_id: String,
    status: String,
    error: Option<String>,
    conversation_id: Option<String>,
    workspace_id: Option<String>,
) -> AutomationResult<Automation> {
    if !matches!(status.as_str(), "completed" | "failed") {
        return Err("invalid automation result status".to_string());
    }
    manager.report(
        &automation_id,
        &status,
        error,
        None,
        conversation_id,
        workspace_id,
    )
}

#[tauri::command]
pub async fn automation_unread_count(
    manager: tauri::State<'_, AutomationManager>,
) -> AutomationResult<usize> {
    Ok(manager.read(|automations| {
        automations
            .iter()
            .flat_map(|automation| automation.runs.iter())
            .filter(|run| run.unread)
            .count()
    }))
}

#[tauri::command]
pub async fn automation_mark_run_read(
    manager: tauri::State<'_, AutomationManager>,
    automation_id: String,
    run_id: String,
) -> AutomationResult<()> {
    manager.update(|automations| {
        let automation = automations
            .iter_mut()
            .find(|automation| automation.id == automation_id)
            .ok_or_else(|| "automation was not found".to_string())?;
        let run = automation
            .runs
            .iter_mut()
            .find(|run| run.id == run_id)
            .ok_or_else(|| "automation run was not found".to_string())?;
        run.unread = false;
        automation.updated_at = Utc::now().to_rfc3339();
        Ok(())
    })
}

#[tauri::command]
pub async fn automation_mark_all_read(
    manager: tauri::State<'_, AutomationManager>,
) -> AutomationResult<()> {
    manager.update(|automations| {
        let now = Utc::now().to_rfc3339();
        for automation in automations {
            let mut changed = false;
            for run in &mut automation.runs {
                if run.unread {
                    run.unread = false;
                    changed = true;
                }
            }
            if changed {
                automation.updated_at = now.clone();
            }
        }
        Ok(())
    })
}

fn trim_run_history(runs: &mut Vec<AutomationRun>) {
    if runs.len() > MAX_RUN_HISTORY {
        runs.drain(..runs.len() - MAX_RUN_HISTORY);
    }
}

async fn enqueue_cloud_task(app: &AppHandle, automation: &Automation) -> AutomationResult<String> {
    let repository_url = automation
        .repository_url
        .clone()
        .ok_or_else(|| "cloud automation requires a repository URL".to_string())?;
    let storage = app.state::<Storage>();
    let session = storage
        .get_auth_session()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "sign in to Halv before using cloud handoff".to_string())?;
    let token = session
        .token
        .ok_or_else(|| "sign in to Halv before using cloud handoff".to_string())?;
    let url = format!(
        "{}/v1/cloud/tasks",
        session.server_url.trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(&CloudTaskRequest {
            automation_id: automation.id.clone(),
            prompt: automation.prompt.clone(),
            repository_url,
            base_ref: automation.base_ref.clone(),
            model: automation.model.clone(),
            effort: automation.effort.clone(),
        })
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("cloud handoff failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("cloud handoff returned HTTP {status}"));
    }
    response
        .json::<CloudTaskResponse>()
        .await
        .map(|task| task.id)
        .map_err(|error| format!("invalid cloud handoff response: {error}"))
}

fn validate_draft(draft: &AutomationDraft) -> AutomationResult<()> {
    if draft.name.trim().is_empty() || draft.name.len() > 120 {
        return Err("automation name is required".to_string());
    }
    if draft.prompt.trim().is_empty() || draft.prompt.len() > MAX_PROMPT_BYTES {
        return Err("automation prompt is empty or too large".to_string());
    }
    if draft.model.trim().is_empty() || draft.effort.trim().is_empty() {
        return Err("automation model and effort are required".to_string());
    }
    if !(1..=525_600).contains(&draft.cadence_minutes) {
        return Err("automation cadence must be between 1 minute and 1 year".to_string());
    }
    normalize_timezone(&draft.timezone)?;
    normalize_rrule(draft.rrule.as_deref())?;
    if matches!(
        draft.conversation_mode,
        AutomationConversationMode::Existing
    ) && draft
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err("same-conversation automations require a conversation".to_string());
    }
    if matches!(
        draft.conversation_mode,
        AutomationConversationMode::Existing
    ) && matches!(
        draft.workspace_strategy,
        AutomationWorkspaceStrategy::NewWorktree
    ) {
        return Err(
            "same-conversation automations cannot create a new worktree for every run".to_string(),
        );
    }
    if matches!(draft.target, AutomationTarget::Cloud)
        && (!matches!(
            draft.workspace_strategy,
            AutomationWorkspaceStrategy::Current
        ) || !matches!(draft.conversation_mode, AutomationConversationMode::New))
    {
        return Err(
            "cloud handoff does not support local worktrees or existing conversations".to_string(),
        );
    }
    if matches!(draft.target, AutomationTarget::Cloud) {
        let url = draft
            .repository_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "cloud automation requires a repository URL".to_string())?;
        let parsed =
            url::Url::parse(url).map_err(|error| format!("invalid repository URL: {error}"))?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("github.com") {
            return Err("cloud repository must be an HTTPS GitHub URL".to_string());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RruleFrequency {
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedRrule {
    frequency: RruleFrequency,
    interval: i64,
    by_minute: Vec<u32>,
    by_hour: Vec<u32>,
    by_day: Vec<Weekday>,
    by_month_day: Vec<u32>,
}

fn normalize_timezone(value: &str) -> AutomationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(default_timezone());
    }
    value
        .parse::<Tz>()
        .map(|timezone| timezone.name().to_string())
        .map_err(|_| format!("unknown IANA time zone `{value}`"))
}

fn normalize_rrule(value: Option<&str>) -> AutomationResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > MAX_RRULE_BYTES {
        return Err("RRULE is too large".to_string());
    }
    let normalized = value
        .strip_prefix("RRULE:")
        .or_else(|| value.strip_prefix("rrule:"))
        .unwrap_or(value)
        .trim()
        .to_ascii_uppercase();
    parse_rrule(&normalized)?;
    Ok(Some(format!("RRULE:{normalized}")))
}

fn parse_rrule(value: &str) -> AutomationResult<ParsedRrule> {
    let value = value.strip_prefix("RRULE:").unwrap_or(value);
    let mut parts = BTreeMap::<String, String>::new();
    for part in value.split(';') {
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| format!("invalid RRULE field `{part}`"))?;
        let key = key.trim().to_ascii_uppercase();
        let value = value.trim().to_ascii_uppercase();
        if key.is_empty() || value.is_empty() || parts.insert(key.clone(), value).is_some() {
            return Err(format!("invalid or repeated RRULE field `{key}`"));
        }
    }
    for key in parts.keys() {
        if !matches!(
            key.as_str(),
            "FREQ" | "INTERVAL" | "BYMINUTE" | "BYHOUR" | "BYDAY" | "BYMONTHDAY" | "WKST"
        ) {
            return Err(format!(
                "RRULE field `{key}` is not supported; use FREQ, INTERVAL, BYMINUTE, BYHOUR, BYDAY, or BYMONTHDAY"
            ));
        }
    }
    if let Some(week_start) = parts.get("WKST") {
        if week_start != "MO" {
            return Err("only WKST=MO is supported".to_string());
        }
    }
    let frequency = match parts.get("FREQ").map(String::as_str) {
        Some("MINUTELY") => RruleFrequency::Minutely,
        Some("HOURLY") => RruleFrequency::Hourly,
        Some("DAILY") => RruleFrequency::Daily,
        Some("WEEKLY") => RruleFrequency::Weekly,
        Some("MONTHLY") => RruleFrequency::Monthly,
        Some(value) => return Err(format!("RRULE frequency `{value}` is not supported")),
        None => return Err("RRULE requires FREQ".to_string()),
    };
    let interval = parts
        .get("INTERVAL")
        .map(|value| value.parse::<i64>())
        .transpose()
        .map_err(|_| "RRULE INTERVAL must be a positive integer".to_string())?
        .unwrap_or(1);
    if !(1..=525_600).contains(&interval) {
        return Err("RRULE INTERVAL must be between 1 and 525600".to_string());
    }
    Ok(ParsedRrule {
        frequency,
        interval,
        by_minute: parse_u32_list(parts.get("BYMINUTE"), 0, 59, "BYMINUTE")?,
        by_hour: parse_u32_list(parts.get("BYHOUR"), 0, 23, "BYHOUR")?,
        by_day: parse_weekdays(parts.get("BYDAY"))?,
        by_month_day: parse_u32_list(parts.get("BYMONTHDAY"), 1, 31, "BYMONTHDAY")?,
    })
}

fn parse_u32_list(
    value: Option<&String>,
    minimum: u32,
    maximum: u32,
    label: &str,
) -> AutomationResult<Vec<u32>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut values = value
        .split(',')
        .map(|item| {
            item.parse::<u32>()
                .map_err(|_| format!("RRULE {label} contains an invalid number"))
        })
        .collect::<AutomationResult<Vec<_>>>()?;
    if values.is_empty()
        || values
            .iter()
            .any(|value| !(minimum..=maximum).contains(value))
    {
        return Err(format!(
            "RRULE {label} values must be between {minimum} and {maximum}"
        ));
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn parse_weekdays(value: Option<&String>) -> AutomationResult<Vec<Weekday>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut weekdays = value
        .split(',')
        .map(|item| match item {
            "MO" => Ok(Weekday::Mon),
            "TU" => Ok(Weekday::Tue),
            "WE" => Ok(Weekday::Wed),
            "TH" => Ok(Weekday::Thu),
            "FR" => Ok(Weekday::Fri),
            "SA" => Ok(Weekday::Sat),
            "SU" => Ok(Weekday::Sun),
            _ => Err(format!(
                "RRULE BYDAY `{item}` is invalid; ordinal weekdays are not supported"
            )),
        })
        .collect::<AutomationResult<Vec<_>>>()?;
    if weekdays.is_empty() {
        return Err("RRULE BYDAY cannot be empty".to_string());
    }
    weekdays.sort_by_key(|weekday| weekday.num_days_from_monday());
    weekdays.dedup();
    Ok(weekdays)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn next_run_for_automation(
    automation: &Automation,
    after: DateTime<Utc>,
) -> AutomationResult<String> {
    let anchor = parse_rfc3339_utc(&automation.created_at).unwrap_or(after);
    next_run_at(
        automation.rrule.as_deref(),
        automation.cadence_minutes,
        &automation.timezone,
        anchor,
        after,
    )
}

fn next_run_at(
    rrule: Option<&str>,
    cadence_minutes: u64,
    timezone: &str,
    anchor: DateTime<Utc>,
    after: DateTime<Utc>,
) -> AutomationResult<String> {
    let Some(rrule) = rrule else {
        return Ok((after + ChronoDuration::minutes(cadence_minutes as i64)).to_rfc3339());
    };
    let rule = parse_rrule(rrule)?;
    let timezone = timezone
        .parse::<Tz>()
        .map_err(|_| format!("unknown IANA time zone `{timezone}`"))?;
    let anchor_local = anchor.with_timezone(&timezone);
    let mut candidate = (after + ChronoDuration::minutes(1))
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .ok_or_else(|| "failed to calculate the next schedule".to_string())?;
    let deadline = after + ChronoDuration::days(400);
    while candidate <= deadline {
        let local = candidate.with_timezone(&timezone);
        if rrule_matches(&rule, &anchor_local, &local) {
            return Ok(candidate.to_rfc3339());
        }
        candidate += ChronoDuration::minutes(1);
    }
    Err("RRULE did not produce a run within the next 400 days".to_string())
}

fn rrule_matches(rule: &ParsedRrule, anchor: &DateTime<Tz>, candidate: &DateTime<Tz>) -> bool {
    if candidate < anchor {
        return false;
    }
    let minute_matches = if rule.by_minute.is_empty() {
        matches!(rule.frequency, RruleFrequency::Minutely) || candidate.minute() == anchor.minute()
    } else {
        rule.by_minute.contains(&candidate.minute())
    };
    let hour_matches = if rule.by_hour.is_empty() {
        matches!(
            rule.frequency,
            RruleFrequency::Minutely | RruleFrequency::Hourly
        ) || candidate.hour() == anchor.hour()
    } else {
        rule.by_hour.contains(&candidate.hour())
    };
    let weekday_matches = if rule.by_day.is_empty() {
        !matches!(rule.frequency, RruleFrequency::Weekly) || candidate.weekday() == anchor.weekday()
    } else {
        rule.by_day.contains(&candidate.weekday())
    };
    let month_day_matches = if rule.by_month_day.is_empty() {
        !matches!(rule.frequency, RruleFrequency::Monthly) || candidate.day() == anchor.day()
    } else {
        rule.by_month_day.contains(&candidate.day())
    };
    if !(minute_matches && hour_matches && weekday_matches && month_day_matches) {
        return false;
    }

    let interval_index = match rule.frequency {
        RruleFrequency::Minutely => candidate.signed_duration_since(*anchor).num_minutes(),
        RruleFrequency::Hourly => candidate.signed_duration_since(*anchor).num_hours(),
        RruleFrequency::Daily => candidate
            .date_naive()
            .signed_duration_since(anchor.date_naive())
            .num_days(),
        RruleFrequency::Weekly => candidate
            .date_naive()
            .signed_duration_since(anchor.date_naive())
            .num_days()
            .div_euclid(7),
        RruleFrequency::Monthly => {
            i64::from(candidate.year() - anchor.year()) * 12 + i64::from(candidate.month())
                - i64::from(anchor.month())
        }
    };
    interval_index >= 0 && interval_index % rule.interval == 0
}

fn load_config(data_dir: &Path) -> AutomationResult<Vec<Automation>> {
    let path = data_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("failed to read automations: {error}"))?,
    )
    .map_err(|error| format!("invalid automation config: {error}"))
}

fn save_config(data_dir: &Path, automations: &[Automation]) -> AutomationResult<()> {
    let temp = data_dir.join(format!(".{CONFIG_FILE}.tmp-{:016x}", rand::random::<u64>()));
    let mut file = fs::File::create(&temp)
        .map_err(|error| format!("failed to create automation config: {error}"))?;
    file.write_all(
        &serde_json::to_vec_pretty(automations)
            .map_err(|error| format!("failed to encode automations: {error}"))?,
    )
    .and_then(|()| file.sync_all())
    .map_err(|error| format!("failed to write automations: {error}"))?;
    fs::rename(temp, data_dir.join(CONFIG_FILE))
        .map_err(|error| format!("failed to replace automation config: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn draft(target: AutomationTarget) -> AutomationDraft {
        AutomationDraft {
            id: None,
            name: "Review nightly".to_string(),
            prompt: "Review the repository".to_string(),
            project_id: 1,
            workspace_id: None,
            model: "gpt-5.5".to_string(),
            effort: "medium".to_string(),
            target,
            repository_url: None,
            base_ref: None,
            cadence_minutes: 60,
            rrule: None,
            timezone: "America/Santiago".to_string(),
            workspace_strategy: AutomationWorkspaceStrategy::Current,
            conversation_mode: AutomationConversationMode::New,
            conversation_id: None,
            enabled: true,
        }
    }

    #[test]
    fn cloud_automations_require_an_https_github_repository() {
        let mut cloud = draft(AutomationTarget::Cloud);
        assert!(validate_draft(&cloud).is_err());
        cloud.repository_url = Some("http://github.com/example/repo".to_string());
        assert!(validate_draft(&cloud).is_err());
        cloud.repository_url = Some("https://github.com/example/repo".to_string());
        assert!(validate_draft(&cloud).is_ok());
    }

    #[test]
    fn legacy_automation_files_default_to_empty_run_history() {
        let automation = serde_json::from_value::<Automation>(json!({
            "id": "automation-1",
            "name": "Nightly review",
            "prompt": "Review the repository",
            "projectId": 1,
            "workspaceId": null,
            "model": "gpt-5.5",
            "effort": "medium",
            "target": "local",
            "repositoryUrl": null,
            "baseRef": null,
            "cadenceMinutes": 1440,
            "enabled": true,
            "nextRunAt": "2026-07-14T00:00:00Z",
            "lastRunAt": null,
            "lastStatus": null,
            "lastError": null,
            "cloudTaskId": null,
            "createdAt": "2026-07-13T00:00:00Z",
            "updatedAt": "2026-07-13T00:00:00Z"
        }))
        .expect("legacy automation should deserialize");

        assert!(automation.runs.is_empty());
        assert_eq!(automation.timezone, "UTC");
        assert_eq!(
            automation.workspace_strategy,
            AutomationWorkspaceStrategy::Current
        );
    }

    #[test]
    fn run_history_keeps_the_most_recent_entries() {
        let mut runs = (0..MAX_RUN_HISTORY + 4)
            .map(|index| AutomationRun {
                id: format!("run-{index}"),
                started_at: "2026-07-13T00:00:00Z".to_string(),
                finished_at: None,
                status: "running".to_string(),
                error: None,
                conversation_id: None,
                cloud_task_id: None,
                workspace_id: None,
                unread: false,
            })
            .collect::<Vec<_>>();

        trim_run_history(&mut runs);

        assert_eq!(runs.len(), MAX_RUN_HISTORY);
        assert_eq!(runs.first().map(|run| run.id.as_str()), Some("run-4"));
    }

    #[test]
    fn advanced_rrules_use_the_saved_timezone() {
        let next = next_run_at(
            Some("RRULE:FREQ=DAILY;BYHOUR=9;BYMINUTE=30"),
            1_440,
            "America/Santiago",
            parse_rfc3339_utc("2026-07-13T12:00:00Z").unwrap(),
            parse_rfc3339_utc("2026-07-13T14:00:00Z").unwrap(),
        )
        .expect("schedule should calculate");

        assert_eq!(next, "2026-07-14T13:30:00+00:00");
    }

    #[test]
    fn monthly_rrules_support_month_day_filters() {
        let next = next_run_at(
            Some("RRULE:FREQ=MONTHLY;BYMONTHDAY=1;BYHOUR=9;BYMINUTE=0"),
            1_440,
            "UTC",
            parse_rfc3339_utc("2026-07-13T00:00:00Z").unwrap(),
            parse_rfc3339_utc("2026-07-13T00:00:00Z").unwrap(),
        )
        .expect("schedule should calculate");

        assert_eq!(next, "2026-08-01T09:00:00+00:00");
    }

    #[test]
    fn existing_conversations_cannot_move_between_worktrees() {
        let mut automation = draft(AutomationTarget::Local);
        automation.conversation_mode = AutomationConversationMode::Existing;
        automation.conversation_id = Some("conversation-1".to_string());
        automation.workspace_strategy = AutomationWorkspaceStrategy::NewWorktree;

        assert!(validate_draft(&automation)
            .expect_err("incompatible routing should fail")
            .contains("cannot create a new worktree"));
    }
}
