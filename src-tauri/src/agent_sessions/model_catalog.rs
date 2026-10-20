use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CliModel {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub reasoning_efforts: Vec<String>,
    pub default_effort: String,
}

fn strings(value: &Value) -> Vec<String> {
    let mut values = Vec::new();
    for value in value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !values.iter().any(|existing| existing == value) {
            values.push(value.to_string());
        }
    }
    values
}

fn parse_codex_models(value: &Value) -> Vec<CliModel> {
    value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["hidden"] != true)
        .filter_map(|row| {
            let id = row["model"].as_str().or_else(|| row["id"].as_str())?;
            let reasoning_efforts = row["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|effort| effort["reasoningEffort"].as_str())
                .map(ToString::to_string)
                .collect();
            Some(CliModel {
                id: id.to_string(),
                label: row["displayName"].as_str().unwrap_or(id).to_string(),
                provider: "codex_cli".to_string(),
                reasoning_efforts,
                default_effort: row["defaultReasoningEffort"]
                    .as_str()
                    .unwrap_or("medium")
                    .to_string(),
            })
        })
        .collect()
}

fn claude_family(id: &str) -> Option<&str> {
    id.strip_prefix("claude-")?
        .split(['-', '['])
        .next()
        .filter(|family| !family.is_empty())
}

fn claude_label_matches_id(label: &str, id: &str) -> bool {
    let Some(family) = claude_family(id) else {
        return false;
    };
    label.to_ascii_lowercase().contains(family)
}

fn claude_label_is_complete(label: &str, id: &str) -> bool {
    claude_label_matches_id(label, id)
        && (!id.bytes().any(|byte| byte.is_ascii_digit())
            || label.bytes().any(|byte| byte.is_ascii_digit()))
}

fn claude_label(row: &Value, id: &str) -> String {
    let display_name = row["displayName"]
        .as_str()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .filter(|label| claude_label_matches_id(label, id));
    let description_name = row["description"]
        .as_str()
        .and_then(|description| description.split_once(" · "))
        .map(|(label, _)| label.trim())
        .filter(|label| !label.is_empty())
        .filter(|label| claude_label_is_complete(label, id));

    match (display_name, description_name) {
        (Some(display), Some(description))
            if !display.bytes().any(|byte| byte.is_ascii_digit())
                && description.bytes().any(|byte| byte.is_ascii_digit()) =>
        {
            description.to_string()
        }
        (Some(display), _) if claude_label_is_complete(display, id) => display.to_string(),
        (None, Some(description)) => description.to_string(),
        _ => id.to_string(),
    }
}

fn merge_claude_model(existing: &mut CliModel, next: CliModel) {
    if existing.label == existing.id && next.label != next.id
        || !existing.label.bytes().any(|byte| byte.is_ascii_digit())
            && next.label.bytes().any(|byte| byte.is_ascii_digit())
    {
        existing.label = next.label;
    }
    for effort in next.reasoning_efforts {
        if !existing.reasoning_efforts.contains(&effort) {
            existing.reasoning_efforts.push(effort);
        }
    }
    existing.default_effort = if existing
        .reasoning_efforts
        .iter()
        .any(|effort| effort == "medium")
    {
        "medium".to_string()
    } else {
        existing
            .reasoning_efforts
            .first()
            .cloned()
            .unwrap_or_else(|| "high".to_string())
    };
}

fn repair_cached_claude_labels(models: &mut [CliModel]) {
    for model in models {
        if model.provider == "claude_cli"
            && !claude_label_is_complete(model.label.trim(), &model.id)
        {
            model.label = model.id.clone();
        }
    }
}

