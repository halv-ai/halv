use crate::{projects, storage::Storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

type EnvironmentResult<T> = Result<T, String>;
const ENVIRONMENT_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 256 * 1024;
const MAX_ACTIONS: usize = 64;
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
// The UI matches this text. It then shows the scripts for approval and tries again.
const APPROVAL_REQUIRED_ERROR: &str = "local environment needs approval before its scripts run";
const CONTENT_CHANGED_ERROR: &str = "local environment changed; review it again";

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEnvironmentPlatformScript {
    #[serde(default)]
    pub script: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEnvironmentLifecycle {
    #[serde(default)]
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub darwin: Option<LocalEnvironmentPlatformScript>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<LocalEnvironmentPlatformScript>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub win32: Option<LocalEnvironmentPlatformScript>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEnvironmentAction {
    pub name: String,
    #[serde(default = "default_action_icon")]
    pub icon: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEnvironmentConfig {
    #[serde(default = "default_environment_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub setup: LocalEnvironmentLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<LocalEnvironmentLifecycle>,
    #[serde(default)]
    pub actions: Vec<LocalEnvironmentAction>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEnvironmentDescriptor {
    pub config_path: String,
    pub workspace_root: String,
    pub inherited: bool,
    pub config: Option<LocalEnvironmentConfig>,
    pub error: Option<String>,
    pub content_sha256: Option<String>,
    pub approved: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedLocalEnvironment {
    pub config_path: PathBuf,
    pub config: LocalEnvironmentConfig,
    // These fields are private, so only this module can mark content as approved.
    content_sha256: String,
    approved: bool,
}

impl LoadedLocalEnvironment {
    // Halv runs a script from the file only after the user approved this exact content.
    // An empty script runs nothing, so it needs no approval.
    fn check_approval(&self, lifecycle: &LocalEnvironmentLifecycle) -> EnvironmentResult<()> {
        if self.approved || platform_script(lifecycle).trim().is_empty() {
            Ok(())
        } else {
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        }
    }

    // Lets a caller refuse before it creates a worktree for the setup script.
    pub(crate) fn check_setup_approval(&self) -> EnvironmentResult<()> {
        self.check_approval(&self.config.setup)
    }
}

fn default_environment_version() -> u32 {
    ENVIRONMENT_VERSION
}

fn default_action_icon() -> String {
    "tool".to_string()
}

#[tauri::command]
pub async fn local_environment_list(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
) -> EnvironmentResult<Vec<LocalEnvironmentDescriptor>> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = projects::validate_registered_project(&project)?;
    list_environments(storage.inner(), project_id, &root)
}

#[tauri::command]
pub async fn local_environment_save(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    config_path: Option<String>,
    filename: Option<String>,
    config: LocalEnvironmentConfig,
) -> EnvironmentResult<LocalEnvironmentDescriptor> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = projects::validate_registered_project(&project)?;
    save_environment(
        storage.inner(),
        project_id,
        &root,
        config_path.as_deref(),
        filename.as_deref(),
        config,
    )
}

#[tauri::command]
pub async fn local_environment_approve(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    config_path: String,
    content_sha256: String,
) -> EnvironmentResult<()> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = projects::validate_registered_project(&project)?;
    approve_environment(
        storage.inner(),
        project_id,
        &root,
        &config_path,
        &content_sha256,
    )
}

#[tauri::command]
pub async fn local_environment_resolve_action(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    config_path: String,
    content_sha256: String,
    action_index: usize,
) -> EnvironmentResult<LocalEnvironmentAction> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = projects::validate_registered_project(&project)?;
    let environment =
        load_environment_for_project(storage.inner(), project_id, &root, &config_path)?;
    resolve_action(&environment, &content_sha256, action_index)
}

#[tauri::command]
pub async fn local_environment_delete(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    config_path: String,
) -> EnvironmentResult<()> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = projects::validate_registered_project(&project)?;
    delete_environment(&root, &config_path)
}

pub(crate) fn load_environment_for_project(
    storage: &Storage,
    project_id: i64,
    project_root: &Path,
    config_path: &str,
) -> EnvironmentResult<LoadedLocalEnvironment> {
    let project_root = project_root
        .canonicalize()
        .map_err(|error| format!("failed to validate project root: {error}"))?;
    let candidate = Path::new(config_path)
        .canonicalize()
        .map_err(|error| format!("failed to locate local environment: {error}"))?;
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| format!("failed to inspect local environment: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("local environment must be a regular TOML file".to_string());
    }
    let allowed = project_root.ancestors().any(|ancestor| {
        let directory = ancestor.join(".codex").join("environments");
        directory
            .canonicalize()
            .ok()
            .is_some_and(|directory| candidate.parent() == Some(directory.as_path()))
    });
    if !allowed {
        return Err("local environment is outside this project's inherited configuration".into());
    }
    let (config, content_sha256) = read_environment_file(&candidate)?;
    let approved = approval_matches(
        storage,
        project_id,
        &candidate.to_string_lossy(),
        &content_sha256,
    )?;
    Ok(LoadedLocalEnvironment {
        config_path: candidate,
        config,
        content_sha256,
        approved,
    })
}

pub(crate) fn run_setup(
    environment: &LoadedLocalEnvironment,
    worktree_root: &Path,
) -> EnvironmentResult<String> {
    environment.check_approval(&environment.config.setup)?;
    run_lifecycle(
        "setup",
        &environment.config.setup,
        worktree_root,
        Some(worktree_root),
    )
}

pub(crate) fn run_cleanup(
    environment: &LoadedLocalEnvironment,
    worktree_root: &Path,
) -> EnvironmentResult<String> {
    let Some(cleanup) = environment.config.cleanup.as_ref() else {
        return Ok(String::new());
    };
    environment.check_approval(cleanup)?;
    run_lifecycle("cleanup", cleanup, worktree_root, Some(worktree_root))
}

// The user approves the content that the UI showed. A changed file needs a new review.
fn approve_environment(
    storage: &Storage,
    project_id: i64,
    project_root: &Path,
    config_path: &str,
    content_sha256: &str,
) -> EnvironmentResult<()> {
    let environment = load_environment_for_project(storage, project_id, project_root, config_path)?;
    if environment.content_sha256 != content_sha256 {
        return Err(CONTENT_CHANGED_ERROR.to_string());
    }
    storage
        .set_local_environment_approval(
            project_id,
            &environment.config_path.to_string_lossy(),
            &environment.content_sha256,
        )
        .map_err(|error| error.to_string())
}

// The UI runs an action only with a command from approved content.
// The hash makes sure that the action is the one that the user clicked.
fn resolve_action(
    environment: &LoadedLocalEnvironment,
    content_sha256: &str,
    action_index: usize,
) -> EnvironmentResult<LocalEnvironmentAction> {
    if environment.content_sha256 != content_sha256 {
        return Err(CONTENT_CHANGED_ERROR.to_string());
    }
    if !environment.approved {
        return Err(APPROVAL_REQUIRED_ERROR.to_string());
    }
    environment
        .config
        .actions
        .get(action_index)
        .cloned()
        .ok_or_else(|| "local environment action does not exist".to_string())
}

fn approval_matches(
    storage: &Storage,
    project_id: i64,
    config_path: &str,
    content_sha256: &str,
) -> EnvironmentResult<bool> {
    let approved = storage
        .local_environment_approval(project_id, config_path)
        .map_err(|error| error.to_string())?;
    Ok(approved.as_deref() == Some(content_sha256))
}

fn sha256_hex(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}

fn list_environments(
    storage: &Storage,
    project_id: i64,
    project_root: &Path,
) -> EnvironmentResult<Vec<LocalEnvironmentDescriptor>> {
    let project_root = project_root
        .canonicalize()
        .map_err(|error| format!("failed to validate project root: {error}"))?;
    let mut rows = Vec::new();
    for ancestor in project_root.ancestors() {
        let directory = ancestor.join(".codex").join("environments");
        let Ok(metadata) = fs::symlink_metadata(&directory) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let mut paths = fs::read_dir(&directory)
            .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                continue;
            }
            let canonical = path.canonicalize().unwrap_or(path);
            let config_path = canonical.to_string_lossy().into_owned();
            let (config, error, content_sha256, approved) = match read_environment_file(&canonical)
            {
                Ok((config, content_sha256)) => {
                    let approved =
                        approval_matches(storage, project_id, &config_path, &content_sha256)?;
                    (Some(config), None, Some(content_sha256), approved)
                }
                Err(error) => (None, Some(error), None, false),
            };
            rows.push(LocalEnvironmentDescriptor {
                config_path,
                workspace_root: ancestor.to_string_lossy().into_owned(),
                inherited: ancestor != project_root,
                config,
                error,
                content_sha256,
                approved,
            });
        }
    }
    Ok(rows)
}

