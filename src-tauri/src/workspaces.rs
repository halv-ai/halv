use crate::{
    app_data,
    chat::ChatManager,
    exec, local_environments, projects,
    storage::{NewWorkspace, Storage, Workspace},
};
use futures_util::StreamExt;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Duration,
};

type WorkspaceResult<T> = Result<T, String>;
const MAX_TRANSFER_FILES: usize = 10_000;
const MAX_TRANSFER_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSFER_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const OVERVIEW_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const OVERVIEW_PROBE_LIMIT: usize = 8;

#[derive(Debug, Clone)]
struct TransferFile {
    relative_path: PathBuf,
    remove_from_source: bool,
    contents: Vec<u8>,
    permissions: fs::Permissions,
}

#[derive(Debug, Clone)]
struct TransferSnapshot {
    base_commit: String,
    source_root: PathBuf,
    staged_patch: Vec<u8>,
    unstaged_patch: Vec<u8>,
    files: Vec<TransferFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatus {
    #[serde(flatten)]
    pub workspace: Workspace,
    pub dirty: bool,
    pub conversation_count: u64,
    pub current_branch: Option<String>,
    pub available: bool,
    pub is_git_repo: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHandoffResult {
    pub workspace: WorkspaceStatus,
    pub warning: Option<String>,
}

/// One workspace of the Projects tree (ADR 0010). It has no `dirty` flag and
/// no conversation count: the tree does not show them, and `git status` is
/// too slow for every workspace of every project.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOverview {
    #[serde(flatten)]
    pub workspace: Workspace,
    pub current_branch: Option<String>,
    pub available: bool,
    pub is_git_repo: bool,
}

/// Lists the workspaces of all projects for the Projects tree.
#[tauri::command]
pub async fn workspace_overview(
    storage: tauri::State<'_, Storage>,
) -> WorkspaceResult<Vec<WorkspaceOverview>> {
    let workspaces = all_workspaces(storage.inner())?;
    Ok(probe_workspaces(workspaces, OVERVIEW_PROBE_TIMEOUT).await)
}

#[tauri::command]
pub async fn workspace_list(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
) -> WorkspaceResult<Vec<WorkspaceStatus>> {
    storage
        .list_workspaces(project_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|workspace| workspace_status(storage.inner(), workspace))
        .collect()
}

#[tauri::command]
pub async fn workspace_list_base_refs(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
) -> WorkspaceResult<Vec<String>> {
    workspace_base_refs(storage.inner(), project_id)
}

fn workspace_base_refs(storage: &Storage, project_id: i64) -> WorkspaceResult<Vec<String>> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = projects::validate_registered_project(&project)?;
    if !git_is_repository(&root)? {
        return Ok(Vec::new());
    }
    let output = managed_git(&root)?
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ])
        .output()
        .map_err(|error| format!("failed to list Git branches: {error}"))?;
    if !output.status.success() {
        return Err(git_error(output, "git for-each-ref"));
    }
    let mut refs = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned non-UTF-8 branch names".to_string())?
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.ends_with("/HEAD"))
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    refs.insert(0, "HEAD".to_string());
    Ok(refs)
}

#[tauri::command]
pub async fn workspace_current(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> WorkspaceResult<Option<WorkspaceStatus>> {
    storage
        .workspace_for_conversation(&conversation_id)
        .map_err(|error| error.to_string())?
        .map(|workspace| workspace_status(storage.inner(), workspace))
        .transpose()
}

#[tauri::command]
pub async fn workspace_create_worktree(
    app: tauri::AppHandle,
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    base_ref: Option<String>,
    branch: Option<String>,
    environment_config_path: Option<String>,
) -> WorkspaceResult<WorkspaceStatus> {
    create_managed_worktree(
        &app,
        storage.inner(),
        project_id,
        base_ref,
        branch,
        environment_config_path,
        None,
    )
}

fn create_managed_worktree(
    app: &tauri::AppHandle,
    storage: &Storage,
    project_id: i64,
    base_ref: Option<String>,
    branch: Option<String>,
    environment_config_path: Option<String>,
    transfer_override: Option<TransferSnapshot>,
) -> WorkspaceResult<WorkspaceStatus> {
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let project_root = projects::validate_registered_project(&project)?;
    let repository_root = git_repository_root(&project_root)?;
    if repository_root != project_root {
        return Err(
            "managed worktrees require the registered project to be the Git repository root"
                .to_string(),
        );
    }
    let environment = environment_config_path
        .as_deref()
        .map(|path| {
            local_environments::load_environment_for_project(
                storage,
                project_id,
                &project_root,
                path,
            )
        })
        .transpose()?;
    if let Some(environment) = environment.as_ref() {
        // Refuse an unapproved setup script before Git creates the worktree.
        environment.check_setup_approval()?;
    }

    let base_ref = normalize_base_ref(base_ref.as_deref())?;
    let base_commit = git_text(
        &project_root,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base_ref}^{{commit}}"),
        ],
    )?;
    let transfer = match transfer_override {
        Some(transfer)
            if transfer.base_commit == base_commit && transfer.source_root == project_root =>
        {
            transfer
        }
        Some(_) => {
            return Err(
                "handoff snapshot no longer matches the selected worktree base".to_string(),
            );
        }
        None => {
            let source_head = git_text(&project_root, ["rev-parse", "HEAD"])?;
            capture_transfer(&project_root, &base_commit, source_head == base_commit)?
        }
    };
    let common_dir = git_common_dir(&project_root)?;
    let suffix = format!("{:08x}", rand::random::<u32>());
    let branch = branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    if let Some(branch) = branch.as_deref() {
        validate_branch(&project_root, branch)?;
    }
    let workspace_name = branch
        .clone()
        .unwrap_or_else(|| format!("Worktree {suffix}"));

    let managed_root = app_data::resolved_app_data_dir(app)
        .map_err(|error| format!("failed to locate Halv app data: {error}"))?
        .join("worktrees")
        .join(&suffix);
    fs::create_dir_all(&managed_root)
        .map_err(|error| format!("failed to create managed worktree directory: {error}"))?;
    let destination = managed_root.join(safe_component(&project.name));
    if destination.exists() {
        return Err("managed worktree destination already exists".to_string());
    }

    let mut worktree_add = managed_git(&project_root)?;
    worktree_add.args(["worktree", "add"]);
    if let Some(branch) = branch.as_deref() {
        worktree_add.args(["-b", branch]);
    } else {
        worktree_add.arg("--detach");
    }
    let output = worktree_add
        .arg(&destination)
        .arg(&base_commit)
        .output()
        .map_err(|error| format!("failed to run git worktree add: {error}"))?;
    ensure_git_success(output, "git worktree add")?;

    let canonical_destination = destination
        .canonicalize()
        .map_err(|error| format!("failed to validate managed worktree: {error}"))?;
    if let Err(error) = apply_transfer(&transfer, &canonical_destination) {
        let _ = force_remove_worktree(&project_root, &canonical_destination);
        let _ = fs::remove_dir_all(&managed_root);
        return Err(format!(
            "failed to copy local changes into worktree: {error}"
        ));
    }
    if let Some(environment) = environment.as_ref() {
        if let Err(error) = local_environments::run_setup(environment, &canonical_destination) {
            let _ = local_environments::run_cleanup(environment, &canonical_destination);
            let _ = managed_git(&project_root).and_then(|mut command| {
                command
                    .args(["worktree", "remove", "--force"])
                    .arg(&canonical_destination);
                command
                    .output()
                    .map_err(|cleanup_error| cleanup_error.to_string())
                    .and_then(|output| ensure_git_success(output, "worktree cleanup"))
            });
            let _ = fs::remove_dir_all(&managed_root);
            return Err(error);
        }
    }
    let saved_environment_config_path = environment
        .as_ref()
        .map(|environment| environment.config_path.to_string_lossy().into_owned());
    let workspace_result = storage.insert_managed_workspace(NewWorkspace {
        project_id,
        name: &workspace_name,
        root_path: &canonical_destination.to_string_lossy(),
        git_common_dir: Some(&common_dir.to_string_lossy()),
        base_ref: Some(&base_ref),
        base_commit: Some(&base_commit),
        branch: branch.as_deref(),
        local_environment_config_path: saved_environment_config_path.as_deref(),
    });
    let workspace = match workspace_result {
        Ok(workspace) => workspace,
        Err(error) => {
            if let Some(environment) = environment.as_ref() {
                let _ = local_environments::run_cleanup(environment, &canonical_destination);
            }
            let _ = managed_git(&project_root).and_then(|mut command| {
                command
                    .args(["worktree", "remove", "--force"])
                    .arg(&canonical_destination);
                command
                    .output()
                    .map_err(|cleanup_error| cleanup_error.to_string())
                    .and_then(|output| ensure_git_success(output, "worktree cleanup"))
            });
            let _ = fs::remove_dir_all(&managed_root);
            return Err(error.to_string());
        }
    };
    workspace_status(storage, workspace)
}

