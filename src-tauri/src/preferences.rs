use crate::{exec::ApprovalPolicy, ponytail::PonytailMode, task_routing::TierAgentAssignments};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::RwLock,
};

const PREFERENCES_FILE_NAME: &str = "preferences.json";

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionNotificationMode {
    Never,
    #[default]
    Background,
    Always,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Personality {
    #[default]
    None,
    Friendly,
    Pragmatic,
}

impl Personality {
    pub fn directive(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Friendly => {
                Some("Be warm, encouraging, and conversational in your explanations.")
            }
            Self::Pragmatic => Some("Be terse, avoid filler, and lead with actions and outcomes."),
        }
    }
}

/// The lane the user forces on every worker. `Auto` follows the coordinator.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkerLanePreference {
    #[default]
    Auto,
    Claude,
    Codex,
}

impl WorkerLanePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentPreferences {
    pub subagent_concurrency_limit: Option<usize>,
    pub completion_notifications: CompletionNotificationMode,
    pub permission_notifications: bool,
    pub question_notifications: bool,
    pub keyboard_shortcuts: BTreeMap<String, String>,
    pub personality: Personality,
    pub ponytail: PonytailMode,
    pub claude_subscription_proxy: bool,
    pub memories_enabled: bool,
    pub task_routing: bool,
    pub worker_lane: WorkerLanePreference,
    pub routing_tier_agents: TierAgentAssignments,
    /// JEV routes the agents of a Claude Code Workflow script (ADR 0014).
    pub workflow_routing: bool,
    /// New Claude panes start with ultracode on.
    pub claude_ultracode: bool,
    pub analytics_enabled: bool,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
    pub default_agent_mode: bool,
    pub default_approval_policy: Option<ApprovalPolicy>,
    pub default_fast_mode: bool,
    /// Downloads a new Halv release in the background. The user still
    /// chooses when Halv restarts.
    pub app_update_auto_download: bool,
    /// The mobile remote gateway is off until the user turns it on (ADR 0009).
    pub remote_enabled: bool,
    /// A relay URL that replaces the default in release builds. `None` uses the default.
    pub remote_relay_url: Option<String>,
    /// A relay URL that replaces the default in debug builds. `None` uses the default.
    /// Each build profile reads only its own key, so a development relay URL never
    /// reaches a release build.
    pub remote_relay_url_dev: Option<String>,
}

impl Default for AgentPreferences {
    fn default() -> Self {
        Self {
            subagent_concurrency_limit: None,
            completion_notifications: CompletionNotificationMode::Background,
            permission_notifications: true,
            question_notifications: true,
            keyboard_shortcuts: BTreeMap::new(),
            personality: Personality::None,
            ponytail: PonytailMode::Off,
            claude_subscription_proxy: true,
            memories_enabled: true,
            task_routing: true,
            worker_lane: WorkerLanePreference::Auto,
            routing_tier_agents: TierAgentAssignments::default(),
            workflow_routing: true,
            claude_ultracode: false,
            analytics_enabled: true,
            default_model: None,
            default_effort: None,
            default_agent_mode: true,
            default_approval_policy: None,
            default_fast_mode: false,
            app_update_auto_download: true,
            remote_enabled: false,
            remote_relay_url: None,
            remote_relay_url_dev: None,
        }
    }
}

#[derive(Debug)]
pub struct PreferencesManager {
    path: PathBuf,
    preferences: RwLock<AgentPreferences>,
}

impl PreferencesManager {
    pub fn open(app_data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(app_data_dir)?;
        let path = app_data_dir.join(PREFERENCES_FILE_NAME);
        let preferences = match fs::read(&path) {
            Ok(contents) => serde_json::from_slice::<AgentPreferences>(&contents)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => AgentPreferences::default(),
            Err(error) => return Err(error),
        };
        validate_subagent_concurrency_limit(preferences.subagent_concurrency_limit)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        validate_keyboard_shortcuts(&preferences.keyboard_shortcuts)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        preferences
            .routing_tier_agents
            .validate_default_catalog()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

        Ok(Self {
            path,
            preferences: RwLock::new(preferences),
        })
    }