// Hashes the same bytes that it parses, so an approval covers exactly what runs.
fn read_environment_file(path: &Path) -> EnvironmentResult<(LocalEnvironmentConfig, String)> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} exceeds the 1 MB local-environment limit",
            path.display()
        ));
    }
    let raw =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let content_sha256 = sha256_hex(&raw);
    let raw = String::from_utf8(raw)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let config = toml::from_str::<LocalEnvironmentConfig>(&raw)
        .map_err(|error| format!("invalid local environment {}: {error}", path.display()))?;
    validate_config(&config)?;
    Ok((config, content_sha256))
}

fn save_environment(
    storage: &Storage,
    project_id: i64,
    project_root: &Path,
    config_path: Option<&str>,
    filename: Option<&str>,
    config: LocalEnvironmentConfig,
) -> EnvironmentResult<LocalEnvironmentDescriptor> {
    validate_config(&config)?;
    let project_root = project_root
        .canonicalize()
        .map_err(|error| format!("failed to validate project root: {error}"))?;
    let directory = project_root.join(".codex").join("environments");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create {}: {error}", directory.display()))?;
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("failed to validate environment directory: {error}"))?;
    let path = match config_path {
        Some(config_path) => validate_direct_path(&directory, config_path)?,
        None => choose_new_path(&directory, filename)?,
    };
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("refusing to replace a symlink or non-file environment".to_string());
        }
    }
    let mut raw = String::from("# THIS IS AUTOGENERATED. DO NOT EDIT MANUALLY\n");
    raw.push_str(
        &toml::to_string_pretty(&config)
            .map_err(|error| format!("failed to encode local environment: {error}"))?,
    );
    if raw.len() as u64 > MAX_CONFIG_BYTES {
        return Err("local environment exceeds the 1 MB limit".to_string());
    }
    let saved_sha256 = sha256_hex(raw.as_bytes());
    let temporary = directory.join(format!(
        ".{}.tmp-{:016x}",
        path.file_name().unwrap().to_string_lossy(),
        rand::random::<u64>()
    ));
    fs::write(&temporary, raw)
        .map_err(|error| format!("failed to write local environment: {error}"))?;
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("failed to install local environment: {error}")
    })?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to validate saved environment: {error}"))?;
    let config_path = canonical.to_string_lossy().into_owned();
    let approved = approval_matches(storage, project_id, &config_path, &saved_sha256)?;
    Ok(LocalEnvironmentDescriptor {
        config_path,
        workspace_root: project_root.to_string_lossy().into_owned(),
        inherited: false,
        config: Some(config),
        error: None,
        content_sha256: Some(saved_sha256),
        approved,
    })
}