#[tauri::command]
pub async fn workspace_create_branch(
    storage: tauri::State<'_, Storage>,
    workspace_id: String,
    branch: String,
) -> WorkspaceResult<WorkspaceStatus> {
    let workspace = storage
        .get_workspace(&workspace_id)
        .map_err(|error| error.to_string())?;
    if workspace.kind != "worktree" || !workspace.managed {
        return Err("branches can be created only in managed worktrees".to_string());
    }
    let root = PathBuf::from(&workspace.root_path);
    if !root.is_dir() {
        return Err("managed worktree is missing from disk".to_string());
    }
    let branch = branch.trim();
    validate_branch(&root, branch)?;
    if git_optional_text(&root, ["symbolic-ref", "--quiet", "--short", "HEAD"])?.is_some() {
        return Err("this worktree already has a branch".to_string());
    }
    let output = managed_git(&root)?
        .args(["switch", "-c", branch])
        .output()
        .map_err(|error| format!("failed to create worktree branch: {error}"))?;
    ensure_git_success(output, "git switch -c")?;
    let workspace = storage
        .set_workspace_branch(&workspace_id, branch)
        .map_err(|error| error.to_string())?;
    workspace_status(storage.inner(), workspace)
}

#[tauri::command]
pub async fn workspace_handoff(
    app: tauri::AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    target: String,
    environment_config_path: Option<String>,
) -> WorkspaceResult<WorkspaceHandoffResult> {
    if chat.is_in_flight(&conversation_id) {
        return Err("wait for the active turn to finish before handing it off".to_string());
    }
    let current = storage
        .workspace_for_conversation(&conversation_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "conversation has no active workspace".to_string())?;
    let project = storage
        .get_project(current.project_id)
        .map_err(|error| error.to_string())?;
    let _project_root = projects::validate_registered_project(&project)?;
    let primary = storage
        .list_workspaces(project.id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|workspace| workspace.kind == "primary")
        .ok_or_else(|| "project has no local checkout".to_string())?;
    let mut associated = if current.kind == "worktree" {
        Some(current.clone())
    } else {
        storage
            .associated_worktree_for_conversation(&conversation_id)
            .map_err(|error| error.to_string())?
    };

    let normalized_target = target.trim().to_ascii_lowercase();
    if !matches!(normalized_target.as_str(), "local" | "worktree") {
        return Err("handoff target must be local or worktree".to_string());
    }
    if (normalized_target == "local" && current.kind == "primary")
        || (normalized_target == "worktree" && current.kind == "worktree")
    {
        return Ok(WorkspaceHandoffResult {
            workspace: workspace_status(storage.inner(), current)?,
            warning: None,
        });
    }

    let (target_workspace, snapshot) = if normalized_target == "worktree" && associated.is_none() {
        if current.kind != "primary" {
            return Err("only a local task can create its associated worktree".to_string());
        }
        let base_commit = git_text(Path::new(&current.root_path), ["rev-parse", "HEAD"])?;
        let snapshot = capture_transfer(Path::new(&current.root_path), &base_commit, true)?;
        let created = create_managed_worktree(
            &app,
            storage.inner(),
            project.id,
            Some("HEAD".to_string()),
            None,
            environment_config_path,
            Some(snapshot.clone()),
        )?;
        let workspace = storage
            .get_workspace(&created.workspace.id)
            .map_err(|error| error.to_string())?;
        associated = Some(workspace.clone());
        (workspace, snapshot)
    } else {
        let associated = associated
            .as_ref()
            .ok_or_else(|| "conversation has no associated worktree".to_string())?;
        let target_workspace = if normalized_target == "local" {
            primary.clone()
        } else {
            associated.clone()
        };
        if !Path::new(&target_workspace.root_path).is_dir() {
            return Err("handoff destination is missing from disk".to_string());
        }
        let base_commit = associated
            .base_commit
            .as_deref()
            .ok_or_else(|| "associated worktree has no recorded base commit".to_string())?;
        let source_root = Path::new(&current.root_path);
        let source_head = git_text(source_root, ["rev-parse", "HEAD"])?;
        if source_head != base_commit && (current.kind == "primary" || current.branch.is_some()) {
            return Err(
                "handoff cannot rewind commits on a checked-out branch; move or commit those changes separately"
                    .to_string(),
            );
        }
        let snapshot = capture_transfer(source_root, base_commit, true)?;
        apply_transfer(&snapshot, Path::new(&target_workspace.root_path))?;
        (target_workspace, snapshot)
    };

    let associated = associated
        .as_ref()
        .ok_or_else(|| "handoff failed to establish an associated worktree".to_string())?;
    let target_workspace = storage
        .handoff_conversation_workspace(&conversation_id, &target_workspace.id, &associated.id)
        .map_err(|error| error.to_string())?;
    let warning = clear_transfer_source(&snapshot).err().map(|error| {
        format!("Handoff completed, but the source checkout was left unchanged: {error}")
    });
    Ok(WorkspaceHandoffResult {
        workspace: workspace_status(storage.inner(), target_workspace)?,
        warning,
    })
}

