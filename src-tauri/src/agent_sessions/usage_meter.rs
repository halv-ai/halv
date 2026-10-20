//! Reads the model and the token classes of an agent session from its own
//! transcript (ADR 0015).
//!
//! Claude Code writes one JSONL transcript per session, plus one file per
//! subagent and per workflow agent. Codex writes one rollout per session.
//! The meter reads only the bytes that came after the previous poll, so a
//! poll every few seconds stays cheap on long sessions.
//!
//! Rules:
//! - Claude Code writes one line per content block, and each line repeats
//!   the usage of the whole API response. The meter counts each response
//!   once, by `message.id`.
//! - `input` counts uncached input only. Codex reports cached input inside
//!   `input_tokens`, so the meter takes the cached and written tokens out.
//! - The first poll that finds the transcript reads the earlier history into
//!   the running total only. The window starts after that poll.

use super::{claude, codex};
use crate::pricing::{self, TokenUsage};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_READ_BYTES_PER_POLL: u64 = 32 * 1024 * 1024;
const ROLLOUT_SEARCH_DEPTH: usize = 4;

/// Token usage with its cost at list prices, for one window or one session.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PricedUsage {
    pub(crate) usage: TokenUsage,
    /// The cost of the prompt classes, each request at its own model rates.
    pub(crate) prompt_cost_usd: f64,
    pub(crate) latest_model: Option<String>,
    /// True when a request had no price card of its own.
    pub(crate) fallback_priced: bool,
}

impl PricedUsage {
    pub(crate) fn add_request(
        &mut self,
        agent_kind: &str,
        model: Option<&str>,
        usage: &TokenUsage,
    ) {
        if usage.is_empty() {
            return;
        }
        let price = pricing::resolve(Some(agent_kind), model);
        self.prompt_cost_usd += price
            .card
            .prompt_cost_usd(usage, Some(usage.prompt_tokens()));
        self.fallback_priced |= price.source == pricing::PricingSource::Fallback;
        self.usage.add(usage);
        if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
            self.latest_model = Some(model.to_string());
        }
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        self.usage.add(&other.usage);
        self.prompt_cost_usd += other.prompt_cost_usd;
        self.fallback_priced |= other.fallback_priced;
        if other.latest_model.is_some() {
            self.latest_model.clone_from(&other.latest_model);
        }
    }

    /// The effective prompt price in US dollars per million prompt tokens.
    pub(crate) fn prompt_rate_usd_per_million(&self) -> Option<f64> {
        let tokens = self.usage.prompt_tokens();
        (tokens > 0).then(|| self.prompt_cost_usd * 1_000_000.0 / tokens as f64)
    }
}

#[derive(Debug, Default)]
struct FileCursor {
    offset: u64,
    partial: String,
}

/// Returns the complete lines that came after the cursor. A file that got
/// shorter starts again from the top.
fn read_new_lines(path: &Path, cursor: &mut FileCursor) -> Vec<String> {
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return Vec::new();
    };
    if length < cursor.offset {
        *cursor = FileCursor::default();
    }
    if length == cursor.offset || file.seek(SeekFrom::Start(cursor.offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    let Ok(read) = file.take(MAX_READ_BYTES_PER_POLL).read_to_end(&mut bytes) else {
        return Vec::new();
    };
    cursor.offset += read as u64;
    let mut text = std::mem::take(&mut cursor.partial);
    text.push_str(&String::from_utf8_lossy(&bytes));
    let complete = text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if !complete {
        cursor.partial = lines.pop().unwrap_or_default();
    }
    lines
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

/// The token classes of one Claude API response.
pub(crate) fn claude_usage(usage: &Value) -> TokenUsage {
    let written = u64_field(usage, "cache_creation_input_tokens");
    let (cache_write_5m, cache_write_1h) = match usage.get("cache_creation") {
        Some(split) => (
            u64_field(split, "ephemeral_5m_input_tokens"),
            u64_field(split, "ephemeral_1h_input_tokens"),
        ),
        None => (written, 0),
    };
    TokenUsage {
        input: u64_field(usage, "input_tokens"),
        cache_write_5m,
        cache_write_1h,
        cache_read: u64_field(usage, "cache_read_input_tokens"),
        output: u64_field(usage, "output_tokens"),
    }
}

/// One assistant line of a Claude transcript: the response key, the model
/// and the usage.
fn claude_line(line: &str) -> Option<(String, Option<String>, TokenUsage)> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = value.get("message")?;
    let usage = claude_usage(message.get("usage")?);
    let key = message
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| value.get("requestId").and_then(Value::as_str))
        .or_else(|| value.get("uuid").and_then(Value::as_str))?
        .to_string();
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty() && !model.starts_with('<'))
        .map(str::to_string);
    Some((key, model, usage))
}