fn delete_environment(project_root: &Path, config_path: &str) -> EnvironmentResult<()> {
    let project_root = project_root
        .canonicalize()
        .map_err(|error| format!("failed to validate project root: {error}"))?;
    let directory = project_root.join(".codex").join("environments");
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("failed to locate local environments: {error}"))?;
    let path = validate_direct_path(&directory, config_path)?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("failed to inspect local environment: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("refusing to delete a symlink or non-file environment".to_string());
    }
    fs::remove_file(path).map_err(|error| format!("failed to delete local environment: {error}"))
}

fn validate_direct_path(directory: &Path, raw: &str) -> EnvironmentResult<PathBuf> {
    let raw = Path::new(raw);
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        directory.join(raw)
    };
    let parent = path
        .parent()
        .ok_or_else(|| "invalid local-environment path".to_string())?;
    let parent = parent
        .canonicalize()
        .map_err(|error| format!("failed to validate environment directory: {error}"))?;
    if parent != directory {
        return Err("local environment must stay in this project's .codex/environments".into());
    }
    validate_filename(path.file_name().and_then(|value| value.to_str()))?;
    Ok(path)
}

fn choose_new_path(directory: &Path, filename: Option<&str>) -> EnvironmentResult<PathBuf> {
    if let Some(filename) = filename.map(str::trim).filter(|value| !value.is_empty()) {
        validate_filename(Some(filename))?;
        let path = directory.join(filename);
        if path.exists() {
            return Err(format!("{filename} already exists"));
        }
        return Ok(path);
    }
    for index in 1..=10_000 {
        let filename = if index == 1 {
            "environment.toml".to_string()
        } else {
            format!("environment-{index}.toml")
        };
        let path = directory.join(filename);
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("could not allocate a local-environment filename".to_string())
}

fn validate_filename(filename: Option<&str>) -> EnvironmentResult<()> {
    let filename = filename.ok_or_else(|| "environment filename is required".to_string())?;
    if filename.is_empty()
        || filename.len() > 120
        || !filename.ends_with(".toml")
        || filename.starts_with('.')
        || filename
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err("environment filename must be a plain .toml filename".to_string());
    }
    Ok(())
}

