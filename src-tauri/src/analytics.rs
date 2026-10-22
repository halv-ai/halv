use crate::{
    app_data, auth,
    boot_record::{BootRecordEntry, BootRecordManager, ShutdownKind},
    preferences::{AnalyticsConsent, PreferencesManager},
    savings::UtcDayUsage,
    storage::{AuthSession, Storage},
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

const INSTALL_ID_FILE_NAME: &str = "analytics-install-id";
const QUEUE_FILE_NAME: &str = "analytics-queue.json";
/// The locale, the last app version and the consent day (ADR 0022).
const STATE_FILE_NAME: &str = "analytics-state.json";
/// The download id of the site link that led to this install (ADR 0021).
const ATTRIBUTION_FILE_NAME: &str = "attribution.json";
/// The app tells the webview about a new download id with this event.
pub const ATTRIBUTION_EVENT: &str = "halv://attribution";
const FIRST_OPEN: &str = "first_open";
/// A dev build on Windows or Linux registers the halv:// scheme only when the
/// developer sets this variable to `1`.
const REGISTER_LINK_ENV: &str = "HALV_REGISTER_LINK";
const DOWNLOAD_LINK_SCHEME: &str = "halv";
const DOWNLOAD_LINK_HOST: &str = "welcome";
const DOWNLOAD_ID_PARAMETER: &str = "d";
const DOWNLOAD_ID_MAX_LENGTH: usize = 64;
const ANALYTICS_EVENTS_PATH: &str = "/v1/analytics/events";
const JOURNEY_EVENTS_PATH: &str = "/v1/analytics/journey";
const JOURNEY_EVENTS: [&str; 13] = [
    "first_open",
    "account_connected",
    "project_selected",
    "first_task_started",
    "first_task_completed",
    "first_task_failed",
    "setup_failed",
    "context_saved",
    "trial_prompted",
    "checkout_opened",
    "trial_confirmed",
    "payment_confirmed",
    "active_day",
];
const ALLOWED_EVENTS: [&str; 5] = [
    "installed",
    "cli_connected",
    "first_saving",
    "trial_started",
    "paid",
];
/// The events that need consent version 2 (ADR 0022). They go to the
/// journey path. The webview cannot record them: the app records them.
const V2_EVENTS: [&str; 5] = [
    "usage_summarized",
    "crash_detected",
    "proxy_failed",
    "app_updated",
    "update_failed",
];
const USAGE_SUMMARIZED: &str = "usage_summarized";
const CRASH_DETECTED: &str = "crash_detected";
const PROXY_FAILED: &str = "proxy_failed";
const APP_UPDATED: &str = "app_updated";
const UPDATE_FAILED: &str = "update_failed";
/// The list version of the consent that covers the v2 events and fields.
const CONSENT_V2: u8 = 2;
/// The agent names of `agents_used` and `agent_kind` (ADR 0022).
const AGENTS: [&str; 8] = [
    "claude_code",
    "codex",
    "gemini",
    "kimi",
    "glm",
    "grok",
    "antigravity",
    "other",
];
/// The UI languages. A batch under consent version 2 has one of them.
const LOCALES: [&str; 11] = [
    "en", "pt-BR", "es", "fr", "de", "it", "nl", "ja", "zh-CN", "zh-TW", "ko",
];
const SESSIONS_BUCKETS: [&str; 4] = ["1", "2-5", "6-20", "21+"];
const PLANS: [&str; 6] = [
    "none",
    "trial",
    "beta_free",
    "basic",
    "unlimited",
    "lifetime",
];
const UPTIME_BUCKETS: [&str; 5] = ["lt_1m", "1m_10m", "10m_1h", "1h_6h", "6h_plus"];
const RSS_BUCKETS: [&str; 6] = [
    "lt_500mb",
    "500mb_1gb",
    "1gb_2gb",
    "2gb_4gb",
    "4gb_plus",
    "unknown",
];
const AUTH_METHODS: [&str; 3] = ["oauth", "api_key", "cli"];
const SURFACES: [&str; 4] = ["pitch", "routing", "rotation", "task_board"];
const TASK_CATEGORIES: [&str; 4] = ["connection", "engine", "task", "cancelled"];
/// The `proxy_failed` categories. The sidecar uses these names. The proxy
/// has no restart budget (it restarts without limit), so no code records
/// `restart_exhausted` yet. The server and the app accept it.
pub(crate) const PROXY_START: &str = "start";
pub(crate) const PROXY_EXIT: &str = "exit";
const PROXY_CATEGORIES: [&str; 3] = [PROXY_START, PROXY_EXIT, "restart_exhausted"];
/// The `update_failed` categories. The app updater uses these names.
pub(crate) const UPDATE_DOWNLOAD: &str = "download";
pub(crate) const UPDATE_SIGNATURE: &str = "signature";
pub(crate) const UPDATE_INSTALL: &str = "install";
const UPDATE_CATEGORIES: [&str; 3] = [UPDATE_DOWNLOAD, UPDATE_SIGNATURE, UPDATE_INSTALL];
/// The v2 fields of each event: `(event, required fields, optional fields)`.
/// `category` is a v1 field. `validate_category` checks it.
const V2_FIELDS: [(&str, &[&str], &[&str]); 7] = [
    (
        USAGE_SUMMARIZED,
        &[
            "day",
            "agents_used",
            "sessions_bucket",
            "tokens_saved",
            "usd_saved",
            "crux_used",
            "routing_used",
            "plan",
        ],
        &[],
    ),
    (
        CRASH_DETECTED,
        &["prev_app_version", "uptime_bucket", "rss_bucket"],
        &[],
    ),
    (PROXY_FAILED, &["recovered"], &[]),
    (APP_UPDATED, &["from_version"], &[]),
    (UPDATE_FAILED, &[], &[]),
    ("account_connected", &[], &["agent_kind", "auth_method"]),
    ("trial_prompted", &[], &["surface"]),
];
const DAY_SECONDS: u64 = 86_400;
/// A queued event older than this time never leaves the device.
const PENDING_RETENTION_SECONDS: u64 = 30 * DAY_SECONDS;
/// The queue keeps the key of a recorded v2 event for this time (ADR 0022).
const V2_KEY_RETENTION_SECONDS: u64 = 40 * DAY_SECONDS;
/// A usage summary covers a UTC day from 7 days ago to yesterday.
const USAGE_DAYS_BACK: u64 = 7;
const USAGE_SUMMARY_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const TOKENS_SAVED_MAX: u64 = 100_000_000_000;
const USD_SAVED_MAX: f64 = 1_000_000.0;
/// The largest whole number that an f64 holds exactly.
const F64_EXACT_INTEGER_MAX: f64 = 9_007_199_254_740_992.0;
const MIB: u64 = 1024 * 1024;

/// What the user lets Halv send (ADR 0021, ADR 0022).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sharing {
    /// The answer is unset or off. Halv sends nothing.
    Nothing,
    /// The answer is on for list version 1: the journey events without the
    /// v2 events, the v2 fields and the locale.
    V1,
    /// The answer is on for list version 2: also the v2 events, the v2
    /// fields and the locale.
    V2,
}

impl Sharing {
    pub(crate) fn from_consent(consent: AnalyticsConsent, consent_version: u8) -> Self {
        match consent {
            AnalyticsConsent::On if consent_version >= CONSENT_V2 => Self::V2,
            AnalyticsConsent::On => Self::V1,
            AnalyticsConsent::Unset | AnalyticsConsent::Off => Self::Nothing,
        }
    }
}

/// One journey event. The JSON object is flat. The fields after
/// `download_id` are the v2 fields of ADR 0022. Each event takes only its
/// own fields.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct AnalyticsEvent {
    event: String,
    at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_version: Option<String>,
    /// The download id of the site link. Only `first_open` has it (ADR 0021).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    download_id: Option<String>,
    /// `usage_summarized`: the UTC day number that the summary covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    day: Option<u64>,
    /// `usage_summarized`: the agents with a session on that day, unique
    /// and sorted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents_used: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sessions_bucket: Option<String>,
    /// `usage_summarized`: the tokens saved on that day, rounded to 2
    /// significant digits. The exact value stays on the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens_saved: Option<u64>,
    /// `usage_summarized`: the dollars saved on that day, rounded to 2
    /// significant digits. The exact value stays on the device.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_amount"
    )]
    usd_saved: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    crux_used: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    routing_used: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    /// `crash_detected`: the app version of the run that did not end well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prev_app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uptime_bucket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rss_bucket: Option<String>,
    /// `proxy_failed`: true when the automatic restart brought the proxy
    /// back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovered: Option<bool>,
    /// `app_updated`: the app version before the update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from_version: Option<String>,
    /// `account_connected`: the agent of the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_method: Option<String>,
    /// `trial_prompted`: the place where the app showed the trial offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    surface: Option<String>,
}

impl AnalyticsEvent {
    fn new(event: &str, at: u64) -> Self {
        Self {
            event: event.to_string(),
            at,
            ..Self::default()
        }
    }

    /// The dedup key in the queue (ADR 0022). The server keeps the first
    /// row of each key.
    fn key(&self) -> String {
        match self.event.as_str() {
            "active_day" | CRASH_DETECTED | PROXY_FAILED | UPDATE_FAILED => {
                format!("{}:{}", self.event, self.at / DAY_SECONDS)
            }
            USAGE_SUMMARIZED => usage_key(self.day.unwrap_or(self.at / DAY_SECONDS)),
            APP_UPDATED => format!(
                "{APP_UPDATED}:{}",
                self.app_version.as_deref().unwrap_or_default()
            ),
            _ => self.event.clone(),
        }
    }

    /// The names of the v2 fields that the event sets. The list leaves out
    /// `category`, a v1 field.
    fn v2_fields(&self) -> Vec<&'static str> {
        [
            ("day", self.day.is_some()),
            ("agents_used", self.agents_used.is_some()),
            ("sessions_bucket", self.sessions_bucket.is_some()),
            ("tokens_saved", self.tokens_saved.is_some()),
            ("usd_saved", self.usd_saved.is_some()),
            ("crux_used", self.crux_used.is_some()),
            ("routing_used", self.routing_used.is_some()),
            ("plan", self.plan.is_some()),
            ("prev_app_version", self.prev_app_version.is_some()),
            ("uptime_bucket", self.uptime_bucket.is_some()),
            ("rss_bucket", self.rss_bucket.is_some()),
            ("recovered", self.recovered.is_some()),
            ("from_version", self.from_version.is_some()),
            ("agent_kind", self.agent_kind.is_some()),
            ("auth_method", self.auth_method.is_some()),
            ("surface", self.surface.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
    }

    /// The event with the v1 fields only.
    fn without_v2_fields(self) -> Self {
        Self {
            event: self.event,
            at: self.at,
            category: self.category,
            app_version: self.app_version,
            download_id: self.download_id,
            ..Self::default()
        }
    }
}

fn usage_key(day: u64) -> String {
    format!("{USAGE_SUMMARIZED}:{day}")
}

/// Writes a whole dollar amount as an integer, for example 23 and not 23.0.
fn serialize_amount<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(amount) if amount.fract() == 0.0 && (0.0..=F64_EXACT_INTEGER_MAX).contains(amount) => {
            serializer.serialize_u64(*amount as u64)
        }
        Some(amount) => serializer.serialize_f64(*amount),
        None => serializer.serialize_none(),
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
struct AnalyticsQueue {
    #[serde(deserialize_with = "read_pending")]
    pending: Vec<AnalyticsEvent>,
    recorded: BTreeSet<String>,
    /// The v2 events (ADR 0022). They have their own list, so an older
    /// release that reads this file ignores them and still starts.
    #[serde(deserialize_with = "read_pending_v2")]
    pending_v2: Vec<AnalyticsEvent>,
    /// The keys of the recorded v2 events, with the time of each event.
    /// The queue drops a key after 40 days.
    #[serde(deserialize_with = "read_recorded_v2")]
    recorded_v2: BTreeMap<String, u64>,
}

/// The fields of a v1 event: the part that a damaged v2 fact never removes.
const V1_FIELDS: [&str; 5] = ["event", "at", "category", "app_version", "download_id"];

/// Reads the v1 list. A v2 fact with a wrong type, for example
/// `"agent_kind": 5`, is dropped from its event. A wrong v1 field keeps
/// failing: the start stops, as before ADR 0022.
fn read_pending<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<AnalyticsEvent>, D::Error> {
    Vec::<serde_json::Value>::deserialize(deserializer)?
        .into_iter()
        .map(|value| {
            AnalyticsEvent::deserialize(&value).or_else(|_| {
                let v1_part = match value {
                    serde_json::Value::Object(mut fields) => {
                        fields.retain(|name, _| V1_FIELDS.contains(&name.as_str()));
                        serde_json::Value::Object(fields)
                    }
                    other => other,
                };
                AnalyticsEvent::deserialize(v1_part).map_err(D::Error::custom)
            })
        })
        .collect()
}

/// Reads the v2 list. The app drops an entry that it cannot read, so a
/// damaged v2 part never stops the start.
fn read_pending_v2<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<AnalyticsEvent>, D::Error> {
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|value| AnalyticsEvent::deserialize(value).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// Reads the v2 keys. The app drops an entry without a whole-number time.
fn read_recorded_v2<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, u64>, D::Error> {
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Object(entries) => entries
            .into_iter()
            .filter_map(|(key, at)| Some((key, at.as_u64()?)))
            .collect(),
        _ => BTreeMap::new(),
    })
}

impl AnalyticsQueue {
    /// Adds a valid event once. `at` of the event is the current time.
    fn record(&mut self, value: AnalyticsEvent) -> Result<bool, String> {
        validate_event_fields(&value, value.at)?;
        let at = value.at;
        let key = value.key();
        if is_v2_event(&value.event) {
            self.prune_v2_keys(at);
            if self.recorded_v2.contains_key(&key) {
                return Ok(false);
            }
            self.recorded_v2.insert(key, at);
            self.pending_v2
                .retain(|value| value.at.saturating_add(PENDING_RETENTION_SECONDS) >= at);
            self.pending_v2.push(value);
            return Ok(true);
        }
        if self.recorded.contains(&key) {
            return Ok(false);
        }

        if value.event == "active_day" {
            self.recorded.retain(|key| !key.starts_with("active_day:"));
        }
        self.recorded.insert(key);
        self.pending
            .retain(|value| value.at.saturating_add(PENDING_RETENTION_SECONDS) >= at);
        self.pending.push(value);
        Ok(true)
    }

    fn acknowledge(&mut self, sent: &[AnalyticsEvent]) {
        let sent_names = sent
            .iter()
            .map(AnalyticsEvent::key)
            .collect::<BTreeSet<_>>();
        self.pending
            .retain(|event| !sent_names.contains(&event.key()));
        self.pending_v2
            .retain(|event| !sent_names.contains(&event.key()));
    }

    /// Drops the v2 keys that are older than 40 days.
    fn prune_v2_keys(&mut self, now: u64) {
        self.recorded_v2
            .retain(|_, at| at.saturating_add(V2_KEY_RETENTION_SECONDS) >= now);
    }

    /// Removes the v2 data: the v2 events and the v2 fields of the other
    /// events. The keys stay, so a later consent does not send old events.
    fn drop_v2_data(&mut self) {
        self.pending_v2.clear();
        self.pending = std::mem::take(&mut self.pending)
            .into_iter()
            .map(AnalyticsEvent::without_v2_fields)
            .collect();
    }