fn parse_claude_models(value: &Value) -> Vec<CliModel> {
    let mut models = Vec::new();
    for row in value["models"].as_array().into_iter().flatten() {
        let Some(id) = row["resolvedModel"]
            .as_str()
            .or_else(|| row["value"].as_str())
        else {
            continue;
        };
        if !id.starts_with("claude-") {
            continue;
        }
        let efforts = strings(&row["supportedEffortLevels"]);
        let default_effort = if efforts.iter().any(|effort| effort == "medium") {
            "medium"
        } else {
            efforts.first().map(String::as_str).unwrap_or("high")
        };
        let model = CliModel {
            id: id.to_string(),
            label: claude_label(row, id),
            provider: "claude_cli".to_string(),
            default_effort: default_effort.to_string(),
            reasoning_efforts: efforts,
        };
        if let Some(existing) = models
            .iter_mut()
            .find(|existing: &&mut CliModel| existing.id == id)
        {
            merge_claude_model(existing, model);
        } else {
            models.push(model);
        }
    }
    models
}

async fn discover(binary: &Path, claude: bool) -> Result<Vec<CliModel>, String> {
    // The CLI runs outside the sandbox and can load project config from its
    // working directory. A sandboxed command can write to the temp directory,
    // so the working directory is private.
    let directory = super::session_files::WorkDirectory::create("models")?;
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let mut command = tokio::process::Command::new(binary);
        crate::process::hide_console_tokio(&mut command);
        // The model list depends on the account plan, so discovery uses
        // the active wallet account.
        let provider = if claude {
            crate::wallet::WalletProvider::Claude
        } else {
            crate::wallet::WalletProvider::Codex
        };
        crate::wallet::probe_environment(provider)?.apply_tokio(&mut command);
        if claude {
            command.args(["--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--no-session-persistence", "--tools", "", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}"]);
        } else {
            command.arg("app-server");
        }
        let mut child = command.current_dir(directory.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().map_err(|error| error.to_string())?;
        let mut input = child.stdin.take().ok_or("CLI input unavailable")?;
        let mut output = BufReader::new(child.stdout.take().ok_or("CLI output unavailable")?).lines();
        let initialize = if claude {
            json!({"type":"control_request","request_id":"halv-models","request":{"subtype":"initialize","hooks":null}})
        } else {
            json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"halv","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}})
        };
        input.write_all(format!("{initialize}\n").as_bytes()).await.map_err(|error| error.to_string())?;
        let mut models = Vec::new();
        while let Some(line) = output.next_line().await.map_err(|error| error.to_string())? {
            let Ok(row) = serde_json::from_str::<Value>(&line) else { continue };
            if claude && row["type"] == "control_response" && row["response"]["request_id"] == "halv-models" {
                models = parse_claude_models(&row["response"]["response"]);
                break;
            }
            if !claude && row["id"] == 1 {
                input.write_all(b"{\"method\":\"initialized\"}\n{\"id\":2,\"method\":\"model/list\",\"params\":{\"limit\":100,\"includeHidden\":false}}\n").await.map_err(|error| error.to_string())?;
            } else if !claude && row["id"] == 2 {
                models.extend(parse_codex_models(&row["result"]));
                if let Some(cursor) = row["result"]["nextCursor"].as_str() {
                    let next = json!({"id":2,"method":"model/list","params":{"limit":100,"includeHidden":false,"cursor":cursor}});
                    input.write_all(format!("{next}\n").as_bytes()).await.map_err(|error| error.to_string())?;
                } else { break; }
            }
        }
        let _ = child.kill().await;
        if models.is_empty() { Err("CLI returned no available models".to_string()) } else { Ok(models) }
    }).await.map_err(|_| "CLI model discovery timed out".to_string()).and_then(|result| result);
    drop(directory);
    result
}

/// The account key makes a wallet switch refresh the model list.
type CatalogCache = Option<(Instant, String, Vec<CliModel>)>;

static CACHE: tokio::sync::Mutex<CatalogCache> = tokio::sync::Mutex::const_new(None);