#[derive(Debug, Default)]
struct ClaudeTranscriptSource {
    main: Option<PathBuf>,
    cursors: HashMap<PathBuf, FileCursor>,
    seen: HashSet<String>,
}

impl ClaudeTranscriptSource {
    fn locate(&mut self, projects: &Path, cwd: &Path, session_id: &str) -> Option<PathBuf> {
        if let Some(main) = self.main.as_ref().filter(|path| path.is_file()) {
            return Some(main.clone());
        }
        let name = format!("{session_id}.jsonl");
        let preferred = claude::transcript_project_directory_in(projects, cwd).join(&name);
        let main = if preferred.is_file() {
            Some(preferred)
        } else {
            fs::read_dir(projects)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(&name))
                .find(|path| path.is_file())
        }?;
        self.main = Some(main.clone());
        Some(main)
    }

    fn files(main: &Path, session_id: &str) -> Vec<PathBuf> {
        let mut files = vec![main.to_path_buf()];
        let Some(subagents) = main
            .parent()
            .map(|directory| directory.join(session_id).join("subagents"))
        else {
            return files;
        };
        let jsonl =
            |path: &PathBuf| path.extension().and_then(|value| value.to_str()) == Some("jsonl");
        let entries = fs::read_dir(&subagents)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path());
        files.extend(entries.filter(jsonl));
        let workflows = fs::read_dir(subagents.join("workflows"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir());
        for workflow in workflows {
            files.extend(
                fs::read_dir(workflow)
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(jsonl),
            );
        }
        files
    }

    fn poll(
        &mut self,
        projects: &Path,
        cwd: &Path,
        session_id: &str,
        agent_kind: &str,
        out: &mut PricedUsage,
    ) -> bool {
        let Some(main) = self.locate(projects, cwd, session_id) else {
            return false;
        };
        for path in Self::files(&main, session_id) {
            let cursor = self.cursors.entry(path.clone()).or_default();
            for line in read_new_lines(&path, cursor) {
                let Some((key, model, usage)) = claude_line(&line) else {
                    continue;
                };
                if self.seen.insert(key) {
                    out.add_request(agent_kind, model.as_deref(), &usage);
                }
            }
        }
        true
    }
}

/// The token classes of one Codex `token_count` usage object.
pub(crate) fn codex_usage(usage: &Value) -> TokenUsage {
    let total_input = u64_field(usage, "input_tokens");
    let cache_read = u64_field(usage, "cached_input_tokens");
    let cache_write = u64_field(usage, "cache_write_input_tokens");
    TokenUsage {
        input: total_input.saturating_sub(cache_read.saturating_add(cache_write)),
        cache_write_5m: cache_write,
        cache_write_1h: 0,
        cache_read,
        output: u64_field(usage, "output_tokens"),
    }
}

#[derive(Debug, Default)]
struct CodexRolloutSource {
    path: Option<PathBuf>,
    cursor: FileCursor,
    model: Option<String>,
    last_total: TokenUsage,
}

fn find_rollout(directory: &Path, suffix: &str, depth: usize) -> Option<PathBuf> {
    let entries = fs::read_dir(directory).ok()?;
    let mut directories = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
        {
            return Some(path);
        }
    }
    // Rollouts live in year/month/day folders; the newest folder comes first.
    directories.sort();
    directories.reverse();
    (depth > 0)
        .then(|| {
            directories
                .iter()
                .find_map(|directory| find_rollout(directory, suffix, depth - 1))
        })
        .flatten()
}

impl CodexRolloutSource {
    fn poll(
        &mut self,
        sessions_root: &Path,
        session_id: &str,
        agent_kind: &str,
        out: &mut PricedUsage,
    ) -> bool {
        if self.path.as_ref().map_or(true, |path| !path.is_file()) {
            self.path = find_rollout(
                sessions_root,
                &format!("-{session_id}.jsonl"),
                ROLLOUT_SEARCH_DEPTH,
            );
        }
        let Some(path) = self.path.clone() else {
            return false;
        };
        for line in read_new_lines(&path, &mut self.cursor) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(payload) = value.get("payload") else {
                continue;
            };
            match value.get("type").and_then(Value::as_str) {
                Some("turn_context") => {
                    if let Some(model) = payload.get("model").and_then(Value::as_str) {
                        self.model = Some(model.to_string());
                    }
                }
                Some("event_msg")
                    if payload.get("type").and_then(Value::as_str) == Some("token_count") =>
                {
                    let Some(info) = payload.get("info") else {
                        continue;
                    };
                    let Some(total) = info.get("total_token_usage").map(codex_usage) else {
                        continue;
                    };
                    let delta = total.saturating_sub(&self.last_total);
                    self.last_total = total;
                    out.add_request(agent_kind, self.model.as_deref(), &delta);
                }
                _ => {}
            }
        }
        true
    }
}