    pub fn get(&self) -> Result<AgentPreferences, String> {
        self.preferences
            .read()
            .map(|preferences| preferences.clone())
            .map_err(|_| "Preferences lock is poisoned".to_string())
    }

    pub fn subagent_concurrency_limit(&self) -> Result<Option<usize>, String> {
        self.get()
            .map(|preferences| preferences.subagent_concurrency_limit)
    }

    pub fn personality(&self) -> Result<Personality, String> {
        self.get().map(|preferences| preferences.personality)
    }

    pub fn ponytail(&self) -> Result<PonytailMode, String> {
        self.get().map(|preferences| preferences.ponytail)
    }

    pub fn claude_subscription_proxy(&self) -> Result<bool, String> {
        self.get()
            .map(|preferences| preferences.claude_subscription_proxy)
    }

    pub fn memories_enabled(&self) -> Result<bool, String> {
        self.get().map(|preferences| preferences.memories_enabled)
    }

    pub fn task_routing(&self) -> Result<bool, String> {
        self.get().map(|preferences| preferences.task_routing)
    }

    pub fn worker_lane(&self) -> Result<WorkerLanePreference, String> {
        self.get().map(|preferences| preferences.worker_lane)
    }

    pub fn routing_tier_agents(&self) -> Result<TierAgentAssignments, String> {
        self.get()
            .map(|preferences| preferences.routing_tier_agents.clone())
    }

    pub fn workflow_routing(&self) -> Result<bool, String> {
        self.get().map(|preferences| preferences.workflow_routing)
    }

    pub fn claude_ultracode(&self) -> Result<bool, String> {
        self.get().map(|preferences| preferences.claude_ultracode)
    }

    pub fn analytics_enabled(&self) -> Result<bool, String> {
        self.get().map(|preferences| preferences.analytics_enabled)
    }

    pub fn app_update_auto_download(&self) -> Result<bool, String> {
        self.get()
            .map(|preferences| preferences.app_update_auto_download)
    }