#[tauri::command]
pub async fn workspace_set_pinned(
    storage: tauri::State<'_, Storage>,
    workspace_id: String,
    pinned: bool,
) -> WorkspaceResult<WorkspaceStatus> {
    let workspace = storage
        .set_workspace_pinned(&workspace_id, pinned)
        .map_err(|error| error.to_string())?;
    workspace_status(storage.inner(), workspace)
}

#[tauri::command]
pub async fn workspace_bind_conversation(
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    workspace_id: String,
) -> WorkspaceResult<WorkspaceStatus> {
    if chat.is_in_flight(&conversation_id) {
        return Err("wait for the active turn to finish before changing workspaces".to_string());
    }
    if !storage
        .get_messages(&conversation_id)
        .map_err(|error| error.to_string())?
        .is_empty()
    {
        return Err(
            "a conversation can change workspaces only before its first message; start a new chat instead"
                .to_string(),
        );
    }
    let workspace = storage
        .bind_conversation_workspace(&conversation_id, &workspace_id)
        .map_err(|error| error.to_string())?;
    workspace_status(storage.inner(), workspace)
}

#[tauri::command]
pub async fn workspace_remove(
    app: tauri::AppHandle,
    storage: tauri::State<'_, Storage>,
    workspace_id: String,
) -> WorkspaceResult<()> {
    let workspace = storage
        .get_workspace(&workspace_id)
        .map_err(|error| error.to_string())?;
    if workspace.kind != "worktree" || !workspace.managed {
        return Err("the primary workspace cannot be removed".to_string());
    }
    if workspace.pinned {
        return Err("unpin the workspace before removing it".to_string());
    }
    if storage
        .workspace_conversation_count(&workspace_id)
        .map_err(|error| error.to_string())?
        > 0
    {
        return Err("workspace is still attached to a conversation".to_string());
    }
    let project = storage
        .get_project(workspace.project_id)
        .map_err(|error| error.to_string())?;
    let project_root = projects::validate_registered_project(&project)?;
    let workspace_root = PathBuf::from(&workspace.root_path);
    validate_managed_workspace_path(&app, &workspace_root)?;
    if workspace_root.exists() && git_dirty(&workspace_root)? {
        return Err("workspace has uncommitted changes and will not be removed".to_string());
    }

    if workspace_root.exists() {
        if let Some(config_path) = workspace
            .local_environment_config_path
            .as_deref()
            .filter(|path| Path::new(path).is_file())
        {
            let environment = local_environments::load_environment_for_project(
                storage.inner(),
                project.id,
                &project_root,
                config_path,
            )?;
            local_environments::run_cleanup(&environment, &workspace_root)?;
        }
        let output = managed_git(&project_root)?
            .args(["worktree", "remove"])
            .arg(&workspace_root)
            .output()
            .map_err(|error| format!("failed to run git worktree remove: {error}"))?;
        ensure_git_success(output, "git worktree remove")?;
    } else {
        let output = managed_git(&project_root)?
            .args(["worktree", "prune"])
            .output()
            .map_err(|error| format!("failed to prune missing worktree: {error}"))?;
        ensure_git_success(output, "git worktree prune")?;
    }
    storage
        .delete_managed_workspace(&workspace_id)
        .map_err(|error| error.to_string())?;
    if let Some(parent) = workspace_root.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

fn all_workspaces(storage: &Storage) -> WorkspaceResult<Vec<Workspace>> {
    let mut workspaces = Vec::new();
    for project in storage.list_projects().map_err(|error| error.to_string())? {
        // The list also creates a missing primary workspace row.
        workspaces.extend(
            storage
                .list_workspaces(project.id)
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(workspaces)
}

// The probes run in parallel, at most OVERVIEW_PROBE_LIMIT at a time. The
// result keeps the storage order.
async fn probe_workspaces(workspaces: Vec<Workspace>, timeout: Duration) -> Vec<WorkspaceOverview> {
    futures_util::stream::iter(
        workspaces
            .into_iter()
            .map(|workspace| probe_workspace(workspace, timeout)),
    )
    .buffered(OVERVIEW_PROBE_LIMIT)
    .collect()
    .await
}

async fn probe_workspace(mut workspace: Workspace, timeout: Duration) -> WorkspaceOverview {
    let root = PathBuf::from(&workspace.root_path);
    let available = root.is_dir();
    if !available {
        workspace.state = "missing".to_string();
        return WorkspaceOverview {
            workspace,
            current_branch: None,
            available,
            is_git_repo: false,
        };
    }
    let (is_git_repo, current_branch) = match head_branch_output(&root, timeout).await {
        Some(output) => head_branch_from_output(&output),
        None => (false, None),
    };
    WorkspaceOverview {
        workspace,
        current_branch,
        available,
        is_git_repo,
    }
}

// One cheap probe: `git rev-parse --abbrev-ref HEAD`. A timeout or a spawn
// error gives None, and the tree then shows the stored branch or name.
async fn head_branch_output(root: &Path, timeout: Duration) -> Option<Output> {
    let mut command = tokio::process::Command::from(managed_git(root).ok()?);
    command
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()
}

/// Returns (is a Git repository, current branch). A detached HEAD and a
/// branch with no commit have no branch name.
fn head_branch_from_output(output: &Output) -> (bool, Option<String>) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if output.status.success() {
        let branch = (!stdout.is_empty() && stdout != "HEAD").then(|| stdout.to_string());
        return (true, branch);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not a git repository") {
        return (false, None);
    }
    // An unborn branch fails with "ambiguous argument 'HEAD'".
    (stderr.contains("ambiguous argument"), None)
}

fn workspace_status(
    storage: &Storage,
    mut workspace: Workspace,
) -> WorkspaceResult<WorkspaceStatus> {
    let root = Path::new(&workspace.root_path);
    let available = root.is_dir();
    if !available {
        workspace.state = "missing".to_string();
    }
    let is_git_repo = available && git_is_repository(root)?;
    let dirty = is_git_repo && git_dirty(root)?;
    let current_branch = if is_git_repo {
        git_optional_text(root, ["symbolic-ref", "--quiet", "--short", "HEAD"])?
    } else {
        None
    };
    let conversation_count = storage
        .workspace_conversation_count(&workspace.id)
        .map_err(|error| error.to_string())?;
    Ok(WorkspaceStatus {
        workspace,
        dirty,
        conversation_count,
        current_branch,
        available,
        is_git_repo,
    })
}

fn capture_transfer(
    source_root: &Path,
    base_commit: &str,
    include_git_changes: bool,
) -> WorkspaceResult<TransferSnapshot> {
    let source_root = source_root
        .canonicalize()
        .map_err(|error| format!("failed to validate transfer source: {error}"))?;
    let staged_patch = if include_git_changes {
        git_bytes(
            &source_root,
            [
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--binary",
                "--full-index",
                base_commit,
                "--",
            ],
        )?
    } else {
        Vec::new()
    };
    let unstaged_patch = if include_git_changes {
        git_bytes(
            &source_root,
            [
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--binary",
                "--full-index",
                "--",
            ],
        )?
    } else {
        Vec::new()
    };

    let mut ordinary = BTreeSet::new();
    if include_git_changes {
        ordinary.extend(git_paths(
            &source_root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?);
    }

    let mut included_ignored = BTreeSet::new();
    if source_root.join(".worktreeinclude").is_file() {
        included_ignored.extend(git_paths(
            &source_root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-from=.worktreeinclude",
                "-z",
            ],
        )?);
    }
    let agents_override = PathBuf::from("AGENTS.override.md");
    if source_root.join(&agents_override).is_file() && !git_tracked(&source_root, &agents_override)?
    {
        included_ignored.insert(agents_override);
    }

    let all_paths = ordinary
        .iter()
        .cloned()
        .chain(included_ignored.iter().cloned())
        .collect::<BTreeSet<_>>();
    if all_paths.len() > MAX_TRANSFER_FILES {
        return Err(format!(
            "worktree transfer exceeds the {MAX_TRANSFER_FILES}-file safety limit"
        ));
    }
    let mut total_bytes = staged_patch.len() as u64 + unstaged_patch.len() as u64;
    let mut files = Vec::with_capacity(all_paths.len());
    for relative_path in all_paths {
        validate_transfer_relative_path(&relative_path)?;
        let source_path = source_root.join(&relative_path);
        let metadata = match fs::symlink_metadata(&source_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "failed to inspect transfer file {}: {error}",
                    relative_path.display()
                ));
            }
        };
        // Codex-managed worktrees deliberately do not copy source symlinks.
        if metadata.file_type().is_symlink() {
            continue;
        }
        if !metadata.is_file() {
            return Err(format!(
                "worktree transfer supports regular files only: {}",
                relative_path.display()
            ));
        }
        if metadata.len() > MAX_TRANSFER_FILE_BYTES {
            return Err(format!(
                "{} exceeds the 64 MB worktree-transfer file limit",
                relative_path.display()
            ));
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_TRANSFER_TOTAL_BYTES {
            return Err("worktree transfer exceeds the 512 MB safety limit".to_string());
        }
        files.push(TransferFile {
            contents: fs::read(&source_path).map_err(|error| {
                format!(
                    "failed to read transfer file {}: {error}",
                    relative_path.display()
                )
            })?,
            permissions: metadata.permissions(),
            remove_from_source: ordinary.contains(&relative_path),
            relative_path,
        });
    }

    Ok(TransferSnapshot {
        base_commit: base_commit.to_string(),
        source_root,
        staged_patch,
        unstaged_patch,
        files,
    })
}

