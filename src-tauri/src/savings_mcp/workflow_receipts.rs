//! Reads Claude Code transcripts for JEV workflow receipts (ADR 0014).
//!
//! Claude Code writes the agents of a Workflow run into
//! `<projects>/<project>/<session>/subagents/workflows/<run>/`. Each agent has
//! `agent-<id>.meta.json`, whose `description` is the `agent()` label, and
//! `agent-<id>.jsonl`, with one line for each streamed assistant block. The
//! lines of one message repeat its usage with growing values, so the reader
//! keeps the last usage of each message id.
//!
//! The reader only reads local files. It never fails a tool call: a missing
//! or unreadable file gives no measurement.
//!
//! The reader prices each message at its own prompt size, so a long-context
//! tier (Claude Haiku 5.5 above 100K prompt tokens) applies to the right
//! requests (ADR 0016).

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const LABEL_PREFIX: &str = "jev:";
const RETRY_SEGMENT: &str = "retry";
const SHORT_ID_CHARS: usize = 8;
/// The coordinator transcript can hold many megabytes. Its last assistant
/// message is near the end.
const COORDINATOR_TAIL_BYTES: u64 = 1024 * 1024;

/// The first part of a task id, as the label uses it.
pub(super) fn short_id(task_id: &str) -> &str {
    task_id
        .char_indices()
        .nth(SHORT_ID_CHARS)
        .map_or(task_id, |(index, _)| &task_id[..index])
}

/// The `agent()` label of the first attempt of a route.
pub(super) fn route_label(task_id: &str, key: &str) -> String {
    format!("{LABEL_PREFIX}{}:{key}", short_id(task_id))
}

/// The `agent()` label of the retry of a route.
pub(super) fn retry_label(task_id: &str, key: &str) -> String {
    format!("{}:{RETRY_SEGMENT}", route_label(task_id, key))
}

/// The route part of an agent label: `jev:<id8>:<key>[:retry][:<suffix>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LabelRef {
    pub short_id: String,
    pub key: String,
    pub retry: bool,
}

pub(super) fn parse_label(label: &str) -> Option<LabelRef> {
    let rest = label.trim().strip_prefix(LABEL_PREFIX)?;
    let mut parts = rest.split(':');
    let short_id = parts.next().filter(|value| !value.is_empty())?;
    let key = parts.next().filter(|value| !value.is_empty())?;
    Some(LabelRef {
        short_id: short_id.to_string(),
        key: key.to_string(),
        retry: parts.next() == Some(RETRY_SEGMENT),
    })
}

/// The tokens of one agent on one model.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ModelUsage {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    /// The list price of the messages, each at its own prompt size.
    pub actual_usd: f64,
}

impl ModelUsage {
    fn add(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_5m_tokens = self
            .cache_write_5m_tokens
            .saturating_add(other.cache_write_5m_tokens);
        self.cache_write_1h_tokens = self
            .cache_write_1h_tokens
            .saturating_add(other.cache_write_1h_tokens);
        self.actual_usd += other.actual_usd;
    }

    /// The list price of one message on its model's card.
    fn priced(mut self) -> Self {
        let count = |value: i64| u64::try_from(value).unwrap_or_default();
        let usage = crate::pricing::TokenUsage {
            input: count(self.input_tokens),
            cache_write_5m: count(self.cache_write_5m_tokens),
            cache_write_1h: count(self.cache_write_1h_tokens),
            cache_read: count(self.cache_read_tokens),
            output: count(self.output_tokens),
        };
        // The observed id is exact, so the plain lookup prices it. A Claude
        // Code pane can run a GLM or Grok model.
        self.actual_usd = crate::pricing::resolve(Some("claude"), Some(&self.model))
            .card
            .cost_usd(&usage, Some(usage.prompt_tokens()));
        self
    }
}