    /// Removes v2 data that is not valid, for example from a damaged file.
    /// The v1 part keeps the strict check of `validate`.
    fn sanitize_v2(&mut self) {
        self.recorded_v2.retain(|key, _| is_valid_v2_key(key));
        let recorded_v2 = &self.recorded_v2;
        let mut keys = BTreeSet::new();
        self.pending_v2.retain(|event| {
            is_v2_event(&event.event)
                && validate_event_fields(event, event.at).is_ok()
                && recorded_v2.contains_key(&event.key())
                && keys.insert(event.key())
        });
        for event in &mut self.pending {
            if validate_v2_fields(event, event.at).is_err() {
                *event = std::mem::take(event).without_v2_fields();
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        let mut pending_names = BTreeSet::new();
        for event in &self.pending {
            if is_v2_event(&event.event) {
                return Err(format!("Misplaced analytics event: {}", event.event));
            }
            validate_event_fields(event, event.at)?;
            if !pending_names.insert(event.key()) {
                return Err(format!("Duplicate queued analytics event: {}", event.event));
            }
            if event.event != "active_day" && !self.recorded.contains(&event.key()) {
                return Err(format!(
                    "Queued analytics event is not marked as recorded: {}",
                    event.event
                ));
            }
        }
        for event in &self.recorded {
            if let Some(day) = event.strip_prefix("active_day:") {
                day.parse::<u64>()
                    .map_err(|_| "Invalid activity day".to_string())?;
            } else {
                validate_event(event)?;
            }
        }
        Ok(())
    }

    /// True after `first_open` left the queue. A download id that arrives
    /// later cannot travel with it anymore.
    fn first_open_sent(&self) -> bool {
        self.recorded.contains(FIRST_OPEN)
            && !self.pending.iter().any(|event| event.event == FIRST_OPEN)
    }
}

/// True for a v2 key of ADR 0022, for example `crash_detected:20383`.
fn is_valid_v2_key(key: &str) -> bool {
    let Some((event, suffix)) = key.split_once(':') else {
        return false;
    };
    match event {
        USAGE_SUMMARIZED | CRASH_DETECTED | PROXY_FAILED | UPDATE_FAILED => {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }
        APP_UPDATED => is_valid_version(suffix),
        _ => false,
    }
}

/// The local facts of ADR 0022 that are not events: the UI language, the
/// app version of the last start, and the day of the version 2 consent.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
struct AnalyticsState {
    /// The UI language. Only a batch under consent version 2 carries it.
    locale: Option<String>,
    /// The app version of the last start. A different version at the next
    /// start records `app_updated`.
    last_app_version: Option<String>,
    /// The UTC day when the install moved to consent version 2. A usage
    /// summary never covers an earlier day.
    consent_v2_day: Option<u64>,
}

impl AnalyticsState {
    /// Drops the values that are not valid, for example from a damaged file.
    fn sanitized(mut self) -> Self {
        self.locale = self.locale.filter(|locale| is_valid_locale(locale));
        self.last_app_version = self
            .last_app_version
            .filter(|version| is_valid_version(version));
        self
    }
}

/// The download id that a site link gave to this install (ADR 0021).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Attribution {
    download_id: String,
    received_at: u64,
}

/// What the webview may show about the download id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionSnapshot {
    pub download_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AnalyticsManager {
    install_id: Arc<str>,
    queue_path: Arc<PathBuf>,
    queue: Arc<Mutex<AnalyticsQueue>>,
    attribution_path: Arc<PathBuf>,
    // Lock order: `queue` first, then `attribution`.
    attribution: Arc<Mutex<Option<Attribution>>>,
    state_path: Arc<PathBuf>,
    // No code holds `state` and another lock at the same time.
    state: Arc<Mutex<AnalyticsState>>,
    flush_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AnalyticsManager {
    pub fn open(app_data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(app_data_dir)?;
        let queue_path = app_data_dir.join(QUEUE_FILE_NAME);
        let attribution_path = app_data_dir.join(ATTRIBUTION_FILE_NAME);
        let state_path = app_data_dir.join(STATE_FILE_NAME);
        // An earlier release created these files with the default mode.
        app_data::restrict_file(&app_data_dir.join(INSTALL_ID_FILE_NAME));
        app_data::restrict_file(&queue_path);
        let install_id = read_or_create_install_id(app_data_dir)?;
        let mut queue = read_queue(&queue_path)?;
        queue.sanitize_v2();
        queue
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let attribution = read_attribution(&attribution_path);
        let state = read_state(&state_path);

        Ok(Self {
            install_id: install_id.into(),
            queue_path: Arc::new(queue_path),
            queue: Arc::new(Mutex::new(queue)),
            attribution_path: Arc::new(attribution_path),
            attribution: Arc::new(Mutex::new(attribution)),
            state_path: Arc::new(state_path),
            state: Arc::new(Mutex::new(state)),
            flush_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    fn install_id(&self) -> String {
        self.install_id.to_string()
    }

    #[cfg(test)]
    fn record(&self, event: &str, at: u64) -> Result<bool, String> {
        self.record_with_context(event, at, None, None)
    }

    #[cfg(test)]
    fn record_with_context(
        &self,
        event: &str,
        at: u64,
        category: Option<String>,
        app_version: Option<String>,
    ) -> Result<bool, String> {
        self.record_event(AnalyticsEvent {
            category,
            app_version,
            ..AnalyticsEvent::new(event, at)
        })
    }

    /// Adds the event to the queue once. `first_open` takes the stored
    /// download id. The caller checks the consent.
    fn record_event(&self, mut event: AnalyticsEvent) -> Result<bool, String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        event.download_id = if event.event == FIRST_OPEN {
            self.download_id()?
        } else {
            None
        };
        let mut next = queue.clone();
        let recorded = next.record(event)?;
        if recorded {
            write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
            *queue = next;
        }
        Ok(recorded)
    }

    /// True when the queue has the key of a v2 event.
    fn has_v2_key(&self, key: &str) -> Result<bool, String> {
        self.queue
            .lock()
            .map(|queue| queue.recorded_v2.contains_key(key))
            .map_err(|_| "Analytics queue lock is poisoned".to_string())
    }

    /// The queued events: the v1 list first, then the v2 list.
    fn pending(&self) -> Result<Vec<AnalyticsEvent>, String> {
        self.queue
            .lock()
            .map(|queue| {
                queue
                    .pending
                    .iter()
                    .chain(&queue.pending_v2)
                    .cloned()
                    .collect()
            })
            .map_err(|_| "Analytics queue lock is poisoned".to_string())
    }

    /// Drops the events that the server no longer accepts: an event older
    /// than 30 days, and a usage summary of a day older than 7 days. It also
    /// drops the v2 keys that are older than 40 days.
    fn expire_pending(&self, now: u64) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut next = queue.clone();
        let today = now / DAY_SECONDS;
        next.pending
            .retain(|event| event.at.saturating_add(PENDING_RETENTION_SECONDS) >= now);
        next.pending_v2.retain(|event| {
            event.at.saturating_add(PENDING_RETENTION_SECONDS) >= now
                && event
                    .day
                    .map_or(true, |day| day.saturating_add(USAGE_DAYS_BACK) >= today)
        });
        next.prune_v2_keys(now);
        if next != *queue {
            write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
            *queue = next;
        }
        Ok(())
    }

    /// Removes the v2 data from the queue, for a consent below version 2.
    /// The in-memory queue changes first, so a failed write never leaves v2
    /// data to send in this session.
    pub(crate) fn drop_v2_data(&self) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut next = queue.clone();
        next.drop_v2_data();
        if next == *queue {
            return Ok(());
        }
        *queue = next;
        write_queue_atomically(&self.queue_path, &queue).map_err(|error| error.to_string())
    }

    fn state(&self) -> Result<AnalyticsState, String> {
        self.state
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "Analytics state lock is poisoned".to_string())
    }

    /// Changes the state in memory and in the file. The memory keeps the
    /// change also when the write fails, because each value is only local.
    fn update_state(&self, change: impl FnOnce(&mut AnalyticsState)) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Analytics state lock is poisoned".to_string())?;
        let mut next = state.clone();
        change(&mut next);
        if next == *state {
            return Ok(());
        }
        *state = next;
        write_state_atomically(&self.state_path, &state).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(crate) fn consent_v2_day(&self) -> Result<Option<u64>, String> {
        self.state().map(|state| state.consent_v2_day)
    }

    /// Stores the UI language. Halv sends it only under consent version 2.
    pub(crate) fn set_locale(&self, locale: &str) -> Result<(), String> {
        if !is_valid_locale(locale) {
            return Err("Invalid analytics locale".to_string());
        }
        self.update_state(|state| state.locale = Some(locale.to_string()))
    }

    /// Stores the current app version and returns the version of the last
    /// start when it is different. A first start returns `None`.
    fn replace_last_app_version(&self, current: &str) -> Result<Option<String>, String> {
        let previous = self.state()?.last_app_version;
        self.update_state(|state| state.last_app_version = Some(current.to_string()))?;
        Ok(previous.filter(|previous| previous != current))
    }

    /// Follows a consent change (ADR 0022). A move to version 2 stores the
    /// current UTC day. A consent below version 2 removes the day, so a
    /// later move to version 2 starts on a new day.
    pub(crate) fn apply_sharing(
        &self,
        previous: Sharing,
        current: Sharing,
        now: u64,
    ) -> Result<(), String> {
        match current {
            Sharing::Nothing => self.clear_pending()?,
            Sharing::V1 => self.drop_v2_data()?,
            Sharing::V2 => {}
        }
        let today = now / DAY_SECONDS;
        self.update_state(|state| {
            state.consent_v2_day = match (previous, current) {
                (Sharing::V2, Sharing::V2) => state.consent_v2_day.or(Some(today)),
                (_, Sharing::V2) => Some(today),
                _ => None,
            };
        })
    }

    fn acknowledge(&self, sent: &[AnalyticsEvent]) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut next = queue.clone();
        next.acknowledge(sent);
        write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
        *queue = next;
        // The server has `first_open` now. The download id has done its job.
        if sent.iter().any(|event| event.event == FIRST_OPEN) {
            self.forget_download_id()?;
        }
        Ok(())
    }

    /// The stored download id, when a site link gave one.
    pub(crate) fn download_id(&self) -> Result<Option<String>, String> {
        self.attribution
            .lock()
            .map(|attribution| {
                attribution
                    .as_ref()
                    .map(|attribution| attribution.download_id.clone())
            })
            .map_err(|_| "Analytics attribution lock is poisoned".to_string())
    }

    /// Stores the download id of a site link, also without consent (ADR
    /// 0021). The first id wins. An id that arrives after `first_open` left
    /// the queue is dropped. Returns true when Halv stored the id.
    pub(crate) fn accept_download_id(&self, download_id: &str, now: u64) -> Result<bool, String> {
        if !is_valid_download_id(download_id) {
            return Err("Invalid download id".to_string());
        }
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut attribution = self
            .attribution
            .lock()
            .map_err(|_| "Analytics attribution lock is poisoned".to_string())?;
        if attribution.is_some() || queue.first_open_sent() {
            return Ok(false);
        }
        let stored = Attribution {
            download_id: download_id.to_string(),
            received_at: now,
        };
        write_attribution_atomically(&self.attribution_path, &stored)
            .map_err(|error| error.to_string())?;
        *attribution = Some(stored);

        // A `first_open` that still waits in the queue takes the id with it.
        let mut next = queue.clone();
        let mut changed = false;
        for event in next
            .pending
            .iter_mut()
            .filter(|event| event.event == FIRST_OPEN && event.download_id.is_none())
        {
            event.download_id = Some(download_id.to_string());
            changed = true;
        }
        if changed {
            write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
            *queue = next;
        }
        Ok(true)
    }

    fn forget_download_id(&self) -> Result<(), String> {
        let mut attribution = self
            .attribution
            .lock()
            .map_err(|_| "Analytics attribution lock is poisoned".to_string())?;
        if attribution.is_none() {
            return Ok(());
        }
        match fs::remove_file(self.attribution_path.as_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        *attribution = None;
        Ok(())
    }

    pub(crate) fn clear_pending(&self) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        if queue.pending.is_empty() && queue.pending_v2.is_empty() {
            return Ok(());
        }
        // The in-memory queue drops its events first, so a failed write
        // never leaves an event to send in this session.
        queue.pending.clear();
        queue.pending_v2.clear();
        write_queue_atomically(&self.queue_path, &queue).map_err(|error| error.to_string())
    }
}

#[derive(Serialize)]
struct AnalyticsBatch<'a> {
    install_id: &'a str,
    app_version: &'a str,
    os: &'a str,
    /// Only a batch under consent version 2 has it. An older server does
    /// not know the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    consent_version: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locale: Option<&'a str>,
    events: &'a [AnalyticsEvent],
}

impl<'a> AnalyticsBatch<'a> {
    /// A batch under `sharing`. Only version 2 adds `consent_version` and
    /// the locale. The caller selects the events with `events_for`.
    fn new(
        install_id: &'a str,
        app_version: &'a str,
        os: &'a str,
        sharing: Sharing,
        locale: Option<&'a str>,
        events: &'a [AnalyticsEvent],
    ) -> Self {
        let v2 = sharing == Sharing::V2;
        Self {
            install_id,
            app_version,
            os,
            consent_version: v2.then_some(CONSENT_V2),
            locale: locale.filter(|_| v2),
            events,
        }
    }
}

/// The events that may leave under `sharing`. Version 1 drops the v2
/// events and strips the v2 fields (ADR 0022).
fn events_for(events: &[AnalyticsEvent], sharing: Sharing) -> Vec<AnalyticsEvent> {
    match sharing {
        Sharing::Nothing => Vec::new(),
        Sharing::V1 => events
            .iter()
            .filter(|event| !is_v2_event(&event.event))
            .cloned()
            .map(AnalyticsEvent::without_v2_fields)
            .collect(),
        Sharing::V2 => events.to_vec(),
    }
}

fn validate_event(event: &str) -> Result<(), String> {
    if ALLOWED_EVENTS.contains(&event) || is_journey_event(event) {
        Ok(())
    } else {
        Err(format!("Unknown analytics event: {event}"))
    }
}

fn is_v2_event(event: &str) -> bool {
    V2_EVENTS.contains(&event)
}

/// The events of the journey path: the 13 events of ADR 0021 and the v2
/// events of ADR 0022.
fn is_journey_event(event: &str) -> bool {
    JOURNEY_EVENTS.contains(&event) || is_v2_event(event)
}

fn is_valid_locale(locale: &str) -> bool {
    LOCALES.contains(&locale)
}