fn apply_transfer(snapshot: &TransferSnapshot, target_root: &Path) -> WorkspaceResult<()> {
    let target_root = target_root
        .canonicalize()
        .map_err(|error| format!("failed to validate transfer destination: {error}"))?;
    if git_text(&target_root, ["rev-parse", "HEAD"])? != snapshot.base_commit {
        return Err("the destination checkout moved away from the task base commit".to_string());
    }
    if git_dirty(&target_root)? {
        return Err("the destination checkout has uncommitted changes".to_string());
    }

    let mut copy_plan = Vec::new();
    for file in &snapshot.files {
        let destination = transfer_target_path(&target_root, &file.relative_path)?;
        if destination.exists() {
            // Matches Codex's .worktreeinclude behavior: local ignored files are
            // useful defaults, but they never overwrite a destination file.
            continue;
        }
        copy_plan.push((file, destination));
    }

    if let Err(error) = apply_git_patch(&target_root, &snapshot.staged_patch, true) {
        rollback_transfer_target(&target_root, &snapshot.base_commit, &[]);
        return Err(error);
    }
    if let Err(error) = apply_git_patch(&target_root, &snapshot.unstaged_patch, false) {
        rollback_transfer_target(&target_root, &snapshot.base_commit, &[]);
        return Err(error);
    }

    let mut copied = Vec::new();
    for (file, destination) in copy_plan {
        if let Some(parent) = destination.parent() {
            if let Err(error) = create_transfer_directories(&target_root, parent) {
                rollback_transfer_target(&target_root, &snapshot.base_commit, &copied);
                return Err(error);
            }
        }
        if let Err(error) = fs::write(&destination, &file.contents) {
            rollback_transfer_target(&target_root, &snapshot.base_commit, &copied);
            return Err(format!(
                "failed to copy {}: {error}",
                file.relative_path.display()
            ));
        }
        if let Err(error) = fs::set_permissions(&destination, file.permissions.clone()) {
            let _ = fs::remove_file(&destination);
            rollback_transfer_target(&target_root, &snapshot.base_commit, &copied);
            return Err(format!(
                "failed to preserve permissions for {}: {error}",
                file.relative_path.display()
            ));
        }
        copied.push(destination);
    }
    Ok(())
}