    pub fn set_subagent_concurrency_limit(
        &self,
        limit: Option<usize>,
    ) -> Result<AgentPreferences, String> {
        validate_subagent_concurrency_limit(limit)?;
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.subagent_concurrency_limit = limit;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_notifications(
        &self,
        completion_notifications: CompletionNotificationMode,
        permission_notifications: bool,
        question_notifications: bool,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.completion_notifications = completion_notifications;
        next.permission_notifications = permission_notifications;
        next.question_notifications = question_notifications;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_keyboard_shortcuts(
        &self,
        shortcuts: BTreeMap<String, String>,
    ) -> Result<AgentPreferences, String> {
        validate_keyboard_shortcuts(&shortcuts)?;
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.keyboard_shortcuts = shortcuts;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_personality(&self, personality: Personality) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.personality = personality;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_ponytail(&self, ponytail: PonytailMode) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.ponytail = ponytail;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_claude_subscription_proxy(
        &self,
        claude_subscription_proxy: bool,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.claude_subscription_proxy = claude_subscription_proxy;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_memories_enabled(&self, memories_enabled: bool) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.memories_enabled = memories_enabled;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_task_routing(&self, task_routing: bool) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.task_routing = task_routing;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_workflow_routing(&self, workflow_routing: bool) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.workflow_routing = workflow_routing;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_claude_ultracode(&self, claude_ultracode: bool) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.claude_ultracode = claude_ultracode;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_worker_lane(
        &self,
        worker_lane: WorkerLanePreference,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.worker_lane = worker_lane;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_routing_tier_agents(
        &self,
        routing_tier_agents: TierAgentAssignments,
    ) -> Result<AgentPreferences, String> {
        routing_tier_agents.validate_default_catalog()?;
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.routing_tier_agents = routing_tier_agents;
        next.worker_lane = WorkerLanePreference::Auto;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_analytics_enabled(
        &self,
        analytics_enabled: bool,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.analytics_enabled = analytics_enabled;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_app_update_auto_download(
        &self,
        app_update_auto_download: bool,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.app_update_auto_download = app_update_auto_download;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_remote_enabled(&self, remote_enabled: bool) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.remote_enabled = remote_enabled;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    // The remote gateway validates the URL before it calls this setter.
    pub fn set_remote_relay_url(
        &self,
        remote_relay_url: Option<String>,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.remote_relay_url = remote_relay_url;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    // The relay URL of debug builds. The remote gateway validates the URL first.
    pub fn set_remote_relay_url_dev(
        &self,
        remote_relay_url_dev: Option<String>,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.remote_relay_url_dev = remote_relay_url_dev;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }

    pub fn set_conversation_defaults(
        &self,
        default_model: String,
        default_effort: String,
        default_agent_mode: bool,
        default_approval_policy: Option<ApprovalPolicy>,
        default_fast_mode: bool,
    ) -> Result<AgentPreferences, String> {
        let default_model = normalize_required_preference(default_model, "model")?;
        let default_effort = normalize_required_preference(default_effort, "effort")?;
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.default_model = Some(default_model);
        next.default_effort = Some(default_effort);
        next.default_agent_mode = default_agent_mode;
        next.default_approval_policy = default_approval_policy;
        next.default_fast_mode = default_fast_mode;
        write_preferences_atomically(&self.path, &next).map_err(|error| error.to_string())?;
        *current = next.clone();
        Ok(next)
    }
}

fn normalize_required_preference(value: String, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(format!("Default {label} must contain 1 to 128 bytes"));
    }
    Ok(value.to_string())
}

fn validate_subagent_concurrency_limit(limit: Option<usize>) -> Result<(), String> {
    if limit == Some(0) {
        Err("Subagent concurrency limit must be at least 1".to_string())
    } else {
        Ok(())
    }
}

fn validate_keyboard_shortcuts(shortcuts: &BTreeMap<String, String>) -> Result<(), String> {
    if shortcuts.len() > 64 {
        return Err("Too many keyboard shortcut overrides".to_string());
    }
    for (id, shortcut) in shortcuts {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(format!("Invalid keyboard shortcut id: {id}"));
        }
        if shortcut.is_empty()
            || shortcut.len() > 64
            || !shortcut.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'+' | b'-' | b'[' | b']' | b'/' | b',' | b'.' | b'`')
            })
        {
            return Err(format!("Invalid keyboard shortcut for {id}"));
        }
    }
    Ok(())
}

fn write_preferences_atomically(path: &Path, preferences: &AgentPreferences) -> io::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(preferences)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    serialized.push(b'\n');

    let temporary_path = path.with_extension("json.tmp");
    let result = (|| {
        let mut temporary = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(&serialized)?;
        temporary.sync_all()?;
        drop(temporary);
        replace_preferences_file(&temporary_path, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_preferences_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary_path, path)
}

#[cfg(windows)]
fn replace_preferences_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temporary = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub fn preferences_get(
    manager: tauri::State<'_, PreferencesManager>,
) -> Result<AgentPreferences, String> {
    manager.get()
}

#[tauri::command]
pub fn preferences_set_subagent_concurrency_limit(
    manager: tauri::State<'_, PreferencesManager>,
    limit: Option<usize>,
) -> Result<AgentPreferences, String> {
    manager.set_subagent_concurrency_limit(limit)
}

#[tauri::command]
pub fn preferences_set_notifications(
    manager: tauri::State<'_, PreferencesManager>,
    completion_notifications: CompletionNotificationMode,
    permission_notifications: bool,
    question_notifications: bool,
) -> Result<AgentPreferences, String> {
    manager.set_notifications(
        completion_notifications,
        permission_notifications,
        question_notifications,
    )
}

#[tauri::command]
pub fn preferences_set_keyboard_shortcuts(
    manager: tauri::State<'_, PreferencesManager>,
    shortcuts: BTreeMap<String, String>,
) -> Result<AgentPreferences, String> {
    manager.set_keyboard_shortcuts(shortcuts)
}

#[tauri::command]
pub fn preferences_set_personality(
    manager: tauri::State<'_, PreferencesManager>,
    personality: Personality,
) -> Result<AgentPreferences, String> {
    manager.set_personality(personality)
}

#[tauri::command]
pub fn preferences_set_ponytail(
    manager: tauri::State<'_, PreferencesManager>,
    mode: PonytailMode,
) -> Result<AgentPreferences, String> {
    manager.set_ponytail(mode)
}

#[tauri::command]
pub fn preferences_set_claude_subscription_proxy(
    manager: tauri::State<'_, PreferencesManager>,
    claude_subscription_proxy: bool,
) -> Result<AgentPreferences, String> {
    manager.set_claude_subscription_proxy(claude_subscription_proxy)
}

#[tauri::command]
pub fn preferences_set_memories_enabled(
    manager: tauri::State<'_, PreferencesManager>,
    memories_enabled: bool,
) -> Result<AgentPreferences, String> {
    manager.set_memories_enabled(memories_enabled)
}

#[tauri::command]
pub fn preferences_set_task_routing(
    manager: tauri::State<'_, PreferencesManager>,
    task_routing: bool,
) -> Result<AgentPreferences, String> {
    manager.set_task_routing(task_routing)
}

#[tauri::command]
pub fn preferences_set_workflow_routing(
    manager: tauri::State<'_, PreferencesManager>,
    workflow_routing: bool,
) -> Result<AgentPreferences, String> {
    manager.set_workflow_routing(workflow_routing)
}

#[tauri::command]
pub fn preferences_set_claude_ultracode(
    manager: tauri::State<'_, PreferencesManager>,
    claude_ultracode: bool,
) -> Result<AgentPreferences, String> {
    manager.set_claude_ultracode(claude_ultracode)
}

#[tauri::command]
pub fn preferences_set_worker_lane(
    manager: tauri::State<'_, PreferencesManager>,
    worker_lane: WorkerLanePreference,
) -> Result<AgentPreferences, String> {
    manager.set_worker_lane(worker_lane)
}

#[tauri::command]
pub fn preferences_set_routing_tier_agents(
    manager: tauri::State<'_, PreferencesManager>,
    routing_tier_agents: TierAgentAssignments,
) -> Result<AgentPreferences, String> {
    manager.set_routing_tier_agents(routing_tier_agents)
}

#[tauri::command]
pub fn preferences_set_analytics_enabled(
    manager: tauri::State<'_, PreferencesManager>,
    analytics: tauri::State<'_, crate::analytics::AnalyticsManager>,
    analytics_enabled: bool,
) -> Result<AgentPreferences, String> {
    let preferences = manager.set_analytics_enabled(analytics_enabled)?;
    if !analytics_enabled {
        analytics.clear_pending()?;
    }
    Ok(preferences)
}

#[tauri::command]
pub fn preferences_set_conversation_defaults(
    manager: tauri::State<'_, PreferencesManager>,
    default_model: String,
    default_effort: String,
    default_agent_mode: bool,
    default_approval_policy: Option<ApprovalPolicy>,
    default_fast_mode: bool,
) -> Result<AgentPreferences, String> {
    manager.set_conversation_defaults(
        default_model,
        default_effort,
        default_agent_mode,
        default_approval_policy,
        default_fast_mode,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("halv-preferences-{name}-{}", rand::random::<u64>()));
            fs::create_dir_all(&path).expect("create preferences test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn task_routing_defaults_on_but_preserves_explicit_opt_out() {
        assert!(AgentPreferences::default().task_routing);
        assert!(
            serde_json::from_str::<AgentPreferences>("{}")
                .unwrap()
                .task_routing
        );
        assert!(
            !serde_json::from_str::<AgentPreferences>(r#"{"taskRouting":false}"#)
                .unwrap()
                .task_routing
        );
    }

    #[test]
    fn workflow_routing_defaults_on_and_ultracode_defaults_off() {
        let defaults = serde_json::from_str::<AgentPreferences>("{}").unwrap();
        assert!(defaults.workflow_routing);
        assert!(!defaults.claude_ultracode);
        let directory = TestDirectory::new("workflow-routing");
        let manager = PreferencesManager::open(&directory.0).unwrap();
        manager.set_workflow_routing(false).unwrap();
        manager.set_claude_ultracode(true).unwrap();
        let reopened = PreferencesManager::open(&directory.0).unwrap();
        assert!(!reopened.workflow_routing().unwrap());
        assert!(reopened.claude_ultracode().unwrap());
        let saved = serde_json::to_value(reopened.get().unwrap()).unwrap();
        assert_eq!(saved["workflowRouting"], false);
        assert_eq!(saved["claudeUltracode"], true);
    }

    #[test]
    fn routing_tier_agents_persist_and_replace_the_legacy_global_lane() {
        let directory = TestDirectory::new("routing-tier-agents");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_worker_lane(WorkerLanePreference::Codex)
            .expect("set legacy worker lane");
        let tier_agents = TierAgentAssignments {
            cheapest: crate::task_routing::TierAgentAssignment {
                lane: crate::task_routing::WorkerLane::Fixed(crate::task_routing::Lane::Claude),
                model: Some("haiku".to_string()),
                effort: None,
            },
            cheap: crate::task_routing::TierAgentAssignment {
                lane: crate::task_routing::WorkerLane::Fixed(crate::task_routing::Lane::Codex),
                model: Some("gpt-6-luna".to_string()),
                effort: Some("high".to_string()),
            },
            medium: crate::task_routing::TierAgentAssignment {
                lane: crate::task_routing::WorkerLane::Fixed(crate::task_routing::Lane::Claude),
                model: Some("opus".to_string()),
                effort: Some("medium".to_string()),
            },
            strong: crate::task_routing::TierAgentAssignment {
                lane: crate::task_routing::WorkerLane::Fixed(crate::task_routing::Lane::Codex),
                model: Some("gpt-6.1-sol".to_string()),
                effort: Some("xhigh".to_string()),
            },
        };

        let saved = manager
            .set_routing_tier_agents(tier_agents.clone())
            .expect("set routing tier agents");
        assert_eq!(saved.routing_tier_agents, tier_agents);
        assert_eq!(saved.worker_lane, WorkerLanePreference::Auto);

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened
                .routing_tier_agents()
                .expect("read routing tier agents"),
            tier_agents
        );
    }

    #[test]
    fn missing_preferences_default_to_unlimited() {
        let directory = TestDirectory::new("default");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");

        assert_eq!(
            manager.get().expect("read preferences"),
            AgentPreferences::default()
        );
        assert!(!directory.0.join(PREFERENCES_FILE_NAME).exists());
    }

    #[test]
    fn concurrency_limit_persists_and_can_return_to_unlimited() {
        let directory = TestDirectory::new("persist");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");

        assert_eq!(
            manager
                .set_subagent_concurrency_limit(Some(3))
                .expect("set limit")
                .subagent_concurrency_limit,
            Some(3)
        );
        assert!(!directory.0.join("preferences.json.tmp").exists());
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened
                .subagent_concurrency_limit()
                .expect("read persisted limit"),
            Some(3)
        );

        reopened
            .set_subagent_concurrency_limit(None)
            .expect("restore unlimited");
        let reopened = PreferencesManager::open(&directory.0).expect("reopen unlimited");
        assert_eq!(
            reopened
                .subagent_concurrency_limit()
                .expect("read unlimited preference"),
            None
        );
    }

    #[test]
    fn notification_preferences_persist_without_resetting_concurrency() {
        let directory = TestDirectory::new("notifications");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_subagent_concurrency_limit(Some(6))
            .expect("set concurrency");

        let saved = manager
            .set_notifications(CompletionNotificationMode::Always, false, true)
            .expect("set notifications");
        assert_eq!(saved.subagent_concurrency_limit, Some(6));
        assert_eq!(
            saved.completion_notifications,
            CompletionNotificationMode::Always
        );
        assert!(!saved.permission_notifications);
        assert!(saved.question_notifications);

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(reopened.get().expect("read preferences"), saved);
    }

    #[test]
    fn personality_preference_roundtrips_without_resetting_other_preferences() {
        let directory = TestDirectory::new("personality");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_subagent_concurrency_limit(Some(5))
            .expect("set concurrency");

        let saved = manager
            .set_personality(Personality::Friendly)
            .expect("set personality");
        assert_eq!(saved.personality, Personality::Friendly);
        assert_eq!(saved.subagent_concurrency_limit, Some(5));

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened.personality().expect("read personality"),
            Personality::Friendly
        );
        assert_eq!(
            reopened
                .set_personality(Personality::Pragmatic)
                .expect("update personality")
                .personality,
            Personality::Pragmatic
        );
    }

    #[test]
    fn ponytail_preference_roundtrips_without_resetting_other_preferences() {
        let directory = TestDirectory::new("ponytail");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_subagent_concurrency_limit(Some(5))
            .expect("set concurrency");

        let saved = manager
            .set_ponytail(PonytailMode::Full)
            .expect("set Ponytail");
        assert_eq!(saved.ponytail, PonytailMode::Full);
        assert_eq!(saved.subagent_concurrency_limit, Some(5));

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened.ponytail().expect("read Ponytail"),
            PonytailMode::Full
        );
        assert_eq!(
            reopened
                .set_ponytail(PonytailMode::Ultra)
                .expect("update Ponytail")
                .ponytail,
            PonytailMode::Ultra
        );
    }

    #[test]
    fn old_preferences_default_ponytail_to_off() {
        let preferences: AgentPreferences = serde_json::from_str(
            r#"{
                "completionNotifications": "always",
                "permissionNotifications": false,
                "questionNotifications": true,
                "keyboardShortcuts": {},
                "personality": "friendly",
                "memoriesEnabled": false
            }"#,
        )
        .expect("deserialize old preferences");

        assert_eq!(preferences.ponytail, PonytailMode::Off);
    }

    #[test]
    fn claude_subscription_proxy_roundtrips_and_defaults_on() {
        let directory = TestDirectory::new("claude-subscription-proxy");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        assert!(manager
            .claude_subscription_proxy()
            .expect("read default Claude subscription proxy preference"));

        let saved = manager
            .set_claude_subscription_proxy(false)
            .expect("disable Claude subscription proxy");
        assert!(!saved.claude_subscription_proxy);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(!reopened
            .claude_subscription_proxy()
            .expect("read persisted Claude subscription proxy preference"));

        let old: AgentPreferences = serde_json::from_str(
            r#"{
                "completionNotifications": "always",
                "permissionNotifications": false,
                "questionNotifications": true,
                "keyboardShortcuts": {},
                "personality": "friendly",
                "ponytail": "off",
                "memoriesEnabled": false
            }"#,
        )
        .expect("deserialize old preferences");
        assert!(old.claude_subscription_proxy);
    }