/// One JEV-labelled workflow agent and its measured use.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MeasuredAgent {
    pub agent_id: String,
    pub workflow_run_id: String,
    pub label: String,
    pub route: LabelRef,
    pub effort: Option<String>,
    /// One entry for each model, ordered by model.
    pub usage: Vec<ModelUsage>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`. The MCP server
/// inherits the environment of the Claude Code process that started it.
pub(super) fn claude_projects_root() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(windows)]
            let home: Option<OsString> = std::env::var_os("USERPROFILE");
            #[cfg(not(windows))]
            let home: Option<OsString> = std::env::var_os("HOME");
            home.map(|home| PathBuf::from(home).join(".claude"))
        })
        .map(|directory| directory.join("projects"))
}

fn valid_session_id(cli_session_id: &str) -> bool {
    !cli_session_id.is_empty()
        && cli_session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn project_directories(projects_root: &Path) -> Vec<PathBuf> {
    fs::read_dir(projects_root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

/// Reads every JEV-labelled workflow agent of one Claude Code session.
pub(super) fn measure_session_agents(
    projects_root: &Path,
    cli_session_id: &str,
) -> Vec<MeasuredAgent> {
    if !valid_session_id(cli_session_id) {
        return Vec::new();
    }
    let mut agents = Vec::new();
    for project in project_directories(projects_root) {
        let runs = project
            .join(cli_session_id)
            .join("subagents")
            .join("workflows");
        for run in fs::read_dir(runs)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
        {
            agents.extend(measure_run(&run));
        }
    }
    agents.sort_by(|left, right| {
        (&left.workflow_run_id, &left.agent_id).cmp(&(&right.workflow_run_id, &right.agent_id))
    });
    agents
}

fn measure_run(run: &Path) -> Vec<MeasuredAgent> {
    let workflow_run_id = run
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let mut agents = Vec::new();
    for meta_path in fs::read_dir(run)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
    {
        let Some(name) = meta_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(agent_id) = name
            .strip_prefix("agent-")
            .and_then(|rest| rest.strip_suffix(".meta.json"))
        else {
            continue;
        };
        let Some(label) = fs::read(&meta_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|meta| meta.get("description")?.as_str().map(str::to_owned))
        else {
            continue;
        };
        let Some(route) = parse_label(&label) else {
            continue;
        };
        let transcript = run.join(format!("agent-{agent_id}.jsonl"));
        let measured = read_agent_transcript(&transcript);
        agents.push(MeasuredAgent {
            agent_id: agent_id.to_string(),
            workflow_run_id: workflow_run_id.clone(),
            label,
            route,
            effort: measured.effort,
            usage: measured.usage,
            started_at: measured.started_at,
            ended_at: measured.ended_at,
        });
    }
    agents
}

#[derive(Default)]
struct TranscriptMeasure {
    effort: Option<String>,
    usage: Vec<ModelUsage>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
}

fn usage_number(usage: &Value, key: &str) -> i64 {
    usage
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .max(0)
}

fn message_usage(model: &str, usage: &Value) -> ModelUsage {
    let cache_creation = usage_number(usage, "cache_creation_input_tokens");
    let split = usage.get("cache_creation");
    let (cache_write_5m_tokens, cache_write_1h_tokens) = match split {
        Some(split) if split.is_object() => (
            usage_number(split, "ephemeral_5m_input_tokens"),
            usage_number(split, "ephemeral_1h_input_tokens"),
        ),
        // Without the split, Claude Code wrote 5-minute cache entries.
        _ => (cache_creation, 0),
    };
    ModelUsage {
        model: model.to_string(),
        input_tokens: usage_number(usage, "input_tokens"),
        output_tokens: usage_number(usage, "output_tokens"),
        cache_read_tokens: usage_number(usage, "cache_read_input_tokens"),
        cache_write_5m_tokens,
        cache_write_1h_tokens,
        // `priced` sets the price once the last usage of the message is known.
        actual_usd: 0.0,
    }
}

/// A real model id. Claude Code writes `<synthetic>` for local messages.
fn real_model(model: &str) -> bool {
    !model.is_empty() && !model.starts_with('<')
}

fn read_agent_transcript(path: &Path) -> TranscriptMeasure {
    let mut measure = TranscriptMeasure::default();
    let Ok(file) = File::open(path) else {
        return measure;
    };
    // The last usage of each message id, in the order of first sight.
    let mut messages: Vec<(String, ModelUsage)> = Vec::new();
    let mut message_index = BTreeMap::<String, usize>::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(timestamp) = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|time| time.with_timezone(&Utc))
        {
            measure.started_at = Some(measure.started_at.map_or(timestamp, |at| at.min(timestamp)));
            measure.ended_at = Some(measure.ended_at.map_or(timestamp, |at| at.max(timestamp)));
        }
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(effort) = value.get("effort").and_then(Value::as_str) {
            measure.effort = Some(effort.to_string());
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(model) = message
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| real_model(model))
        else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| value.get("uuid").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let usage = message_usage(model, usage);
        match message_index.get(&id) {
            Some(&index) => messages[index].1 = usage,
            None => {
                message_index.insert(id.clone(), messages.len());
                messages.push((id, usage));
            }
        }
    }
    let mut per_model = BTreeMap::<String, ModelUsage>::new();
    for (_, usage) in messages {
        let usage = usage.priced();
        per_model
            .entry(usage.model.clone())
            .or_insert_with(|| ModelUsage {
                model: usage.model.clone(),
                ..ModelUsage::default()
            })
            .add(&usage);
    }
    measure.usage = per_model.into_values().collect();
    measure
}

/// The model and the effort of the last assistant message in the transcript
/// of the coordinator session.
pub(super) fn observe_coordinator(
    projects_root: &Path,
    cli_session_id: &str,
) -> Option<(String, Option<String>)> {
    if !valid_session_id(cli_session_id) {
        return None;
    }
    let name = format!("{cli_session_id}.jsonl");
    let transcript = project_directories(projects_root)
        .into_iter()
        .map(|project| project.join(&name))
        .find(|path| path.is_file())?;
    let mut file = File::open(transcript).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(COORDINATOR_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let mut lines = text.lines().collect::<Vec<_>>();
    if start > 0 && !lines.is_empty() {
        // The first line of the tail can be a part of a line.
        lines.remove(0);
    }
    lines.into_iter().rev().find_map(|line| {
        let value = serde_json::from_str::<Value>(line).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        let model = value
            .pointer("/message/model")
            .and_then(Value::as_str)
            .filter(|model| real_model(model))?;
        let effort = value
            .get("effort")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Some((model.to_string(), effort))
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use serde_json::json;

    /// Writes one workflow agent into a fake Claude projects folder.
    pub(crate) fn write_agent(
        projects_root: &Path,
        cli_session_id: &str,
        run: &str,
        agent_id: &str,
        label: &str,
        lines: &[Value],
    ) {
        let directory = projects_root
            .join("-tmp-project")
            .join(cli_session_id)
            .join("subagents")
            .join("workflows")
            .join(run);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("agent-{agent_id}.meta.json")),
            json!({"agentType": "workflow-subagent", "description": label}).to_string(),
        )
        .unwrap();
        let body = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(directory.join(format!("agent-{agent_id}.jsonl")), body).unwrap();
    }

    pub(crate) fn assistant_line(
        message_id: &str,
        model: &str,
        output_tokens: i64,
        timestamp: &str,
    ) -> Value {
        json!({
            "type": "assistant",
            "timestamp": timestamp,
            "effort": "high",
            "message": {
                "id": message_id,
                "model": model,
                "usage": {
                    "input_tokens": 2,
                    "output_tokens": output_tokens,
                    "cache_read_input_tokens": 1000,
                    "cache_creation_input_tokens": 300,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 200,
                        "ephemeral_1h_input_tokens": 100
                    }
                }
            }
        })
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "halv-workflow-receipts-{name}-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn labels_round_trip_and_mark_retries() {
        let task_id = "9f2a61c0-1234-4abc-8def-000000000000";
        assert_eq!(route_label(task_id, "scout"), "jev:9f2a61c0:scout");
        assert_eq!(retry_label(task_id, "scout"), "jev:9f2a61c0:scout:retry");
        assert_eq!(
            parse_label("jev:9f2a61c0:scout:src/a.rs"),
            Some(LabelRef {
                short_id: "9f2a61c0".to_string(),
                key: "scout".to_string(),
                retry: false,
            })
        );
        assert!(parse_label("jev:9f2a61c0:fix:retry:2").unwrap().retry);
        assert_eq!(parse_label("verify:scout"), None);
        assert_eq!(parse_label("jev:9f2a61c0"), None);
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn agents_keep_the_last_usage_of_each_message_and_split_models() {
        let root = temp_root("agents");
        write_agent(
            &root,
            "session-1",
            "wf_1",
            "a1",
            "jev:9f2a61c0:scout:1",
            &[
                json!({"type": "user", "timestamp": "2026-10-15T10:00:00Z"}),
                assistant_line("m1", "claude-sonnet-5-5", 8, "2026-10-15T10:00:05Z"),
                assistant_line("m1", "claude-sonnet-5-5", 40, "2026-10-15T10:00:06Z"),
                assistant_line("m2", "claude-sonnet-5-5", 10, "2026-10-15T10:01:00Z"),
                assistant_line("m3", "claude-opus-5-5", 5, "2026-10-15T10:02:00Z"),
                assistant_line("m4", "<synthetic>", 0, "2026-10-15T10:02:30Z"),
            ],
        );
        write_agent(
            &root,
            "session-1",
            "wf_1",
            "a2",
            "verify:not-routed",
            &[assistant_line(
                "m9",
                "claude-opus-5-5",
                99,
                "2026-10-15T10:00:00Z",
            )],
        );
        let agents = measure_session_agents(&root, "session-1");
        assert_eq!(agents.len(), 1);
        let agent = &agents[0];
        assert_eq!(agent.agent_id, "a1");
        assert_eq!(agent.workflow_run_id, "wf_1");
        assert_eq!(agent.route.key, "scout");
        assert_eq!(agent.effort.as_deref(), Some("high"));
        assert_eq!(agent.usage.len(), 2);
        let opus = &agent.usage[0];
        let sonnet = &agent.usage[1];
        assert_eq!(opus.model, "claude-opus-5-5");
        assert_eq!(opus.output_tokens, 5);
        assert_eq!(sonnet.model, "claude-sonnet-5-5");
        assert_eq!(sonnet.output_tokens, 50);
        assert_eq!(sonnet.input_tokens, 4);
        assert_eq!(sonnet.cache_read_tokens, 2000);
        assert_eq!(sonnet.cache_write_5m_tokens, 400);
        assert_eq!(sonnet.cache_write_1h_tokens, 200);
        assert_eq!(
            agent.ended_at.unwrap() - agent.started_at.unwrap(),
            chrono::Duration::seconds(150)
        );
        assert!(measure_session_agents(&root, "../session-1").is_empty());
        assert!(measure_session_agents(&root, "missing").is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_coordinator_model_comes_from_the_last_assistant_message() {
        let root = temp_root("coordinator");
        let project = root.join("-tmp-project");
        fs::create_dir_all(&project).unwrap();
        let lines = [
            assistant_line("m1", "claude-sonnet-5-5", 1, "2026-10-15T10:00:00Z"),
            {
                let mut line = assistant_line("m2", "claude-opus-5-5", 1, "2026-10-15T10:01:00Z");
                line["effort"] = json!("xhigh");
                line
            },
            json!({"type": "user", "timestamp": "2026-10-15T10:02:00Z"}),
            assistant_line("m3", "<synthetic>", 0, "2026-10-15T10:03:00Z"),
        ];
        fs::write(
            project.join("session-1.jsonl"),
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert_eq!(
            observe_coordinator(&root, "session-1"),
            Some(("claude-opus-5-5".to_string(), Some("xhigh".to_string())))
        );
        assert_eq!(observe_coordinator(&root, "session-2"), None);
        fs::remove_dir_all(root).unwrap();
    }
}