#[derive(Debug)]
enum UsageSource {
    Claude(ClaudeTranscriptSource),
    Codex(CodexRolloutSource),
    None,
}

/// The usage meter of one agent session.
#[derive(Debug)]
pub(crate) struct SessionUsageMeter {
    agent_kind: &'static str,
    source: UsageSource,
    primed: bool,
    running: PricedUsage,
}

impl SessionUsageMeter {
    /// Claude, GLM and Grok sessions run Claude Code, so they have Claude
    /// transcripts. Codex sessions have rollouts. Other agents have no
    /// transcript reader yet.
    pub(crate) fn new(agent_kind: &'static str) -> Self {
        let source = match agent_kind {
            "claude" | "zai" | "grok" => UsageSource::Claude(ClaudeTranscriptSource::default()),
            "codex" => UsageSource::Codex(CodexRolloutSource::default()),
            _ => UsageSource::None,
        };
        Self {
            agent_kind,
            source,
            primed: false,
            running: PricedUsage::default(),
        }
    }

    pub(crate) fn running(&self) -> &PricedUsage {
        &self.running
    }

    /// Reads the usage that came after the previous poll.
    pub(crate) fn poll(&mut self, cli_session_id: Option<&str>, cwd: &Path) -> PricedUsage {
        let Some(session_id) = cli_session_id.filter(|id| is_safe_session_id(id)) else {
            return PricedUsage::default();
        };
        let mut window = PricedUsage::default();
        let found = match &mut self.source {
            UsageSource::Claude(source) => {
                claude::transcript_projects_directory().is_some_and(|projects| {
                    source.poll(&projects, cwd, session_id, self.agent_kind, &mut window)
                })
            }
            UsageSource::Codex(source) => codex::sessions_root()
                .is_some_and(|root| source.poll(&root, session_id, self.agent_kind, &mut window)),
            UsageSource::None => false,
        };
        self.running.merge(&window);
        if !found {
            return PricedUsage::default();
        }
        if !self.primed {
            self.primed = true;
            return PricedUsage {
                latest_model: window.latest_model,
                ..PricedUsage::default()
            };
        }
        window
    }

    #[cfg(test)]
    fn poll_paths(&mut self, projects: &Path, cwd: &Path, session_id: &str) -> PricedUsage {
        let mut window = PricedUsage::default();
        let found = match &mut self.source {
            UsageSource::Claude(source) => {
                source.poll(projects, cwd, session_id, self.agent_kind, &mut window)
            }
            UsageSource::Codex(source) => {
                source.poll(projects, session_id, self.agent_kind, &mut window)
            }
            UsageSource::None => false,
        };
        self.running.merge(&window);
        if found && !self.primed {
            self.primed = true;
            return PricedUsage::default();
        }
        window
    }
}