fn clear_transfer_source(snapshot: &TransferSnapshot) -> WorkspaceResult<()> {
    let current_staged = git_bytes(
        &snapshot.source_root,
        [
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=dirty",
            "--binary",
            "--full-index",
            &snapshot.base_commit,
            "--",
        ],
    )?;
    let current_unstaged = git_bytes(
        &snapshot.source_root,
        [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=dirty",
            "--binary",
            "--full-index",
            "--",
        ],
    )?;
    if current_staged != snapshot.staged_patch || current_unstaged != snapshot.unstaged_patch {
        return Err(
            "the source checkout changed during handoff; its newer changes were left in place"
                .to_string(),
        );
    }
    for file in snapshot.files.iter().filter(|file| file.remove_from_source) {
        let current =
            fs::read(snapshot.source_root.join(&file.relative_path)).map_err(|error| {
                format!(
                    "the source file {} changed during handoff: {error}",
                    file.relative_path.display()
                )
            })?;
        if current != file.contents {
            return Err(format!(
                "the source file {} changed during handoff; it was left in place",
                file.relative_path.display()
            ));
        }
    }
    let output = managed_git(&snapshot.source_root)?
        .args(["reset", "--hard", &snapshot.base_commit])
        .output()
        .map_err(|error| format!("failed to reset handoff source: {error}"))?;
    ensure_git_success(output, "git reset after handoff")?;
    for file in snapshot.files.iter().filter(|file| file.remove_from_source) {
        let path = snapshot.source_root.join(&file.relative_path);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(&path).map_err(|error| {
                    format!(
                        "handoff copied changes but could not clean {}: {error}",
                        file.relative_path.display()
                    )
                })?;
                remove_empty_transfer_parents(&snapshot.source_root, path.parent());
            }
            Ok(_) => {
                return Err(format!(
                    "handoff copied changes but refused to remove non-file {}",
                    file.relative_path.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "handoff copied changes but could not inspect {}: {error}",
                    file.relative_path.display()
                ));
            }
        }
    }
    Ok(())
}

fn apply_git_patch(root: &Path, patch: &[u8], index: bool) -> WorkspaceResult<()> {
    if patch.is_empty() {
        return Ok(());
    }
    let mut command = managed_git(root)?;
    command.args(["apply", "--binary", "--whitespace=nowarn"]);
    if index {
        command.arg("--index");
    }
    let mut child = command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start git apply: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "failed to open git apply input".to_string())?
        .write_all(patch)
        .map_err(|error| format!("failed to send patch to git apply: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for git apply: {error}"))?;
    ensure_git_success(output, "git apply")
}

fn rollback_transfer_target(root: &Path, base_commit: &str, copied: &[PathBuf]) {
    let _ = managed_git(root).and_then(|mut command| {
        command.args(["reset", "--hard", base_commit]);
        command
            .output()
            .map_err(|error| error.to_string())
            .and_then(|output| ensure_git_success(output, "handoff rollback"))
    });
    for path in copied.iter().rev() {
        let _ = fs::remove_file(path);
        remove_empty_transfer_parents(root, path.parent());
    }
}

fn transfer_target_path(root: &Path, relative_path: &Path) -> WorkspaceResult<PathBuf> {
    validate_transfer_relative_path(relative_path)?;
    let mut current = root.to_path_buf();
    let components = relative_path.components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let std::path::Component::Normal(component) = component else {
            return Err("invalid worktree-transfer path".to_string());
        };
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "worktree-transfer parent is not a regular directory: {}",
                    current.display()
                ));
            }
        }
    }
    Ok(root.join(relative_path))
}

fn create_transfer_directories(root: &Path, parent: &Path) -> WorkspaceResult<()> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "worktree-transfer destination escaped its checkout".to_string())?;
    let _ = transfer_target_path(root, &relative.join("placeholder"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create transfer directory: {error}"))
}

fn validate_transfer_relative_path(path: &Path) -> WorkspaceResult<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "invalid worktree-transfer path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn remove_empty_transfer_parents(root: &Path, mut parent: Option<&Path>) {
    while let Some(path) = parent {
        if path == root || !path.starts_with(root) {
            break;
        }
        if fs::remove_dir(path).is_err() {
            break;
        }
        parent = path.parent();
    }
}

fn git_paths(root: &Path, args: &[&str]) -> WorkspaceResult<Vec<PathBuf>> {
    let bytes = git_bytes(root, args.iter().copied())?;
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            String::from_utf8(path.to_vec())
                .map(PathBuf::from)
                .map_err(|_| "worktree transfer does not support non-UTF-8 paths".to_string())
        })
        .collect()
}

fn git_tracked(root: &Path, relative_path: &Path) -> WorkspaceResult<bool> {
    let output = managed_git(root)?
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative_path)
        .output()
        .map_err(|error| format!("failed to inspect tracked files: {error}"))?;
    Ok(output.status.success())
}

fn force_remove_worktree(project_root: &Path, worktree_root: &Path) -> WorkspaceResult<()> {
    let output = managed_git(project_root)?
        .args(["worktree", "remove", "--force"])
        .arg(worktree_root)
        .output()
        .map_err(|error| format!("failed to clean up worktree: {error}"))?;
    ensure_git_success(output, "worktree cleanup")
}

fn git_repository_root(root: &Path) -> WorkspaceResult<PathBuf> {
    let path = git_text(root, ["rev-parse", "--show-toplevel"])?;
    PathBuf::from(path)
        .canonicalize()
        .map_err(|error| format!("failed to validate Git repository root: {error}"))
}