fn validate_config(config: &LocalEnvironmentConfig) -> EnvironmentResult<()> {
    if config.version != ENVIRONMENT_VERSION {
        return Err(format!(
            "unsupported local-environment version {}; expected {ENVIRONMENT_VERSION}",
            config.version
        ));
    }
    let name = config.name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err("environment name must contain 1 to 120 printable characters".into());
    }
    validate_lifecycle("setup", &config.setup)?;
    if let Some(cleanup) = config.cleanup.as_ref() {
        validate_lifecycle("cleanup", cleanup)?;
    }
    if config.actions.len() > MAX_ACTIONS {
        return Err(format!(
            "local environments support at most {MAX_ACTIONS} actions"
        ));
    }
    for action in &config.actions {
        if action.name.trim().is_empty()
            || action.name.chars().count() > 120
            || action.name.chars().any(char::is_control)
        {
            return Err("action names must contain 1 to 120 printable characters".into());
        }
        if action.command.trim().is_empty() || action.command.len() > MAX_SCRIPT_BYTES {
            return Err("action commands must contain 1 to 262,144 bytes".into());
        }
        if !matches!(action.icon.as_str(), "tool" | "run" | "debug" | "test") {
            return Err(format!("unsupported action icon `{}`", action.icon));
        }
        if action
            .platform
            .as_deref()
            .is_some_and(|platform| !matches!(platform, "darwin" | "linux" | "win32"))
        {
            return Err("action platform must be darwin, linux, or win32".into());
        }
    }
    Ok(())
}

fn validate_lifecycle(label: &str, lifecycle: &LocalEnvironmentLifecycle) -> EnvironmentResult<()> {
    for (platform, script) in [
        ("default", Some(lifecycle.script.as_str())),
        (
            "darwin",
            lifecycle.darwin.as_ref().map(|value| value.script.as_str()),
        ),
        (
            "linux",
            lifecycle.linux.as_ref().map(|value| value.script.as_str()),
        ),
        (
            "win32",
            lifecycle.win32.as_ref().map(|value| value.script.as_str()),
        ),
    ] {
        if script.is_some_and(|script| script.len() > MAX_SCRIPT_BYTES) {
            return Err(format!("{label} {platform} script exceeds 262,144 bytes"));
        }
    }
    Ok(())
}

fn run_lifecycle(
    label: &str,
    lifecycle: &LocalEnvironmentLifecycle,
    cwd: &Path,
    worktree_root: Option<&Path>,
) -> EnvironmentResult<String> {
    let script = platform_script(lifecycle);
    if script.trim().is_empty() {
        return Ok(String::new());
    }
    let mut command = lifecycle_command(script)?;
    command.current_dir(cwd);
    if let Some(worktree_root) = worktree_root {
        command.env("CODEX_WORKTREE_PATH", worktree_root);
        command.env("HALV_WORKTREE_PATH", worktree_root);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run local-environment {label}: {error}"))?;
    lifecycle_output(label, output)
}

fn platform_script(lifecycle: &LocalEnvironmentLifecycle) -> &str {
    let platform = if cfg!(target_os = "macos") {
        lifecycle.darwin.as_ref()
    } else if cfg!(target_os = "windows") {
        lifecycle.win32.as_ref()
    } else {
        lifecycle.linux.as_ref()
    };
    platform
        .map(|value| value.script.as_str())
        .filter(|script| !script.trim().is_empty())
        .unwrap_or(&lifecycle.script)
}

fn lifecycle_command(script: &str) -> EnvironmentResult<Command> {
    #[cfg(windows)]
    {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        crate::process::hide_console(&mut command);
        Ok(command)
    }
    #[cfg(not(windows))]
    {
        let shell = env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let mut command = Command::new(&shell);
        let shell_name = shell
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("sh");
        if matches!(shell_name, "bash" | "zsh" | "ksh") {
            command.args(["-lc", script]);
        } else {
            command.args(["-c", script]);
        }
        Ok(command)
    }
}

fn lifecycle_output(label: &str, output: Output) -> EnvironmentResult<String> {
    let stdout = truncate_output(&output.stdout);
    let stderr = truncate_output(&output.stderr);
    let combined = match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => String::new(),
    };
    if output.status.success() {
        Ok(combined)
    } else if combined.is_empty() {
        Err(format!(
            "local-environment {label} failed with status {}",
            output.status
        ))
    } else {
        Err(format!("local-environment {label} failed: {combined}"))
    }
}

