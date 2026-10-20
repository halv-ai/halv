type Result<T, E = String> = std::result::Result<T, E>;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tauri::{Emitter, Manager};

use super::AgentKind;

#[derive(Debug, Clone)]
pub(super) struct MemorySource {
    pub session_id: String,
    pub kind: AgentKind,
    pub path: PathBuf,
}

#[derive(Debug, PartialEq)]
struct CompletedTurn {
    key: String,
    user: String,
    assistant: String,
}

fn text_content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text" | "input_text" | "output_text") => {
                block.get("text").and_then(Value::as_str)
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Only user text and final assistant text enter memory extraction. Tool output,
// reasoning, interrupted turns, and trailing partial JSON are deliberately excluded.
fn latest_completed_turn(contents: &str, kind: &AgentKind) -> Option<CompletedTurn> {
    let mut user = String::new();
    let mut assistant = String::new();
    let mut turn_id = String::new();
    let mut completed = None;
    for line in contents.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let row_type = row.get("type").and_then(Value::as_str).unwrap_or_default();
        if *kind == AgentKind::Codex {
            let payload = &row["payload"];
            if row_type == "event_msg" {
                match payload["type"].as_str().unwrap_or_default() {
                    "task_started" => {
                        user.clear();
                        assistant.clear();
                        turn_id = payload["turn_id"].as_str().unwrap_or_default().to_string();
                    }
                    "user_message" => {
                        user = payload["message"].as_str().unwrap_or_default().to_string()
                    }
                    "agent_message" if payload["phase"].as_str() != Some("commentary") => {
                        assistant = payload["message"].as_str().unwrap_or_default().to_string();
                    }
                    "task_complete" => {
                        if let Some(final_text) = payload["last_agent_message"].as_str() {
                            assistant = final_text.to_string();
                        }
                        if let Some(id) = payload["turn_id"].as_str() {
                            turn_id = id.to_string();
                        }
                        if !turn_id.is_empty()
                            && !user.trim().is_empty()
                            && !assistant.trim().is_empty()
                        {
                            completed = Some(CompletedTurn {
                                key: turn_id.clone(),
                                user: user.clone(),
                                assistant: assistant.clone(),
                            });
                        }
                        user.clear();
                        assistant.clear();
                    }
                    "task_aborted" | "turn_aborted" => {
                        user.clear();
                        assistant.clear();
                    }
                    _ => {}
                }
            } else if row_type == "response_item" && payload["type"] == "message" {
                match payload["role"].as_str() {
                    Some("user") if !turn_id.is_empty() && user.is_empty() => {
                        user = text_content(&payload["content"])
                    }
                    Some("assistant") if payload["phase"] == "final_answer" => {
                        assistant = text_content(&payload["content"])
                    }
                    _ => {}
                }
            }
        } else {
            let message = &row["message"];
            if row_type == "user" && row["isMeta"] != true {
                let text = text_content(&message["content"]);
                if !text.trim().is_empty() {
                    user = text;
                }
            } else if row_type == "assistant" && message["stop_reason"] == "end_turn" {
                let text = text_content(&message["content"]);
                let id = row["uuid"]
                    .as_str()
                    .or_else(|| message["id"].as_str())
                    .unwrap_or_default();
                if !user.trim().is_empty() && !text.trim().is_empty() && !id.is_empty() {
                    completed = Some(CompletedTurn {
                        key: id.to_string(),
                        user: user.clone(),
                        assistant: text,
                    });
                }
                user.clear();
            }
        }
    }
    completed
}