fn is_safe_session_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "halv-usage-{name}-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(path: &Path, lines: &[&str]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn assistant(id: &str, model: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{id}-line","message":{{"id":"{id}","model":"{model}","usage":{usage}}}}}"#
        )
    }

    const OPUS_USAGE: &str = r#"{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":10000,"output_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":200}}"#;

    #[test]
    fn claude_usage_keeps_the_ttl_split() {
        let usage = claude_usage(&serde_json::from_str(OPUS_USAGE).unwrap());
        assert_eq!(
            usage,
            TokenUsage {
                input: 2,
                cache_write_5m: 100,
                cache_write_1h: 200,
                cache_read: 10_000,
                output: 50,
            }
        );
        // Old transcripts have no split; their writes count as 5-minute writes.
        let old = claude_usage(
            &serde_json::from_str(r#"{"input_tokens":1,"cache_creation_input_tokens":40}"#)
                .unwrap(),
        );
        assert_eq!(old.cache_write_5m, 40);
    }

    #[test]
    fn reads_new_lines_once_and_counts_each_response_once() {
        let projects = temp_dir("claude");
        let cwd = Path::new("/work/demo");
        let session = "11111111-2222-4333-8444-555555555555";
        let project_dir = claude::transcript_project_directory_in(&projects, cwd);
        let main = project_dir.join(format!("{session}.jsonl"));
        append(
            &main,
            &[&assistant("msg_old", "claude-opus-5-5", OPUS_USAGE)],
        );

        let mut meter = SessionUsageMeter::new("claude");
        // The first poll primes the meter with the earlier history.
        let primed = meter.poll_paths(&projects, cwd, session);
        assert!(primed.usage.is_empty());
        assert_eq!(meter.running().usage.cache_read, 10_000);

        // Two lines of one response, a subagent and a workflow agent.
        append(
            &main,
            &[
                &assistant("msg_1", "claude-opus-5-5", OPUS_USAGE),
                &assistant("msg_1", "claude-opus-5-5", OPUS_USAGE),
            ],
        );
        append(
            &project_dir.join(session).join("subagents/agent-a.jsonl"),
            &[&assistant("msg_2", "claude-haiku-5-5", OPUS_USAGE)],
        );
        append(
            &project_dir
                .join(session)
                .join("subagents/workflows/wf_1/agent-b.jsonl"),
            &[&assistant("msg_3", "claude-sonnet-5-5", OPUS_USAGE)],
        );
        let window = meter.poll_paths(&projects, cwd, session);
        assert_eq!(window.usage.cache_read, 30_000);
        assert_eq!(window.usage.cache_write_1h, 600);
        assert!(!window.fallback_priced);
        // Each response at its own model price.
        let expected = (2.0 * 4.0 + 100.0 * 5.0 + 200.0 * 8.0 + 10_000.0 * 0.2)
            + (2.0 * 0.1 + 100.0 * 0.125 + 200.0 * 0.2 + 10_000.0 * 0.01)
            + (2.0 * 2.0 + 100.0 * 2.5 + 200.0 * 4.0 + 10_000.0 * 0.1);
        assert!((window.prompt_cost_usd * 1_000_000.0 - expected).abs() < 1e-6);

        // Nothing new: an empty window.
        assert!(meter.poll_paths(&projects, cwd, session).usage.is_empty());
        let _ = fs::remove_dir_all(projects);
    }

    #[test]
    fn a_partial_line_waits_for_its_end() {
        let dir = temp_dir("partial");
        let path = dir.join("t.jsonl");
        let line = assistant("msg_p", "claude-opus-5-5", OPUS_USAGE);
        let (head, tail) = line.split_at(20);
        fs::write(&path, head).unwrap();
        let mut cursor = FileCursor::default();
        assert!(read_new_lines(&path, &mut cursor).is_empty());
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{tail}").unwrap();
        assert_eq!(read_new_lines(&path, &mut cursor), vec![line]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn codex_rollouts_give_deltas_of_the_running_total() {
        let root = temp_dir("codex");
        let session = "01a13baa-3166-7333-af24-5885a65fc636";
        let path = root
            .join("2026/10/14")
            .join(format!("rollout-2026-10-14T15-26-12-{session}.jsonl"));
        append(
            &path,
            &[r#"{"type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#],
        );
        let mut meter = SessionUsageMeter::new("codex");
        meter.poll_paths(&root, Path::new("/"), session);
        append(
            &path,
            &[
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":800,"cache_write_input_tokens":100,"output_tokens":20}}}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":3000,"cached_input_tokens":2500,"cache_write_input_tokens":200,"output_tokens":50}}}}"#,
            ],
        );
        let window = meter.poll_paths(&root, Path::new("/"), session);
        assert_eq!(
            window.usage,
            TokenUsage {
                input: 300,
                cache_write_5m: 200,
                cache_write_1h: 0,
                cache_read: 2_500,
                output: 50,
            }
        );
        assert_eq!(window.latest_model.as_deref(), Some("gpt-6.1-sol"));
        let rate = window.prompt_rate_usd_per_million().unwrap();
        let expected = (300.0 * 2.0 + 200.0 * 2.5 + 2_500.0 * 0.1) / 3_000.0;
        assert!((rate - expected).abs() < 1e-9, "{rate}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unsafe_session_ids_read_nothing() {
        let mut meter = SessionUsageMeter::new("claude");
        assert!(meter.poll(Some("../etc"), Path::new("/")).usage.is_empty());
        assert!(meter.poll(None, Path::new("/")).usage.is_empty());
        assert!(SessionUsageMeter::new("kimi")
            .poll(Some("abc"), Path::new("/"))
            .usage
            .is_empty());
    }
}