/// A version is 1 to 32 characters of `[A-Za-z0-9.+-]`, as on the server.
fn is_valid_version(version: &str) -> bool {
    (1..=32).contains(&version.len())
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// Checks an event with the rules of the server (ADR 0021, ADR 0022). `now`
/// sets the days that a usage summary can cover.
fn validate_event_fields(event: &AnalyticsEvent, now: u64) -> Result<(), String> {
    validate_event(&event.event)?;
    validate_category(&event.event, event.category.as_deref())?;
    validate_download_id(&event.event, event.download_id.as_deref())?;
    validate_v2_fields(event, now)
}

/// Checks the v2 fields. Each event sets only its own fields and all of its
/// required fields.
fn validate_v2_fields(event: &AnalyticsEvent, now: u64) -> Result<(), String> {
    let none: &[&str] = &[];
    let (required, optional) = V2_FIELDS
        .iter()
        .find(|(name, _, _)| *name == event.event)
        .map_or((none, none), |(_, required, optional)| {
            (*required, *optional)
        });
    let fields = event.v2_fields();
    if let Some(field) = fields
        .iter()
        .find(|field| !required.contains(field) && !optional.contains(field))
    {
        return Err(format!(
            "Invalid analytics field {field} on {}",
            event.event
        ));
    }
    if let Some(field) = required.iter().find(|field| !fields.contains(field)) {
        return Err(format!(
            "Missing analytics field {field} on {}",
            event.event
        ));
    }
    let outside = |value: &Option<String>, allowed: &[&str]| {
        value
            .as_deref()
            .is_some_and(|value| !allowed.contains(&value))
    };
    if let Some(day) = event.day {
        if !summary_day_is_open(day, now / DAY_SECONDS) {
            return Err("Invalid analytics usage day".to_string());
        }
    }
    if event.agents_used.as_ref().is_some_and(|agents| {
        agents.is_empty()
            || agents.len() > AGENTS.len()
            || agents.iter().any(|agent| !AGENTS.contains(&agent.as_str()))
            || !agents.windows(2).all(|pair| pair[0] < pair[1])
    }) {
        return Err("Invalid analytics agents".to_string());
    }
    if event
        .tokens_saved
        .is_some_and(|tokens| tokens > TOKENS_SAVED_MAX || round2_tokens(tokens) != tokens)
    {
        return Err("Invalid analytics tokens saved".to_string());
    }
    if event.usd_saved.is_some_and(|usd| !is_rounded_amount(usd)) {
        return Err("Invalid analytics dollars saved".to_string());
    }
    if [&event.prev_app_version, &event.from_version]
        .into_iter()
        .any(|version| version.as_deref().is_some_and(|v| !is_valid_version(v)))
    {
        return Err("Invalid analytics version".to_string());
    }
    if outside(&event.sessions_bucket, &SESSIONS_BUCKETS)
        || outside(&event.plan, &PLANS)
        || outside(&event.uptime_bucket, &UPTIME_BUCKETS)
        || outside(&event.rss_bucket, &RSS_BUCKETS)
        || outside(&event.agent_kind, &AGENTS)
        || outside(&event.auth_method, &AUTH_METHODS)
        || outside(&event.surface, &SURFACES)
    {
        return Err(format!("Invalid analytics value on {}", event.event));
    }
    Ok(())
}

/// The server accepts a usage summary of a day from 7 days ago to yesterday.
fn summary_day_is_open(day: u64, today: u64) -> bool {
    day < today && day.saturating_add(USAGE_DAYS_BACK) >= today
}

/// Rounds a whole number to its 2 most significant decimal digits. Half
/// rounds up: 1234 gives 1200, 1250 gives 1300, 99 stays 99.
fn round2_tokens(value: u64) -> u64 {
    if value < 100 {
        return value;
    }
    let scale = 10_u64.pow(value.ilog10() - 1);
    let (head, rest) = (value / scale, value % scale);
    let head = if rest.saturating_mul(2) >= scale {
        head + 1
    } else {
        head
    };
    head.saturating_mul(scale)
}

/// Rounds an amount to its 2 most significant decimal digits. Half rounds
/// up: 0.0456 gives 0.046, 12.5 gives 13, 1250 gives 1300. The result is the
/// nearest f64 to the decimal value, so it prints without noise.
fn round2_amount(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    // The shortest decimal form of the f64, for example "1.2345e3".
    let text = format!("{value:e}");
    let Some((mantissa, exponent)) = text.split_once('e') else {
        return 0.0;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return 0.0;
    };
    let digits = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|digit| u32::from(digit - b'0'))
        .chain(std::iter::repeat(0))
        .take(3)
        .collect::<Vec<_>>();
    let head = digits[0] * 10 + digits[1] + u32::from(digits[2] >= 5);
    format!("{head}e{}", exponent - 1).parse().unwrap_or(0.0)
}

/// True for an amount that `round2_amount` does not change, within the
/// tolerance of the server.
fn is_rounded_amount(value: f64) -> bool {
    value.is_finite()
        && (0.0..=USD_SAVED_MAX).contains(&value)
        && (value - round2_amount(value)).abs() <= 1e-9 * value.max(1.0)
}

fn validate_download_id(event: &str, download_id: Option<&str>) -> Result<(), String> {
    match download_id {
        None => Ok(()),
        Some(value) if event == FIRST_OPEN && is_valid_download_id(value) => Ok(()),
        Some(_) => Err("Invalid analytics download id".to_string()),
    }
}

/// A download id is opaque: 1 to 64 characters of `[A-Za-z0-9_-]`.
fn is_valid_download_id(value: &str) -> bool {
    (1..=DOWNLOAD_ID_MAX_LENGTH).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Returns the download id of a `halv://welcome?d=<download_id>` link. Any
/// other URL, or a link with a bad or repeated id, gives `None`.
pub(crate) fn download_id_from_url(url: &url::Url) -> Option<String> {
    let host_matches = url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case(DOWNLOAD_LINK_HOST));
    if url.scheme() != DOWNLOAD_LINK_SCHEME
        || !host_matches
        || !matches!(url.path(), "" | "/")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let mut values = url
        .query_pairs()
        .filter(|(key, _)| key == DOWNLOAD_ID_PARAMETER)
        .map(|(_, value)| value.into_owned());
    let download_id = values.next()?;
    if values.next().is_some() {
        return None;
    }
    is_valid_download_id(&download_id).then_some(download_id)
}

/// `first_task_failed` and `setup_failed` take the categories of ADR 0021.
/// `proxy_failed` and `update_failed` need a category of ADR 0022. Other
/// events take none.
fn validate_category(event: &str, category: Option<&str>) -> Result<(), String> {
    let allowed: &[&str] = match event {
        "first_task_failed" | "setup_failed" => &TASK_CATEGORIES,
        PROXY_FAILED => &PROXY_CATEGORIES,
        UPDATE_FAILED => &UPDATE_CATEGORIES,
        _ => &[],
    };
    if category.is_some_and(|value| !allowed.contains(&value)) {
        return Err("Invalid analytics failure category".to_string());
    }
    if category.is_none() && matches!(event, PROXY_FAILED | UPDATE_FAILED) {
        return Err(format!("Missing analytics category on {event}"));
    }
    Ok(())
}

fn unix_time_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("System time is before the Unix epoch: {error}"))
}

fn analytics_os() -> Result<&'static str, String> {
    match std::env::consts::OS {
        "macos" => Ok("macos"),
        "windows" => Ok("windows"),
        "linux" => Ok("linux"),
        os => Err(format!("Unsupported analytics operating system: {os}")),
    }
}

fn read_or_create_install_id(app_data_dir: &Path) -> io::Result<String> {
    let path = app_data_dir.join(INSTALL_ID_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let install_id = contents.trim();
            if is_uuid_v4(install_id) {
                return Ok(install_id.to_string());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let install_id = new_uuid_v4();
    write_text_atomically(&path, &format!("{install_id}\n"))?;
    Ok(install_id)
}

fn new_uuid_v4() -> String {
    let mut bytes = rand::random::<[u8; 16]>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[14] == b'4'
        && bytes[18] == b'-'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

fn read_queue(path: &Path) -> io::Result<AnalyticsQueue> {
    match fs::read(path) {
        Ok(contents) => serde_json::from_slice(&contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AnalyticsQueue::default()),
        Err(error) => Err(error),
    }
}

/// Reads the stored download id. A damaged file counts as no id, so it never
/// stops the app from starting.
fn read_attribution(path: &Path) -> Option<Attribution> {
    app_data::restrict_file(path);
    let contents = fs::read(path).ok()?;
    serde_json::from_slice::<Attribution>(&contents)
        .ok()
        .filter(|attribution| is_valid_download_id(&attribution.download_id))
}

fn write_attribution_atomically(path: &Path, attribution: &Attribution) -> io::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(attribution)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    serialized.push(b'\n');
    write_bytes_atomically(path, &serialized)
}

/// Reads the analytics state. A damaged file counts as an empty state, so it
/// never stops the app from starting.
fn read_state(path: &Path) -> AnalyticsState {
    app_data::restrict_file(path);
    fs::read(path)
        .ok()
        .and_then(|contents| serde_json::from_slice::<AnalyticsState>(&contents).ok())
        .unwrap_or_default()
        .sanitized()
}

fn write_state_atomically(path: &Path, state: &AnalyticsState) -> io::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(state)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    serialized.push(b'\n');
    write_bytes_atomically(path, &serialized)
}

fn write_queue_atomically(path: &Path, queue: &AnalyticsQueue) -> io::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(queue)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    serialized.push(b'\n');
    write_bytes_atomically(path, &serialized)
}

fn write_text_atomically(path: &Path, contents: &str) -> io::Result<()> {
    write_bytes_atomically(path, contents.as_bytes())
}

fn write_bytes_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary_path = path.with_extension("tmp");
    let result = (|| {
        let mut temporary = app_data::private_file_options()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(contents)?;
        temporary.sync_all()?;
        drop(temporary);
        replace_file(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary_path, path)
}

#[cfg(windows)]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
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

#[cfg(test)]
async fn flush_pending(
    manager: &AnalyticsManager,
    server_url: &str,
    app_version: &str,
) -> Result<(), String> {
    flush_pending_guarded(manager, server_url, app_version, &|| Ok(Sharing::V1)).await
}

/// Sends the queue under the consent that `sharing` reads (ADR 0021, ADR
/// 0022). The gate runs again before each request, so a consent change
/// also stops or strips the later batches of a running flush.
async fn flush_pending_guarded(
    manager: &AnalyticsManager,
    server_url: &str,
    app_version: &str,
    sharing: &(dyn Fn() -> Result<Sharing, String> + Sync),
) -> Result<(), String> {
    let _flush_guard = manager.flush_lock.lock().await;
    match sharing()? {
        Sharing::Nothing => return manager.clear_pending(),
        Sharing::V1 => manager.drop_v2_data()?,
        Sharing::V2 => {}
    }
    manager.expire_pending(unix_time_seconds()?)?;
    let events = manager.pending()?;
    if events.is_empty() {
        return Ok(());
    }
    let locale = manager.state()?.locale;

    // Separate the corrected journey from legacy definitions. An older server
    // returns 404 here: retain the queue until the coordinated server rollout.
    for (path, selected) in [
        (
            ANALYTICS_EVENTS_PATH,
            events
                .iter()
                .filter(|event| ALLOWED_EVENTS.contains(&event.event.as_str()))
                .cloned()
                .collect::<Vec<_>>(),
        ),
        (
            JOURNEY_EVENTS_PATH,
            events
                .iter()
                .filter(|event| is_journey_event(&event.event))
                .cloned()
                .collect::<Vec<_>>(),
        ),
    ] {
        for batch in selected.chunks(10) {
            // The legacy path never carries v2 data.
            let level = match sharing()? {
                Sharing::Nothing => return manager.clear_pending(),
                Sharing::V2 if path == JOURNEY_EVENTS_PATH => Sharing::V2,
                Sharing::V1 | Sharing::V2 => Sharing::V1,
            };
            let mut sent = events_for(batch, level);
            if sent.is_empty() {
                continue;
            }
            let send = SendContext {
                server_url,
                path,
                install_id: &manager.install_id,
                app_version,
                locale: locale.as_deref(),
            };
            let mut status = send.post(&sent, level).await?;
            // A server without the v2 contract rejects the batch with HTTP
            // 400. Send the v1 part again one time without the v2 data. The
            // v2 events stay in the queue for a later try (ADR 0022).
            if status == reqwest::StatusCode::BAD_REQUEST && level == Sharing::V2 {
                if sharing()? == Sharing::Nothing {
                    return manager.clear_pending();
                }
                sent = events_for(batch, Sharing::V1);
                if sent.is_empty() {
                    continue;
                }
                status = send.post(&sent, Sharing::V1).await?;
            }
            // A server that does not know `download_id` rejects the batch
            // with HTTP 400. Send the batch again without the id, so the
            // journey does not stop (ADR 0021).
            if status == reqwest::StatusCode::BAD_REQUEST
                && sent.iter().any(|event| event.download_id.is_some())
            {
                if sharing()? == Sharing::Nothing {
                    return manager.clear_pending();
                }
                let without_download_id = sent
                    .iter()
                    .cloned()
                    .map(|event| AnalyticsEvent {
                        download_id: None,
                        ..event
                    })
                    .collect::<Vec<_>>();
                status = send.post(&without_download_id, Sharing::V1).await?;
            }
            if !status.is_success() {
                return Err(format!("Analytics request returned HTTP {status}"));
            }
            manager.acknowledge(&sent)?;
        }
    }
    Ok(())
}

/// The fixed parts of the requests of one batch.
struct SendContext<'a> {
    server_url: &'a str,
    path: &'a str,
    install_id: &'a str,
    app_version: &'a str,
    locale: Option<&'a str>,
}

impl SendContext<'_> {
    async fn post(
        &self,
        events: &[AnalyticsEvent],
        sharing: Sharing,
    ) -> Result<reqwest::StatusCode, String> {
        let batch = AnalyticsBatch::new(
            self.install_id,
            self.app_version,
            analytics_os()?,
            sharing,
            self.locale,
            events,
        );
        auth::api_client()
            .post(auth::api_url(self.server_url, self.path))
            .json(&batch)
            .send()
            .await
            .map(|response| response.status())
            .map_err(|error| format!("Analytics request failed: {error}"))
    }
}

/// Records one event only when the consent allows it (ADR 0021, ADR 0022).
/// No consent drops the event and the queue, and does not mark the event
/// as recorded. Version 1 drops a v2 event and strips the v2 fields of a
/// v1 event. Returns true when the consent allows the event.
fn record_if_shared(
    manager: &AnalyticsManager,
    sharing: Sharing,
    event: AnalyticsEvent,
) -> Result<bool, String> {
    validate_event(&event.event)?;
    let event = match sharing {
        Sharing::Nothing => {
            manager.clear_pending()?;
            return Ok(false);
        }
        Sharing::V1 if is_v2_event(&event.event) => return Ok(false),
        Sharing::V1 => event.without_v2_fields(),
        Sharing::V2 => event,
    };
    manager.record_event(event)?;
    Ok(true)
}

/// Reads the consent from the preferences.
fn current_sharing(preferences: &PreferencesManager) -> Result<Sharing, String> {
    preferences.get().map(|preferences| {
        Sharing::from_consent(
            preferences.analytics_consent,
            preferences.analytics_consent_version,
        )
    })
}