fn read_completed_turn(source: &MemorySource) -> Result<Option<CompletedTurn>> {
    let mut file = std::fs::File::open(&source.path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let mut window = 512 * 1024_u64;
    loop {
        let offset = length.saturating_sub(window);
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(window)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let contents = String::from_utf8_lossy(&bytes);
        let contents = if offset > 0 {
            contents
                .split_once('\n')
                .map(|(_, rest)| rest)
                .unwrap_or_default()
        } else {
            &contents
        };
        if let Some(mut turn) = latest_completed_turn(contents, &source.kind) {
            turn.user = turn.user.chars().take(6000).collect();
            turn.assistant = turn.assistant.chars().take(8000).collect();
            return Ok(Some(turn));
        }
        if offset == 0 || window >= 8 * 1024 * 1024 {
            return Ok(None);
        }
        window *= 2;
    }
}

pub(crate) async fn request_claude_memories(
    instructions: &str,
    prompt: &str,
) -> Result<String, String> {
    let binary = super::claude::find_binary()
        .ok_or("Claude Code is not installed. Connect it in AI connections first.")?;
    let account = crate::wallet::launch_environment(crate::wallet::WalletProvider::Claude)?;
    run_claude_memories(binary, &account, instructions, prompt).await
}

async fn run_claude_memories(
    binary: PathBuf,
    account: &crate::wallet::AccountEnvironment,
    instructions: &str,
    prompt: &str,
) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    // The CLI flags turn off settings, tools, and MCP servers. The private
    // working directory also keeps out project files of a sandboxed command.
    let directory = super::session_files::WorkDirectory::create("memory")?;
    let result = async {
        let mut command = tokio::process::Command::new(binary);
        crate::process::hide_console_tokio(&mut command);
        account.apply_tokio(&mut command);
        command.args(["--print", "--model", "haiku", "--output-format", "json", "--no-session-persistence", "--tools", "", "--disable-slash-commands", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}"])
            .arg("--system-prompt").arg(instructions).current_dir(directory.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let mut input = child.stdin.take().ok_or("Claude Code input is unavailable")?;
        input.write_all(prompt.as_bytes()).await.map_err(|e| e.to_string())?;
        drop(input);
        let output = tokio::time::timeout(std::time::Duration::from_secs(90), child.wait_with_output()).await
            .map_err(|_| "Claude Code memory extraction timed out".to_string())?.map_err(|e| e.to_string())?;
        if !output.status.success() { return Err(format!("Claude Code memory extraction failed ({}). Check your connection in AI connections.", output.status)); }
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("Invalid Claude Code memory response: {e}"))?;
        if value["is_error"] == true { return Err("Claude Code could not extract memories. Check your subscription and connection.".to_string()); }
        value["result"].as_str().map(ToString::to_string).ok_or_else(|| "Claude Code returned no memory response".to_string())
    }.await;
    drop(directory);
    result
}

type ScanStamp = (u64, std::time::SystemTime, std::time::Instant);
static CHECKED: OnceLock<Mutex<HashMap<String, ScanStamp>>> = OnceLock::new();

static PENDING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static GENERATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) fn schedule(app: &tauri::AppHandle, source: MemorySource) {
    if !app
        .state::<crate::preferences::PreferencesManager>()
        .memories_enabled()
        .unwrap_or(false)
    {
        return;
    }
    let Ok(metadata) = std::fs::metadata(&source.path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    let checked = CHECKED.get_or_init(Default::default);
    {
        let checked = checked.lock().unwrap();
        if checked
            .get(&source.session_id)
            .is_some_and(|(size, previous, time)| {
                *size == metadata.len()
                    && *previous == modified
                    && time.elapsed() < std::time::Duration::from_secs(300)
            })
        {
            return;
        }
    }
    let pending = PENDING.get_or_init(Default::default);
    if !pending.lock().unwrap().insert(source.session_id.clone()) {
        return;
    }
    checked.lock().unwrap().insert(
        source.session_id.clone(),
        (metadata.len(), modified, std::time::Instant::now()),
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = generate(&app, &source, false).await {
            eprintln!("Terminal memory generation failed: {error:#}");
        }
        PENDING
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .remove(&source.session_id);
    });
}

async fn generate(app: &tauri::AppHandle, source: &MemorySource, force: bool) -> Result<usize> {
    let _guard = GENERATION_LOCK.lock().await;
    if !app
        .state::<crate::preferences::PreferencesManager>()
        .memories_enabled()?
    {
        return Ok(0);
    }
    let storage = app.state::<crate::storage::Storage>();
    if !storage
        .agent_memory_subscription_session(&source.session_id)
        .map_err(|e| e.to_string())?
    {
        return Ok(0);
    }
    let Some(turn) = read_completed_turn(source)? else {
        return Ok(0);
    };
    let key = format!("{}:{}", source.session_id, turn.key);
    if !force
        && storage
            .agent_memory_turn_processed(&key)
            .map_err(|e| e.to_string())?
    {
        return Ok(0);
    }
    let count = crate::chat::generate_terminal_agent_memories(
        app,
        &source.session_id,
        &source.kind,
        &turn.user,
        &turn.assistant,
    )
    .await?;
    // Recheck consent after the network call as well as before it.
    if app
        .state::<crate::preferences::PreferencesManager>()
        .memories_enabled()?
    {
        storage
            .mark_agent_memory_turn_processed(&key)
            .map_err(|e| e.to_string())?;
        app.emit("agent-memories-changed", ())
            .map_err(|e| e.to_string())?;
    }
    Ok(count)
}