fn git_common_dir(root: &Path) -> WorkspaceResult<PathBuf> {
    let path = PathBuf::from(git_text(root, ["rev-parse", "--git-common-dir"])?);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    path.canonicalize()
        .map_err(|error| format!("failed to validate Git common directory: {error}"))
}

fn git_is_repository(root: &Path) -> WorkspaceResult<bool> {
    let output = managed_git(root)?
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|error| format!("failed to inspect Git repository: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(false);
        }
        return Err(git_error(output, "git rev-parse"));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| "git returned non-UTF-8 repository status".to_string())?;
    Ok(value.trim() == "true")
}

fn git_dirty(root: &Path) -> WorkspaceResult<bool> {
    let output = managed_git(root)?
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=dirty",
        ])
        .output()
        .map_err(|error| format!("failed to inspect workspace status: {error}"))?;
    if !output.status.success() {
        return Err(git_error(output, "git status"));
    }
    Ok(!output.stdout.is_empty())
}

fn validate_branch(root: &Path, branch: &str) -> WorkspaceResult<()> {
    if branch.len() > 200 || branch.starts_with('-') || branch.chars().any(char::is_control) {
        return Err("invalid worktree branch name".to_string());
    }
    let output = managed_git(root)?
        .args(["check-ref-format", "--branch", branch])
        .output()
        .map_err(|error| format!("failed to validate branch: {error}"))?;
    ensure_git_success(output, "git check-ref-format")
}

fn normalize_base_ref(base_ref: Option<&str>) -> WorkspaceResult<String> {
    let base_ref = base_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("HEAD");
    if base_ref.len() > 200 || base_ref.starts_with('-') || base_ref.chars().any(char::is_control) {
        return Err("invalid base ref".to_string());
    }
    Ok(base_ref.to_string())
}

fn validate_managed_workspace_path(app: &tauri::AppHandle, root: &Path) -> WorkspaceResult<()> {
    let managed_root = app_data::resolved_app_data_dir(app)
        .map_err(|error| format!("failed to locate Halv app data: {error}"))?
        .join("worktrees");
    fs::create_dir_all(&managed_root)
        .map_err(|error| format!("failed to validate managed worktree root: {error}"))?;
    let managed_root = managed_root
        .canonicalize()
        .map_err(|error| format!("failed to validate managed worktree root: {error}"))?;
    let candidate = if root.exists() {
        root.canonicalize()
            .map_err(|error| format!("failed to validate workspace path: {error}"))?
    } else {
        root.to_path_buf()
    };
    if !candidate.starts_with(&managed_root) {
        return Err("refusing to remove a workspace outside Halv app data".to_string());
    }
    Ok(())
}

fn safe_component(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    if value.is_empty() {
        "workspace".to_string()
    } else {
        value
    }
}

fn managed_git(cwd: &Path) -> WorkspaceResult<Command> {
    exec::managed_git_command(cwd)
}

fn git_bytes<'a>(root: &Path, args: impl IntoIterator<Item = &'a str>) -> WorkspaceResult<Vec<u8>> {
    let output = managed_git(root)?
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(git_error(output, "git"));
    }
    Ok(output.stdout)
}

fn git_text<'a>(root: &Path, args: impl IntoIterator<Item = &'a str>) -> WorkspaceResult<String> {
    let value = String::from_utf8(git_bytes(root, args)?)
        .map_err(|_| "git returned non-UTF-8 output".to_string())?;
    Ok(value.trim().to_string())
}

fn git_optional_text<'a>(
    root: &Path,
    args: impl IntoIterator<Item = &'a str>,
) -> WorkspaceResult<Option<String>> {
    let output = managed_git(root)?
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| "git returned non-UTF-8 output".to_string())?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_string()))
}

fn ensure_git_success(output: Output, label: &str) -> WorkspaceResult<()> {
    if output.status.success() {
        Ok(())
    } else {
        Err(git_error(output, label))
    }
}

fn git_error(output: Output, label: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{label} failed with exit status {}", output.status)
    } else {
        format!("{label} failed: {stderr}")
    }
}

/// The targets of "Open in Finder" and "Open in editor" in the Projects
/// tree (ADR 0010, stage S7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceOpenTarget {
    FileManager,
    Editor,
}

/// Opens the root of a workspace in the file manager or in a code editor.
/// The command takes a workspace id and never a raw path, so it opens only
/// a registered workspace root.
#[tauri::command]
pub async fn workspace_open_in(
    storage: tauri::State<'_, Storage>,
    workspace_id: String,
    target: WorkspaceOpenTarget,
) -> WorkspaceResult<()> {
    let workspace = storage
        .get_workspace(&workspace_id)
        .map_err(|error| error.to_string())?;
    let root = PathBuf::from(&workspace.root_path);
    if !root.is_dir() {
        return Err("workspace is no longer available on disk".to_string());
    }
    match target {
        WorkspaceOpenTarget::FileManager => tauri_plugin_opener::open_path(&root, None::<&str>)
            .map_err(|error| format!("failed to open the folder: {error}")),
        WorkspaceOpenTarget::Editor => open_in_editor(&root),
    }
}

/// The code editors that "Open in editor" tries, in this order. On macOS
/// they are app bundles; elsewhere they are commands on PATH.
const EDITOR_APPS: &[&str] = &[
    "Visual Studio Code",
    "Cursor",
    "Windsurf",
    "Zed",
    "Sublime Text",
    "Nova",
    "BBEdit",
];
const EDITOR_COMMANDS: &[&str] = &["code", "cursor", "windsurf", "zed", "subl"];
const NO_EDITOR_ERROR: &str = "no code editor was found; install Visual Studio Code, Cursor or Zed";

/// The first installed editor app bundle in the given folders.
fn first_editor_app(dirs: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    EDITOR_APPS.iter().find_map(|name| {
        dirs.iter()
            .map(|dir| dir.join(format!("{name}.app")))
            .find(|candidate| exists(candidate))
    })
}

/// The first editor command on a PATH value. Windows also tries the
/// `.cmd` and `.exe` forms.
fn first_editor_command(path: &std::ffi::OsStr, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let suffixes: &[&str] = if cfg!(windows) {
        &[".cmd", ".exe", ""]
    } else {
        &[""]
    };
    let dirs = std::env::split_paths(path).collect::<Vec<_>>();
    EDITOR_COMMANDS.iter().find_map(|name| {
        dirs.iter().find_map(|dir| {
            suffixes
                .iter()
                .map(|suffix| dir.join(format!("{name}{suffix}")))
                .find(|candidate| exists(candidate))
        })
    })
}