/// Builds an event from the webview. The webview records the journey
/// events of ADR 0021 with the optional facts of ADR 0022: `agent_kind` and
/// `auth_method` on `account_connected`, and `surface` on `trial_prompted`.
/// The app records the v2 events itself.
fn webview_event(
    event: &str,
    category: Option<String>,
    props: Option<serde_json::Map<String, serde_json::Value>>,
    at: u64,
) -> Result<AnalyticsEvent, String> {
    validate_event(event)?;
    if is_v2_event(event) {
        return Err(format!("Halv records the analytics event {event} itself"));
    }
    let mut value = AnalyticsEvent {
        category,
        ..AnalyticsEvent::new(event, at)
    };
    for (key, prop) in props.into_iter().flatten() {
        let serde_json::Value::String(text) = prop else {
            return Err(format!("Invalid analytics property: {key}"));
        };
        match key.as_str() {
            "agent_kind" => value.agent_kind = Some(text),
            "auth_method" => value.auth_method = Some(text),
            "surface" => value.surface = Some(text),
            _ => return Err(format!("Unknown analytics property: {key}")),
        }
    }
    validate_event_fields(&value, at)?;
    Ok(value)
}

/// Reads the site links that open Halv (ADR 0021): the link that started the
/// app and each later link. The deep-link plugin must be registered.
pub fn listen_for_download_links(app: &AppHandle) {
    use tauri_plugin_deep_link::DeepLinkExt;

    let deep_link = app.deep_link();
    if needs_runtime_scheme_registration(
        std::env::consts::OS,
        runs_as_appimage(app),
        dev_build_registers_scheme(),
    ) {
        if let Err(error) = deep_link.register_all() {
            log_failure(&format!(
                "Halv could not register the halv:// link: {error}"
            ));
        }
    }
    if let Ok(Some(urls)) = deep_link.get_current() {
        accept_download_links(app, &urls);
    }
    let handle = app.clone();
    deep_link.on_open_url(move |event| accept_download_links(&handle, &event.urls()));
}

/// Tells if Halv must register the halv:// scheme when it starts. The MSI,
/// deb and rpm installers register it from `tauri.conf.json`. An AppImage has
/// no installer, so it registers the scheme itself. A dev build registers the
/// scheme only when the developer asks for it, because the registration takes
/// the link from an installed release app. macOS registers the scheme from
/// the app bundle and cannot register it at runtime.
fn needs_runtime_scheme_registration(os: &str, appimage: bool, dev_registration: bool) -> bool {
    match os {
        "linux" => appimage || dev_registration,
        "windows" => dev_registration,
        _ => false,
    }
}

/// Tells if the developer asked a dev build to register the halv:// scheme
/// with `HALV_REGISTER_LINK=1`. A release build always answers no.
fn dev_build_registers_scheme() -> bool {
    cfg!(debug_assertions) && register_link_requested(std::env::var(REGISTER_LINK_ENV).ok())
}

fn register_link_requested(value: Option<String>) -> bool {
    value.as_deref() == Some("1")
}

#[cfg(target_os = "linux")]
fn runs_as_appimage(app: &AppHandle) -> bool {
    app.env().appimage.is_some()
}

#[cfg(not(target_os = "linux"))]
fn runs_as_appimage(_app: &AppHandle) -> bool {
    false
}

fn accept_download_links(app: &AppHandle, urls: &[url::Url]) {
    let Some(download_id) = urls.iter().find_map(download_id_from_url) else {
        return;
    };
    let Some(manager) = app.try_state::<AnalyticsManager>() else {
        return;
    };
    let accepted =
        unix_time_seconds().and_then(|now| manager.accept_download_id(&download_id, now));
    match accepted {
        Ok(true) => {
            let _ = app.emit(
                ATTRIBUTION_EVENT,
                AttributionSnapshot {
                    download_id: Some(download_id),
                },
            );
        }
        Ok(false) => {}
        Err(error) => log_failure(&error),
    }
}

async fn flush_for_app(app: &AppHandle) -> Result<(), String> {
    let manager = app.state::<AnalyticsManager>().inner().clone();
    let preferences = app.state::<PreferencesManager>();
    if current_sharing(&preferences)? == Sharing::Nothing {
        return manager.clear_pending();
    }
    let server_url =
        auth::auth_server_url(&app.state::<Storage>()).map_err(|error| error.to_string())?;
    let app_version = app.package_info().version.to_string();
    flush_pending_guarded(&manager, &server_url, &app_version, &|| {
        current_sharing(&preferences)
    })
    .await
}

fn spawn_flush(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = flush_for_app(&app).await {
            log_failure(&error);
        }
    });
}

fn log_failure(error: &str) {
    #[cfg(debug_assertions)]
    eprintln!("{error}");
    #[cfg(not(debug_assertions))]
    let _ = error;
}

/// Records an event of the app itself (ADR 0022) with the current app
/// version, under the current consent. An error only goes to the log.
/// Returns true when the queue can have a new event to send.
fn record_native(app: &AppHandle, event: AnalyticsEvent) -> bool {
    let (Some(manager), Some(preferences)) = (
        app.try_state::<AnalyticsManager>(),
        app.try_state::<PreferencesManager>(),
    ) else {
        return false;
    };
    record_with_consent(
        &manager,
        &preferences,
        &app.package_info().version.to_string(),
        event,
    )
}

/// `record_native` on the managers. Returns true when the queue can have a
/// new event to send.
fn record_with_consent(
    manager: &AnalyticsManager,
    preferences: &PreferencesManager,
    app_version: &str,
    event: AnalyticsEvent,
) -> bool {
    let event = AnalyticsEvent {
        app_version: Some(app_version.to_string()),
        ..event
    };
    current_sharing(preferences)
        .and_then(|sharing| record_if_shared(manager, sharing, event))
        .unwrap_or_else(|error| {
            log_failure(&error);
            false
        })
}

/// Records `proxy_failed` for the Headroom proxy (ADR 0022), after the
/// outcome is known. The sidecar calls it.
pub(crate) fn record_proxy_failure(app: &AppHandle, category: &str, recovered: bool) {
    let Ok(now) = unix_time_seconds() else {
        return;
    };
    if record_native(app, proxy_failed_event(category, recovered, now)) {
        spawn_flush(app);
    }
}

/// Records `update_failed` (ADR 0022). The app updater calls it.
pub(crate) fn record_update_failure(app: &AppHandle, category: &str) {
    let Ok(now) = unix_time_seconds() else {
        return;
    };
    if record_native(app, update_failed_event(category, now)) {
        spawn_flush(app);
    }
}

fn proxy_failed_event(category: &str, recovered: bool, now: u64) -> AnalyticsEvent {
    AnalyticsEvent {
        category: Some(category.to_string()),
        recovered: Some(recovered),
        ..AnalyticsEvent::new(PROXY_FAILED, now)
    }
}

fn update_failed_event(category: &str, now: u64) -> AnalyticsEvent {
    AnalyticsEvent {
        category: Some(category.to_string()),
        ..AnalyticsEvent::new(UPDATE_FAILED, now)
    }
}

/// Runs the start work of ADR 0022. Call it after the boot record started
/// this run and after the app manages the analytics manager.
///
/// 1. A new app version records `app_updated`. The app stores the version
///    also without consent, because it is only local.
/// 2. An unclean end of the previous run records `crash_detected`.
/// 3. A version 2 consent without a stored day starts the summaries today.
/// 4. The usage summaries run now and every 6 hours.
pub(crate) fn on_app_start(app: &AppHandle) {
    let (Some(manager), Some(preferences)) = (
        app.try_state::<AnalyticsManager>(),
        app.try_state::<PreferencesManager>(),
    ) else {
        return;
    };
    let Ok(now) = unix_time_seconds() else {
        return;
    };
    let boot_history = app
        .try_state::<BootRecordManager>()
        .map(|boot_record| boot_record.history(2))
        .unwrap_or_default();
    let recorded = record_start_events(
        &manager,
        &preferences,
        &app.package_info().version.to_string(),
        &boot_history,
        now,
    );
    if recorded {
        spawn_flush(app);
    }
    spawn_usage_summaries(app.clone());
}

/// Steps 1 to 3 of `on_app_start` on the managers. `boot_history` is the
/// boot record, newest run first. Returns true when it recorded an event.
fn record_start_events(
    manager: &AnalyticsManager,
    preferences: &PreferencesManager,
    app_version: &str,
    boot_history: &[BootRecordEntry],
    now: u64,
) -> bool {
    let mut recorded = false;
    match manager.replace_last_app_version(app_version) {
        Ok(Some(from_version)) => {
            recorded |= record_with_consent(
                manager,
                preferences,
                app_version,
                app_updated_event(&from_version, now),
            );
        }
        Ok(None) => {}
        Err(error) => log_failure(&error),
    }
    if let Some(event) = crash_event(boot_history, now) {
        recorded |= record_with_consent(manager, preferences, app_version, event);
    }
    if current_sharing(preferences).ok() == Some(Sharing::V2) {
        if let Err(error) = manager.update_state(|state| {
            state.consent_v2_day.get_or_insert(now / DAY_SECONDS);
        }) {
            log_failure(&error);
        }
    }
    recorded
}

fn app_updated_event(from_version: &str, now: u64) -> AnalyticsEvent {
    AnalyticsEvent {
        from_version: Some(from_version.to_string()),
        ..AnalyticsEvent::new(APP_UPDATED, now)
    }
}

/// `crash_detected` when the previous run did not end well. `history` is the
/// boot record, newest run first. The current run is `Running`.
fn crash_event(history: &[BootRecordEntry], now: u64) -> Option<AnalyticsEvent> {
    let previous = history
        .iter()
        .find(|entry| entry.shutdown_kind != ShutdownKind::Running)?;
    if previous.shutdown_kind != ShutdownKind::Unclean || !is_valid_version(&previous.app_version) {
        return None;
    }
    let started_at = chrono::DateTime::parse_from_rfc3339(&previous.started_at).ok()?;
    let last_alive_at = chrono::DateTime::parse_from_rfc3339(&previous.last_alive_at).ok()?;
    let uptime_seconds = u64::try_from((last_alive_at - started_at).num_seconds()).unwrap_or(0);
    Some(AnalyticsEvent {
        prev_app_version: Some(previous.app_version.clone()),
        uptime_bucket: Some(uptime_bucket(uptime_seconds).to_string()),
        rss_bucket: Some(rss_bucket(previous.max_rss_bytes).to_string()),
        ..AnalyticsEvent::new(CRASH_DETECTED, now)
    })
}

fn uptime_bucket(seconds: u64) -> &'static str {
    match seconds {
        0..=59 => "lt_1m",
        60..=599 => "1m_10m",
        600..=3_599 => "10m_1h",
        3_600..=21_599 => "1h_6h",
        _ => "6h_plus",
    }
}

/// The peak memory of a run. The limits use binary units: 1 GB is 1024 MiB.
fn rss_bucket(bytes: Option<u64>) -> &'static str {
    let Some(bytes) = bytes else {
        return "unknown";
    };
    match bytes / MIB {
        0..=499 => "lt_500mb",
        500..=1_023 => "500mb_1gb",
        1_024..=2_047 => "1gb_2gb",
        2_048..=4_095 => "2gb_4gb",
        _ => "4gb_plus",
    }
}

fn spawn_usage_summaries(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(USAGE_SUMMARY_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // The first tick is at once.
            interval.tick().await;
            let handle = app.clone();
            let recorded = tauri::async_runtime::spawn_blocking(move || summarize_usage(&handle))
                .await
                .unwrap_or(false);
            if recorded {
                if let Err(error) = flush_for_app(&app).await {
                    log_failure(&error);
                }
            }
        }
    });
}

/// Records `usage_summarized` for each closed UTC day with usage that has
/// no summary yet. It runs only under consent version 2 and never covers a
/// day before the version 2 consent. Returns true when it recorded an event.
fn summarize_usage(app: &AppHandle) -> bool {
    let (Some(manager), Some(preferences), Some(storage)) = (
        app.try_state::<AnalyticsManager>(),
        app.try_state::<PreferencesManager>(),
        app.try_state::<Storage>(),
    ) else {
        return false;
    };
    unix_time_seconds()
        .and_then(|now| {
            record_usage_summaries(
                &manager,
                &preferences,
                &storage,
                &app.package_info().version.to_string(),
                now,
            )
        })
        .unwrap_or_else(|error| {
            log_failure(&error);
            false
        })
}

/// `summarize_usage` on the managers, at the time `now`. Returns true when
/// it recorded an event.
fn record_usage_summaries(
    manager: &AnalyticsManager,
    preferences: &PreferencesManager,
    storage: &Storage,
    app_version: &str,
    now: u64,
) -> Result<bool, String> {
    if current_sharing(preferences)? != Sharing::V2 {
        return Ok(false);
    }
    let Some(consent_day) = manager.state()?.consent_v2_day else {
        return Ok(false);
    };
    let session = storage
        .get_auth_session()
        .map_err(|error| error.to_string())?;
    let plan = summary_plan(session.as_ref(), now);
    let events = usage_events(
        now,
        consent_day,
        plan,
        |day| manager.has_v2_key(&usage_key(day)),
        |day| {
            storage
                .utc_day_usage(&utc_day_start(day), &utc_day_start(day + 1))
                .map_err(|error| error.to_string())
        },
    )?;
    // Each event gets its own record, so this loop never stops early.
    let mut recorded = false;
    for event in events {
        recorded |= record_with_consent(manager, preferences, app_version, event);
    }
    Ok(recorded)
}

/// The usage summaries to record now. `summarized` tells if a day has a
/// summary already. `usage_of` reads the local usage of a day. A day before
/// `consent_day` is never read.
fn usage_events(
    now: u64,
    consent_day: u64,
    plan: &'static str,
    summarized: impl Fn(u64) -> Result<bool, String>,
    mut usage_of: impl FnMut(u64) -> Result<UtcDayUsage, String>,
) -> Result<Vec<AnalyticsEvent>, String> {
    let mut events = Vec::new();
    for day in summary_days(now / DAY_SECONDS, consent_day) {
        if summarized(day)? {
            continue;
        }
        events.extend(usage_event(day, &usage_of(day)?, plan, now));
    }
    Ok(events)
}

/// The days that a usage summary can cover now: from 7 days ago to
/// yesterday, and never before the day of the version 2 consent.
fn summary_days(today: u64, consent_day: u64) -> std::ops::Range<u64> {
    today.saturating_sub(USAGE_DAYS_BACK).max(consent_day)..today
}

/// The start of a UTC day in RFC 3339.
fn utc_day_start(day: u64) -> String {
    i64::try_from(day.saturating_mul(DAY_SECONDS))
        .ok()
        .and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0))
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The summary of one UTC day, or `None` without an agent session. The
/// event takes only rounded values and buckets: the exact values stay here.
fn usage_event(
    day: u64,
    usage: &UtcDayUsage,
    plan: &'static str,
    now: u64,
) -> Option<AnalyticsEvent> {
    // The contract needs at least one agent and one session. A day with
    // savings but without an agent session has no valid summary.
    if usage.agent_kinds.is_empty() {
        return None;
    }
    let agents_used = usage
        .agent_kinds
        .iter()
        .map(|kind| agent_name(kind).to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(AnalyticsEvent {
        day: Some(day),
        agents_used: Some(agents_used),
        sessions_bucket: Some(sessions_bucket(usage.agent_kinds.len()).to_string()),
        tokens_saved: Some(round2_tokens(
            u64::try_from(usage.tokens_saved)
                .unwrap_or(0)
                .min(TOKENS_SAVED_MAX),
        )),
        usd_saved: Some(round2_amount(usage.dollars_usd.min(USD_SAVED_MAX))),
        crux_used: Some(usage.crux_tokens_saved > 0),
        routing_used: Some(usage.routed_tasks > 0),
        plan: Some(plan.to_string()),
        ..AnalyticsEvent::new(USAGE_SUMMARIZED, now)
    })
}

fn sessions_bucket(sessions: usize) -> &'static str {
    match sessions {
        0..=1 => "1",
        2..=5 => "2-5",
        6..=20 => "6-20",
        _ => "21+",
    }
}