#[tauri::command]
pub async fn storage_rebuild_agent_memories(app: tauri::AppHandle) -> Result<usize, String> {
    if !app
        .state::<crate::preferences::PreferencesManager>()
        .memories_enabled()?
    {
        return Err("Enable global memories before extracting recent history.".to_string());
    }
    let sources =
        super::transcript_watcher::recent_memory_sources(&app).map_err(|e| e.to_string())?;
    let mut added = 0;
    for source in sources {
        added += generate(&app, &source, true)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fake Claude Code gives its working directory as the memory response.
    #[cfg(unix)]
    #[tokio::test]
    async fn memory_extraction_runs_claude_in_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;

        let bin = std::env::temp_dir().join(format!(
            "halv-fake-claude-{}",
            crate::storage::new_uuid_v4()
        ));
        std::fs::create_dir_all(&bin).expect("fake claude directory should exist");
        let claude = bin.join("claude");
        std::fs::write(
            &claude,
            r#"#!/bin/sh
cat >/dev/null
printf '{"result":"%s"}\n' "$(pwd -P)"
"#,
        )
        .expect("fake claude should write");
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755))
            .expect("fake claude should be executable");

        let response = run_claude_memories(
            claude,
            &crate::wallet::AccountEnvironment::default(),
            "instructions",
            "prompt",
        )
        .await
        .expect("fake claude memory extraction should work");

        let directory = PathBuf::from(response);
        // `pwd -P` resolves links, such as /var to /private/var on macOS.
        let root = super::super::session_files::root().expect("tests have a session root");
        let root = root
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .expect("session root parent should resolve")
            .join(root.file_name().expect("session root should have a name"));
        assert_eq!(directory.parent(), Some(root.as_path()));
        assert!(directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("memory-")));
        assert!(!directory.exists());
        std::fs::remove_dir_all(bin).expect("fake claude directory should be removable");
    }

    #[test]
    fn codex_extracts_only_completed_turn_and_excludes_tools() {
        let rows = [
            serde_json::json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"a"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"Use pnpm in this project"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","output":"not a memory"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"a","last_agent_message":"I will use pnpm."}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"b"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"unfinished"}}),
        ];
        let turn = latest_completed_turn(
            &rows
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            &AgentKind::Codex,
        )
        .unwrap();
        assert_eq!(turn.key, "a");
        assert_eq!(turn.user, "Use pnpm in this project");
        assert_eq!(turn.assistant, "I will use pnpm.");
    }

    #[test]
    fn claude_ignores_tool_round_trips_and_partial_responses() {
        let rows = [
            serde_json::json!({"type":"user","message":{"content":"Prefer concise replies"}}),
            serde_json::json!({"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"Looking up..."}]}}),
            serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","content":"do not memorize"}]}}),
            serde_json::json!({"type":"assistant","uuid":"done","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Understood."}]}}),
        ];
        let data = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let turn = latest_completed_turn(&(data + "\n{\"type\":"), &AgentKind::Claude).unwrap();
        assert_eq!(turn.key, "done");
        assert_eq!(turn.user, "Prefer concise replies");
        assert_eq!(turn.assistant, "Understood.");
    }

    #[test]
    fn reads_completed_turn_across_large_tool_output() {
        let path = std::env::temp_dir().join(format!(
            "halv-memory-test-{}.jsonl",
            crate::storage::new_uuid_v4()
        ));
        let rows = [
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"Use pnpm"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","output":"x".repeat(700 * 1024)}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"long","last_agent_message":"Saved your preference."}}),
        ];
        std::fs::write(
            &path,
            rows.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let source = MemorySource {
            session_id: "session".into(),
            kind: AgentKind::Codex,
            path: path.clone(),
        };
        let turn = read_completed_turn(&source).unwrap().unwrap();
        assert_eq!(turn.user, "Use pnpm");
        assert_eq!(turn.key, "long");
        assert_eq!(turn.assistant, "Saved your preference.");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incomplete_transcripts_do_not_generate_memories() {
        assert!(latest_completed_turn(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"unfinished"}}"#,
            &AgentKind::Codex
        )
        .is_none());
        assert!(latest_completed_turn(r#"{"type":"assistant","uuid":"x","message":{"stop_reason":"tool_use","content":"thinking"}}"#, &AgentKind::Claude).is_none());
    }
}