fn open_in_editor(root: &Path) -> WorkspaceResult<()> {
    if cfg!(target_os = "macos") {
        let mut dirs = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications"));
        }
        let app = first_editor_app(&dirs, Path::is_dir).ok_or(NO_EDITOR_ERROR)?;
        let status = Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&app)
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("failed to open the editor: {error}"))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("failed to open {}", app.display()))
        };
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let command = first_editor_command(&path, Path::is_file).ok_or(NO_EDITOR_ERROR)?;
    let mut command = Command::new(command);
    command
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The editor shows its own window. The flag only hides the console of
    // a `.cmd` shim such as `code.cmd`.
    crate::process::hide_console(&mut command)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open the editor: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_editor_takes_the_first_installed_editor_in_order() {
        let dirs = [
            PathBuf::from("/Applications"),
            PathBuf::from("/Users/u/Applications"),
        ];
        let installed = |found: &'static [&'static str]| {
            move |path: &Path| found.iter().any(|item| Path::new(item) == path)
        };
        assert_eq!(
            first_editor_app(
                &dirs,
                installed(&["/Users/u/Applications/Zed.app", "/Applications/Cursor.app"])
            ),
            Some(PathBuf::from("/Applications/Cursor.app"))
        );
        assert_eq!(
            first_editor_app(&dirs, installed(&["/Users/u/Applications/Zed.app"])),
            Some(PathBuf::from("/Users/u/Applications/Zed.app"))
        );
        assert_eq!(first_editor_app(&dirs, installed(&[])), None);

        let path = std::env::join_paths(["/usr/bin", "/opt/bin"]).unwrap();
        let suffix = if cfg!(windows) { ".cmd" } else { "" };
        let zed = format!("/usr/bin/zed{suffix}");
        let code = format!("/opt/bin/code{suffix}");
        let found = [zed.clone(), code.clone()];
        assert_eq!(
            first_editor_command(&path, |candidate| found
                .iter()
                .any(|item| Path::new(item) == candidate)),
            Some(PathBuf::from(code))
        );
        assert_eq!(first_editor_command(&path, |_| false), None);
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "halv-workspace-{name}-{:032x}",
            rand::random::<u128>()
        ))
    }

    fn run_git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn base_refs_and_branch_components_reject_option_injection() {
        assert!(normalize_base_ref(Some("--upload-pack=evil")).is_err());
        assert!(normalize_base_ref(Some("main\nHEAD")).is_err());
        assert_eq!(normalize_base_ref(None).unwrap(), "HEAD");
        assert_eq!(safe_component("repo name/branch"), "repo_name_branch");
    }

    #[test]
    fn workspace_status_handles_a_non_git_project() {
        let base = test_root("non-git-status");
        let root = base.join("project");
        fs::create_dir_all(&root).expect("project should create");
        let root = root.canonicalize().expect("project should canonicalize");
        let storage = Storage::open(base.join("halv.sqlite3")).expect("storage should open");
        let project = storage
            .upsert_project(
                "plain project",
                root.to_str().expect("path should be UTF-8"),
            )
            .expect("project should insert");
        let workspace = storage
            .list_workspaces(project.id)
            .expect("workspaces should list")
            .into_iter()
            .next()
            .expect("primary workspace should exist");

        let status = workspace_status(&storage, workspace).expect("workspace status should load");

        assert!(status.available);
        assert!(!status.is_git_repo);
        assert!(!status.dirty);
        assert_eq!(status.current_branch, None);
        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn workspace_overview_probes_every_workspace_of_every_project() {
        let base = test_root("overview");
        let repo = base.join("repo");
        let plain = base.join("plain");
        let detached = base.join("detached");
        fs::create_dir_all(&repo).expect("repo should create");
        fs::create_dir_all(&plain).expect("plain folder should create");
        let repo = repo.canonicalize().expect("repo should canonicalize");
        let plain = plain
            .canonicalize()
            .expect("plain folder should canonicalize");
        run_git(&repo, &["init", "-q", "-b", "trunk"]);
        run_git(&repo, &["config", "user.email", "halv@example.invalid"]);
        run_git(&repo, &["config", "user.name", "Halv Test"]);
        fs::write(repo.join("README.md"), "base\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-qm", "base"]);
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                detached.to_str().unwrap(),
            ],
        );
        let storage = Storage::open(base.join("halv.sqlite3")).expect("storage should open");
        let repo_project = storage
            .upsert_project("repo", repo.to_str().expect("path should be UTF-8"))
            .expect("repo project should insert");
        let plain_project = storage
            .upsert_project("plain", plain.to_str().expect("path should be UTF-8"))
            .expect("plain project should insert");
        let detached_row = storage
            .insert_managed_workspace(NewWorkspace {
                project_id: repo_project.id,
                name: "detached",
                root_path: detached.to_str().expect("path should be UTF-8"),
                git_common_dir: None,
                base_ref: Some("HEAD"),
                base_commit: None,
                branch: Some("halv/old-name"),
                local_environment_config_path: None,
            })
            .expect("detached worktree should insert");
        let missing_row = storage
            .insert_managed_workspace(NewWorkspace {
                project_id: repo_project.id,
                name: "missing",
                root_path: base.join("gone").to_str().expect("path should be UTF-8"),
                git_common_dir: None,
                base_ref: Some("HEAD"),
                base_commit: None,
                branch: Some("halv/gone"),
                local_environment_config_path: None,
            })
            .expect("missing worktree should insert");

        let overview = probe_workspaces(
            all_workspaces(&storage).expect("workspaces should list"),
            Duration::from_secs(10),
        )
        .await;
        let find = |id: &str| {
            overview
                .iter()
                .find(|row| row.workspace.id == id)
                .unwrap_or_else(|| panic!("{id} should be in the overview"))
        };

        assert_eq!(overview.len(), 4);
        let primary = find(&format!("primary-{}", repo_project.id));
        assert!(primary.available && primary.is_git_repo);
        assert_eq!(primary.current_branch.as_deref(), Some("trunk"));
        let detached = find(&detached_row.id);
        assert!(detached.available && detached.is_git_repo);
        assert_eq!(detached.current_branch, None);
        let missing = find(&missing_row.id);
        assert!(!missing.available && !missing.is_git_repo);
        assert_eq!(missing.workspace.state, "missing");
        let plain = find(&format!("primary-{}", plain_project.id));
        assert!(plain.available && !plain.is_git_repo);
        assert_eq!(plain.current_branch, None);
        let json = serde_json::to_value(primary).expect("overview should serialize");
        assert_eq!(json["currentBranch"], "trunk");
        assert_eq!(json["kind"], "primary");
        assert!(json.get("dirty").is_none());

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn head_branch_output_maps_branch_detached_unborn_and_plain_folders() {
        use std::os::unix::process::ExitStatusExt;
        let output = |code: i32, stdout: &str, stderr: &str| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        assert_eq!(
            head_branch_from_output(&output(0, "feat/x\n", "")),
            (true, Some("feat/x".to_string()))
        );
        assert_eq!(
            head_branch_from_output(&output(0, "HEAD\n", "")),
            (true, None)
        );
        assert_eq!(
            head_branch_from_output(&output(
                128,
                "HEAD\n",
                "fatal: ambiguous argument 'HEAD': unknown revision"
            )),
            (true, None)
        );
        assert_eq!(
            head_branch_from_output(&output(
                128,
                "",
                "fatal: not a git repository (or any of the parent directories): .git"
            )),
            (false, None)
        );
    }

    #[test]
    fn workspace_list_base_refs_returns_empty_for_a_non_git_project() {
        let base = test_root("non-git-base-refs");
        let root = base.join("project");
        fs::create_dir_all(&root).expect("project should create");
        let root = root.canonicalize().expect("project should canonicalize");
        let storage = Storage::open(base.join("halv.sqlite3")).expect("storage should open");
        let project = storage
            .upsert_project(
                "plain project",
                root.to_str().expect("path should be UTF-8"),
            )
            .expect("project should insert");

        let refs = workspace_base_refs(&storage, project.id)
            .expect("non-Git project should return base refs");

        assert!(refs.is_empty());
        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn transfer_capture_does_not_start_git_diff_in_submodules() {
        let base = test_root("transfer-submodule");
        let source = base.join("source");
        let sentinel = base.join("submodule-diff-program-ran");
        fs::create_dir_all(&source).expect("source should create");
        run_git(&source, &["init", "-q"]);
        run_git(&source, &["config", "user.email", "halv@example.invalid"]);
        run_git(&source, &["config", "user.name", "Halv Test"]);
        fs::write(source.join("tracked.txt"), "base\n").expect("tracked file should write");
        run_git(&source, &["add", "tracked.txt"]);
        run_git(&source, &["commit", "-qm", "base"]);
        crate::exec::plant_submodule_diff_program(&source, &sentinel);
        let base_commit = run_git(&source, &["rev-parse", "HEAD"]);

        let unstaged = capture_transfer(&source, &base_commit, true)
            .expect("transfer snapshot should capture");
        assert!(
            !sentinel.exists(),
            "the transfer must not start git diff in a submodule"
        );
        assert!(
            String::from_utf8_lossy(&unstaged.unstaged_patch).contains("+Subproject commit "),
            "the unstaged patch should show the gitlink"
        );
        run_git(&source, &["add", "sub"]);
        let staged = capture_transfer(&source, &base_commit, true)
            .expect("transfer snapshot should capture");
        assert!(
            String::from_utf8_lossy(&staged.staged_patch).contains("+Subproject commit "),
            "the staged patch should show the gitlink"
        );
        assert!(
            !sentinel.exists(),
            "the transfer must not start git diff in a submodule"
        );

        run_git(&source, &["diff", "--cached"]);
        assert!(
            sentinel.exists(),
            "plain Git should run the submodule diff program"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn transfer_preserves_staged_unstaged_untracked_and_included_ignored_files() {
        let base = test_root("transfer");
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir_all(&source).expect("source should create");
        run_git(&source, &["init", "-q"]);
        run_git(&source, &["config", "user.email", "halv@example.invalid"]);
        run_git(&source, &["config", "user.name", "Halv Test"]);
        fs::write(source.join("tracked.txt"), "base\n").unwrap();
        fs::write(source.join(".gitignore"), ".env\n").unwrap();
        fs::write(source.join(".worktreeinclude"), ".env\n").unwrap();
        run_git(&source, &["add", "."]);
        run_git(&source, &["commit", "-qm", "base"]);
        let base_commit = run_git(&source, &["rev-parse", "HEAD"]);

        fs::write(source.join("tracked.txt"), "staged\n").unwrap();
        run_git(&source, &["add", "tracked.txt"]);
        fs::write(source.join("tracked.txt"), "working\n").unwrap();
        fs::write(source.join("new.txt"), "untracked\n").unwrap();
        fs::write(source.join(".env"), "SECRET=test-only\n").unwrap();

        run_git(
            &source,
            &[
                "worktree",
                "add",
                "--detach",
                target.to_str().unwrap(),
                &base_commit,
            ],
        );
        let snapshot = capture_transfer(&source, &base_commit, true)
            .expect("transfer snapshot should capture");
        apply_transfer(&snapshot, &target).expect("transfer should apply");

        assert_eq!(
            fs::read_to_string(target.join("tracked.txt")).unwrap(),
            "working\n"
        );
        assert_eq!(run_git(&target, &["show", ":tracked.txt"]), "staged");
        assert_eq!(
            fs::read_to_string(target.join("new.txt")).unwrap(),
            "untracked\n"
        );
        assert_eq!(
            fs::read_to_string(target.join(".env")).unwrap(),
            "SECRET=test-only\n"
        );

        clear_transfer_source(&snapshot).expect("source should clean after handoff");
        assert_eq!(
            fs::read_to_string(source.join("tracked.txt")).unwrap(),
            "base\n"
        );
        assert!(!source.join("new.txt").exists());
        assert_eq!(
            fs::read_to_string(source.join(".env")).unwrap(),
            "SECRET=test-only\n"
        );

        run_git(
            &source,
            &["worktree", "remove", "--force", target.to_str().unwrap()],
        );
        let _ = fs::remove_dir_all(base);
    }
}