/// The `AGENT` name of a stored agent kind (ADR 0022). An agent kind that
/// the list does not have becomes `other`.
fn agent_name(storage_name: &str) -> &'static str {
    match storage_name {
        "claude" => "claude_code",
        "codex" => "codex",
        "gemini" => "gemini",
        "kimi" => "kimi",
        "zai" => "glm",
        "grok" => "grok",
        "antigravity" => "antigravity",
        _ => "other",
    }
}

/// The plan of the summary, from the cached entitlements. A signed-out
/// install has `none`. A live trial has `trial`. A plan that the list does
/// not have also gives `none`.
fn summary_plan(session: Option<&AuthSession>, now: u64) -> &'static str {
    let Some(session) = session.filter(|session| session.token.is_some()) else {
        return "none";
    };
    let entitlements = session
        .entitlements_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<auth::Entitlements>(value).ok());
    let trialing = entitlements.as_ref().is_some_and(|entitlements| {
        entitlements
            .trial_end
            .is_some_and(|end| u64::try_from(end).is_ok_and(|end| end > now))
    });
    if trialing {
        return "trial";
    }
    let plan = entitlements
        .as_ref()
        .map(|entitlements| entitlements.plan.as_str())
        .or(session.plan.as_deref())
        .unwrap_or_default();
    PLANS
        .into_iter()
        .find(|known| *known == plan)
        .unwrap_or("none")
}