    #[test]
    fn explicit_false_claude_subscription_proxy_overrides_default() {
        let preferences: AgentPreferences = serde_json::from_str(
            r#"{
                "claudeSubscriptionProxy": false
            }"#,
        )
        .expect("deserialize explicit Claude subscription proxy preference");

        assert!(!preferences.claude_subscription_proxy);
    }

    #[test]
    fn memories_enabled_defaults_on_and_roundtrips() {
        let directory = TestDirectory::new("memories-enabled");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        assert!(manager.memories_enabled().expect("read default"));

        let saved = manager
            .set_memories_enabled(false)
            .expect("disable memories");
        assert!(!saved.memories_enabled);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(!reopened.memories_enabled().expect("read persisted value"));
    }

    #[test]
    fn analytics_enabled_defaults_on_and_roundtrips() {
        let directory = TestDirectory::new("analytics-enabled");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        assert!(manager.analytics_enabled().expect("read default"));

        let saved = manager
            .set_analytics_enabled(false)
            .expect("disable analytics");
        assert!(!saved.analytics_enabled);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(!reopened.analytics_enabled().expect("read persisted value"));

        let old: AgentPreferences = serde_json::from_str(r#"{ "memoriesEnabled": false }"#)
            .expect("deserialize old preferences");
        assert!(old.analytics_enabled);
    }

    #[test]
    fn app_update_auto_download_defaults_on_and_roundtrips() {
        assert!(AgentPreferences::default().app_update_auto_download);
        assert!(
            serde_json::from_str::<AgentPreferences>(r#"{"analyticsEnabled":false}"#)
                .unwrap()
                .app_update_auto_download
        );

        let directory = TestDirectory::new("app-update-auto-download");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_analytics_enabled(false)
            .expect("set analytics preference");
        let saved = manager
            .set_app_update_auto_download(false)
            .expect("set auto-download preference");
        assert!(!saved.app_update_auto_download);
        assert!(!saved.analytics_enabled);

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(!reopened
            .app_update_auto_download()
            .expect("read auto-download preference"));
        assert!(!reopened
            .analytics_enabled()
            .expect("read analytics preference"));
    }

    #[test]
    fn old_preferences_receive_conversation_defaults() {
        let preferences: AgentPreferences = serde_json::from_str(
            r#"{
                "completionNotifications": "always",
                "permissionNotifications": false,
                "questionNotifications": true,
                "keyboardShortcuts": {},
                "personality": "friendly",
                "memoriesEnabled": false
            }"#,
        )
        .expect("deserialize old preferences");

        assert_eq!(preferences.default_model, None);
        assert_eq!(preferences.default_effort, None);
        assert!(preferences.default_agent_mode);
        assert_eq!(preferences.default_approval_policy, None);
        assert!(!preferences.default_fast_mode);
    }

    #[test]
    fn conversation_defaults_roundtrip_without_resetting_other_preferences() {
        let directory = TestDirectory::new("conversation-defaults");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_personality(Personality::Pragmatic)
            .expect("set personality");

        let saved = manager
            .set_conversation_defaults(
                " gpt-5.6-sol ".to_string(),
                " xhigh ".to_string(),
                false,
                Some(ApprovalPolicy::OnFailure),
                true,
            )
            .expect("set conversation defaults");
        assert_eq!(saved.default_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(saved.default_effort.as_deref(), Some("xhigh"));
        assert!(!saved.default_agent_mode);
        assert_eq!(
            saved.default_approval_policy,
            Some(ApprovalPolicy::OnFailure)
        );
        assert!(saved.default_fast_mode);
        assert_eq!(saved.personality, Personality::Pragmatic);

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(reopened.get().expect("read preferences"), saved);
    }

    #[test]
    fn zero_is_rejected_without_changing_the_persisted_value() {
        let directory = TestDirectory::new("zero");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_subagent_concurrency_limit(Some(4))
            .expect("set initial limit");

        let error = manager
            .set_subagent_concurrency_limit(Some(0))
            .expect_err("zero must be rejected");
        assert!(error.contains("at least 1"));
        assert_eq!(
            manager
                .subagent_concurrency_limit()
                .expect("read unchanged preference"),
            Some(4)
        );
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened
                .subagent_concurrency_limit()
                .expect("read persisted preference"),
            Some(4)
        );
    }

    #[test]
    fn concurrency_limit_has_no_application_upper_bound() {
        let directory = TestDirectory::new("unbounded");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");

        manager
            .set_subagent_concurrency_limit(Some(usize::MAX))
            .expect("large limits should be accepted");
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened
                .subagent_concurrency_limit()
                .expect("read large limit"),
            Some(usize::MAX)
        );
    }

    #[test]
    fn keyboard_shortcuts_persist_and_preserve_other_preferences() {
        let directory = TestDirectory::new("keyboard-shortcuts");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_subagent_concurrency_limit(Some(7))
            .expect("set concurrency");
        let shortcuts = BTreeMap::from([
            ("application.settings".to_string(), "mod+comma".to_string()),
            ("input.dictation".to_string(), "ctrl+shift+d".to_string()),
        ]);

        let saved = manager
            .set_keyboard_shortcuts(shortcuts.clone())
            .expect("set shortcuts");
        assert_eq!(saved.keyboard_shortcuts, shortcuts);
        assert_eq!(saved.subagent_concurrency_limit, Some(7));

        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened.get().expect("read preferences").keyboard_shortcuts,
            shortcuts
        );
    }

    #[test]
    fn invalid_keyboard_shortcuts_are_rejected() {
        let directory = TestDirectory::new("invalid-keyboard-shortcuts");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        let error = manager
            .set_keyboard_shortcuts(BTreeMap::from([(
                "application settings".to_string(),
                "mod+comma".to_string(),
            )]))
            .expect_err("invalid id must fail");
        assert!(error.contains("Invalid keyboard shortcut id"));
        assert!(manager
            .get()
            .expect("read preferences")
            .keyboard_shortcuts
            .is_empty());
    }
}
