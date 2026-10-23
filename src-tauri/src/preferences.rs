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

/// The answer of the user to the usage events question (ADR 0021).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsConsent {
    /// The user did not answer yet. Halv sends no event.
    #[default]
    Unset,
    /// The user agreed. Halv sends the journey events.
    On,
    /// The user said no. Halv sends no event.
    Off,
}

impl AnalyticsConsent {
    /// Converts a preferences file without `analyticsConsent`. The old default
    /// sent events and never asked, so only an explicit `false` is an answer.
    fn from_legacy_analytics_enabled(analytics_enabled: Option<bool>) -> Self {
        match analytics_enabled {
            Some(false) => Self::Off,
            _ => Self::Unset,
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
    /// Turns on the Halv Engine (Headroom) for new sessions. Off sends every
    /// pane lane direct to its upstream. A file without the key reads `false`.
    pub headroom_enabled: bool,
    /// Sends the Claude subscription lane through the Halv Engine. It applies
    /// only while `headroom_enabled` is on. A file without the key reads `true`.
    pub claude_subscription_proxy: bool,
    pub memories_enabled: bool,
    pub task_routing: bool,
    pub worker_lane: WorkerLanePreference,
    pub routing_tier_agents: TierAgentAssignments,
    /// JEV routes the agents of a Claude Code Workflow script (ADR 0014).
    pub workflow_routing: bool,
    /// New Claude panes start with ultracode on.
    pub claude_ultracode: bool,
    /// Halv sends usage events only when this is `On` (ADR 0021).
    pub analytics_consent: AnalyticsConsent,
    /// Always `analytics_consent == On`. The file keeps it for older app
    /// versions. On load, it is only the input of the migration.
    pub analytics_enabled: bool,
    /// The list version that the "on" answer covers: 1 or 2 (ADR 0022). A
    /// file without the key reads 1.
    pub analytics_consent_version: u8,
    /// The newest list version that the app asked about: 0, 1 or 2 (ADR
    /// 0022). A file without the key reads 1 after an answer, else 0.
    pub analytics_consent_asked: u8,
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
    /// Shows the Dot pin in the sidebar and the Dot entries in the command
    /// palette. Off until the user turns it on. A missing key reads `false`.
    pub show_dot_shortcut: bool,
}

impl AgentPreferences {
    /// The Claude subscription lane uses the Halv Engine only when both the
    /// master switch and the nested opt-in are on.
    pub fn effective_claude_subscription_proxy(&self) -> bool {
        self.headroom_enabled && self.claude_subscription_proxy
    }
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
            headroom_enabled: false,
            claude_subscription_proxy: false,
            memories_enabled: true,
            task_routing: true,
            worker_lane: WorkerLanePreference::Auto,
            routing_tier_agents: TierAgentAssignments::default(),
            workflow_routing: true,
            claude_ultracode: false,
            analytics_consent: AnalyticsConsent::Unset,
            analytics_enabled: false,
            analytics_consent_version: 1,
            analytics_consent_asked: 0,
            default_model: None,
            default_effort: None,
            default_agent_mode: true,
            default_approval_policy: None,
            default_fast_mode: false,
            app_update_auto_download: true,
            remote_enabled: false,
            remote_relay_url: None,
            remote_relay_url_dev: None,
            show_dot_shortcut: false,
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
            Ok(contents) => parse_preferences(&contents)
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

    pub fn headroom_enabled(&self) -> Result<bool, String> {
        self.get().map(|preferences| preferences.headroom_enabled)
    }

    /// The Claude subscription lane uses the Halv Engine only when both the
    /// master switch and the nested opt-in are on.
    pub fn effective_claude_subscription_proxy(&self) -> Result<bool, String> {
        self.get()
            .map(|preferences| preferences.effective_claude_subscription_proxy())
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

    /// True only when the user agreed to usage events.
    #[cfg(test)]
    pub fn analytics_enabled(&self) -> Result<bool, String> {
        self.analytics_consent()
            .map(|consent| consent == AnalyticsConsent::On)
    }

    #[cfg(test)]
    pub fn analytics_consent(&self) -> Result<AnalyticsConsent, String> {
        self.get().map(|preferences| preferences.analytics_consent)
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

    pub fn set_headroom_enabled(&self, headroom_enabled: bool) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.headroom_enabled = headroom_enabled;
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

    pub fn set_show_dot_shortcut(
        &self,
        show_dot_shortcut: bool,
    ) -> Result<AgentPreferences, String> {
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.show_dot_shortcut = show_dot_shortcut;
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

    /// Saves the answer of the user. `Unset` is not an answer.
    ///
    /// "On" covers list version 2 unless `consent_version` is 1 (ADR 0022).
    /// "Off" keeps the version. Each answer sets the asked version to 2.
    ///
    /// An answer that shares less fails closed: the running session uses it
    /// before the write, so a failed write still keeps the "no". The error
    /// tells the caller that the answer did not persist.
    pub fn set_analytics_consent(
        &self,
        consent: AnalyticsConsent,
        consent_version: Option<u8>,
    ) -> Result<AgentPreferences, String> {
        if consent == AnalyticsConsent::Unset {
            return Err("Usage events consent must be on or off".to_string());
        }
        if consent == AnalyticsConsent::On && consent_version.is_some_and(|v| !matches!(v, 1 | 2)) {
            return Err("Usage events consent version must be 1 or 2".to_string());
        }
        let mut current = self
            .preferences
            .write()
            .map_err(|_| "Preferences lock is poisoned".to_string())?;
        let mut next = current.clone();
        next.analytics_consent = consent;
        next.analytics_enabled = consent == AnalyticsConsent::On;
        if consent == AnalyticsConsent::On {
            next.analytics_consent_version = consent_version.unwrap_or(2);
        }
        next.analytics_consent_asked = 2;
        if sharing_rank(&next) < sharing_rank(&current) {
            *current = next.clone();
        }
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

/// Reads a preferences file. A file without `analyticsConsent` comes from a
/// version that sent usage events without asking (ADR 0021). A file without
/// `analyticsConsentVersion` or `analyticsConsentAsked` comes from a version
/// before ADR 0022. A file without `headroomEnabled` comes from 1.0.0 or
/// older. It reads `false`: 1.0.1 turns the Halv Engine off for every install,
/// because the engine kept crashing. A file without `claudeSubscriptionProxy`
/// reads `true`, the old default. It has no effect while the engine is off.
fn parse_preferences(contents: &[u8]) -> serde_json::Result<AgentPreferences> {
    let mut value = serde_json::from_slice::<serde_json::Value>(contents)?;
    let has_consent = value.get("analyticsConsent").is_some();
    let legacy_analytics_enabled = value
        .get("analyticsEnabled")
        .and_then(serde_json::Value::as_bool);
    // A value outside its range counts as a missing key, so a damaged value
    // never stops the app from starting.
    if let Some(object) = value.as_object_mut() {
        for (key, missing) in [
            ("headroomEnabled", false),
            ("claudeSubscriptionProxy", true),
        ] {
            if !object.get(key).is_some_and(serde_json::Value::is_boolean) {
                object.insert(key.to_string(), serde_json::Value::Bool(missing));
            }
        }
        for (key, max) in [("analyticsConsentVersion", 2), ("analyticsConsentAsked", 2)] {
            let valid = object
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|number| number <= max);
            if !valid {
                object.remove(key);
            }
        }
    }
    let has_asked = value.get("analyticsConsentAsked").is_some();
    let mut preferences = serde_json::from_value::<AgentPreferences>(value)?;
    if !has_consent {
        preferences.analytics_consent =
            AnalyticsConsent::from_legacy_analytics_enabled(legacy_analytics_enabled);
    }
    preferences.analytics_enabled = preferences.analytics_consent == AnalyticsConsent::On;
    // Version 0 is not a list version. An "on" answer covers at least 1.
    preferences.analytics_consent_version = preferences.analytics_consent_version.max(1);
    if !has_asked {
        preferences.analytics_consent_asked =
            u8::from(preferences.analytics_consent != AnalyticsConsent::Unset);
    }
    Ok(preferences)
}

/// How much an answer shares: nothing, list version 1, or list version 2.
fn sharing_rank(preferences: &AgentPreferences) -> u8 {
    if preferences.analytics_consent == AnalyticsConsent::On {
        preferences.analytics_consent_version
    } else {
        0
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

/// Saves the Halv Engine switch. The change applies to new sessions only.
/// On starts the engine now. Off keeps a running engine alive until quit, so
/// running proxied panes keep working.
#[tauri::command]
pub fn preferences_set_headroom_enabled(
    manager: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, crate::sidecar::SidecarManager>,
    headroom_enabled: bool,
) -> Result<AgentPreferences, String> {
    let saved = manager.set_headroom_enabled(headroom_enabled)?;
    sidecar.set_enabled(headroom_enabled);
    if headroom_enabled {
        // `startup` reads the switch again before the child spawns, so a
        // quick "off" after this "on" leaves no engine running.
        let sidecar = sidecar.inner().clone();
        tauri::async_runtime::spawn(async move { sidecar.startup().await });
    }
    Ok(saved)
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
pub fn preferences_set_show_dot_shortcut(
    manager: tauri::State<'_, PreferencesManager>,
    show_dot_shortcut: bool,
) -> Result<AgentPreferences, String> {
    manager.set_show_dot_shortcut(show_dot_shortcut)
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
pub fn preferences_set_analytics_consent(
    manager: tauri::State<'_, PreferencesManager>,
    analytics: tauri::State<'_, crate::analytics::AnalyticsManager>,
    consent: AnalyticsConsent,
    consent_version: Option<u8>,
) -> Result<AgentPreferences, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("System time is before the Unix epoch: {error}"))?;
    apply_analytics_consent(&manager, &analytics, consent, consent_version, now)
}

/// Saves the answer and makes the queue follow it (ADR 0021, ADR 0022).
///
/// An answer that shares less holds for the running session also when a
/// write fails. The manager keeps the answer in memory, and the queue drops
/// the events or the v2 data before the result goes back.
fn apply_analytics_consent(
    manager: &PreferencesManager,
    analytics: &crate::analytics::AnalyticsManager,
    consent: AnalyticsConsent,
    consent_version: Option<u8>,
    now: u64,
) -> Result<AgentPreferences, String> {
    use crate::analytics::Sharing;
    let sharing = |preferences: &AgentPreferences| {
        Sharing::from_consent(
            preferences.analytics_consent,
            preferences.analytics_consent_version,
        )
    };
    let previous = sharing(&manager.get()?);
    let preferences = manager.set_analytics_consent(consent, consent_version);
    let current = sharing(&manager.get()?);
    analytics.apply_sharing(previous, current, now)?;
    preferences
}

/// The older boolean form of `preferences_set_analytics_consent`.
#[tauri::command]
pub fn preferences_set_analytics_enabled(
    manager: tauri::State<'_, PreferencesManager>,
    analytics: tauri::State<'_, crate::analytics::AnalyticsManager>,
    analytics_enabled: bool,
) -> Result<AgentPreferences, String> {
    preferences_set_analytics_consent(
        manager,
        analytics,
        if analytics_enabled {
            AnalyticsConsent::On
        } else {
            AnalyticsConsent::Off
        },
        None,
    )
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
    fn dot_shortcut_is_hidden_by_default_and_persists_when_shown() {
        assert!(!AgentPreferences::default().show_dot_shortcut);
        // A preferences file from an older release has no key and reads false.
        assert!(
            !serde_json::from_str::<AgentPreferences>("{}")
                .unwrap()
                .show_dot_shortcut
        );
        assert!(
            !serde_json::from_str::<AgentPreferences>(r#"{"remoteEnabled":true}"#)
                .unwrap()
                .show_dot_shortcut
        );
        let directory = TestDirectory::new("dot-shortcut");
        let manager = PreferencesManager::open(&directory.0).unwrap();
        assert!(!manager.get().unwrap().show_dot_shortcut);
        manager.set_show_dot_shortcut(true).unwrap();
        let reopened = PreferencesManager::open(&directory.0).unwrap();
        assert!(reopened.get().unwrap().show_dot_shortcut);
        let saved = serde_json::to_value(reopened.get().unwrap()).unwrap();
        assert_eq!(saved["showDotShortcut"], true);
        reopened.set_show_dot_shortcut(false).unwrap();
        let reopened_off = PreferencesManager::open(&directory.0).unwrap();
        assert!(!reopened_off.get().unwrap().show_dot_shortcut);
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
    fn claude_subscription_proxy_roundtrips_and_defaults_off() {
        let directory = TestDirectory::new("claude-subscription-proxy");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        assert!(!manager.get().unwrap().claude_subscription_proxy);

        let saved = manager
            .set_claude_subscription_proxy(true)
            .expect("enable Claude subscription proxy");
        assert!(saved.claude_subscription_proxy);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(reopened.get().unwrap().claude_subscription_proxy);

        let old = parse_preferences(
            br#"{
                "completionNotifications": "always",
                "permissionNotifications": false,
                "questionNotifications": true,
                "keyboardShortcuts": {},
                "personality": "friendly",
                "ponytail": "off",
                "memoriesEnabled": false
            }"#,
        )
        .expect("parse old preferences");
        assert!(old.claude_subscription_proxy);
        assert!(!old.headroom_enabled);
    }

    #[test]
    fn explicit_false_claude_subscription_proxy_overrides_default() {
        let preferences = parse_preferences(
            br#"{
                "claudeSubscriptionProxy": false
            }"#,
        )
        .expect("parse explicit Claude subscription proxy preference");

        assert!(!preferences.claude_subscription_proxy);
        assert!(!preferences.headroom_enabled);
        assert!(!preferences.effective_claude_subscription_proxy());
    }

    #[test]
    fn a_new_install_has_the_halv_engine_off() {
        let directory = TestDirectory::new("headroom-new-install");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        let preferences = manager.get().unwrap();

        assert!(!preferences.headroom_enabled);
        assert!(!preferences.claude_subscription_proxy);
        assert!(!manager.headroom_enabled().unwrap());
        assert!(!manager.effective_claude_subscription_proxy().unwrap());
    }

    #[test]
    fn a_file_without_the_engine_key_turns_the_engine_off() {
        // A 1.0.0 file holds the whole struct but no `headroomEnabled` key.
        // 1.0.1 turns the engine off for it. The subscription opt-in keeps
        // its stored value, but it has no effect while the engine is off.
        for (subscription, expected_subscription) in
            [(Some(true), true), (Some(false), false), (None, true)]
        {
            let directory = TestDirectory::new("headroom-migration");
            let mut file = serde_json::json!({
                "personality": "friendly",
                "analyticsConsent": "off",
            });
            if let Some(value) = subscription {
                file["claudeSubscriptionProxy"] = serde_json::Value::Bool(value);
            }
            fs::write(
                directory.0.join(PREFERENCES_FILE_NAME),
                serde_json::to_vec(&file).unwrap(),
            )
            .unwrap();

            let manager = PreferencesManager::open(&directory.0).expect("open old preferences");
            let preferences = manager.get().unwrap();
            assert!(!preferences.headroom_enabled);
            assert_eq!(preferences.claude_subscription_proxy, expected_subscription);
            assert!(!preferences.effective_claude_subscription_proxy());
            assert!(!manager.effective_claude_subscription_proxy().unwrap());
        }
        // A damaged value counts as a missing key.
        let damaged = parse_preferences(br#"{ "headroomEnabled": "yes" }"#)
            .expect("a damaged engine value must not stop the app");
        assert!(!damaged.headroom_enabled);
        assert!(damaged.claude_subscription_proxy);
    }

    #[test]
    fn an_explicit_engine_value_keeps_its_value() {
        for value in [true, false] {
            let preferences = parse_preferences(
                format!(r#"{{ "headroomEnabled": {value}, "claudeSubscriptionProxy": true }}"#)
                    .as_bytes(),
            )
            .expect("parse an explicit engine value");
            assert_eq!(preferences.headroom_enabled, value);
            assert_eq!(preferences.effective_claude_subscription_proxy(), value);
        }
    }

    #[test]
    fn an_explicit_engine_choice_survives_reopen_and_other_saves() {
        let directory = TestDirectory::new("headroom-explicit");
        fs::write(
            directory.0.join(PREFERENCES_FILE_NAME),
            br#"{ "headroomEnabled": false, "claudeSubscriptionProxy": true }"#,
        )
        .unwrap();
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        assert!(!manager.headroom_enabled().unwrap());
        // The nested opt-in applies only while the master switch is on.
        assert!(!manager.effective_claude_subscription_proxy().unwrap());

        let saved = manager
            .set_headroom_enabled(true)
            .expect("turn the engine on");
        assert!(saved.headroom_enabled);
        assert!(manager.effective_claude_subscription_proxy().unwrap());
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(reopened.headroom_enabled().unwrap());

        reopened
            .set_headroom_enabled(false)
            .expect("turn the engine off");
        reopened
            .set_memories_enabled(false)
            .expect("save an unrelated preference");
        let reopened_off = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(!reopened_off.headroom_enabled().unwrap());
        let file: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.0.join(PREFERENCES_FILE_NAME)).unwrap())
                .unwrap();
        assert_eq!(file["headroomEnabled"], false);
        assert_eq!(file["claudeSubscriptionProxy"], true);
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
    fn analytics_consent_defaults_unset_and_roundtrips() {
        let directory = TestDirectory::new("analytics-consent");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        assert_eq!(
            manager.analytics_consent().expect("read default"),
            AnalyticsConsent::Unset
        );
        assert!(!manager.analytics_enabled().expect("read default"));
        assert!(manager
            .set_analytics_consent(AnalyticsConsent::Unset, None)
            .is_err());

        let saved = manager
            .set_analytics_consent(AnalyticsConsent::On, None)
            .expect("agree to analytics");
        assert_eq!(saved.analytics_consent, AnalyticsConsent::On);
        assert!(saved.analytics_enabled);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert!(reopened.analytics_enabled().expect("read persisted value"));

        let saved = reopened
            .set_analytics_consent(AnalyticsConsent::Off, None)
            .expect("decline analytics");
        assert_eq!(saved.analytics_consent, AnalyticsConsent::Off);
        let file = serde_json::from_slice::<serde_json::Value>(
            &fs::read(directory.0.join(PREFERENCES_FILE_NAME)).expect("read preferences file"),
        )
        .expect("parse preferences file");
        assert_eq!(file["analyticsConsent"], "off");
        assert_eq!(file["analyticsEnabled"], false);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(
            reopened.analytics_consent().expect("read persisted value"),
            AnalyticsConsent::Off
        );
    }

    #[test]
    fn analytics_off_holds_in_memory_when_the_write_fails() {
        let directory = TestDirectory::new("analytics-off-write-fails");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_analytics_consent(AnalyticsConsent::On, None)
            .expect("agree to analytics");
        assert!(manager.analytics_enabled().expect("read consent"));

        // A directory at the temporary path makes the atomic write fail.
        let temporary_path = directory
            .0
            .join(PREFERENCES_FILE_NAME)
            .with_extension("json.tmp");
        fs::create_dir_all(&temporary_path).expect("block the temporary file");

        assert!(manager
            .set_analytics_consent(AnalyticsConsent::Off, None)
            .is_err());
        assert!(!manager.analytics_enabled().expect("read consent"));
        assert_eq!(
            manager.analytics_consent().expect("read consent"),
            AnalyticsConsent::Off
        );
    }

    #[test]
    fn old_analytics_default_migrates_to_unset_not_on() {
        let cases = [
            // The old default: the key is missing or true, and nobody asked.
            (r#"{ "memoriesEnabled": false }"#, AnalyticsConsent::Unset),
            (r#"{ "analyticsEnabled": true }"#, AnalyticsConsent::Unset),
            // The user turned the old switch off. That is an answer.
            (r#"{ "analyticsEnabled": false }"#, AnalyticsConsent::Off),
            // A new file keeps its answer and ignores the derived boolean.
            (
                r#"{ "analyticsEnabled": false, "analyticsConsent": "on" }"#,
                AnalyticsConsent::On,
            ),
            (
                r#"{ "analyticsEnabled": true, "analyticsConsent": "unset" }"#,
                AnalyticsConsent::Unset,
            ),
        ];
        for (contents, expected) in cases {
            let directory = TestDirectory::new("analytics-migration");
            fs::write(directory.0.join(PREFERENCES_FILE_NAME), contents)
                .expect("write old preferences");
            let manager = PreferencesManager::open(&directory.0).expect("open old preferences");
            let preferences = manager.get().expect("read preferences");
            assert_eq!(preferences.analytics_consent, expected, "{contents}");
            assert_eq!(
                preferences.analytics_enabled,
                expected == AnalyticsConsent::On,
                "{contents}"
            );
            assert_eq!(
                manager.analytics_enabled().expect("read analytics gate"),
                expected == AnalyticsConsent::On
            );
        }
    }

    #[test]
    fn analytics_consent_version_and_asked_migrate_from_older_files() {
        // (file, consent, version, asked)
        let cases = [
            // A fresh install was not asked yet.
            ("{}", AnalyticsConsent::Unset, 1, 0),
            // An "on" answer from before ADR 0022 covers list version 1.
            (
                r#"{ "analyticsConsent": "on" }"#,
                AnalyticsConsent::On,
                1,
                1,
            ),
            (
                r#"{ "analyticsConsent": "off" }"#,
                AnalyticsConsent::Off,
                1,
                1,
            ),
            (
                r#"{ "analyticsEnabled": false }"#,
                AnalyticsConsent::Off,
                1,
                1,
            ),
            (
                r#"{ "analyticsConsent": "on", "analyticsConsentVersion": 2, "analyticsConsentAsked": 2 }"#,
                AnalyticsConsent::On,
                2,
                2,
            ),
            // A damaged value never stops the start and never adds sharing.
            (
                r#"{ "analyticsConsent": "on", "analyticsConsentVersion": 7, "analyticsConsentAsked": 300 }"#,
                AnalyticsConsent::On,
                1,
                1,
            ),
            (
                r#"{ "analyticsConsent": "on", "analyticsConsentVersion": 0 }"#,
                AnalyticsConsent::On,
                1,
                1,
            ),
            (
                r#"{ "analyticsConsent": "unset", "analyticsConsentVersion": "2" }"#,
                AnalyticsConsent::Unset,
                1,
                0,
            ),
        ];
        for (contents, consent, version, asked) in cases {
            let preferences = parse_preferences(contents.as_bytes()).expect(contents);
            assert_eq!(preferences.analytics_consent, consent, "{contents}");
            assert_eq!(preferences.analytics_consent_version, version, "{contents}");
            assert_eq!(preferences.analytics_consent_asked, asked, "{contents}");
        }
        let saved = serde_json::to_value(AgentPreferences::default()).unwrap();
        assert_eq!(saved["analyticsConsentVersion"], 1);
        assert_eq!(saved["analyticsConsentAsked"], 0);
    }

    #[test]
    fn each_answer_sets_the_list_version_and_marks_version_2_as_asked() {
        let directory = TestDirectory::new("analytics-consent-version");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        // "On" without a version covers list version 2.
        let saved = manager
            .set_analytics_consent(AnalyticsConsent::On, None)
            .expect("agree");
        assert_eq!(
            (
                saved.analytics_consent_version,
                saved.analytics_consent_asked
            ),
            (2, 2)
        );
        // "No" on the notice keeps list version 1.
        let saved = manager
            .set_analytics_consent(AnalyticsConsent::On, Some(1))
            .expect("keep version 1");
        assert_eq!(
            (
                saved.analytics_consent_version,
                saved.analytics_consent_asked
            ),
            (1, 2)
        );
        // "Off" keeps the version.
        let saved = manager
            .set_analytics_consent(AnalyticsConsent::Off, Some(2))
            .expect("decline");
        assert_eq!(saved.analytics_consent, AnalyticsConsent::Off);
        assert_eq!(saved.analytics_consent_version, 1);
        for version in [0, 3] {
            assert!(manager
                .set_analytics_consent(AnalyticsConsent::On, Some(version))
                .is_err());
        }
        assert_eq!(
            manager.analytics_consent().unwrap(),
            AnalyticsConsent::Off,
            "a bad version changes nothing"
        );
        manager
            .set_analytics_consent(AnalyticsConsent::On, Some(2))
            .expect("agree to version 2");
        let file = serde_json::from_slice::<serde_json::Value>(
            &fs::read(directory.0.join(PREFERENCES_FILE_NAME)).expect("read preferences file"),
        )
        .expect("parse preferences file");
        assert_eq!(file["analyticsConsentVersion"], 2);
        assert_eq!(file["analyticsConsentAsked"], 2);
        let reopened = PreferencesManager::open(&directory.0).expect("reopen preferences");
        assert_eq!(reopened.get().unwrap().analytics_consent_version, 2);
    }

    #[test]
    fn a_move_to_version_1_holds_in_memory_when_the_write_fails() {
        let directory = TestDirectory::new("analytics-version-write-fails");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        manager
            .set_analytics_consent(AnalyticsConsent::On, Some(2))
            .expect("agree to version 2");
        let temporary_path = directory
            .0
            .join(PREFERENCES_FILE_NAME)
            .with_extension("json.tmp");
        fs::create_dir_all(&temporary_path).expect("block the temporary file");
        assert!(manager
            .set_analytics_consent(AnalyticsConsent::On, Some(1))
            .is_err());
        assert_eq!(manager.get().unwrap().analytics_consent_version, 1);
    }

    #[test]
    fn the_answer_moves_the_analytics_queue_and_the_consent_day() {
        use crate::analytics::{AnalyticsManager, Sharing};
        let directory = TestDirectory::new("analytics-consent-queue");
        let manager = PreferencesManager::open(&directory.0).expect("open preferences");
        let analytics = AnalyticsManager::open(&directory.0).expect("open analytics");
        let day = 20_000;
        let now = day * 86_400 + 60;

        let saved = apply_analytics_consent(&manager, &analytics, AnalyticsConsent::On, None, now)
            .expect("agree to version 2");
        assert_eq!(saved.analytics_consent_version, 2);
        assert_eq!(analytics.consent_v2_day().unwrap(), Some(day));
        // A later "on" for version 2 keeps the first day.
        apply_analytics_consent(
            &manager,
            &analytics,
            AnalyticsConsent::On,
            Some(2),
            now + 86_400,
        )
        .unwrap();
        assert_eq!(analytics.consent_v2_day().unwrap(), Some(day));
        // Version 1 removes the day. A new move to version 2 starts later.
        apply_analytics_consent(
            &manager,
            &analytics,
            AnalyticsConsent::On,
            Some(1),
            now + 86_400,
        )
        .unwrap();
        assert_eq!(analytics.consent_v2_day().unwrap(), None);
        apply_analytics_consent(
            &manager,
            &analytics,
            AnalyticsConsent::On,
            Some(2),
            now + 3 * 86_400,
        )
        .unwrap();
        assert_eq!(analytics.consent_v2_day().unwrap(), Some(day + 3));
        apply_analytics_consent(&manager, &analytics, AnalyticsConsent::Off, None, now).unwrap();
        assert_eq!(analytics.consent_v2_day().unwrap(), None);
        assert_eq!(
            Sharing::from_consent(
                manager.analytics_consent().unwrap(),
                manager.get().unwrap().analytics_consent_version
            ),
            Sharing::Nothing
        );
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
            .set_analytics_consent(AnalyticsConsent::Off, None)
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