#[tauri::command]
pub async fn analytics_record(
    app: AppHandle,
    event: String,
    category: Option<String>,
    props: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<(), String> {
    let at = unix_time_seconds()?;
    let event = AnalyticsEvent {
        app_version: Some(app.package_info().version.to_string()),
        ..webview_event(&event, category, props, at)?
    };
    let manager = app.state::<AnalyticsManager>().inner().clone();
    let sharing = current_sharing(&app.state::<PreferencesManager>())?;
    if !record_if_shared(&manager, sharing, event)? {
        return Ok(());
    }
    spawn_flush(&app);
    Ok(())
}

/// Stores the UI language (ADR 0022). The webview calls it at start and at
/// each language change. Only a batch under consent version 2 carries it.
#[tauri::command]
pub fn analytics_set_locale(
    manager: tauri::State<'_, AnalyticsManager>,
    locale: String,
) -> Result<(), String> {
    manager.set_locale(&locale)
}

#[tauri::command]
pub async fn analytics_flush(app: AppHandle) -> Result<(), String> {
    let result = flush_for_app(&app).await;
    if let Err(error) = &result {
        log_failure(error);
    }
    result
}

#[tauri::command]
pub fn analytics_install_id(manager: tauri::State<'_, AnalyticsManager>) -> String {
    manager.install_id()
}

/// The stored download id. It stays `None` after `first_open` reached the
/// server, because Halv then deletes it.
#[tauri::command]
pub fn analytics_attribution_get(
    manager: tauri::State<'_, AnalyticsManager>,
) -> Result<AttributionSnapshot, String> {
    Ok(AttributionSnapshot {
        download_id: manager.download_id()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    /// The consent gate with an answer of list version 1.
    fn record_if_consented(
        manager: &AnalyticsManager,
        consent: AnalyticsConsent,
        event: &str,
        at: u64,
        category: Option<String>,
        app_version: Option<String>,
    ) -> Result<bool, String> {
        record_if_shared(
            manager,
            Sharing::from_consent(consent, 1),
            AnalyticsEvent {
                category,
                app_version,
                ..AnalyticsEvent::new(event, at)
            },
        )
    }

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("halv-analytics-{name}-{}", rand::random::<u64>()));
            fs::create_dir_all(&path).expect("create analytics test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn allowlist_accepts_only_funnel_events() {
        for event in ALLOWED_EVENTS {
            assert!(validate_event(event).is_ok(), "{event} should be allowed");
        }
        assert!(validate_event("signed_in").is_err());
        assert!(validate_event("").is_err());
    }

    #[test]
    fn queue_dedupes_and_persists_recorded_events() {
        let directory = TestDirectory::new("queue-dedupe");
        let manager = AnalyticsManager::open(&directory.0).expect("open analytics manager");
        assert!(manager.record("installed", 10).expect("record event"));
        assert!(!manager.record("installed", 20).expect("dedupe event"));
        assert_eq!(
            manager.pending().expect("read pending events"),
            vec![AnalyticsEvent::new("installed", 10)]
        );

        let reopened = AnalyticsManager::open(&directory.0).expect("reopen analytics manager");
        assert!(!reopened.record("installed", 30).expect("dedupe event"));
        assert_eq!(reopened.pending().expect("read pending events").len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn analytics_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TestDirectory::new("modes");
        let files = [
            directory.0.join(INSTALL_ID_FILE_NAME),
            directory.0.join(QUEUE_FILE_NAME),
        ];
        let mode = |path: &Path| {
            fs::metadata(path)
                .expect("analytics file should exist")
                .permissions()
                .mode()
                & 0o777
        };
        let manager = AnalyticsManager::open(&directory.0).expect("open analytics manager");
        manager.record("installed", 10).expect("record event");
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }

        // An earlier release left the files readable by others.
        for file in &files {
            fs::set_permissions(file, fs::Permissions::from_mode(0o644)).expect("change mode");
        }
        AnalyticsManager::open(&directory.0).expect("reopen analytics manager");
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }
    }

    #[test]
    fn acknowledging_events_clears_pending_but_keeps_dedupe_state() {
        let directory = TestDirectory::new("queue-acknowledge");
        let manager = AnalyticsManager::open(&directory.0).expect("open analytics manager");
        manager.record("installed", 10).expect("record installed");
        manager
            .record("cli_connected", 20)
            .expect("record CLI connection");
        let sent = vec![manager.pending().expect("read pending events")[0].clone()];
        manager.acknowledge(&sent).expect("acknowledge event");

        let reopened = AnalyticsManager::open(&directory.0).expect("reopen analytics manager");
        assert_eq!(
            reopened.pending().expect("read pending events"),
            vec![AnalyticsEvent::new("cli_connected", 20)]
        );
        assert!(!reopened.record("installed", 30).expect("dedupe event"));
    }

    #[test]
    fn install_id_is_v4_and_persists() {
        let directory = TestDirectory::new("install-id");
        let first = AnalyticsManager::open(&directory.0)
            .expect("open analytics manager")
            .install_id();
        assert!(is_uuid_v4(&first));
        let second = AnalyticsManager::open(&directory.0)
            .expect("reopen analytics manager")
            .install_id();
        assert_eq!(first, second);
    }
    #[test]
    fn daily_activity_dedupes_per_day_and_retains_unsent_days() {
        let directory = TestDirectory::new("activity-days");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert!(manager.record("active_day", 100 * 86_400).unwrap());
        assert!(!manager.record("active_day", 100 * 86_400 + 42).unwrap());
        assert!(manager.record("active_day", 101 * 86_400).unwrap());
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        let pending = reopened.pending().unwrap();
        assert_eq!(pending.len(), 2);
        reopened.acknowledge(&pending[..1]).unwrap();
        assert_eq!(reopened.pending().unwrap()[0].at, 101 * 86_400);
    }

    #[test]
    fn queued_journey_keeps_recorded_version_and_rejects_error_text() {
        let directory = TestDirectory::new("journey-context");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager
            .record_with_context("first_task_completed", 100, None, Some("0.6.0".into()))
            .unwrap();
        assert_eq!(
            manager.pending().unwrap()[0].app_version.as_deref(),
            Some("0.6.0")
        );
        assert!(manager
            .record_with_context("first_task_failed", 101, Some("/private/path".into()), None)
            .is_err());
        manager
            .record_with_context("first_task_failed", 101, Some("engine".into()), None)
            .unwrap();
        assert_eq!(manager.pending().unwrap().len(), 2);
        manager.clear_pending().unwrap();
        assert!(manager.pending().unwrap().is_empty());
    }

    #[test]
    fn expired_queue_is_removed_before_retry_and_stays_removed_after_restart() {
        let directory = TestDirectory::new("expired-queue");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager.record("first_open", 100).unwrap();
        manager.record("first_task_started", 200).unwrap();
        manager.expire_pending(100 + 30 * 86_400 + 1).unwrap();
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(reopened.pending().unwrap().len(), 1);
        assert_eq!(reopened.pending().unwrap()[0].event, "first_task_started");
        assert!(!reopened
            .record("first_open", 100 + 30 * 86_400 + 2)
            .unwrap());
    }

    #[tokio::test]
    async fn disabled_sharing_drops_queue_without_any_network_request() {
        let directory = TestDirectory::new("privacy-disabled");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager
            .record("first_open", unix_time_seconds().unwrap())
            .unwrap();
        flush_pending_guarded(&manager, "http://127.0.0.1:1", "0.6.0", &|| {
            Ok(Sharing::Nothing)
        })
        .await
        .unwrap();
        assert!(AnalyticsManager::open(&directory.0)
            .unwrap()
            .pending()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn old_server_keeps_new_journey_queued_for_rollout_retry() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 8192];
            let size = stream.read(&mut buffer).unwrap();
            assert!(
                String::from_utf8_lossy(&buffer[..size]).starts_with("POST /v1/analytics/journey ")
            );
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let directory = TestDirectory::new("old-server");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager
            .record("first_open", unix_time_seconds().unwrap())
            .unwrap();
        assert!(flush_pending(&manager, &url, "0.6.0").await.is_err());
        server.join().unwrap();
        assert_eq!(manager.pending().unwrap().len(), 1);
    }

    fn link(value: &str) -> url::Url {
        url::Url::parse(value).unwrap()
    }

    #[test]
    fn download_link_parser_accepts_only_a_valid_welcome_link() {
        let max = "a".repeat(DOWNLOAD_ID_MAX_LENGTH);
        let accepted = [
            ("halv://welcome?d=dl_8Hc-2", "dl_8Hc-2"),
            ("halv://welcome/?d=abc", "abc"),
            ("halv://WELCOME?d=abc", "abc"),
            ("halv://welcome?utm_source=site&d=abc", "abc"),
            ("halv://welcome?d=abc#top", "abc"),
        ];
        for (value, expected) in accepted {
            assert_eq!(
                download_id_from_url(&link(value)).as_deref(),
                Some(expected),
                "{value}"
            );
        }
        assert_eq!(
            download_id_from_url(&link(&format!("halv://welcome?d={max}"))),
            Some(max.clone())
        );

        let rejected = [
            // Bad characters, also after percent decoding.
            "halv://welcome?d=abc.def",
            "halv://welcome?d=abc%20def",
            "halv://welcome?d=abc%2Fdef",
            "halv://welcome?d=%C3%A9t%C3%A9",
            "halv://welcome?d=",
            // Too long.
            &format!("halv://welcome?d={max}a"),
            // Wrong host, path, scheme or authority.
            "halv://settings?d=abc",
            "halv://welcome/extra?d=abc",
            "https://welcome?d=abc",
            "https://halv.ai/welcome?d=abc",
            "halv://user@welcome?d=abc",
            "halv://welcome:8080?d=abc",
            "halv:welcome?d=abc",
            // No query, no id, or a repeated id.
            "halv://welcome",
            "halv://welcome?download=abc",
            "halv://welcome?d=abc&d=def",
        ];
        for value in rejected {
            assert_eq!(download_id_from_url(&link(value)), None, "{value}");
        }
    }

    #[test]
    fn only_installs_without_an_installer_register_the_link_scheme() {
        assert!(needs_runtime_scheme_registration("linux", true, false));
        assert!(needs_runtime_scheme_registration("linux", false, true));
        assert!(needs_runtime_scheme_registration("windows", false, true));
        assert!(!needs_runtime_scheme_registration("linux", false, false));
        assert!(!needs_runtime_scheme_registration("windows", false, false));
        assert!(!needs_runtime_scheme_registration("macos", false, true));
        assert!(!needs_runtime_scheme_registration("macos", false, false));
    }

    /// The bundler writes the Exec line of the deb, rpm and AppImage launcher
    /// from this template. Without %u the launcher starts Halv without the
    /// halv:// link (tauri-apps/tauri#15928).
    #[test]
    fn linux_launcher_template_passes_the_link_to_halv() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        for package in ["deb", "rpm"] {
            assert_eq!(
                config["bundle"]["linux"][package]["desktopTemplate"], "linux/halv.desktop",
                "{package}"
            );
        }
        assert_eq!(
            config["plugins"]["deep-link"]["desktop"]["schemes"],
            serde_json::json!([DOWNLOAD_LINK_SCHEME])
        );
        let template = include_str!("../linux/halv.desktop");
        assert!(template.lines().any(|line| line == "Exec={{exec}} %u"));
        assert!(template
            .lines()
            .any(|line| line == "MimeType={{mime_type}}"));
    }

    #[test]
    fn a_dev_build_registers_the_link_scheme_only_when_asked() {
        assert!(register_link_requested(Some("1".to_string())));
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 1")] {
            assert!(
                !register_link_requested(value.map(str::to_string)),
                "{value:?}"
            );
        }
    }

    #[test]
    fn unset_or_declined_consent_drops_events_without_marking_them() {
        let directory = TestDirectory::new("consent-gate");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        for consent in [AnalyticsConsent::Unset, AnalyticsConsent::Off] {
            assert!(!record_if_consented(&manager, consent, FIRST_OPEN, 100, None, None).unwrap());
            assert!(manager.pending().unwrap().is_empty());
        }
        assert!(record_if_consented(
            &manager,
            AnalyticsConsent::Unset,
            "not_an_event",
            100,
            None,
            None
        )
        .is_err());

        // A later yes still records the event that the unset answer dropped.
        assert!(
            record_if_consented(&manager, AnalyticsConsent::On, FIRST_OPEN, 200, None, None)
                .unwrap()
        );
        assert_eq!(manager.pending().unwrap().len(), 1);
        assert_eq!(manager.pending().unwrap()[0].at, 200);

        // Unset again drops the queue.
        record_if_consented(
            &manager,
            AnalyticsConsent::Unset,
            "active_day",
            300,
            None,
            None,
        )
        .unwrap();
        assert!(manager.pending().unwrap().is_empty());
    }

    #[test]
    fn download_id_is_stored_without_consent_and_sent_with_first_open() {
        let directory = TestDirectory::new("attribution-before-consent");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(manager.download_id().unwrap(), None);
        assert!(manager.accept_download_id("bad id", 10).is_err());

        // The link arrives while the answer is unset. Halv stores the id.
        assert!(
            !record_if_consented(&manager, AnalyticsConsent::Unset, FIRST_OPEN, 5, None, None)
                .unwrap()
        );
        assert!(manager.accept_download_id("site-download_1", 10).unwrap());
        // The first id wins.
        assert!(!manager.accept_download_id("site-download_2", 11).unwrap());
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(
            reopened.download_id().unwrap().as_deref(),
            Some("site-download_1")
        );

        // The user agrees later. `first_open` carries the id.
        record_if_consented(&reopened, AnalyticsConsent::On, FIRST_OPEN, 20, None, None).unwrap();
        record_if_consented(
            &reopened,
            AnalyticsConsent::On,
            "active_day",
            20,
            None,
            None,
        )
        .unwrap();
        let pending = reopened.pending().unwrap();
        assert_eq!(pending[0].download_id.as_deref(), Some("site-download_1"));
        assert_eq!(pending[1].download_id, None);
        let serialized = serde_json::to_value(&pending[0]).unwrap();
        assert_eq!(serialized["download_id"], "site-download_1");
        assert!(serde_json::to_value(&pending[1])
            .unwrap()
            .get("download_id")
            .is_none());

        // After the server has `first_open`, Halv deletes the id and drops
        // any later link.
        reopened.acknowledge(&pending).unwrap();
        assert_eq!(reopened.download_id().unwrap(), None);
        assert!(!directory.0.join(ATTRIBUTION_FILE_NAME).exists());
        assert!(!reopened.accept_download_id("site-download_3", 30).unwrap());
        assert_eq!(reopened.download_id().unwrap(), None);
    }

    #[test]
    fn download_id_that_arrives_shortly_after_first_open_joins_the_queued_event() {
        let directory = TestDirectory::new("attribution-after-first-open");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager.record(FIRST_OPEN, 100).unwrap();
        assert_eq!(manager.pending().unwrap()[0].download_id, None);
        assert!(manager.accept_download_id("late_link", 101).unwrap());
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(
            reopened.pending().unwrap()[0].download_id.as_deref(),
            Some("late_link")
        );
        // Only `first_open` may carry the id.
        let mut queue = AnalyticsQueue::default();
        assert!(queue
            .record(AnalyticsEvent {
                download_id: Some("late_link".into()),
                ..AnalyticsEvent::new("active_day", 1)
            })
            .is_err());
    }

    #[test]
    fn damaged_attribution_file_counts_as_no_download_id() {
        let directory = TestDirectory::new("attribution-damaged");
        fs::write(directory.0.join(ATTRIBUTION_FILE_NAME), "{not json").unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(manager.download_id().unwrap(), None);
        fs::write(
            directory.0.join(ATTRIBUTION_FILE_NAME),
            r#"{"download_id":"bad id","received_at":1}"#,
        )
        .unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(manager.download_id().unwrap(), None);
        assert!(manager.accept_download_id("good_id", 2).unwrap());
    }

    #[tokio::test]
    async fn server_without_download_id_support_gets_the_batch_again_without_it() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    let size = stream.read(&mut buffer).unwrap_or(0);
                    if size == 0 {
                        break;
                    }
                    data.extend_from_slice(&buffer[..size]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&data).to_string();
                let body = text
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let status = if body.contains("download_id") {
                    "400 Bad Request"
                } else {
                    "200 OK"
                };
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                bodies.push(body);
            }
            bodies
        });
        let directory = TestDirectory::new("attribution-old-server");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager.accept_download_id("site_link", 1).unwrap();
        manager
            .record(FIRST_OPEN, unix_time_seconds().unwrap())
            .unwrap();
        flush_pending(&manager, &url, "1.0.0").await.unwrap();
        let bodies = server.join().unwrap();
        assert!(
            bodies[0].contains(r#""download_id":"site_link""#),
            "{}",
            bodies[0]
        );
        assert!(!bodies[1].contains("download_id"), "{}", bodies[1]);
        assert!(manager.pending().unwrap().is_empty());
        assert_eq!(manager.download_id().unwrap(), None);
    }

    // The tests below cover analytics v2 (ADR 0022, issue #58).

    const NOW: u64 = 1_792_000_000;

    fn today() -> u64 {
        NOW / DAY_SECONDS
    }

    fn props(pairs: &[(&str, &str)]) -> Option<serde_json::Map<String, serde_json::Value>> {
        Some(
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), serde_json::json!(value)))
                .collect(),
        )
    }

    fn boot_entry(
        app_version: &str,
        started_at: &str,
        last_alive_at: &str,
        shutdown_kind: ShutdownKind,
        max_rss_bytes: Option<u64>,
    ) -> BootRecordEntry {
        BootRecordEntry {
            boot_id: format!("{:032x}", rand::random::<u128>()),
            app_version: app_version.to_string(),
            started_at: started_at.to_string(),
            last_alive_at: last_alive_at.to_string(),
            shutdown_at: None,
            shutdown_kind,
            max_rss_bytes,
        }
    }

    fn usage(agent_kinds: &[&str], tokens_saved: i64, dollars_usd: f64) -> UtcDayUsage {
        UtcDayUsage {
            agent_kinds: agent_kinds.iter().map(|kind| kind.to_string()).collect(),
            tokens_saved,
            dollars_usd,
            crux_tokens_saved: 0,
            routed_tasks: 0,
        }
    }

    /// The accepted events of the golden payload, built by the app code.
    fn golden_events(now: u64) -> Vec<AnalyticsEvent> {
        let day_usage = UtcDayUsage {
            crux_tokens_saved: 40,
            ..usage(&["codex", "claude", "claude"], 123_456, 1.49)
        };
        let history = [
            boot_entry(
                "1.1.0",
                "2026-10-22T10:00:00.000Z",
                "2026-10-22T10:00:00.000Z",
                ShutdownKind::Running,
                None,
            ),
            boot_entry(
                "1.0.1",
                "2026-10-22T09:00:00.000Z",
                "2026-10-22T09:30:00.000Z",
                ShutdownKind::Unclean,
                Some(1536 * MIB),
            ),
        ];
        vec![
            usage_event(now / DAY_SECONDS - 1, &day_usage, "trial", now).unwrap(),
            crash_event(&history, now).unwrap(),
            proxy_failed_event(PROXY_EXIT, true, now),
            app_updated_event("1.0.1", now),
            update_failed_event(UPDATE_SIGNATURE, now),
            webview_event(
                "account_connected",
                None,
                props(&[("agent_kind", "glm"), ("auth_method", "api_key")]),
                now,
            )
            .unwrap(),
            webview_event(
                "trial_prompted",
                None,
                props(&[("surface", "routing")]),
                now,
            )
            .unwrap(),
        ]
    }

    /// Removes the line breaks of a golden payload from the contract.
    fn golden(text: &str, now: u64) -> String {
        text.replace("TODAY_MINUS_1", &(now / DAY_SECONDS - 1).to_string())
            .replace("NOW", &now.to_string())
            .replace("\n ", "")
            .replace('\n', "")
    }

    #[test]
    fn native_events_serialize_to_the_accepted_golden_payloads() {
        let events = golden_events(NOW);
        for event in &events {
            validate_event_fields(event, NOW).unwrap();
        }
        let batch = AnalyticsBatch::new(
            "install-golden-1",
            "1.1.0",
            "windows",
            Sharing::V2,
            Some("pt-BR"),
            &events,
        );
        let expected = golden(
            r#"{"install_id":"install-golden-1","app_version":"1.1.0","os":"windows","consent_version":2,"locale":"pt-BR","events":[
 {"event":"usage_summarized","at":NOW,"day":TODAY_MINUS_1,"agents_used":["claude_code","codex"],"sessions_bucket":"2-5","tokens_saved":120000,"usd_saved":1.5,"crux_used":true,"routing_used":false,"plan":"trial"},
 {"event":"crash_detected","at":NOW,"prev_app_version":"1.0.1","uptime_bucket":"10m_1h","rss_bucket":"1gb_2gb"},
 {"event":"proxy_failed","at":NOW,"category":"exit","recovered":true},
 {"event":"app_updated","at":NOW,"from_version":"1.0.1"},
 {"event":"update_failed","at":NOW,"category":"signature"},
 {"event":"account_connected","at":NOW,"agent_kind":"glm","auth_method":"api_key"},
 {"event":"trial_prompted","at":NOW,"surface":"routing"}]}"#,
            NOW,
        );
        assert_eq!(serde_json::to_string(&batch).unwrap(), expected);

        let first_open = vec![AnalyticsEvent {
            download_id: Some("site_abc-1".to_string()),
            ..AnalyticsEvent::new(FIRST_OPEN, NOW)
        }];
        let batch = AnalyticsBatch::new(
            "install-golden-2",
            "1.1.0",
            "macos",
            Sharing::V1,
            Some("pt-BR"),
            &first_open,
        );
        assert_eq!(
            serde_json::to_string(&batch).unwrap(),
            golden(
                r#"{"install_id":"install-golden-2","app_version":"1.1.0","os":"macos","events":[{"event":"first_open","at":NOW,"download_id":"site_abc-1"}]}"#,
                NOW
            )
        );
    }

    #[test]
    fn version_1_sends_no_v2_event_no_v2_field_and_no_locale() {
        let events = events_for(&golden_events(NOW), Sharing::V1);
        let batch = AnalyticsBatch::new(
            "install-golden-1",
            "1.1.0",
            "windows",
            Sharing::V1,
            Some("pt-BR"),
            &events,
        );
        assert_eq!(
            serde_json::to_string(&batch).unwrap(),
            golden(
                r#"{"install_id":"install-golden-1","app_version":"1.1.0","os":"windows","events":[{"event":"account_connected","at":NOW},{"event":"trial_prompted","at":NOW}]}"#,
                NOW
            )
        );
        assert!(events_for(&golden_events(NOW), Sharing::Nothing).is_empty());
    }

    #[test]
    fn the_app_rejects_each_rejected_golden_value() {
        let summary = || golden_events(NOW).remove(0);
        let rejected = [
            AnalyticsEvent {
                tokens_saved: Some(123_456),
                ..summary()
            },
            AnalyticsEvent {
                usd_saved: Some(1.234),
                ..summary()
            },
            AnalyticsEvent {
                agents_used: Some(vec!["codex".into(), "claude_code".into()]),
                ..summary()
            },
            AnalyticsEvent {
                agents_used: Some(Vec::new()),
                ..summary()
            },
            AnalyticsEvent {
                agents_used: Some(vec!["codex".into(), "codex".into()]),
                ..summary()
            },
            AnalyticsEvent {
                day: Some(today()),
                ..summary()
            },
            AnalyticsEvent {
                day: Some(today() - 8),
                ..summary()
            },
            AnalyticsEvent {
                plan: Some("pro".into()),
                ..summary()
            },
            AnalyticsEvent {
                plan: None,
                ..summary()
            },
            AnalyticsEvent {
                surface: Some("routing".into()),
                ..AnalyticsEvent::new("account_connected", NOW)
            },
            AnalyticsEvent {
                recovered: Some(true),
                ..update_failed_event(UPDATE_DOWNLOAD, NOW)
            },
            update_failed_event(PROXY_EXIT, NOW),
            AnalyticsEvent::new(UPDATE_FAILED, NOW),
            AnalyticsEvent {
                rss_bucket: Some("8gb".into()),
                ..golden_events(NOW).remove(1)
            },
            AnalyticsEvent {
                from_version: Some("1.0 beta".into()),
                ..app_updated_event("1.0.1", NOW)
            },
            AnalyticsEvent {
                agent_kind: Some("claude".into()),
                ..AnalyticsEvent::new("account_connected", NOW)
            },
            AnalyticsEvent {
                category: Some("exit".into()),
                ..AnalyticsEvent::new("first_task_failed", NOW)
            },
        ];
        for event in rejected {
            assert!(
                validate_event_fields(&event, NOW).is_err(),
                "{}",
                serde_json::to_string(&event).unwrap()
            );
            assert!(AnalyticsQueue::default().record(event).is_err());
        }

        // The webview cannot send a nested object, an unknown field or a
        // fact of another event. It cannot record a v2 event either.
        let nested = Some(
            [("agent_kind".to_string(), serde_json::json!({"name": "glm"}))]
                .into_iter()
                .collect(),
        );
        assert!(webview_event("account_connected", None, nested, NOW).is_err());
        for (event, pairs) in [
            ("account_connected", &[("prompt", "hello")][..]),
            ("account_connected", &[("surface", "routing")][..]),
            ("trial_prompted", &[("agent_kind", "glm")][..]),
            ("trial_prompted", &[("surface", "settings")][..]),
            ("first_open", &[("auth_method", "oauth")][..]),
        ] {
            assert!(
                webview_event(event, None, props(pairs), NOW).is_err(),
                "{event} {pairs:?}"
            );
        }
        for event in V2_EVENTS {
            assert!(webview_event(event, None, None, NOW).is_err(), "{event}");
        }
        assert!(webview_event("setup_failed", Some("engine".into()), props(&[]), NOW).is_ok());
    }

    #[test]
    fn rounding_keeps_two_significant_digits_and_rounds_half_up() {
        for (exact, rounded) in [
            (0, 0),
            (7, 7),
            (99, 99),
            (100, 100),
            (1_234, 1_200),
            (1_250, 1_300),
            (1_249, 1_200),
            (99_500, 100_000),
            (123_456, 120_000),
            (TOKENS_SAVED_MAX, TOKENS_SAVED_MAX),
        ] {
            assert_eq!(round2_tokens(exact), rounded, "{exact}");
        }
        for (exact, rounded) in [
            (0.0, "0"),
            (-3.0, "0"),
            (f64::NAN, "0"),
            (7.0, "7"),
            (12.5, "13"),
            (99.0, "99"),
            (0.123, "0.12"),
            (0.0456, "0.046"),
            (1.49, "1.5"),
            (23.4, "23"),
            (449.9, "450"),
            (1_234.5, "1200"),
            (999_999.0, "1000000"),
        ] {
            let event = AnalyticsEvent {
                usd_saved: Some(round2_amount(exact)),
                ..AnalyticsEvent::default()
            };
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["usd_saved"].to_string(), rounded, "{exact}");
            assert!(is_rounded_amount(round2_amount(exact)), "{exact}");
        }
        assert!(!is_rounded_amount(1.234));
        assert!(!is_rounded_amount(USD_SAVED_MAX * 2.0));
    }

    #[test]
    fn the_exact_savings_never_reach_the_serialized_event() {
        let event = usage_event(
            today() - 1,
            &usage(&["codex"], 123_456, 1.234),
            "basic",
            NOW,
        )
        .unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""tokens_saved":120000"#), "{json}");
        assert!(json.contains(r#""usd_saved":1.2"#), "{json}");
        assert!(
            !json.contains("123456") && !json.contains("1.234"),
            "{json}"
        );
        // An exact value that a bug puts in the event never enters the queue.
        let mut queue = AnalyticsQueue::default();
        assert!(queue
            .record(AnalyticsEvent {
                tokens_saved: Some(123_456),
                ..event.clone()
            })
            .is_err());
        assert!(queue.record(event).unwrap());
        // A value above the range of the server becomes the maximum.
        let huge = usage_event(
            today() - 1,
            &usage(&["codex"], i64::MAX, 1e12),
            "basic",
            NOW,
        )
        .unwrap();
        assert_eq!(huge.tokens_saved, Some(TOKENS_SAVED_MAX));
        assert_eq!(huge.usd_saved, Some(USD_SAVED_MAX));
        validate_event_fields(&huge, NOW).unwrap();
    }

    #[test]
    fn buckets_and_agent_names_follow_the_contract() {
        use crate::agent_sessions::AgentKind;
        for (seconds, bucket) in [
            (0, "lt_1m"),
            (59, "lt_1m"),
            (60, "1m_10m"),
            (599, "1m_10m"),
            (600, "10m_1h"),
            (3_599, "10m_1h"),
            (3_600, "1h_6h"),
            (21_599, "1h_6h"),
            (21_600, "6h_plus"),
        ] {
            assert_eq!(uptime_bucket(seconds), bucket, "{seconds}");
        }
        for (bytes, bucket) in [
            (None, "unknown"),
            (Some(0), "lt_500mb"),
            (Some(500 * MIB - 1), "lt_500mb"),
            (Some(500 * MIB), "500mb_1gb"),
            (Some(1024 * MIB), "1gb_2gb"),
            (Some(2048 * MIB), "2gb_4gb"),
            (Some(4096 * MIB), "4gb_plus"),
        ] {
            assert_eq!(rss_bucket(bytes), bucket, "{bytes:?}");
        }
        for (sessions, bucket) in [
            (1, "1"),
            (2, "2-5"),
            (5, "2-5"),
            (6, "6-20"),
            (20, "6-20"),
            (21, "21+"),
        ] {
            assert_eq!(sessions_bucket(sessions), bucket, "{sessions}");
        }
        for (kind, name) in [
            (AgentKind::Claude, "claude_code"),
            (AgentKind::Codex, "codex"),
            (AgentKind::Gemini, "gemini"),
            (AgentKind::Kimi, "kimi"),
            (AgentKind::Zai, "glm"),
            (AgentKind::Grok, "grok"),
            (AgentKind::Antigravity, "antigravity"),
        ] {
            assert_eq!(agent_name(kind.storage_name()), name);
        }
        assert_eq!(agent_name("cursor"), "other");
        let event = usage_event(
            today() - 1,
            &usage(&["zai", "cursor", "claude", "zai"], 0, 0.0),
            "none",
            NOW,
        )
        .unwrap();
        assert_eq!(
            event.agents_used,
            Some(vec!["claude_code".into(), "glm".into(), "other".into()])
        );
        assert_eq!(event.sessions_bucket.as_deref(), Some("2-5"));
        assert_eq!(event.crux_used, Some(false));
        assert_eq!(event.routing_used, Some(false));
    }

    #[test]
    fn a_crash_needs_an_unclean_previous_run() {
        let current = || {
            boot_entry(
                "1.1.0",
                "2026-10-22T10:00:00.000Z",
                "2026-10-22T10:00:00.000Z",
                ShutdownKind::Running,
                None,
            )
        };
        let previous = |kind, version: &str| {
            boot_entry(
                version,
                "2026-10-21T08:00:00.000Z",
                "2026-10-21T15:00:00.000Z",
                kind,
                None,
            )
        };
        let crash =
            crash_event(&[current(), previous(ShutdownKind::Unclean, "1.0.1")], NOW).unwrap();
        assert_eq!(crash.uptime_bucket.as_deref(), Some("6h_plus"));
        assert_eq!(crash.rss_bucket.as_deref(), Some("unknown"));
        assert_eq!(crash.key(), format!("crash_detected:{}", today()));
        assert!(crash_event(&[current(), previous(ShutdownKind::Clean, "1.0.1")], NOW).is_none());
        assert!(crash_event(&[current()], NOW).is_none());
        assert!(crash_event(
            &[current(), previous(ShutdownKind::Unclean, "bad version")],
            NOW
        )
        .is_none());
        let mut damaged = previous(ShutdownKind::Unclean, "1.0.1");
        damaged.started_at = "not a time".to_string();
        assert!(crash_event(&[current(), damaged], NOW).is_none());
    }

    #[test]
    fn summaries_never_cover_a_day_before_the_version_2_consent() {
        let today = today();
        assert_eq!(summary_days(today, today - 2), today - 2..today);
        assert_eq!(summary_days(today, today - 30), today - 7..today);
        assert!(summary_days(today, today).is_empty());
        assert!(summary_days(today, today + 1).is_empty());

        let consent_day = today - 3;
        let mut read = Vec::new();
        let events = usage_events(
            NOW,
            consent_day,
            "basic",
            |day| Ok(day == today - 2),
            |day| {
                read.push(day);
                Ok(if day == today - 1 {
                    usage(&[], 500, 0.5)
                } else {
                    usage(&["codex"], 500, 0.5)
                })
            },
        )
        .unwrap();
        // The day before consent is never read. A summarized day is not read
        // again. A day without an agent session has no summary.
        assert_eq!(read, vec![today - 3, today - 1]);
        assert_eq!(
            events.iter().map(|event| event.day).collect::<Vec<_>>(),
            vec![Some(today - 3)]
        );
        assert_eq!(events[0].key(), format!("usage_summarized:{}", today - 3));
    }

    #[test]
    fn the_summary_plan_comes_from_the_cached_entitlements() {
        let session =
            |token: Option<&str>, plan: Option<&str>, entitlements: Option<String>| AuthSession {
                server_url: "https://api.halv.ai".to_string(),
                token: token.map(str::to_string),
                email: None,
                plan: plan.map(str::to_string),
                entitlements_json: entitlements,
                synced_at: None,
            };
        let entitlements = |plan: &str, trial_end: Option<u64>| {
            let mut value = serde_json::json!({
                "plan": plan, "beta": false, "billingRequired": true, "canUseApp": true,
                "message": null, "checkoutUrl": null
            });
            if let Some(end) = trial_end {
                value["trialEnd"] = serde_json::json!(end);
            }
            Some(value.to_string())
        };
        assert_eq!(summary_plan(None, NOW), "none");
        assert_eq!(
            summary_plan(
                Some(&session(None, Some("basic"), entitlements("basic", None))),
                NOW
            ),
            "none"
        );
        for plan in ["beta_free", "basic", "unlimited", "lifetime", "trial"] {
            assert_eq!(
                summary_plan(
                    Some(&session(Some("token"), None, entitlements(plan, None))),
                    NOW
                ),
                plan
            );
        }
        assert_eq!(
            summary_plan(
                Some(&session(
                    Some("token"),
                    None,
                    entitlements("basic", Some(NOW + 60))
                )),
                NOW
            ),
            "trial"
        );
        assert_eq!(
            summary_plan(
                Some(&session(
                    Some("token"),
                    None,
                    entitlements("basic", Some(NOW - 60))
                )),
                NOW
            ),
            "basic"
        );
        assert_eq!(
            summary_plan(
                Some(&session(Some("token"), None, entitlements("pro", None))),
                NOW
            ),
            "none"
        );
        assert_eq!(
            summary_plan(Some(&session(Some("token"), Some("lifetime"), None)), NOW),
            "lifetime"
        );
    }

    #[test]
    fn the_queue_gate_drops_or_strips_by_consent_version() {
        let directory = TestDirectory::new("v2-record-gate");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let now = unix_time_seconds().unwrap();
        // Unset and off record nothing, also no v2 event.
        for event in golden_events(now) {
            assert!(!record_if_shared(&manager, Sharing::Nothing, event).unwrap());
        }
        assert!(manager.pending().unwrap().is_empty());

        // Version 1 drops the v2 events and strips the v2 facts.
        for event in golden_events(now) {
            record_if_shared(&manager, Sharing::V1, event).unwrap();
        }
        let pending = manager.pending().unwrap();
        assert_eq!(
            pending,
            vec![
                AnalyticsEvent::new("account_connected", now),
                AnalyticsEvent::new("trial_prompted", now)
            ]
        );
        let queue = manager.queue.lock().unwrap().clone();
        assert!(queue.recorded_v2.is_empty());

        // Version 2 keeps them. The v2 events have their own list and keys.
        let directory = TestDirectory::new("v2-record-gate-on");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        for event in golden_events(now) {
            assert!(record_if_shared(&manager, Sharing::V2, event).unwrap());
        }
        let queue = manager.queue.lock().unwrap().clone();
        assert_eq!(queue.pending.len(), 2);
        assert_eq!(queue.pending[0].agent_kind.as_deref(), Some("glm"));
        assert_eq!(queue.pending_v2.len(), 5);
        assert!(queue.recorded.iter().all(|key| !key.contains(':')));
        assert_eq!(queue.recorded_v2.len(), 5);
        // A move to version 1 removes the v2 data from the queue.
        manager.drop_v2_data().unwrap();
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        let queue = reopened.queue.lock().unwrap().clone();
        assert!(queue.pending_v2.is_empty());
        assert!(queue
            .pending
            .iter()
            .all(|event| event.v2_fields().is_empty()));
    }

    #[test]
    fn v2_keys_dedupe_and_leave_the_queue_after_40_days() {
        let mut queue = AnalyticsQueue::default();
        let day = 20_000;
        let at = day * DAY_SECONDS + 10;
        assert!(queue
            .record(proxy_failed_event(PROXY_START, false, at))
            .unwrap());
        assert!(!queue
            .record(proxy_failed_event(PROXY_EXIT, true, at + 60))
            .unwrap());
        assert!(queue
            .record(proxy_failed_event(PROXY_EXIT, true, at + DAY_SECONDS))
            .unwrap());
        let updated = AnalyticsEvent {
            app_version: Some("1.1.0".into()),
            ..app_updated_event("1.0.1", at)
        };
        assert_eq!(updated.key(), "app_updated:1.1.0");
        assert!(queue.record(updated.clone()).unwrap());
        assert!(!queue.record(updated).unwrap());
        assert_eq!(
            queue.recorded_v2.keys().cloned().collect::<Vec<_>>(),
            vec![
                "app_updated:1.1.0".to_string(),
                format!("proxy_failed:{day}"),
                format!("proxy_failed:{}", day + 1),
            ]
        );
        // The keys of the first day are older than 40 days now.
        let later = at + 40 * DAY_SECONDS + DAY_SECONDS / 2;
        assert!(queue
            .record(update_failed_event(UPDATE_INSTALL, later))
            .unwrap());
        assert_eq!(
            queue.recorded_v2.keys().cloned().collect::<Vec<_>>(),
            vec![
                format!("proxy_failed:{}", day + 1),
                format!("update_failed:{}", later / DAY_SECONDS),
            ]
        );
        // Only the event of the new key waits: the others are past 30 days.
        assert_eq!(queue.pending_v2.len(), 1);
    }

    #[test]
    fn a_damaged_v2_part_of_the_queue_never_stops_the_start() {
        let directory = TestDirectory::new("v2-damaged-queue");
        let now = unix_time_seconds().unwrap();
        let file = serde_json::json!({
            "pending": [{"event": "account_connected", "at": now, "agent_kind": "claude"}],
            "recorded": ["account_connected"],
            "pending_v2": [
                {"event": "proxy_failed", "at": now, "category": "exit"},
                {"event": "update_failed", "at": now, "category": "install"}
            ],
            "recorded_v2": {
                "not_an_event:1": now,
                "update_failed:x": now,
                (format!("update_failed:{}", now / DAY_SECONDS)): now
            }
        });
        fs::write(directory.0.join(QUEUE_FILE_NAME), file.to_string()).unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let queue = manager.queue.lock().unwrap().clone();
        assert_eq!(
            queue.pending,
            vec![AnalyticsEvent::new("account_connected", now)]
        );
        assert_eq!(queue.pending_v2.len(), 1);
        assert_eq!(queue.pending_v2[0].event, UPDATE_FAILED);
        assert_eq!(queue.recorded_v2.len(), 1);
    }

    #[test]
    fn a_wrong_type_in_the_v2_part_of_the_queue_never_stops_the_start() {
        let directory = TestDirectory::new("v2-type-errors");
        let now = unix_time_seconds().unwrap();
        let day = now / DAY_SECONDS;
        let file = serde_json::json!({
            "pending": [
                {"event": "account_connected", "at": now, "agent_kind": 5, "tokens_saved": -5},
                {"event": "trial_prompted", "at": now, "surface": "pitch"}
            ],
            "recorded": ["account_connected", "trial_prompted"],
            "pending_v2": [
                {"event": "update_failed", "at": now, "category": "install", "tokens_saved": -5},
                {"event": "update_failed", "at": now, "category": "install", "usd_saved": "1.5"},
                {"event": "proxy_failed", "at": now, "category": "exit", "recovered": true},
                "text",
                5
            ],
            "recorded_v2": {
                "update_failed:20000": "x",
                (format!("update_failed:{day}")): -3,
                "crash_detected:20001": 1.5,
                (format!("proxy_failed:{day}")): now
            }
        });
        fs::write(directory.0.join(QUEUE_FILE_NAME), file.to_string()).unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let queue = manager.queue.lock().unwrap().clone();
        // The v1 events stay. The wrong v2 fact leaves its event.
        assert_eq!(
            queue.pending,
            vec![
                AnalyticsEvent::new("account_connected", now),
                AnalyticsEvent {
                    surface: Some("pitch".into()),
                    ..AnalyticsEvent::new("trial_prompted", now)
                }
            ]
        );
        assert_eq!(queue.pending_v2.len(), 1);
        assert_eq!(queue.pending_v2[0].event, PROXY_FAILED);
        assert_eq!(
            queue.recorded_v2.keys().cloned().collect::<Vec<_>>(),
            vec![format!("proxy_failed:{day}")]
        );

        // A part that is not the right shape gives an empty v2 part.
        let file = serde_json::json!({
            "pending": [{"event": "installed", "at": now}],
            "recorded": ["installed"],
            "pending_v2": {"event": "proxy_failed"},
            "recorded_v2": ["proxy_failed:1"]
        });
        fs::write(directory.0.join(QUEUE_FILE_NAME), file.to_string()).unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let queue = manager.queue.lock().unwrap().clone();
        assert_eq!(queue.pending, vec![AnalyticsEvent::new("installed", now)]);
        assert!(queue.pending_v2.is_empty());
        assert!(queue.recorded_v2.is_empty());
    }

    #[test]
    fn a_wrong_type_in_the_v1_part_of_the_queue_still_fails_the_open() {
        let directory = TestDirectory::new("v1-type-errors");
        let now = unix_time_seconds().unwrap();
        for pending in [
            serde_json::json!([{"event": "installed", "at": "x"}]),
            serde_json::json!([{"event": "installed", "at": now, "category": 5}]),
            serde_json::json!({"event": "installed"}),
        ] {
            let file = serde_json::json!({"pending": pending, "recorded": ["installed"]});
            fs::write(directory.0.join(QUEUE_FILE_NAME), file.to_string()).unwrap();
            let error = AnalyticsManager::open(&directory.0).expect_err("v1 part stays strict");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{pending}");
        }
    }

    /// A local database with one turn of an agent session per row:
    /// `(session, agent kind, UTC day, tokens saved, dollars saved)`. A
    /// session starts on the day of its first row.
    fn usage_storage(turns: &[(&str, &str, u64, i64, f64)]) -> Storage {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        crate::storage::run_migrations(&connection).unwrap();
        for (index, (session, kind, day, tokens_saved, dollars)) in turns.iter().enumerate() {
            let time = utc_day_start(*day).replace("T00:00:00Z", "T12:00:00Z");
            connection
                .execute(
                    "INSERT OR IGNORE INTO agent_sessions
                        (id, agent_kind, lane, mode, cwd, status, created_at,
                         last_active_at)
                     VALUES (?1, ?2, ?2, 'terminal_embedded', '/tmp', 'completed',
                             ?3, ?3)",
                    rusqlite::params![session, kind, time],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO agent_session_turns
                        (session_id, turn_index, input_tokens, output_tokens,
                         cache_read_tokens, cache_creation_tokens, cost_usd,
                         savings_tokens, savings_usd, pricing_source,
                         crux_tokens_saved, savings_source, has_message_snapshot,
                         created_at, occurred_local_date, usd_method)
                     VALUES (?1, ?2, 20, 10, 3, 0, NULL, ?3, ?4, 'model_table', 0,
                             'measured', 0, ?5, ?6, 'input_once')",
                    rusqlite::params![session, index, tokens_saved, dollars, time, &time[..10]],
                )
                .unwrap();
        }
        Storage::from_test_connection(connection, "2026-08-20T00:00:00Z")
    }

    fn consenting_managers(
        name: &str,
        consent_version: u8,
    ) -> (TestDirectory, AnalyticsManager, PreferencesManager) {
        let directory = TestDirectory::new(name);
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let preferences = PreferencesManager::open(&directory.0).unwrap();
        preferences
            .set_analytics_consent(AnalyticsConsent::On, Some(consent_version))
            .unwrap();
        (directory, manager, preferences)
    }

    #[test]
    fn the_usage_summaries_follow_the_consent_and_match_the_local_meter() {
        let today = today();
        let storage = usage_storage(&[
            ("a", "codex", today - 3, 12_345, 0.4567),
            ("a", "codex", today - 2, 3_000, 0.25),
            ("b", "zai", today - 2, 99_999, 1.25),
            ("c", "claude", today - 1, 1_234_567, 23.45),
        ]);

        // No consent and version 1 record nothing.
        let directory = TestDirectory::new("usage-no-consent");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let preferences = PreferencesManager::open(&directory.0).unwrap();
        assert!(!record_usage_summaries(&manager, &preferences, &storage, "1.1.0", NOW).unwrap());
        let (_v1_directory, v1_manager, v1_preferences) = consenting_managers("usage-v1", 1);
        assert!(
            !record_usage_summaries(&v1_manager, &v1_preferences, &storage, "1.1.0", NOW).unwrap()
        );
        assert!(manager.pending().unwrap().is_empty());
        assert!(v1_manager.pending().unwrap().is_empty());

        // Version 2 without a stored day records nothing: the day is not set.
        let (_directory, manager, preferences) = consenting_managers("usage-v2", 2);
        assert!(!record_usage_summaries(&manager, &preferences, &storage, "1.1.0", NOW).unwrap());
        // Version 2 from 3 days ago covers the 3 closed days since.
        manager
            .apply_sharing(Sharing::Nothing, Sharing::V2, NOW - 3 * DAY_SECONDS)
            .unwrap();
        assert!(record_usage_summaries(&manager, &preferences, &storage, "1.1.0", NOW).unwrap());
        let events = manager.pending().unwrap();
        assert_eq!(events.len(), 3);
        let body = |event: &AnalyticsEvent| serde_json::to_value(event).unwrap();
        assert_eq!(
            body(&events[0]),
            serde_json::json!({
                "event": "usage_summarized", "at": NOW, "app_version": "1.1.0",
                "day": today - 3, "agents_used": ["codex"], "sessions_bucket": "1",
                "tokens_saved": 12_000, "usd_saved": 0.46, "crux_used": false,
                "routing_used": false, "plan": "none"
            })
        );
        assert_eq!(
            body(&events[1]),
            serde_json::json!({
                "event": "usage_summarized", "at": NOW, "app_version": "1.1.0",
                "day": today - 2, "agents_used": ["codex", "glm"],
                "sessions_bucket": "2-5", "tokens_saved": 100_000, "usd_saved": 1.5,
                "crux_used": false, "routing_used": false, "plan": "none"
            })
        );
        assert_eq!(
            body(&events[2]),
            serde_json::json!({
                "event": "usage_summarized", "at": NOW, "app_version": "1.1.0",
                "day": today - 1, "agents_used": ["claude_code"],
                "sessions_bucket": "1", "tokens_saved": 1_200_000, "usd_saved": 23,
                "crux_used": false, "routing_used": false, "plan": "none"
            })
        );

        // The summed dollars and tokens match the local meter, within the
        // rounding of 2 significant digits (at most 5 % of each day).
        let meter = storage.total_savings().unwrap();
        let usd: f64 = events.iter().filter_map(|event| event.usd_saved).sum();
        let tokens: u64 = events.iter().filter_map(|event| event.tokens_saved).sum();
        assert!((usd - meter.dollars_usd).abs() <= meter.dollars_usd * 0.05);
        assert!(
            (tokens as f64 - meter.tokens_saved as f64).abs() <= meter.tokens_saved as f64 * 0.05
        );

        // A second run does not record a day twice.
        record_usage_summaries(&manager, &preferences, &storage, "1.1.0", NOW).unwrap();
        assert_eq!(manager.pending().unwrap().len(), 3);

        // A consent that starts yesterday never reads an earlier day.
        let (_directory, late_manager, late_preferences) = consenting_managers("usage-late", 2);
        late_manager
            .apply_sharing(Sharing::Nothing, Sharing::V2, NOW - DAY_SECONDS)
            .unwrap();
        record_usage_summaries(&late_manager, &late_preferences, &storage, "1.1.0", NOW).unwrap();
        let days = late_manager
            .pending()
            .unwrap()
            .iter()
            .map(|event| event.day)
            .collect::<Vec<_>>();
        assert_eq!(days, vec![Some(today - 1)]);
    }

    #[test]
    fn the_start_records_the_update_and_the_crash_under_version_2_only() {
        let history = [
            boot_entry(
                "1.1.0",
                "2026-10-22T10:00:00.000Z",
                "2026-10-22T10:00:00.000Z",
                ShutdownKind::Running,
                None,
            ),
            boot_entry(
                "1.0.1",
                "2026-10-22T09:00:00.000Z",
                "2026-10-22T09:30:00.000Z",
                ShutdownKind::Unclean,
                Some(1536 * MIB),
            ),
        ];

        let (_directory, manager, preferences) = consenting_managers("start-v2", 2);
        // The first start has no stored version, so it records no update.
        assert!(!record_start_events(
            &manager,
            &preferences,
            "1.0.1",
            &[],
            NOW
        ));
        assert_eq!(manager.consent_v2_day().unwrap(), Some(today()));
        assert!(record_start_events(
            &manager,
            &preferences,
            "1.1.0",
            &history,
            NOW
        ));
        let events = manager.pending().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, APP_UPDATED);
        assert_eq!(events[0].from_version.as_deref(), Some("1.0.1"));
        assert_eq!(events[0].app_version.as_deref(), Some("1.1.0"));
        assert_eq!(events[1].event, CRASH_DETECTED);
        assert_eq!(events[1].prev_app_version.as_deref(), Some("1.0.1"));
        assert_eq!(events[1].uptime_bucket.as_deref(), Some("10m_1h"));
        assert_eq!(events[1].rss_bucket.as_deref(), Some("1gb_2gb"));
        // The same start again adds nothing.
        record_start_events(&manager, &preferences, "1.1.0", &history, NOW);
        assert_eq!(manager.pending().unwrap().len(), 2);

        // Version 1 and no answer store the version, and record nothing.
        let (_v1_directory, v1_manager, v1_preferences) = consenting_managers("start-v1", 1);
        let directory = TestDirectory::new("start-unset");
        let unset_manager = AnalyticsManager::open(&directory.0).unwrap();
        let unset_preferences = PreferencesManager::open(&directory.0).unwrap();
        for (manager, preferences) in [
            (&v1_manager, &v1_preferences),
            (&unset_manager, &unset_preferences),
        ] {
            record_start_events(manager, preferences, "1.0.1", &[], NOW);
            assert!(!record_start_events(
                manager,
                preferences,
                "1.1.0",
                &history,
                NOW
            ));
            assert!(manager.pending().unwrap().is_empty());
            assert_eq!(manager.consent_v2_day().unwrap(), None);
            assert_eq!(
                manager.state().unwrap().last_app_version.as_deref(),
                Some("1.1.0")
            );
        }
    }

    #[test]
    fn the_state_keeps_a_valid_locale_and_the_last_app_version() {
        let directory = TestDirectory::new("v2-state");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert!(manager.set_locale("pt-PT").is_err());
        assert!(manager.set_locale("").is_err());
        for locale in LOCALES {
            manager.set_locale(locale).unwrap();
        }
        manager.set_locale("pt-BR").unwrap();
        assert_eq!(manager.replace_last_app_version("1.0.1").unwrap(), None);
        assert_eq!(manager.replace_last_app_version("1.0.1").unwrap(), None);
        assert_eq!(
            manager
                .replace_last_app_version("1.1.0")
                .unwrap()
                .as_deref(),
            Some("1.0.1")
        );
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        let state = reopened.state().unwrap();
        assert_eq!(state.locale.as_deref(), Some("pt-BR"));
        assert_eq!(state.last_app_version.as_deref(), Some("1.1.0"));

        fs::write(
            directory.0.join(STATE_FILE_NAME),
            r#"{"locale":"xx","last_app_version":"1.0 beta","consent_v2_day":3}"#,
        )
        .unwrap();
        let state = AnalyticsManager::open(&directory.0)
            .unwrap()
            .state()
            .unwrap();
        assert_eq!(
            state,
            AnalyticsState {
                consent_v2_day: Some(3),
                ..AnalyticsState::default()
            }
        );
        fs::write(directory.0.join(STATE_FILE_NAME), "{damaged").unwrap();
        assert_eq!(
            AnalyticsManager::open(&directory.0)
                .unwrap()
                .state()
                .unwrap(),
            AnalyticsState::default()
        );
    }

    #[test]
    fn a_flush_drops_a_summary_that_the_server_no_longer_accepts() {
        let directory = TestDirectory::new("v2-expire");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let at = 20_000 * DAY_SECONDS + 100;
        let summary = usage_event(20_000 - 1, &usage(&["codex"], 0, 0.0), "none", at).unwrap();
        manager.record_event(summary).unwrap();
        manager.expire_pending(at + 6 * DAY_SECONDS).unwrap();
        assert_eq!(manager.pending().unwrap().len(), 1);
        manager.expire_pending(at + 7 * DAY_SECONDS).unwrap();
        assert!(manager.pending().unwrap().is_empty());
        // The key stays, so the day never gets a second summary.
        assert!(manager.has_v2_key(&usage_key(20_000 - 1)).unwrap());
    }

    /// A local server that answers each request with the next status. It
    /// returns the request bodies.
    fn capture_server(
        statuses: Vec<&'static str>,
    ) -> (String, std::thread::JoinHandle<Vec<serde_json::Value>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            statuses
                .into_iter()
                .map(|status| {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut data = Vec::new();
                    let mut buffer = [0; 8192];
                    let body = loop {
                        let size = stream.read(&mut buffer).unwrap_or(0);
                        data.extend_from_slice(&buffer[..size]);
                        let text = String::from_utf8_lossy(&data).to_string();
                        let Some(end) = text.find("\r\n\r\n") else {
                            assert!(size > 0, "the request ended early");
                            continue;
                        };
                        let length = text[..end]
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + length || size == 0 {
                            break text[end + 4..].to_string();
                        }
                    };
                    stream
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .unwrap();
                    serde_json::from_str(&body).unwrap()
                })
                .collect()
        });
        (url, server)
    }

    fn queue_golden_events(manager: &AnalyticsManager, now: u64) {
        manager.set_locale("pt-BR").unwrap();
        record_if_shared(manager, Sharing::V2, AnalyticsEvent::new(FIRST_OPEN, now)).unwrap();
        for event in golden_events(now) {
            record_if_shared(manager, Sharing::V2, event).unwrap();
        }
    }

    fn event_names(batch: &serde_json::Value) -> Vec<String> {
        batch["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["event"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_version_1_flush_sends_no_v2_data() {
        let (url, server) = capture_server(vec!["200 OK"]);
        let directory = TestDirectory::new("v2-flush-v1");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        queue_golden_events(&manager, unix_time_seconds().unwrap());
        // The user answered "no" to the notice after the events were queued.
        flush_pending_guarded(&manager, &url, "1.1.0", &|| Ok(Sharing::V1))
            .await
            .unwrap();
        let bodies = server.join().unwrap();
        let body = &bodies[0];
        assert!(body.get("consent_version").is_none(), "{body}");
        assert!(body.get("locale").is_none(), "{body}");
        assert_eq!(
            event_names(body),
            vec!["first_open", "account_connected", "trial_prompted"]
        );
        for event in body["events"].as_array().unwrap() {
            let mut fields = event
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            fields.sort();
            assert_eq!(fields, vec!["at", "event"], "{event}");
        }
        assert!(manager.pending().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_version_2_flush_sends_the_v2_events_fields_and_locale() {
        let (url, server) = capture_server(vec!["200 OK"]);
        let directory = TestDirectory::new("v2-flush-v2");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        let now = unix_time_seconds().unwrap();
        queue_golden_events(&manager, now);
        flush_pending_guarded(&manager, &url, "1.1.0", &|| Ok(Sharing::V2))
            .await
            .unwrap();
        let body = &server.join().unwrap()[0];
        assert_eq!(body["consent_version"], 2);
        assert_eq!(body["locale"], "pt-BR");
        assert_eq!(
            event_names(body),
            vec![
                "first_open",
                "account_connected",
                "trial_prompted",
                "usage_summarized",
                "crash_detected",
                "proxy_failed",
                "app_updated",
                "update_failed"
            ]
        );
        assert_eq!(body["events"][1]["agent_kind"], "glm");
        assert_eq!(body["events"][3]["tokens_saved"], 120_000);
        assert_eq!(body["events"][3]["day"], now / DAY_SECONDS - 1);
        assert!(manager.pending().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_old_server_gets_the_v1_part_again_and_the_v2_events_wait() {
        let (url, server) = capture_server(vec!["400 Bad Request", "200 OK"]);
        let directory = TestDirectory::new("v2-flush-old-server");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        queue_golden_events(&manager, unix_time_seconds().unwrap());
        flush_pending_guarded(&manager, &url, "1.1.0", &|| Ok(Sharing::V2))
            .await
            .unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies[0]["consent_version"], 2);
        assert!(bodies[1].get("consent_version").is_none());
        assert!(bodies[1].get("locale").is_none());
        assert_eq!(
            event_names(&bodies[1]),
            vec!["first_open", "account_connected", "trial_prompted"]
        );
        assert!(bodies[1]["events"][1].get("agent_kind").is_none());
        let pending = manager.pending().unwrap();
        assert_eq!(pending.len(), 5);
        assert!(pending.iter().all(|event| is_v2_event(&event.event)));
    }

    #[tokio::test]
    async fn nothing_leaves_without_consent() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let directory = TestDirectory::new("v2-flush-nothing");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        queue_golden_events(&manager, unix_time_seconds().unwrap());
        flush_pending_guarded(&manager, &url, "1.1.0", &|| Ok(Sharing::Nothing))
            .await
            .unwrap();
        assert!(manager.pending().unwrap().is_empty());
        assert!(AnalyticsManager::open(&directory.0)
            .unwrap()
            .pending()
            .unwrap()
            .is_empty());
        assert_eq!(
            listener.accept().map(|_| ()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}