fn truncate_output(bytes: &[u8]) -> String {
    let keep = bytes.len().min(MAX_OUTPUT_BYTES);
    let mut value = String::from_utf8_lossy(&bytes[..keep]).trim().to_string();
    if keep < bytes.len() {
        value.push_str("\n… output truncated");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPOSITORY_ENVIRONMENT: &str = r#"version = 1
name = "Repository"

[setup]
script = "printf setup > setup.txt"

[cleanup]
script = "printf cleanup > cleanup.txt"

[[actions]]
name = "Run"
command = "npm run dev"
"#;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "halv-local-environment-{name}-{:032x}",
            rand::random::<u128>()
        ))
    }

    fn register_project(storage: &Storage, root: &Path) -> (PathBuf, i64) {
        fs::create_dir_all(root).expect("project should create");
        let root = root.canonicalize().expect("project should canonicalize");
        let project = storage
            .upsert_project("project", root.to_str().expect("path should be UTF-8"))
            .expect("project should insert");
        (root, project.id)
    }

    fn test_project(name: &str) -> (PathBuf, PathBuf, Storage, i64) {
        let base = test_root(name);
        fs::create_dir_all(&base).expect("base should create");
        let storage = Storage::open(base.join("halv.sqlite3")).expect("storage should open");
        let (root, project_id) = register_project(&storage, &base.join("project"));
        (base, root, storage, project_id)
    }

    // Writes the file that a cloned repository ships, without Halv's editor.
    fn write_environment(directory: &Path, contents: &str) -> String {
        let directory = directory.join(".codex/environments");
        fs::create_dir_all(&directory).expect("environment directory should create");
        let path = directory.join("environment.toml");
        fs::write(&path, contents).expect("environment should write");
        path.canonicalize()
            .expect("environment should canonicalize")
            .to_string_lossy()
            .into_owned()
    }

    fn listed_sha256(storage: &Storage, project_id: i64, root: &Path) -> (String, bool) {
        let listed =
            list_environments(storage, project_id, root).expect("environments should list");
        assert_eq!(listed.len(), 1);
        (
            listed[0]
                .content_sha256
                .clone()
                .expect("listed environment should have a hash"),
            listed[0].approved,
        )
    }

    fn sample_config() -> LocalEnvironmentConfig {
        LocalEnvironmentConfig {
            version: 1,
            name: "Local dev".to_string(),
            setup: LocalEnvironmentLifecycle {
                script: "printf default > default.txt".to_string(),
                darwin: Some(LocalEnvironmentPlatformScript {
                    script: "printf darwin > platform.txt".to_string(),
                }),
                ..Default::default()
            },
            cleanup: Some(LocalEnvironmentLifecycle {
                script: "printf cleanup > cleanup.txt".to_string(),
                ..Default::default()
            }),
            actions: vec![LocalEnvironmentAction {
                name: "Run".to_string(),
                icon: "run".to_string(),
                command: "npm run dev".to_string(),
                platform: None,
            }],
        }
    }

    #[test]
    fn official_toml_shape_round_trips_and_lists_inherited_configs() {
        let (parent, project, storage, project_id) = test_project("roundtrip");
        let saved = save_environment(&storage, project_id, &project, None, None, sample_config())
            .expect("environment should save");
        let raw = fs::read_to_string(&saved.config_path).expect("config should read");
        assert!(raw.contains("version = 1"));
        assert!(raw.contains("[setup.darwin]"));
        assert!(raw.contains("[[actions]]"));
        assert_eq!(
            read_environment_file(Path::new(&saved.config_path)).unwrap(),
            (sample_config(), sha256_hex(raw.as_bytes()))
        );
        assert_eq!(saved.content_sha256, Some(sha256_hex(raw.as_bytes())));
        assert!(!saved.approved);

        let inherited_dir = parent.join(".codex/environments");
        fs::create_dir_all(&inherited_dir).expect("inherited directory should create");
        fs::write(
            inherited_dir.join("parent.toml"),
            "version = 1\nname = \"Parent\"\n[setup]\nscript = \"\"\n",
        )
        .expect("inherited config should write");
        let listed =
            list_environments(&storage, project_id, &project).expect("environments should list");
        assert_eq!(listed.len(), 2);
        assert!(!listed[0].inherited);
        assert!(listed[1].inherited);
        assert_eq!(listed[0].content_sha256, saved.content_sha256);

        drop(storage);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn lifecycle_prefers_platform_override_and_runs_cleanup() {
        let root = test_root("lifecycle");
        fs::create_dir_all(&root).expect("root should create");
        let environment = LoadedLocalEnvironment {
            config_path: root.join("unused.toml"),
            config: sample_config(),
            content_sha256: String::new(),
            approved: true,
        };
        run_setup(&environment, &root).expect("setup should run");
        if cfg!(target_os = "macos") {
            assert_eq!(
                fs::read_to_string(root.join("platform.txt")).unwrap(),
                "darwin"
            );
            assert!(!root.join("default.txt").exists());
        } else {
            assert_eq!(
                fs::read_to_string(root.join("default.txt")).unwrap(),
                "default"
            );
        }
        run_cleanup(&environment, &root).expect("cleanup should run");
        assert_eq!(
            fs::read_to_string(root.join("cleanup.txt")).unwrap(),
            "cleanup"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn repository_scripts_do_not_run_without_an_approval() {
        let (base, root, storage, project_id) = test_project("unapproved");
        let config_path = write_environment(&root, REPOSITORY_ENVIRONMENT);
        let worktree = base.join("worktree");
        fs::create_dir_all(&worktree).expect("worktree should create");
        let (listed_hash, approved) = listed_sha256(&storage, project_id, &root);
        assert!(!approved);

        let environment = load_environment_for_project(&storage, project_id, &root, &config_path)
            .expect("environment should load");
        assert_eq!(
            environment.check_setup_approval(),
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        );
        assert_eq!(
            run_setup(&environment, &worktree),
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        );
        assert_eq!(
            run_cleanup(&environment, &worktree),
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        );
        assert_eq!(
            resolve_action(&environment, &listed_hash, 0),
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        );
        assert!(!worktree.join("setup.txt").exists());
        assert!(!worktree.join("cleanup.txt").exists());

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn an_approval_lets_the_approved_scripts_and_actions_run() {
        let (base, root, storage, project_id) = test_project("approved");
        let config_path = write_environment(&root, REPOSITORY_ENVIRONMENT);
        let worktree = base.join("worktree");
        fs::create_dir_all(&worktree).expect("worktree should create");
        let (listed_hash, _) = listed_sha256(&storage, project_id, &root);

        approve_environment(&storage, project_id, &root, &config_path, &listed_hash)
            .expect("approval should save");
        assert_eq!(
            listed_sha256(&storage, project_id, &root),
            (listed_hash.clone(), true)
        );
        let environment = load_environment_for_project(&storage, project_id, &root, &config_path)
            .expect("environment should load");
        environment
            .check_setup_approval()
            .expect("setup should be approved");
        run_setup(&environment, &worktree).expect("setup should run");
        run_cleanup(&environment, &worktree).expect("cleanup should run");
        assert_eq!(
            fs::read_to_string(worktree.join("setup.txt")).unwrap(),
            "setup"
        );
        assert_eq!(
            fs::read_to_string(worktree.join("cleanup.txt")).unwrap(),
            "cleanup"
        );
        let action = resolve_action(&environment, &listed_hash, 0).expect("action should resolve");
        assert_eq!(action.command, "npm run dev");
        assert!(resolve_action(&environment, &listed_hash, 1).is_err());

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn a_content_change_needs_a_new_approval() {
        let (base, root, storage, project_id) = test_project("changed");
        let config_path = write_environment(&root, REPOSITORY_ENVIRONMENT);
        let worktree = base.join("worktree");
        fs::create_dir_all(&worktree).expect("worktree should create");
        let (approved_hash, _) = listed_sha256(&storage, project_id, &root);
        approve_environment(&storage, project_id, &root, &config_path, &approved_hash)
            .expect("approval should save");

        // An upstream change replaces the approved script.
        write_environment(
            &root,
            &REPOSITORY_ENVIRONMENT
                .replace("printf setup > setup.txt", "printf changed > changed.txt"),
        );
        let (changed_hash, approved) = listed_sha256(&storage, project_id, &root);
        assert_ne!(changed_hash, approved_hash);
        assert!(!approved);
        let environment = load_environment_for_project(&storage, project_id, &root, &config_path)
            .expect("environment should load");
        assert_eq!(
            run_setup(&environment, &worktree),
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        );
        assert!(!worktree.join("changed.txt").exists());
        assert_eq!(
            resolve_action(&environment, &approved_hash, 0),
            Err(CONTENT_CHANGED_ERROR.to_string())
        );

        // The user cannot approve the new content with the hash of the old content.
        assert_eq!(
            approve_environment(&storage, project_id, &root, &config_path, &approved_hash),
            Err(CONTENT_CHANGED_ERROR.to_string())
        );
        assert!(!listed_sha256(&storage, project_id, &root).1);

        approve_environment(&storage, project_id, &root, &config_path, &changed_hash)
            .expect("approval of the new content should save");
        let environment = load_environment_for_project(&storage, project_id, &root, &config_path)
            .expect("environment should load");
        run_setup(&environment, &worktree).expect("approved setup should run");
        assert_eq!(
            fs::read_to_string(worktree.join("changed.txt")).unwrap(),
            "changed"
        );

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn an_environment_without_scripts_needs_no_approval() {
        let (base, root, storage, project_id) = test_project("no-scripts");
        let config_path = write_environment(
            &root,
            "version = 1\nname = \"Actions only\"\n[[actions]]\nname = \"Run\"\ncommand = \"npm run dev\"\n",
        );
        let environment = load_environment_for_project(&storage, project_id, &root, &config_path)
            .expect("environment should load");
        environment
            .check_setup_approval()
            .expect("an empty setup should need no approval");
        assert_eq!(run_setup(&environment, &root), Ok(String::new()));
        assert_eq!(run_cleanup(&environment, &root), Ok(String::new()));

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn approvals_belong_to_one_project() {
        let (base, first_root, storage, first_id) = test_project("project-scope");
        let (second_root, second_id) = register_project(&storage, &base.join("second"));
        // Both projects inherit the same file from their parent directory.
        let config_path = write_environment(&base, REPOSITORY_ENVIRONMENT);
        let (listed_hash, _) = listed_sha256(&storage, first_id, &first_root);
        approve_environment(&storage, first_id, &first_root, &config_path, &listed_hash)
            .expect("approval should save");

        assert!(listed_sha256(&storage, first_id, &first_root).1);
        assert!(!listed_sha256(&storage, second_id, &second_root).1);
        let environment =
            load_environment_for_project(&storage, second_id, &second_root, &config_path)
                .expect("environment should load");
        assert_eq!(
            environment.check_setup_approval(),
            Err(APPROVAL_REQUIRED_ERROR.to_string())
        );

        // A removed project takes its approvals with it, so a reused id starts clean.
        storage
            .remove_project(first_id)
            .expect("project should remove");
        assert_eq!(
            storage
                .local_environment_approval(first_id, &config_path)
                .expect("approval should query"),
            None
        );

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn direct_writes_cannot_escape_the_project_environment_directory() {
        let (base, root, storage, project_id) = test_project("escape");
        let outside = root.parent().unwrap().join("outside.toml");
        let error = save_environment(
            &storage,
            project_id,
            &root,
            Some(&outside.to_string_lossy()),
            None,
            sample_config(),
        )
        .expect_err("outside save must fail");
        assert!(error.contains(".codex/environments"));
        assert!(!outside.exists());
        drop(storage);
        let _ = fs::remove_dir_all(base);
    }
}