pub(crate) async fn available_models(app: &tauri::AppHandle) -> Vec<CliModel> {
    let account_key = format!(
        "{}|{}",
        crate::wallet::usage_cache_key(crate::wallet::WalletProvider::Claude),
        crate::wallet::usage_cache_key(crate::wallet::WalletProvider::Codex)
    );
    let mut cache = CACHE.lock().await;
    if let Some((checked, cached_key, models)) = cache.as_ref() {
        if cached_key == &account_key && checked.elapsed() < Duration::from_secs(60) {
            return models.clone();
        }
    }
    let path = crate::app_data::resolved_app_data_dir(app)
        .ok()
        .map(|path| path.join("cli-models.json"));
    let mut models = cache
        .as_ref()
        .map(|(_, _, models)| models.clone())
        .unwrap_or_else(|| {
            path.as_ref()
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| serde_json::from_slice::<Vec<CliModel>>(&bytes).ok())
                .unwrap_or_default()
        });
    repair_cached_claude_labels(&mut models);
    let codex = async {
        match super::codex::find_binary() {
            Some(binary) => discover(&binary, false).await,
            None => Err("Codex is not installed".to_string()),
        }
    };
    let claude = async {
        match super::claude::find_binary() {
            Some(binary) => discover(&binary, true).await,
            None => Err("Claude Code is not installed".to_string()),
        }
    };
    let (codex, claude) = tokio::join!(codex, claude);
    for (provider, result) in [("codex_cli", codex), ("claude_cli", claude)] {
        if let Ok(fresh) = result {
            models.retain(|model| model.provider != provider);
            models.extend(fresh);
        }
    }
    if let Some(path) = path {
        if let Ok(bytes) = serde_json::to_vec(&models) {
            let _ = crate::exec::apply_patch::atomic_write_file(&path, &bytes);
        }
    }
    *cache = Some((Instant::now(), account_key, models.clone()));
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires locally installed CLIs; reads account model metadata only"]
    async fn live_cli_model_discovery() {
        for (binary, claude) in [
            (super::super::codex::find_binary(), false),
            (super::super::claude::find_binary(), true),
        ] {
            let binary = binary.expect("CLI must be installed for this integration check");
            let models = discover(&binary, claude)
                .await
                .expect("live CLI discovery failed");
            assert!(!models.is_empty());
            println!(
                "{} models: {:?}",
                if claude { "Claude" } else { "Codex" },
                models.iter().map(|model| &model.label).collect::<Vec<_>>()
            );
        }
    }

    // A fake Codex gives its working directory as the model label.
    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_runs_codex_in_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;

        let bin =
            std::env::temp_dir().join(format!("halv-fake-codex-{}", crate::storage::new_uuid_v4()));
        std::fs::create_dir_all(&bin).expect("fake codex directory should exist");
        let codex = bin.join("codex");
        std::fs::write(
            &codex,
            r#"#!/bin/sh
read -r _
printf '%s\n' '{"id":1,"result":{}}'
read -r _
read -r _
printf '{"id":2,"result":{"data":[{"model":"fake","displayName":"%s"}]}}\n' "$(pwd -P)"
"#,
        )
        .expect("fake codex should write");
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755))
            .expect("fake codex should be executable");

        let models = discover(&codex, false)
            .await
            .expect("fake codex discovery should work");

        let directory = std::path::PathBuf::from(&models[0].label);
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
            .is_some_and(|name| name.starts_with("models-")));
        assert!(!directory.exists());
        std::fs::remove_dir_all(bin).expect("fake codex directory should be removable");
    }

    #[test]
    fn codex_models_and_efforts_come_from_account_capabilities() {
        let models = parse_codex_models(
            &json!({"data":[{"model":"gpt-future","displayName":"Future model","hidden":false,"supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"ultra"}],"defaultReasoningEffort":"high"},{"model":"hidden-model","hidden":true}]}),
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-future");
        assert_eq!(models[0].reasoning_efforts, ["high", "ultra"]);
    }

    #[test]
    fn claude_uses_live_names_versions_ids_and_efforts() {
        let models = parse_claude_models(&json!({"models":[
            {"value":"default","displayName":"Default (recommended)","description":"Opus 5.5 · Best for everyday, complex tasks","resolvedModel":"claude-opus-5-5","supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"opus","displayName":"Opus 5.5","description":"Most capable for ambitious work","resolvedModel":"claude-opus-5-5","supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"claude-fable-5-1","displayName":"Fable 5.1","description":"For your toughest challenges","resolvedModel":"claude-fable-5-1","supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"sonnet","displayName":"Sonnet 5","description":"Most efficient for everyday tasks","resolvedModel":"claude-sonnet-5","supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"haiku","displayName":"Haiku 4.5","description":"Fastest for quick answers","resolvedModel":"claude-haiku-4-5-20251001","supportedEffortLevels":null}
        ]}));
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].id, "claude-opus-5-5");
        assert_eq!(models[0].label, "Opus 5.5");
        assert_eq!(models[0].default_effort, "medium");
        assert_eq!(
            models[0].reasoning_efforts,
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(models[1].label, "Fable 5.1");
        assert_eq!(models[2].label, "Sonnet 5");
        assert_eq!(models[3].label, "Haiku 4.5");
        assert!(models[3].reasoning_efforts.is_empty());
        assert_eq!(models[3].default_effort, "high");
    }

    #[test]
    fn claude_merges_aliases_and_keeps_the_richest_versioned_label() {
        let models = parse_claude_models(&json!({"models":[
            {"value":"default","resolvedModel":"claude-future[1m]","displayName":"Future","description":"Future 9.1 · Best for coding","supportedEffortLevels":["high","high"]},
            {"value":"future","resolvedModel":"claude-future[1m]","displayName":"Future 9.1","description":"Marketing sentence","supportedEffortLevels":["medium","max"]}
        ]}));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-future[1m]");
        assert_eq!(models[0].label, "Future 9.1");
        assert_eq!(models[0].reasoning_efforts, ["high", "medium", "max"]);
        assert_eq!(models[0].default_effort, "medium");
    }

    #[test]
    fn claude_missing_fields_fall_back_to_the_exact_model_id() {
        let models = parse_claude_models(&json!({"models":[
            {"value":"claude-unknown-7-2","description":"For all of your work"},
            {"value":"sonnet","description":"No resolved model"},
            {"value":"claude-empty","displayName":"   ","description":"   "},
            {"value":"claude-alias-8-2","displayName":"Alias","description":"Fastest for work"},
            {"value":"not-claude","displayName":"Other"}
        ]}));
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].label, "claude-unknown-7-2");
        assert_eq!(models[1].label, "claude-empty");
        assert_eq!(models[2].label, "claude-alias-8-2");
    }

    #[test]
    fn cached_marketing_copy_is_replaced_by_the_model_id() {
        let mut models = vec![
            CliModel {
                id: "claude-fable-5-1".to_string(),
                label: "For your toughest challenges".to_string(),
                provider: "claude_cli".to_string(),
                reasoning_efforts: vec!["high".to_string()],
                default_effort: "high".to_string(),
            },
            CliModel {
                id: "claude-sonnet-5".to_string(),
                label: "Sonnet".to_string(),
                provider: "claude_cli".to_string(),
                reasoning_efforts: vec!["medium".to_string()],
                default_effort: "medium".to_string(),
            },
            CliModel {
                id: "claude-opus-5-5".to_string(),
                label: "Opus 5.5".to_string(),
                provider: "claude_cli".to_string(),
                reasoning_efforts: vec!["medium".to_string()],
                default_effort: "medium".to_string(),
            },
        ];
        repair_cached_claude_labels(&mut models);
        assert_eq!(models[0].label, "claude-fable-5-1");
        assert_eq!(models[1].label, "claude-sonnet-5");
        assert_eq!(models[2].label, "Opus 5.5");
    }
}
