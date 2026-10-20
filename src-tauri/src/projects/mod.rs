mod nested_repos;

pub use nested_repos::NestedRepoScan;

use crate::{
    exec::{self, BranchStatus},
    extensions::{self, ExtensionManager},
    storage::{Project, Storage},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    ops::ControlFlow,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

type ProjectResult<T> = Result<T, String>;
const FILE_SEARCH_LIMIT: usize = 20;
const FALLBACK_WALK_LIMIT: usize = 5000;
const REVIEW_DIFF_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const REVIEW_UNTRACKED_LIMIT: usize = 200;
const REVIEW_CONTEXT_LIMIT: usize = 10_000;
const GIT_REPO_SCAN_MAX_DEPTH: usize = 3;
const GIT_REPO_SCAN_ENTRY_LIMIT: usize = 5_000;
const GIT_REPO_SCAN_LIMIT: usize = 20;
const GIT_REPO_INSPECT_LIMIT: usize = 250;
const GIT_REPO_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const GIT_REPO_STATUS_OUTPUT_LIMIT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileSearchResult {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReviewSnapshot {
    pub id: String,
    pub project_id: i64,
    pub head: Option<String>,
    pub generated_at: String,
    pub cwd: String,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectReviewScope {
    #[default]
    WorkingTree,
    Staged,
    Unstaged,
    LastTurn,
    Branch,
    Commit,
}

impl ProjectReviewScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::WorkingTree => "working_tree",
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::LastTurn => "last_turn",
            Self::Branch => "branch",
            Self::Commit => "commit",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectReviewAction {
    Stage,
    Unstage,
    Revert,
}

#[derive(Debug)]
struct ReviewDiff {
    diff: Vec<u8>,
    identity: Option<String>,
    truncated: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct ResolvedReviewRoot {
    project_id: i64,
    root: PathBuf,
    git: ReviewGit,
}

/// Selects how a review starts git in its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewGit {
    /// The root is a registered project or workspace root. The agent sandbox
    /// does not let an agent write the `.git` of such a root.
    Managed,
    /// The root is not a registered root, for example a nested repository
    /// that the repository scan found. A running agent can write such a
    /// repository, thus git runs in the read-only sandbox.
    Sandboxed,
}

impl ReviewGit {
    /// Prepares a git command with `args` in `root`. For a sandboxed review,
    /// the function returns an error when this platform has no sandbox.
    fn command<I, S>(self, root: &Path, args: I) -> ProjectResult<Command>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let command = match self {
            Self::Managed => {
                let mut command = exec::managed_git_command(root)?;
                command.args(args);
                command
            }
            Self::Sandboxed => exec::sandboxed_git_command(root, args)?,
        };
        #[cfg(test)]
        let command = tests::with_injected_review_git_config(command);
        Ok(command)
    }
}

#[tauri::command]
pub async fn project_add_local(
    storage: tauri::State<'_, Storage>,
    path: String,
) -> ProjectResult<Project> {
    let root = canonical_project_root(&path)?;
    register_project(&storage, &root)
}

#[tauri::command]
pub async fn project_clone_github(
    storage: tauri::State<'_, Storage>,
    url: String,
    dest_parent: Option<String>,
) -> ProjectResult<Project> {
    // Reject anything that isn't a plain GitHub repo reference BEFORE touching
    // a subprocess: git's `ext::`/`fd::` transports and `-`-leading positionals
    // would otherwise turn a pasted "URL" into arbitrary command execution.
    let url = validate_github_url(url.trim())?;

    let parent = clone_parent(dest_parent.as_deref())?;
    let repo_name = infer_repo_dir_name(&url)?;
    let destination = parent.join(&repo_name);

    let output = if gh_is_authenticated() {
        // `gh repo clone <slug-or-url> -- <dir> [git-args]`: the `--` marks the
        // end of gh args; the URL is a positional gh already treats literally.
        let mut command = Command::new("gh");
        command.current_dir(&parent);
        exec::harden_git_environment(&mut command)?;
        crate::process::hide_console(&mut command);
        command
            .args(["repo", "clone"])
            .arg(&url)
            .arg("--")
            .arg(&destination)
            .output()
            .map_err(|error| format!("failed to run gh repo clone: {error}"))?
    } else {
        // `--` ends git option parsing so a `-`-leading URL can't be an option.
        exec::managed_git_command(&parent)?
            .arg("clone")
            .arg("--")
            .arg(&url)
            .arg(&destination)
            .output()
            .map_err(|error| format!("failed to run git clone: {error}"))?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("clone failed with exit status {}", output.status)
        } else {
            stderr
        });
    }

    let root = canonical_project_root_path(&destination)?;
    register_project(&storage, &root)
}

#[tauri::command]
pub async fn project_list(storage: tauri::State<'_, Storage>) -> ProjectResult<Vec<Project>> {
    storage.list_projects().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn project_remove(
    storage: tauri::State<'_, Storage>,
    manager: tauri::State<'_, ExtensionManager>,
    id: i64,
) -> ProjectResult<()> {
    remove_registered_project(storage.inner(), manager.inner(), id)
}

pub(crate) fn remove_registered_project(
    storage: &Storage,
    manager: &ExtensionManager,
    id: i64,
) -> ProjectResult<()> {
    let workspaces = storage
        .list_workspaces(id)
        .map_err(|error| error.to_string())?;
    if workspaces.iter().any(|workspace| workspace.managed) {
        return Err(
            "remove this project's managed worktrees before removing the project".to_string(),
        );
    }
    // The Crux approval uses the project id. Halv ends the approval before it
    // deletes the project row. Thus a new project with the same id asks again.
    let roots = workspaces
        .iter()
        .map(|workspace| PathBuf::from(&workspace.root_path))
        .collect::<Vec<_>>();
    extensions::crux_revoke_project_approval(manager, id, &roots)?;
    storage
        .remove_project(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn project_open(storage: tauri::State<'_, Storage>, id: i64) -> ProjectResult<Project> {
    let project = storage.get_project(id).map_err(|error| error.to_string())?;
    validate_registered_project(&project)?;
    storage.open_project(id).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn project_set_trust(
    storage: tauri::State<'_, Storage>,
    id: i64,
    level: String,
) -> ProjectResult<Project> {
    storage
        .set_project_trust(id, level.trim())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn project_set_approval_policy(
    storage: tauri::State<'_, Storage>,
    id: i64,
    policy: String,
) -> ProjectResult<Project> {
    storage
        .set_project_approval_policy(id, policy.trim())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn project_git_status(
    storage: tauri::State<'_, Storage>,
    id: i64,
    workspace_id: Option<String>,
) -> ProjectResult<BranchStatus> {
    let root = project_workspace_root(&storage, id, workspace_id.as_deref())?;
    Ok(exec::git_status(&root))
}

#[tauri::command]
pub async fn project_create_draft_pr(
    storage: tauri::State<'_, Storage>,
    id: i64,
    workspace_id: Option<String>,
) -> ProjectResult<String> {
    let root = project_workspace_root(&storage, id, workspace_id.as_deref())?;
    exec::create_draft_pr(&root)
}

#[tauri::command]
pub async fn project_commit(
    storage: tauri::State<'_, Storage>,
    id: i64,
    workspace_id: Option<String>,
    message: String,
) -> ProjectResult<String> {
    let root = project_workspace_root(&storage, id, workspace_id.as_deref())?;
    commit_staged_changes(&root, &message)
}

#[tauri::command]
pub async fn project_push(
    storage: tauri::State<'_, Storage>,
    id: i64,
    workspace_id: Option<String>,
) -> ProjectResult<String> {
    let root = project_workspace_root(&storage, id, workspace_id.as_deref())?;
    push_current_branch(&root)
}

fn commit_staged_changes(root: &Path, message: &str) -> ProjectResult<String> {
    let message = message.trim();
    if message.is_empty() || message.chars().count() > 500 || message.contains('\0') {
        return Err("commit message must contain 1 to 500 characters".to_string());
    }
    let staged = exec::managed_git_command(root)?
        .args([
            "diff",
            "--cached",
            "--quiet",
            "--exit-code",
            "--no-ext-diff",
            "--no-textconv",
        ])
        .output()
        .map_err(|error| format!("failed to inspect staged changes: {error}"))?;
    match staged.status.code() {
        Some(0) => return Err("stage at least one change before committing".to_string()),
        Some(1) => {}
        _ => return Err(git_failure("failed to inspect staged changes", &staged)),
    }
    let output = exec::managed_git_command(root)?
        .args(["commit", "-m"])
        .arg(message)
        .output()
        .map_err(|error| format!("failed to run git commit: {error}"))?;
    if !output.status.success() {
        return Err(git_failure("git commit failed", &output));
    }
    Ok(git_success("Committed staged changes.", &output))
}

fn push_current_branch(root: &Path) -> ProjectResult<String> {
    let branch_output = exec::managed_git_command(root)?
        .args(["branch", "--show-current"])
        .output()
        .map_err(|error| format!("failed to read the current branch: {error}"))?;
    if !branch_output.status.success() {
        return Err(git_failure(
            "failed to read the current branch",
            &branch_output,
        ));
    }
    let branch = String::from_utf8(branch_output.stdout)
        .map_err(|_| "Git returned a non-UTF-8 branch name".to_string())?;
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("create or select a branch before pushing".to_string());
    }
    let output = exec::managed_git_command(root)?
        .args(["push", "--set-upstream", "origin", "--"])
        .arg(branch)
        .output()
        .map_err(|error| format!("failed to run git push: {error}"))?;
    if !output.status.success() {
        return Err(git_failure("git push failed", &output));
    }
    Ok(git_success("Pushed current branch.", &output))
}

fn git_success(fallback: &str, output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stdout.is_empty() {
        stdout
    } else if !stderr.is_empty() {
        stderr
    } else {
        fallback.to_string()
    }
}

fn git_failure(prefix: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    if detail.is_empty() {
        format!("{prefix} with status {}", output.status)
    } else {
        format!("{prefix}: {detail}")
    }
}

#[tauri::command]
pub async fn project_search_files(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
    query: String,
) -> ProjectResult<Vec<ProjectFileSearchResult>> {
    let root = project_workspace_root(&storage, project_id, workspace_id.as_deref())?;
    search_project_files(&root, &query)
}

#[tauri::command]
pub async fn project_review_snapshot(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
    scope: Option<ProjectReviewScope>,
    reference: Option<String>,
    conversation_id: Option<String>,
) -> ProjectResult<ProjectReviewSnapshot> {
    let root = project_workspace_root(&storage, project_id, workspace_id.as_deref())?;
    let scope = scope.unwrap_or_default();
    let recorded = if scope == ProjectReviewScope::LastTurn {
        Some(last_turn_review_diff(
            &storage,
            project_id,
            &root,
            conversation_id.as_deref(),
        )?)
    } else {
        None
    };
    review_snapshot(
        project_id,
        &root,
        ReviewGit::Managed,
        scope,
        reference.as_deref(),
        recorded,
    )
}

#[tauri::command]
pub async fn project_git_root_for_path(path: String) -> Option<String> {
    let path = Path::new(path.trim());
    git_root_for_path(path)
}

fn git_root_for_path(path: &Path) -> Option<String> {
    let directory = if path.is_dir() { path } else { path.parent()? };
    let output = exec::managed_git_command(directory)
        .ok()?
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8(output.stdout).ok()?;
    let root = root.trim();
    if root.is_empty() || root.chars().any(char::is_control) {
        return None;
    }
    Some(root.to_string())
}

/// Terminal adoption (ADR 0020): the git root of a terminal folder, when
/// Halv may add that root as a project by itself. A terminal outside every
/// project becomes a tab of a new project only in this case. Otherwise the
/// terminal stays in the Chats area of the sidebar.
#[tauri::command]
pub async fn project_adoption_root(app: tauri::AppHandle, path: String) -> Option<String> {
    let app_data = crate::app_data::resolved_app_data_dir(&app).ok();
    adoption_root_for_path(Path::new(path.trim()), &AdoptionGuards::current(app_data))
}

/// The folders that terminal adoption never adds as a project.
struct AdoptionGuards {
    /// The home folder. Its root and the folders above it are refused.
    home: Option<PathBuf>,
    /// The app data folder and the temporary folders. Every folder in them
    /// is refused.
    blocked: Vec<PathBuf>,
}

impl AdoptionGuards {
    fn current(app_data: Option<PathBuf>) -> Self {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        #[cfg(not(windows))]
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut blocked = vec![std::env::temp_dir()];
        #[cfg(unix)]
        blocked.extend([PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]);
        blocked.extend(app_data);
        Self {
            home: home.map(canonical_or_same),
            blocked: blocked.into_iter().map(canonical_or_same).collect(),
        }
    }
}

fn canonical_or_same(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn adoption_root_for_path(path: &Path, guards: &AdoptionGuards) -> Option<String> {
    let root = git_root_for_path(path)?;
    // A linked worktree belongs to its main repository, so the terminal
    // goes to the project of that repository (ADR 0019 skips it too).
    let root = main_worktree_root(Path::new(&root))
        .map(|main| main.to_string_lossy().into_owned())
        .unwrap_or(root);
    adoption_root_allowed(Path::new(&root), guards).then_some(root)
}

/// The main working tree of a linked worktree. A linked worktree has a
/// `.git` file that points to `<main>/.git/worktrees/<name>`. Returns `None`
/// for any other folder, and for a worktree of a bare repository.
fn main_worktree_root(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if !fs::metadata(&marker).is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 4096) {
        return None;
    }
    let content = fs::read_to_string(&marker).ok()?;
    let gitdir = content
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|gitdir| !gitdir.is_empty())?;
    let gitdir = root.join(gitdir);
    let worktrees = gitdir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees.parent()?;
    if common.file_name()? != ".git" {
        return None;
    }
    let main = common.parent()?;
    main.join(".git")
        .is_dir()
        .then(|| canonical_or_same(main.to_path_buf()))
}

/// A git root that adoption may add: not the filesystem root, not the home
/// folder or a folder above it, and not in a blocked folder. The checks
/// compare whole path components, so `/tmpfiles` is not in `/tmp`.
fn adoption_root_allowed(root: &Path, guards: &AdoptionGuards) -> bool {
    let root = canonical_or_same(root.to_path_buf());
    if root.parent().is_none() {
        return false;
    }
    if guards
        .home
        .as_ref()
        .is_some_and(|home| home.starts_with(&root))
    {
        return false;
    }
    !guards
        .blocked
        .iter()
        .any(|blocked| root.starts_with(blocked))
}

#[tauri::command]
pub async fn project_git_repos_under_path(path: String) -> Vec<String> {
    git_repos_under_path(Path::new(path.trim()))
}

fn git_repos_under_path(path: &Path) -> Vec<String> {
    if !path.is_dir() || git_root_for_path(path).is_some() {
        return Vec::new();
    }
    let Ok(root) = path.canonicalize() else {
        return Vec::new();
    };
    let rules = nested_repos::RepoWalkRules {
        max_depth: GIT_REPO_SCAN_MAX_DEPTH,
        entry_limit: GIT_REPO_SCAN_ENTRY_LIMIT,
        skip_directory: |name, _| name.starts_with('.') || name == "node_modules",
        gitignore: false,
        bare_repositories: false,
    };
    let mut repos = Vec::new();
    let mut inspected_repos = 0;
    nested_repos::walk_repositories(&root, &rules, |repo| {
        if inspected_repos >= GIT_REPO_INSPECT_LIMIT {
            return ControlFlow::Break(());
        }
        inspected_repos += 1;
        if git_repo_has_changes(repo) {
            repos.push(repo.to_string_lossy().into_owned());
        }
        if repos.len() >= GIT_REPO_SCAN_LIMIT {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    repos
}

/// Scans a folder project for nested git repositories (ADR 0019). The
/// result is empty when the folder is a repository or is inside one.
#[tauri::command]
pub async fn project_scan_nested_repos(path: String) -> ProjectResult<NestedRepoScan> {
    let root = canonical_project_root(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        // A folder that Halv cannot read, for example before a macOS
        // permission prompt, is an error and not an empty folder. Then the
        // one-time scan tries again at the next start.
        fs::read_dir(&root)
            .map_err(|error| format!("failed to read folder `{}`: {error}", root.display()))?;
        Ok(nested_repos::scan_nested_repos(&root))
    })
    .await
    .map_err(|error| format!("failed to scan for nested repositories: {error}"))?
}

/// Reports whether a repository below the scan root has changes. A running
/// agent can write this repository, thus git runs in the read-only sandbox.
/// Without a sandbox, the scan does not start git and reports no changes.
fn git_repo_has_changes(root: &Path) -> bool {
    let Ok(command) = git_repo_status_command(root) else {
        return false;
    };
    exec::run_process_with_limits(
        command,
        GIT_REPO_STATUS_TIMEOUT,
        GIT_REPO_STATUS_OUTPUT_LIMIT_BYTES,
    )
    .is_ok_and(|output| {
        if !output.status.success() {
            return false;
        }
        let status = String::from_utf8_lossy(&output.output);
        let mut lines = status.lines();
        let Some(branch) = lines.next() else {
            return false;
        };
        if !branch.starts_with("##") {
            return true;
        }
        branch.contains("[ahead ") || lines.any(|line| !line.starts_with("##"))
    })
}

fn git_repo_status_command(root: &Path) -> Result<Command, String> {
    exec::sandboxed_git_command(
        root,
        [
            "status",
            "--porcelain",
            "--branch",
            "--untracked-files=normal",
            "--ignore-submodules=dirty",
        ],
    )
}

#[tauri::command]
pub async fn project_review_snapshot_for_cwd(
    storage: tauri::State<'_, Storage>,
    cwd: String,
    scope: Option<ProjectReviewScope>,
    reference: Option<String>,
    conversation_id: Option<String>,
) -> ProjectResult<ProjectReviewSnapshot> {
    let resolved = resolve_review_root(&storage, None, None, Some(&cwd))?;
    let scope = scope.unwrap_or_default();
    let recorded = if scope == ProjectReviewScope::LastTurn {
        Some(last_turn_review_diff(
            &storage,
            resolved.project_id,
            &resolved.root,
            conversation_id.as_deref(),
        )?)
    } else {
        None
    };
    review_snapshot(
        resolved.project_id,
        &resolved.root,
        resolved.git,
        scope,
        reference.as_deref(),
        recorded,
    )
}

#[tauri::command]
pub async fn project_review_file_diff(
    storage: tauri::State<'_, Storage>,
    cwd: String,
    path: String,
    scope: Option<ProjectReviewScope>,
    reference: Option<String>,
    context_lines: usize,
) -> ProjectResult<ProjectReviewSnapshot> {
    let resolved = resolve_review_root(&storage, None, None, Some(&cwd))?;
    review_file_snapshot(
        resolved.project_id,
        &resolved.root,
        resolved.git,
        &path,
        scope.unwrap_or_default(),
        reference.as_deref(),
        context_lines,
    )
}

#[tauri::command]
pub async fn project_review_open_file(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
    path: String,
) -> ProjectResult<()> {
    let root = project_workspace_root(&storage, project_id, workspace_id.as_deref())?;
    let file = resolve_review_file(&root, &path)?;
    tauri_plugin_opener::open_path(file, None::<&str>)
        .map_err(|error| format!("failed to open review file: {error}"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn project_review_apply_action(
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
    cwd: Option<String>,
    snapshot_id: String,
    scope: ProjectReviewScope,
    reference: Option<String>,
    path: Option<String>,
    hunk_index: Option<usize>,
    action: ProjectReviewAction,
) -> ProjectResult<ProjectReviewSnapshot> {
    let resolved = resolve_review_root(
        &storage,
        Some(project_id),
        workspace_id.as_deref(),
        cwd.as_deref(),
    )?;
    apply_review_action(
        resolved.project_id,
        &resolved.root,
        resolved.git,
        &snapshot_id,
        scope,
        reference.as_deref(),
        path.as_deref(),
        hunk_index,
        action,
    )?;
    review_snapshot(
        resolved.project_id,
        &resolved.root,
        resolved.git,
        scope,
        reference.as_deref(),
        None,
    )
}

fn review_snapshot(
    project_id: i64,
    root: &Path,
    git: ReviewGit,
    scope: ProjectReviewScope,
    reference: Option<&str>,
    recorded: Option<ReviewDiff>,
) -> ProjectResult<ProjectReviewSnapshot> {
    let head = git_optional_text(root, git, ["rev-parse", "--verify", "HEAD"])?;
    let ReviewDiff {
        diff,
        identity,
        truncated,
    } = review_diff_for_scope(root, git, scope, reference, head.as_deref(), recorded)?;
    snapshot_from_review_diff(project_id, root, scope, head, identity, diff, truncated)
}

fn review_file_snapshot(
    project_id: i64,
    root: &Path,
    git: ReviewGit,
    path: &str,
    scope: ProjectReviewScope,
    reference: Option<&str>,
    context_lines: usize,
) -> ProjectResult<ProjectReviewSnapshot> {
    validate_review_path(path)?;
    if context_lines > REVIEW_CONTEXT_LIMIT {
        return Err(format!(
            "Diff context cannot exceed {REVIEW_CONTEXT_LIMIT} lines"
        ));
    }
    let head = git_optional_text(root, git, ["rev-parse", "--verify", "HEAD"])?;
    let ReviewDiff {
        diff,
        identity,
        truncated,
    } = review_file_diff_for_scope(
        root,
        git,
        path,
        scope,
        reference,
        head.as_deref(),
        context_lines,
    )?;
    snapshot_from_review_diff(project_id, root, scope, head, identity, diff, truncated)
}

fn snapshot_from_review_diff(
    project_id: i64,
    root: &Path,
    scope: ProjectReviewScope,
    head: Option<String>,
    identity: Option<String>,
    raw_diff: Vec<u8>,
    mut truncated: bool,
) -> ProjectResult<ProjectReviewSnapshot> {
    let keep = raw_diff.len().min(REVIEW_DIFF_LIMIT_BYTES);
    truncated |= keep < raw_diff.len();
    let bounded_diff = raw_diff[..keep].to_vec();
    let diff = String::from_utf8_lossy(&bounded_diff).into_owned();
    let mut digest = Sha256::new();
    digest.update(scope.as_str());
    digest.update([0]);
    digest.update(head.as_deref().unwrap_or("unborn"));
    digest.update([0]);
    digest.update(identity.as_deref().unwrap_or(""));
    digest.update([0]);
    digest.update(diff.as_bytes());
    let id = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ProjectReviewSnapshot {
        id,
        project_id,
        head,
        generated_at: Utc::now().to_rfc3339(),
        cwd: root.to_string_lossy().into_owned(),
        diff,
        truncated,
    })
}

fn review_diff_for_scope(
    root: &Path,
    git: ReviewGit,
    scope: ProjectReviewScope,
    reference: Option<&str>,
    head: Option<&str>,
    recorded: Option<ReviewDiff>,
) -> ProjectResult<ReviewDiff> {
    match scope {
        ProjectReviewScope::WorkingTree => working_tree_review_diff(root, git, head.is_some()),
        ProjectReviewScope::Staged => Ok(ReviewDiff {
            diff: git_diff_output(
                root,
                git,
                [
                    "-c",
                    "core.quotePath=false",
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=dirty",
                    "--no-color",
                    "--find-renames",
                    "--binary",
                    "--",
                ],
            )?,
            identity: None,
            truncated: false,
        }),
        ProjectReviewScope::Unstaged => unstaged_review_diff(root, git),
        ProjectReviewScope::LastTurn => {
            recorded.ok_or_else(|| "A conversation is required to review the last turn".to_string())
        }
        ProjectReviewScope::Branch => {
            let head =
                head.ok_or_else(|| "Branch review requires an existing HEAD commit".to_string())?;
            let resolved = resolve_review_commit(root, git, reference, "branch")?;
            let range = format!("{resolved}...{head}");
            Ok(ReviewDiff {
                diff: git_diff_output_owned(
                    root,
                    git,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=dirty",
                        "--no-color",
                        "--find-renames",
                        "--binary",
                        &range,
                        "--",
                    ],
                    "git diff",
                )?,
                identity: Some(resolved),
                truncated: false,
            })
        }
        ProjectReviewScope::Commit => {
            let resolved = resolve_review_commit(root, git, reference, "commit")?;
            Ok(ReviewDiff {
                diff: git_diff_output_owned(
                    root,
                    git,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "show",
                        "--format=",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=dirty",
                        "--no-color",
                        "--find-renames",
                        "--binary",
                        &resolved,
                        "--",
                    ],
                    "git show",
                )?,
                identity: Some(resolved),
                truncated: false,
            })
        }
    }
}

fn review_file_diff_for_scope(
    root: &Path,
    git: ReviewGit,
    path: &str,
    scope: ProjectReviewScope,
    reference: Option<&str>,
    head: Option<&str>,
    context_lines: usize,
) -> ProjectResult<ReviewDiff> {
    let context = format!("--unified={context_lines}");
    let mut diff = Vec::new();
    let mut truncated = false;
    let identity = match scope {
        ProjectReviewScope::WorkingTree => {
            if head.is_some() {
                append_review_diff(
                    &mut diff,
                    &mut truncated,
                    git_diff_output_owned(
                        root,
                        git,
                        &[
                            "-c",
                            "core.quotePath=false",
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--ignore-submodules=dirty",
                            "--no-color",
                            "--find-renames",
                            "--binary",
                            &context,
                            "HEAD",
                            "--",
                            path,
                        ],
                        "git diff",
                    )?,
                );
            } else {
                append_review_diff(
                    &mut diff,
                    &mut truncated,
                    git_diff_output_owned(
                        root,
                        git,
                        &[
                            "-c",
                            "core.quotePath=false",
                            "diff",
                            "--cached",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--ignore-submodules=dirty",
                            "--no-color",
                            "--find-renames",
                            "--binary",
                            &context,
                            "--",
                            path,
                        ],
                        "git diff",
                    )?,
                );
                append_review_diff(
                    &mut diff,
                    &mut truncated,
                    git_diff_output_owned(
                        root,
                        git,
                        &[
                            "-c",
                            "core.quotePath=false",
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--ignore-submodules=dirty",
                            "--no-color",
                            "--find-renames",
                            "--binary",
                            &context,
                            "--",
                            path,
                        ],
                        "git diff",
                    )?,
                );
            }
            append_untracked_review_file_diff(
                root,
                git,
                path,
                &context,
                &mut diff,
                &mut truncated,
            )?;
            None
        }
        ProjectReviewScope::Staged => {
            append_review_diff(
                &mut diff,
                &mut truncated,
                git_diff_output_owned(
                    root,
                    git,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "diff",
                        "--cached",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=dirty",
                        "--no-color",
                        "--find-renames",
                        "--binary",
                        &context,
                        "--",
                        path,
                    ],
                    "git diff",
                )?,
            );
            None
        }
        ProjectReviewScope::Unstaged => {
            append_review_diff(
                &mut diff,
                &mut truncated,
                git_diff_output_owned(
                    root,
                    git,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=dirty",
                        "--no-color",
                        "--find-renames",
                        "--binary",
                        &context,
                        "--",
                        path,
                    ],
                    "git diff",
                )?,
            );
            append_untracked_review_file_diff(
                root,
                git,
                path,
                &context,
                &mut diff,
                &mut truncated,
            )?;
            None
        }
        ProjectReviewScope::LastTurn => {
            return Err("Context expansion is unavailable for last-turn reviews".to_string());
        }
        ProjectReviewScope::Branch => {
            let head =
                head.ok_or_else(|| "Branch review requires an existing HEAD commit".to_string())?;
            let resolved = resolve_review_commit(root, git, reference, "branch")?;
            let range = format!("{resolved}...{head}");
            append_review_diff(
                &mut diff,
                &mut truncated,
                git_diff_output_owned(
                    root,
                    git,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=dirty",
                        "--no-color",
                        "--find-renames",
                        "--binary",
                        &context,
                        &range,
                        "--",
                        path,
                    ],
                    "git diff",
                )?,
            );
            Some(resolved)
        }
        ProjectReviewScope::Commit => {
            let resolved = resolve_review_commit(root, git, reference, "commit")?;
            append_review_diff(
                &mut diff,
                &mut truncated,
                git_diff_output_owned(
                    root,
                    git,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "show",
                        "--format=",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=dirty",
                        "--no-color",
                        "--find-renames",
                        "--binary",
                        &context,
                        &resolved,
                        "--",
                        path,
                    ],
                    "git show",
                )?,
            );
            Some(resolved)
        }
    };
    Ok(ReviewDiff {
        diff,
        identity,
        truncated,
    })
}

fn working_tree_review_diff(
    root: &Path,
    git: ReviewGit,
    has_head: bool,
) -> ProjectResult<ReviewDiff> {
    let mut diff = Vec::new();
    let mut truncated = false;
    if has_head {
        append_review_diff(
            &mut diff,
            &mut truncated,
            git_diff_output(
                root,
                git,
                [
                    "-c",
                    "core.quotePath=false",
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=dirty",
                    "--no-color",
                    "--find-renames",
                    "--binary",
                    "HEAD",
                    "--",
                ],
            )?,
        );
    } else {
        append_review_diff(
            &mut diff,
            &mut truncated,
            git_diff_output(
                root,
                git,
                [
                    "-c",
                    "core.quotePath=false",
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=dirty",
                    "--no-color",
                    "--find-renames",
                    "--binary",
                    "--",
                ],
            )?,
        );
        append_review_diff(
            &mut diff,
            &mut truncated,
            git_diff_output(
                root,
                git,
                [
                    "-c",
                    "core.quotePath=false",
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=dirty",
                    "--no-color",
                    "--find-renames",
                    "--binary",
                    "--",
                ],
            )?,
        );
    }
    append_untracked_review_diff(root, git, &mut diff, &mut truncated)?;
    Ok(ReviewDiff {
        diff,
        identity: None,
        truncated,
    })
}

fn unstaged_review_diff(root: &Path, git: ReviewGit) -> ProjectResult<ReviewDiff> {
    let mut diff = Vec::new();
    let mut truncated = false;
    append_review_diff(
        &mut diff,
        &mut truncated,
        git_diff_output(
            root,
            git,
            [
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--no-color",
                "--find-renames",
                "--binary",
                "--",
            ],
        )?,
    );
    append_untracked_review_diff(root, git, &mut diff, &mut truncated)?;
    Ok(ReviewDiff {
        diff,
        identity: None,
        truncated,
    })
}

fn append_untracked_review_diff(
    root: &Path,
    git: ReviewGit,
    diff: &mut Vec<u8>,
    truncated: &mut bool,
) -> ProjectResult<()> {
    if *truncated {
        return Ok(());
    }
    let untracked = git_output(
        root,
        git,
        ["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    if !untracked.status.success() {
        return Err(git_output_error(untracked, "git ls-files"));
    }
    for (index, raw_path) in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if index >= REVIEW_UNTRACKED_LIMIT {
            *truncated = true;
            break;
        }
        let path = std::str::from_utf8(raw_path)
            .map_err(|_| "Git returned a non-UTF-8 untracked path".to_string())?;
        let output = git
            .command(
                root,
                [
                    "-c",
                    "core.quotePath=false",
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--binary",
                    "--",
                    null_device(),
                    path,
                ],
            )?
            .output()
            .map_err(|error| format!("failed to diff untracked file: {error}"))?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(git_output_error(output, "git diff --no-index"));
        }
        append_review_diff(diff, truncated, output.stdout);
        if *truncated {
            break;
        }
    }
    Ok(())
}

fn append_untracked_review_file_diff(
    root: &Path,
    git: ReviewGit,
    path: &str,
    context: &str,
    diff: &mut Vec<u8>,
    truncated: &mut bool,
) -> ProjectResult<()> {
    if *truncated {
        return Ok(());
    }
    let untracked = git
        .command(
            root,
            [
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                path,
            ],
        )?
        .output()
        .map_err(|error| format!("failed to inspect untracked file: {error}"))?;
    if !untracked.status.success() {
        return Err(git_output_error(untracked, "git ls-files"));
    }
    if !untracked
        .stdout
        .split(|byte| *byte == 0)
        .any(|candidate| candidate == path.as_bytes())
    {
        return Ok(());
    }
    let output = git
        .command(
            root,
            [
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--binary",
                context,
                "--",
                null_device(),
                path,
            ],
        )?
        .output()
        .map_err(|error| format!("failed to diff untracked file: {error}"))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(git_output_error(output, "git diff --no-index"));
    }
    append_review_diff(diff, truncated, output.stdout);
    Ok(())
}

fn last_turn_review_diff(
    storage: &Storage,
    project_id: i64,
    root: &Path,
    conversation_id: Option<&str>,
) -> ProjectResult<ReviewDiff> {
    let conversation_id = conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Open a project conversation to review its last turn".to_string())?;
    let project = storage
        .project_for_conversation(conversation_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "The conversation is not bound to a project".to_string())?;
    if project.id != project_id {
        return Err("The conversation belongs to a different project".to_string());
    }
    let turn = storage
        .list_turn_diffs(conversation_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .last()
        .ok_or_else(|| "This conversation has no recorded turn diff".to_string())?;
    let recorded_cwd = Path::new(&turn.cwd)
        .canonicalize()
        .map_err(|_| "The recorded turn workspace is no longer available".to_string())?;
    let review_root = root
        .canonicalize()
        .map_err(|_| "The selected review workspace is no longer available".to_string())?;
    if !recorded_cwd.starts_with(&review_root) {
        return Err("The recorded turn belongs to a different workspace".to_string());
    }
    Ok(ReviewDiff {
        diff: turn.diff.into_bytes(),
        identity: Some(turn.turn_id),
        truncated: false,
    })
}

fn resolve_review_commit(
    root: &Path,
    git: ReviewGit,
    reference: Option<&str>,
    label: &str,
) -> ProjectResult<String> {
    let reference = reference
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Enter a {label} name or commit"))?;
    if reference.len() > 256 || reference.chars().any(char::is_control) {
        return Err(format!("Invalid {label} reference"));
    }
    let spec = format!("{reference}^{{commit}}");
    let output = git
        .command(root, ["rev-parse", "--verify", "--end-of-options", &spec])?
        .output()
        .map_err(|error| format!("failed to resolve {label}: {error}"))?;
    if !output.status.success() {
        return Err(format!("Could not resolve {label} `{reference}`"));
    }
    let resolved = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned a non-UTF-8 commit ID".to_string())?;
    let resolved = resolved.trim();
    if resolved.len() < 40
        || !resolved
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(format!("Git returned an invalid {label} commit ID"));
    }
    Ok(resolved.to_string())
}

#[allow(clippy::too_many_arguments)]
fn apply_review_action(
    project_id: i64,
    root: &Path,
    git: ReviewGit,
    expected_snapshot_id: &str,
    scope: ProjectReviewScope,
    reference: Option<&str>,
    path: Option<&str>,
    hunk_index: Option<usize>,
    action: ProjectReviewAction,
) -> ProjectResult<()> {
    if let Some(path) = path {
        validate_review_path(path)?;
    } else if hunk_index.is_some() {
        return Err("A hunk action requires a file path".to_string());
    }
    let allowed = matches!(
        (scope, action),
        (ProjectReviewScope::Unstaged, ProjectReviewAction::Stage)
            | (ProjectReviewScope::Unstaged, ProjectReviewAction::Revert)
            | (ProjectReviewScope::Staged, ProjectReviewAction::Unstage)
    );
    if !allowed {
        return Err(
            "This action is only available from the staged or unstaged review scopes".to_string(),
        );
    }

    let current = review_snapshot(project_id, root, git, scope, reference, None)?;
    if current.truncated {
        return Err("Actions are disabled for a truncated review snapshot".to_string());
    }
    if current.id != expected_snapshot_id {
        return Err("The diff changed. Refresh the review before applying this action".to_string());
    }
    let patch = match (path, hunk_index) {
        (Some(path), Some(hunk_index)) => {
            let file_patch = review_file_patch(&current.diff, path).ok_or_else(|| {
                "The selected file is not present in this review snapshot".to_string()
            })?;
            review_hunk_patch(file_patch, hunk_index)
                .ok_or_else(|| "The selected hunk is no longer available".to_string())?
        }
        (Some(path), None) => review_file_patch(&current.diff, path)
            .ok_or_else(|| "The selected file is not present in this review snapshot".to_string())?
            .to_string(),
        (None, None) => current.diff.clone(),
        (None, Some(_)) => unreachable!("hunk actions without paths are rejected above"),
    };
    match action {
        ProjectReviewAction::Stage => run_git_apply(root, &patch, true, false),
        ProjectReviewAction::Unstage => run_git_apply(root, &patch, true, true),
        ProjectReviewAction::Revert => run_git_apply(root, &patch, false, true),
    }
}

fn validate_review_path(path: &str) -> ProjectResult<()> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.chars().any(char::is_control)
        || !candidate
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Invalid project-relative review path".to_string());
    }
    Ok(())
}

fn resolve_review_file(root: &Path, path: &str) -> ProjectResult<PathBuf> {
    validate_review_path(path)?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("failed to resolve project root: {error}"))?;
    let file = root
        .join(path)
        .canonicalize()
        .map_err(|_| "The selected review file no longer exists".to_string())?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err("The selected review path is outside the project or is not a file".to_string());
    }
    Ok(file)
}

fn review_file_patch<'a>(diff: &'a str, path: &str) -> Option<&'a str> {
    let mut starts = diff
        .match_indices("diff --git ")
        .filter_map(|(index, _)| {
            (index == 0 || diff.as_bytes().get(index.wrapping_sub(1)) == Some(&b'\n'))
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return None;
    }
    starts.push(diff.len());
    starts.windows(2).find_map(|range| {
        let patch = &diff[range[0]..range[1]];
        review_patch_matches_path(patch, path).then_some(patch)
    })
}

fn review_patch_matches_path(patch: &str, path: &str) -> bool {
    let old_marker = format!("a/{path}");
    let new_marker = format!("b/{path}");
    patch.lines().any(|line| {
        path_marker_matches(line, "--- ", &old_marker)
            || path_marker_matches(line, "+++ ", &new_marker)
            || path_marker_matches(line, "rename from ", path)
            || path_marker_matches(line, "rename to ", path)
            || line == format!("diff --git {old_marker} {new_marker}")
    })
}

fn path_marker_matches(line: &str, prefix: &str, expected: &str) -> bool {
    line.strip_prefix(prefix)
        .and_then(|value| value.split('\t').next())
        == Some(expected)
}

fn review_hunk_patch(file_patch: &str, hunk_index: usize) -> Option<String> {
    let hunk_starts = file_patch
        .match_indices("@@ ")
        .filter_map(|(index, _)| {
            (index == 0 || file_patch.as_bytes().get(index.wrapping_sub(1)) == Some(&b'\n'))
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let start = *hunk_starts.get(hunk_index)?;
    let end = hunk_starts
        .get(hunk_index + 1)
        .copied()
        .unwrap_or(file_patch.len());
    let mut patch = String::with_capacity(start + end - start + 1);
    patch.push_str(&file_patch[..hunk_starts[0]]);
    patch.push_str(&file_patch[start..end]);
    if !patch.ends_with('\n') {
        patch.push('\n');
    }
    Some(patch)
}

fn run_git_apply(root: &Path, patch: &str, cached: bool, reverse: bool) -> ProjectResult<()> {
    let mut command = exec::managed_git_command(root)?;
    command.arg("apply").arg("--whitespace=nowarn");
    if cached {
        command.arg("--cached");
    }
    if reverse {
        command.arg("--reverse");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run git apply: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "failed to open git apply stdin".to_string())?
        .write_all(patch.as_bytes())
        .map_err(|error| format!("failed to write git patch: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for git apply: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git_output_error(output, "git apply"))
    }
}

fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

fn append_review_diff(target: &mut Vec<u8>, truncated: &mut bool, chunk: Vec<u8>) {
    if *truncated || chunk.is_empty() {
        return;
    }
    if !target.is_empty() && !target.ends_with(b"\n") {
        target.push(b'\n');
    }
    let remaining = REVIEW_DIFF_LIMIT_BYTES.saturating_sub(target.len());
    let keep = remaining.min(chunk.len());
    target.extend_from_slice(&chunk[..keep]);
    if keep < chunk.len() {
        *truncated = true;
    }
}

fn git_diff_output<'a>(
    root: &Path,
    git: ReviewGit,
    args: impl IntoIterator<Item = &'a str>,
) -> ProjectResult<Vec<u8>> {
    let output = git_output(root, git, args)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_output_error(output, "git diff"))
    }
}

fn git_diff_output_owned(
    root: &Path,
    git: ReviewGit,
    args: &[&str],
    label: &str,
) -> ProjectResult<Vec<u8>> {
    let output = git_output(root, git, args.iter().copied())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_output_error(output, label))
    }
}

fn git_optional_text<'a>(
    root: &Path,
    git: ReviewGit,
    args: impl IntoIterator<Item = &'a str>,
) -> ProjectResult<Option<String>> {
    let output = git_output(root, git, args)?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned non-UTF-8 output".to_string())?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_string()))
}

fn git_output<'a>(
    root: &Path,
    git: ReviewGit,
    args: impl IntoIterator<Item = &'a str>,
) -> ProjectResult<std::process::Output> {
    git.command(root, args)?
        .output()
        .map_err(|error| format!("failed to run git: {error}"))
}

fn git_output_error(output: std::process::Output, label: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{label} failed with exit status {}", output.status)
    } else {
        format!("{label} failed: {stderr}")
    }
}

fn resolve_review_root(
    storage: &Storage,
    project_id: Option<i64>,
    workspace_id: Option<&str>,
    cwd: Option<&str>,
) -> ProjectResult<ResolvedReviewRoot> {
    if let Some(cwd) = cwd {
        let cwd = cwd.trim();
        if cwd.is_empty() {
            return Err("Review cwd cannot be empty".to_string());
        }
        let cwd = canonical_project_root_path(Path::new(cwd))?;
        let output = exec::managed_git_command(&cwd)?
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|error| format!("failed to resolve review Git root: {error}"))?;
        if !output.status.success() {
            let error = git_output_error(output, "git rev-parse");
            if error.to_ascii_lowercase().contains("not a git repository") {
                return Err("not a git repository".to_string());
            }
            return Err(error);
        }
        let git_root = String::from_utf8(output.stdout)
            .map_err(|_| "Git returned a non-UTF-8 review root".to_string())?;
        let git_root = git_root.trim();
        if git_root.is_empty() || git_root.chars().any(char::is_control) {
            return Err("Git returned an invalid review root".to_string());
        }
        let git_root = canonical_project_root_path(Path::new(git_root))?;
        let mut matches = Vec::new();
        let mut registered_root = false;
        for project in storage.list_projects().map_err(|error| error.to_string())? {
            if let Ok(root) = validate_registered_project(&project) {
                if git_root.starts_with(&root) {
                    matches.push((root.components().count(), project.id));
                    registered_root |= git_root == root;
                }
            }
            for workspace in storage
                .list_workspaces(project.id)
                .map_err(|error| error.to_string())?
            {
                if workspace.state != "available" {
                    continue;
                }
                if let Ok(root) = validate_registered_project_root(Path::new(&workspace.root_path))
                {
                    if git_root.starts_with(&root) {
                        matches.push((root.components().count(), project.id));
                        registered_root |= git_root == root;
                    }
                }
            }
        }
        let project_id = matches
            .into_iter()
            .max_by_key(|(specificity, _)| *specificity)
            .map(|(_, project_id)| project_id)
            .unwrap_or_else(|| {
                #[cfg(debug_assertions)]
                eprintln!("Review uses unregistered Git root: {}", git_root.display());
                0
            });
        // The agent sandbox protects only the `.git` of a registered root.
        // A running agent can write other repositories, for example a
        // nested repository that the repository scan found. Thus git runs
        // in the read-only sandbox for each other root.
        let git = if registered_root {
            ReviewGit::Managed
        } else {
            ReviewGit::Sandboxed
        };
        return Ok(ResolvedReviewRoot {
            project_id,
            root: git_root,
            git,
        });
    }

    let project_id = project_id.ok_or_else(|| "A project is required for review".to_string())?;
    let root = project_workspace_root(storage, project_id, workspace_id)?;
    Ok(ResolvedReviewRoot {
        project_id,
        root,
        git: ReviewGit::Managed,
    })
}

fn project_workspace_root(
    storage: &Storage,
    project_id: i64,
    workspace_id: Option<&str>,
) -> ProjectResult<PathBuf> {
    if let Some(workspace_id) = workspace_id {
        let workspace = storage
            .get_workspace(workspace_id)
            .map_err(|error| error.to_string())?;
        if workspace.project_id != project_id || workspace.state != "available" {
            return Err("workspace is unavailable or belongs to another project".to_string());
        }
        return validate_registered_project_root(Path::new(&workspace.root_path));
    }
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    validate_registered_project(&project)
}

fn register_project(storage: &Storage, root: &Path) -> ProjectResult<Project> {
    let root_path = root.to_string_lossy().to_string();
    let name = root
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&root_path);
    storage
        .upsert_project(name, &root_path)
        .map_err(|error| error.to_string())
}

pub(crate) fn canonical_project_root(path: &str) -> ProjectResult<PathBuf> {
    let expanded = expand_tilde(path.trim())?;
    canonical_project_root_path(&expanded)
}

pub(crate) fn validate_registered_project(project: &Project) -> ProjectResult<PathBuf> {
    validate_registered_project_root(Path::new(&project.root_path))
}

pub(crate) fn validate_registered_project_root(path: &Path) -> ProjectResult<PathBuf> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "project path is no longer available; remove and re-add it: {} ({error})",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "project path identity changed; remove and re-add it: {}",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "failed to revalidate project path `{}`: {error}",
            path.display()
        )
    })?;
    if canonical != path {
        return Err(format!(
            "project path identity changed; remove and re-add it: {}",
            path.display()
        ));
    }
    Ok(canonical)
}

fn canonical_project_root_path(path: &Path) -> ProjectResult<PathBuf> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize path `{}`: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "project path must be an existing directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn clone_parent(dest_parent: Option<&str>) -> ProjectResult<PathBuf> {
    let parent = match dest_parent.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) => expand_tilde(path)?,
        None => default_clone_parent()?,
    };
    canonical_project_root_path(&parent)
}

fn default_clone_parent() -> ProjectResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set; pass a destination parent".to_string())?;
    Ok(home.join("programming"))
}

fn expand_tilde(path: &str) -> ProjectResult<PathBuf> {
    if path.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    if path == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_string())?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(path))
}

/// Accept only unambiguous GitHub repo references and return the URL to clone.
/// Allowed forms: `https://github.com/<owner>/<repo>[.git]`,
/// `git@github.com:<owner>/<repo>[.git]`, and bare `<owner>/<repo>` slugs
/// (normalized to an https URL). Everything else — other hosts, git transport
/// schemes (`ext::`, `fd::`, `file://`), leading `-`, whitespace — is rejected.
fn validate_github_url(url: &str) -> ProjectResult<String> {
    if url.is_empty() {
        return Err("GitHub repository URL cannot be empty".to_string());
    }
    if url.starts_with('-')
        || url.contains("::")
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("unsupported repository URL".to_string());
    }

    let is_valid_seg = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let check = |owner: &str, repo: &str| -> ProjectResult<()> {
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if is_valid_seg(owner) && is_valid_seg(repo) {
            Ok(())
        } else {
            Err("unsupported repository URL".to_string())
        }
    };

    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        check(owner, repo)?;
        return Ok(url.to_string());
    }
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        check(owner, repo)?;
        return Ok(url.to_string());
    }
    // Bare owner/repo slug (no scheme, exactly one slash).
    if !url.contains(':') {
        let mut parts = url.splitn(2, '/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        if !repo.contains('/') {
            check(owner, repo)?;
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            return Ok(format!("https://github.com/{owner}/{repo}"));
        }
    }

    Err("unsupported repository URL".to_string())
}

fn infer_repo_dir_name(url: &str) -> ProjectResult<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let last_separator = trimmed
        .rfind(['/', ':'])
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut name = trimmed[last_separator..].trim();
    if let Some(stripped) = name.strip_suffix(".git") {
        name = stripped;
    }
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("could not infer a safe clone directory name from repository URL".to_string());
    }
    Ok(name.to_string())
}

fn gh_is_authenticated() -> bool {
    let mut command = Command::new("gh");
    command.args(["auth", "status"]);
    crate::process::hide_console(&mut command)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn search_project_files(
    project_root: &Path,
    query: &str,
) -> ProjectResult<Vec<ProjectFileSearchResult>> {
    let root = project_root
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize project root: {error}"))?;
    let mut candidates =
        git_file_candidates(&root).unwrap_or_else(|| fallback_file_candidates(&root));
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);

    let query = query.trim();
    if query.is_empty() {
        candidates.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.path.cmp(&right.path))
        });
        return Ok(candidates
            .into_iter()
            .take(FILE_SEARCH_LIMIT)
            .map(|candidate| ProjectFileSearchResult {
                path: candidate.path,
            })
            .collect());
    }

    let mut scored = candidates
        .into_iter()
        .filter_map(|candidate| {
            file_match_score(&candidate.path, query).map(|score| (score, candidate))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        left_score
            .cmp(right_score)
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(scored
        .into_iter()
        .take(FILE_SEARCH_LIMIT)
        .map(|(_, candidate)| ProjectFileSearchResult {
            path: candidate.path,
        })
        .collect())
}

#[derive(Debug, Clone)]
struct FileSearchCandidate {
    path: String,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FileMatchScore {
    outside_basename: bool,
    path_len: usize,
    first_match: usize,
    match_sum: usize,
}

fn git_file_candidates(root: &Path) -> Option<Vec<FileSearchCandidate>> {
    let files = git_project_files(root)?;
    Some(
        files
            .into_iter()
            .filter_map(|path| candidate_for_relative_path(root, &path))
            .collect(),
    )
}

pub(crate) fn git_project_files(root: &Path) -> Option<Vec<String>> {
    let output = exec::managed_git_command(root)
        .ok()?
        .args(["ls-files", "-co", "--exclude-standard"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Some(files)
}

fn fallback_file_candidates(root: &Path) -> Vec<FileSearchCandidate> {
    let mut candidates = Vec::new();
    let mut visited_files = 0_usize;
    walk_project_files(root, root, &mut visited_files, &mut candidates);
    candidates
}

fn walk_project_files(
    root: &Path,
    dir: &Path,
    visited_files: &mut usize,
    candidates: &mut Vec<FileSearchCandidate>,
) {
    if *visited_files >= FALLBACK_WALK_LIMIT {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if *visited_files >= FALLBACK_WALK_LIMIT {
            break;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let path = entry.path();
        if file_type.is_dir() {
            if should_skip_fallback_dir(&entry.file_name()) {
                continue;
            }
            walk_project_files(root, &path, visited_files, candidates);
        } else if file_type.is_symlink() {
            continue;
        } else if file_type.is_file() {
            *visited_files += 1;
            if let Some(relative) = path_to_project_relative(root, &path) {
                candidates.push(FileSearchCandidate {
                    path: relative,
                    modified: path
                        .symlink_metadata()
                        .and_then(|metadata| metadata.modified())
                        .ok(),
                });
            }
        }
    }
}

fn should_skip_fallback_dir(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    matches!(name.as_ref(), ".git" | "node_modules" | "target" | "dist") || name.starts_with('.')
}

fn candidate_for_relative_path(root: &Path, relative_path: &str) -> Option<FileSearchCandidate> {
    let full_path = root.join(relative_path);
    let metadata = full_path.symlink_metadata().ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    Some(FileSearchCandidate {
        path: relative_path.replace('\\', "/"),
        modified: metadata.modified().ok(),
    })
}

fn path_to_project_relative(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn file_match_score(path: &str, query: &str) -> Option<FileMatchScore> {
    let path_positions = subsequence_positions(path, query)?;
    let basename = Path::new(path)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(path);
    let basename_positions = subsequence_positions(basename, query);
    let positions = basename_positions.as_ref().unwrap_or(&path_positions);

    Some(FileMatchScore {
        outside_basename: basename_positions.is_none(),
        path_len: path.chars().count(),
        first_match: positions.first().copied().unwrap_or(usize::MAX),
        match_sum: positions.iter().sum(),
    })
}

fn subsequence_positions(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return Some(Vec::new());
    }

    let mut positions = Vec::new();
    let haystack = haystack.to_lowercase();
    let mut haystack_chars = haystack.chars().enumerate();
    for needle_char in needle.chars() {
        let (index, _) = haystack_chars.find(|(_, haystack_char)| *haystack_char == needle_char)?;
        positions.push(index);
    }
    Some(positions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{new_uuid_v4, NewWorkspace};
    use std::fs;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-projects-{name}-{}", new_uuid_v4()))
    }

    fn run_test_git(root: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git should run")
    }

    fn assert_test_git(root: &Path, args: &[&str]) {
        let output = run_test_git(root, args);
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn initialize_review_test_repo(root: &Path) -> PathBuf {
        fs::create_dir_all(root).expect("repository root should create");
        assert_test_git(root, &["init", "-q"]);
        assert_test_git(root, &["config", "user.email", "halv@example.invalid"]);
        assert_test_git(root, &["config", "user.name", "Halv Tests"]);
        fs::write(root.join("tracked.txt"), "base\n").expect("tracked file should write");
        assert_test_git(root, &["add", "tracked.txt"]);
        assert_test_git(root, &["commit", "-qm", "initial"]);
        root.canonicalize().expect("repository should canonicalize")
    }

    /// Reports whether Halv can start git in the read-only sandbox here.
    /// Without the sandbox, the scan reports no repositories, and the review
    /// of a nested repository returns an error.
    fn git_sandbox_available() -> bool {
        let probe = test_path("git-sandbox-probe");
        fs::create_dir_all(&probe).expect("probe directory should create");
        let available = exec::sandboxed_git_command(&probe, ["--version"])
            .and_then(|mut command| command.output().map_err(|error| error.to_string()))
            .is_ok_and(|output| output.status.success());
        let _ = fs::remove_dir_all(probe);
        if !available {
            eprintln!("the read-only sandbox cannot start git here; sandbox assertions skipped");
        }
        available
    }

    thread_local! {
        /// Config entries that a test adds to each review git command.
        static REVIEW_GIT_TEST_CONFIG: std::cell::RefCell<Vec<(String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Adds the config entries of the current test to a review git command.
    pub(super) fn with_injected_review_git_config(mut command: Command) -> Command {
        REVIEW_GIT_TEST_CONFIG
            .with(|entries| exec::add_command_config(&mut command, entries.borrow().clone()));
        command
    }

    /// Adds config entries to each review git command on this thread until
    /// the guard drops.
    struct InjectedReviewGitConfig;

    impl InjectedReviewGitConfig {
        fn set(entries: Vec<(String, String)>) -> Self {
            REVIEW_GIT_TEST_CONFIG.with(|current| *current.borrow_mut() = entries);
            Self
        }
    }

    impl Drop for InjectedReviewGitConfig {
        fn drop(&mut self) {
            REVIEW_GIT_TEST_CONFIG.with(|current| current.borrow_mut().clear());
        }
    }

    /// Sets an old time stamp on a file, so that git must read the file
    /// again to compare it with the index.
    fn make_stat_dirty(path: &Path) {
        fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| {
                file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
            })
            .expect("time stamp should change");
    }

    #[test]
    fn commit_and_push_publish_the_current_branch() {
        let base = test_path("publish");
        let root = base.join("project");
        let remote = base.join("remote.git");
        fs::create_dir_all(&root).expect("project root should create");
        fs::create_dir_all(&remote).expect("remote root should create");
        assert_test_git(&root, &["init", "-q"]);
        assert_test_git(&root, &["config", "user.email", "halv@example.invalid"]);
        assert_test_git(&root, &["config", "user.name", "Halv Tests"]);
        assert_test_git(&remote, &["init", "--bare", "-q"]);

        fs::write(root.join("tracked.txt"), "base\n").expect("base file should write");
        assert_test_git(&root, &["add", "tracked.txt"]);
        assert_test_git(&root, &["commit", "-qm", "initial"]);
        fs::write(root.join("tracked.txt"), "changed\n").expect("change should write");
        assert_test_git(&root, &["add", "tracked.txt"]);

        let commit = commit_staged_changes(&root, "publish from Halv")
            .expect("staged changes should commit");
        assert!(!commit.trim().is_empty());
        let subject = run_test_git(&root, &["log", "-1", "--pretty=%s"]);
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "publish from Halv"
        );
        assert!(commit_staged_changes(&root, "nothing else").is_err());
        assert!(commit_staged_changes(&root, "   ").is_err());

        let remote_path = remote.to_string_lossy().to_string();
        assert_test_git(&root, &["remote", "add", "origin", &remote_path]);
        let branch = run_test_git(&root, &["branch", "--show-current"]);
        let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
        let pushed = push_current_branch(&root).expect("current branch should push");
        assert!(!pushed.trim().is_empty());
        let refname = format!("refs/heads/{branch}");
        assert_test_git(&remote, &["show-ref", "--verify", &refname]);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn canonicalization_rejects_non_directories() {
        let file_path = test_path("file");
        fs::write(&file_path, "not a directory").expect("test file should write");

        let error = canonical_project_root(&file_path.to_string_lossy())
            .expect_err("file paths must be rejected");
        assert!(error.contains("project path must be an existing directory"));

        let missing_path = test_path("missing");
        let missing_error = canonical_project_root(&missing_path.to_string_lossy())
            .expect_err("missing paths must be rejected");
        assert!(missing_error.contains("failed to canonicalize path"));

        let _ = fs::remove_file(file_path);
    }

    #[cfg(unix)]
    #[test]
    fn registered_project_rejects_root_replaced_by_symlink() {
        let base = test_path("root-swap");
        let root = base.join("project");
        let outside = base.join("outside");
        fs::create_dir_all(&root).expect("project root should create");
        fs::create_dir_all(&outside).expect("outside root should create");
        let registered = root.canonicalize().expect("project should canonicalize");
        fs::remove_dir(&root).expect("project root should remove");
        std::os::unix::fs::symlink(&outside, &root).expect("replacement symlink should create");

        let error = validate_registered_project_root(&registered)
            .expect_err("replaced project roots must be rejected");

        assert!(error.contains("identity changed"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn validate_github_url_accepts_only_github_forms() {
        assert_eq!(
            validate_github_url("https://github.com/openai/codex.git").unwrap(),
            "https://github.com/openai/codex.git"
        );
        assert_eq!(
            validate_github_url("git@github.com:openai/codex.git").unwrap(),
            "git@github.com:openai/codex.git"
        );
        assert_eq!(
            validate_github_url("openai/codex").unwrap(),
            "https://github.com/openai/codex"
        );
        // RCE / injection vectors and non-github hosts must all be rejected.
        for bad in [
            "ext::sh -c 'touch /tmp/pwned'",
            "-oProxyCommand=sh",
            "--upload-pack=touch /tmp/x",
            "file:///etc/passwd",
            "https://evil.com/openai/codex",
            "git@gitlab.com:openai/codex.git",
            "https://github.com/../../etc",
            "https://github.com/openai",
            "fd::17/foo",
        ] {
            assert!(validate_github_url(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn repo_dir_name_is_inferred_from_supported_urls() {
        assert_eq!(
            infer_repo_dir_name("https://github.com/openai/codex.git")
                .expect("https url should parse"),
            "codex"
        );
        assert_eq!(
            infer_repo_dir_name("git@github.com:openai/codex.git").expect("ssh url should parse"),
            "codex"
        );
        assert_eq!(
            infer_repo_dir_name("openai/codex").expect("slug should parse"),
            "codex"
        );
    }

    #[test]
    fn project_search_files_uses_bounded_fallback_and_fuzzy_scores() {
        let root = test_path("search-root");
        fs::create_dir_all(root.join("src")).expect("src should create");
        fs::create_dir_all(root.join("docs")).expect("docs should create");
        fs::create_dir_all(root.join("node_modules/pkg")).expect("skip dir should create");
        fs::create_dir_all(root.join(".hidden")).expect("hidden dir should create");
        fs::write(root.join("src/main.rs"), "fn main() {}").expect("main should write");
        fs::write(root.join("src/mod_file.rs"), "pub mod x;").expect("mod should write");
        fs::write(root.join("docs/main-notes.md"), "notes").expect("docs should write");
        fs::write(root.join("node_modules/pkg/main.js"), "skip").expect("skip should write");
        fs::write(root.join(".hidden/main.txt"), "skip").expect("hidden should write");

        let results = search_project_files(&root, "main").expect("search should run");
        let paths = results
            .iter()
            .map(|result| result.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths[0], "src/main.rs");
        assert!(paths.contains(&"docs/main-notes.md"));
        assert!(!paths.contains(&"node_modules/pkg/main.js"));
        assert!(!paths.contains(&".hidden/main.txt"));

        let empty = search_project_files(&root, "").expect("empty search should run");
        assert!(empty.len() <= FILE_SEARCH_LIMIT);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_git_root_resolves_from_directories_and_files() {
        let base = test_path("git-root-for-path");
        let root = initialize_review_test_repo(&base.join("project"));
        let nested = root.join("src/nested");
        fs::create_dir_all(&nested).expect("repository subdirectory should create");
        let file = nested.join("main.rs");
        fs::write(&file, "fn main() {}\n").expect("repository file should write");
        let expected = root.to_string_lossy().into_owned();

        assert_eq!(
            tauri::async_runtime::block_on(project_git_root_for_path(
                nested.to_string_lossy().into_owned(),
            )),
            Some(expected.clone())
        );
        assert_eq!(
            tauri::async_runtime::block_on(project_git_root_for_path(
                file.to_string_lossy().into_owned(),
            )),
            Some(expected)
        );
        assert_eq!(
            tauri::async_runtime::block_on(project_git_root_for_path(
                base.join("missing").to_string_lossy().into_owned(),
            )),
            None
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn adoption_refuses_the_home_root_the_app_data_and_temporary_folders() {
        let guards = AdoptionGuards {
            home: Some(PathBuf::from("/halv-adoption-test/home/ada")),
            blocked: vec![
                PathBuf::from("/halv-adoption-test/tmp"),
                PathBuf::from("/halv-adoption-test/home/ada/Library/Application Support/ai.halv"),
            ],
        };
        let allowed = |path: &str| adoption_root_allowed(Path::new(path), &guards);
        assert!(allowed("/halv-adoption-test/home/ada/code/halv"));
        assert!(
            !allowed("/halv-adoption-test/home/ada"),
            "the home root is refused"
        );
        assert!(
            !allowed("/halv-adoption-test/home"),
            "a folder above home is refused"
        );
        assert!(!allowed("/"), "the filesystem root is refused");
        assert!(!allowed("/halv-adoption-test/tmp/scratch"));
        assert!(!allowed(
            "/halv-adoption-test/home/ada/Library/Application Support/ai.halv/worktrees/x"
        ));
        // Whole components only.
        assert!(allowed("/halv-adoption-test/tmpfiles/repo"));
        // Without a known home, the other rules still hold.
        let no_home = AdoptionGuards {
            home: None,
            blocked: Vec::new(),
        };
        assert!(adoption_root_allowed(
            Path::new("/halv-adoption-test/x"),
            &no_home
        ));
        assert!(!adoption_root_allowed(Path::new("/"), &no_home));
    }

    #[test]
    fn a_linked_worktree_adopts_into_its_main_repository() {
        let base = test_path("adoption-linked-worktree");
        let main = base.join("app");
        fs::create_dir_all(main.join(".git/worktrees/feature"))
            .expect("main repository marker should create");
        let main = main.canonicalize().expect("main should canonicalize");
        let absolute = base.join("wt-feature");
        fs::create_dir_all(&absolute).expect("worktree should create");
        fs::write(
            absolute.join(".git"),
            format!(
                "gitdir: {}\n",
                main.join(".git/worktrees/feature").display()
            ),
        )
        .expect("worktree marker should write");
        assert_eq!(main_worktree_root(&absolute), Some(main.clone()));

        let relative = base.join("tmp/wt-relative");
        fs::create_dir_all(&relative).expect("relative worktree should create");
        fs::write(
            relative.join(".git"),
            "gitdir: ../../app/.git/worktrees/feature\n",
        )
        .expect("relative marker should write");
        assert_eq!(main_worktree_root(&relative), Some(main.clone()));

        // A submodule and a worktree of a bare repository keep their own root.
        let module = base.join("module");
        fs::create_dir_all(&module).expect("module should create");
        fs::write(module.join(".git"), "gitdir: ../app/.git/modules/module\n")
            .expect("module marker should write");
        assert_eq!(main_worktree_root(&module), None);
        let bare_worktree = base.join("bare-worktree");
        fs::create_dir_all(base.join("bare.git/worktrees/one")).expect("bare should create");
        fs::create_dir_all(&bare_worktree).expect("bare worktree should create");
        fs::write(
            bare_worktree.join(".git"),
            "gitdir: ../bare.git/worktrees/one\n",
        )
        .expect("bare marker should write");
        assert_eq!(main_worktree_root(&bare_worktree), None);
        assert_eq!(main_worktree_root(&main), None);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn adoption_root_is_the_git_root_of_an_allowed_folder() {
        let base = test_path("adoption-root");
        let root = initialize_review_test_repo(&base.join("project"));
        let nested = root.join("src");
        fs::create_dir_all(&nested).expect("repository subdirectory should create");
        let open = AdoptionGuards {
            home: None,
            blocked: Vec::new(),
        };
        assert_eq!(
            adoption_root_for_path(&nested, &open),
            Some(root.to_string_lossy().into_owned())
        );
        // The test repository is in the temporary folder, which is refused.
        let current = AdoptionGuards::current(None);
        assert_eq!(adoption_root_for_path(&nested, &current), None);
        // A folder outside git has no root.
        let plain = base.join("plain");
        fs::create_dir_all(&plain).expect("plain folder should create");
        assert_eq!(adoption_root_for_path(&plain, &open), None);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_finds_changed_repos_within_bounds() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-under-path");
        fs::create_dir_all(&base).expect("scan root should create");
        let clean = initialize_review_test_repo(&base.join("clean"));
        let dirty = initialize_review_test_repo(&base.join("group/dirty"));
        fs::write(dirty.join("tracked.txt"), "changed\n").expect("dirty file should write");

        let hidden = initialize_review_test_repo(&base.join(".hidden/repo"));
        fs::write(hidden.join("tracked.txt"), "changed\n").expect("hidden dirty file should write");
        let dependency = initialize_review_test_repo(&base.join("node_modules/repo"));
        fs::write(dependency.join("tracked.txt"), "changed\n")
            .expect("dependency dirty file should write");
        let nested = initialize_review_test_repo(&base.join("one/two/repo"));
        fs::write(nested.join("tracked.txt"), "changed\n").expect("nested dirty file should write");
        let too_deep = initialize_review_test_repo(&base.join("three/levels/under/repo"));
        fs::write(too_deep.join("tracked.txt"), "changed\n").expect("deep dirty file should write");

        let repos = git_repos_under_path(&base);
        assert_eq!(
            repos,
            vec![
                dirty.to_string_lossy().into_owned(),
                nested.to_string_lossy().into_owned(),
            ]
        );
        assert!(!repos.contains(&clean.to_string_lossy().into_owned()));
        assert!(!repos.contains(&too_deep.to_string_lossy().into_owned()));

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_repo_scan_does_not_run_repository_programs() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-programs");
        let scan_root = base.join("scan");
        let sentinel = base.join("program-ran");
        let nested = initialize_review_test_repo(&scan_root.join("nested"));
        exec::plant_repository_programs(&nested, &sentinel);

        let repos = git_repos_under_path(&scan_root);
        assert_eq!(repos, vec![nested.to_string_lossy().into_owned()]);
        assert!(
            !sentinel.exists(),
            "the repository scan must not run programs from the repository config"
        );

        assert_test_git(&nested, &["status", "--porcelain"]);
        assert!(
            sentinel.exists(),
            "the planted programs should run in plain Git"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn review_diffs_do_not_run_repository_programs() {
        let base = test_path("review-programs");
        let sentinel = base.join("program-ran");
        let root = initialize_review_test_repo(&base.join("project"));
        exec::plant_repository_programs(&root, &sentinel);
        fs::write(root.join("tracked.txt"), "changed\n").expect("tracked file should change");
        fs::write(root.join("untracked.txt"), "untracked\n").expect("untracked file should write");

        for scope in [
            ProjectReviewScope::WorkingTree,
            ProjectReviewScope::Unstaged,
            ProjectReviewScope::Staged,
        ] {
            review_snapshot(1, &root, ReviewGit::Managed, scope, None, None)
                .expect("review diff should load");
            review_file_snapshot(1, &root, ReviewGit::Managed, "tracked.txt", scope, None, 3)
                .expect("review file diff should load");
        }
        review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Commit,
            Some("HEAD"),
            None,
        )
        .expect("commit review should load");
        assert!(
            !sentinel.exists(),
            "review diffs must not run programs from the repository config"
        );

        assert_test_git(&root, &["diff"]);
        assert!(
            sentinel.exists(),
            "the planted programs should run in plain Git"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn review_diffs_keep_a_standard_repository_lfs_filter() {
        let base = test_path("review-lfs");
        let root = initialize_review_test_repo(&base.join("project"));
        let lfs = exec::FakeGitLfs::new(&base);
        exec::plant_standard_lfs_filter(&root, "*.bin");
        fs::write(root.join("large.bin"), "large content\n").expect("file should write");
        lfs.git(&root, &["add", "large.bin"]);
        lfs.git(&root, &["commit", "-q", "-m", "large file"]);
        // The clean filter gives the same pointer for the new content, so
        // the diff must not show the file.
        fs::write(root.join("large.bin"), "other large content\n").expect("file should change");
        fs::write(root.join("tracked.txt"), "changed\n").expect("tracked file should change");
        let runs = lfs.runs().len();

        let _environment = lfs.activate();
        for scope in [
            ProjectReviewScope::WorkingTree,
            ProjectReviewScope::Unstaged,
        ] {
            let snapshot = review_snapshot(1, &root, ReviewGit::Managed, scope, None, None)
                .expect("review diff should load");
            assert!(snapshot.diff.contains("tracked.txt"), "{scope:?}");
            assert!(!snapshot.diff.contains("large.bin"), "{scope:?}");
        }
        assert!(
            lfs.runs().len() > runs,
            "review diffs must keep a standard git-lfs filter from the repository config"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn repository_scan_and_review_do_not_start_git_in_submodules() {
        let base = test_path("submodule-programs");
        let scan_root = base.join("scan");
        let sentinel = base.join("submodule-program-ran");
        let nested = initialize_review_test_repo(&scan_root.join("nested"));
        exec::plant_submodule_program(&nested, &sentinel);
        fs::write(nested.join("untracked.txt"), "untracked\n")
            .expect("untracked file should write");

        let repos = git_repos_under_path(&scan_root);
        if git_sandbox_available() {
            assert_eq!(repos, vec![nested.to_string_lossy().into_owned()]);
        }
        for scope in [
            ProjectReviewScope::WorkingTree,
            ProjectReviewScope::Unstaged,
            ProjectReviewScope::Staged,
        ] {
            review_snapshot(1, &nested, ReviewGit::Managed, scope, None, None)
                .expect("review diff should load");
            review_file_snapshot(1, &nested, ReviewGit::Managed, "sub", scope, None, 3)
                .expect("review file diff should load");
        }
        review_snapshot(
            1,
            &nested,
            ReviewGit::Managed,
            ProjectReviewScope::Commit,
            Some("HEAD"),
            None,
        )
        .expect("commit review should load");
        assert!(
            !sentinel.exists(),
            "the scan and the review diffs must not start git in a submodule"
        );

        assert_test_git(&nested, &["diff"]);
        assert!(
            sentinel.exists(),
            "plain Git should run the submodule filter"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_repo_scan_runs_git_in_the_read_only_sandbox() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-sandbox");
        let sentinel = base.join("escaped-program-ran");
        let root = initialize_review_test_repo(&base.join("nested"));
        fs::write(root.join(".gitattributes"), "*.txt filter=escaped\n")
            .expect("attributes should write");
        assert_test_git(&root, &["add", ".gitattributes"]);
        assert_test_git(&root, &["commit", "-qm", "attributes"]);
        make_stat_dirty(&root.join("tracked.txt"));
        let script = base.join("escaped-program");
        exec::write_sentinel_program(&script, &sentinel, "cat");
        // This entry acts as a filter that the overrides did not see. For
        // example, a FIFO can give the config query a different config.
        let escaped = vec![(
            "filter.escaped.clean".to_string(),
            format!("'{}'", script.display()),
        )];

        let mut command = git_repo_status_command(&root).expect("scan command should prepare");
        exec::add_command_config(&mut command, escaped.clone());
        let output = command.output().expect("scan command should run");
        assert!(
            output.status.success(),
            "sandboxed status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).starts_with("##"),
            "sandboxed status should write the branch line first"
        );
        assert!(
            !sentinel.exists(),
            "a program in the scan must not write files"
        );

        let mut command = exec::managed_git_command(&root).expect("managed Git should prepare");
        exec::add_command_config(&mut command, escaped);
        command
            .args(["status", "--porcelain"])
            .output()
            .expect("managed Git should run");
        assert!(
            sentinel.exists(),
            "the program should write its file outside the sandbox"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn nested_repository_review_runs_git_in_the_read_only_sandbox() {
        let base = test_path("nested-review-sandbox");
        let workspace = base.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace should create");
        let workspace = workspace
            .canonicalize()
            .expect("workspace should canonicalize");
        let nested = initialize_review_test_repo(&workspace.join("nested"));
        fs::write(nested.join(".gitattributes"), "*.txt filter=escaped\n")
            .expect("attributes should write");
        assert_test_git(&nested, &["add", ".gitattributes"]);
        assert_test_git(&nested, &["commit", "-qm", "attributes"]);
        fs::write(nested.join("tracked.txt"), "changed\n").expect("tracked file should change");
        make_stat_dirty(&nested.join("tracked.txt"));
        fs::write(nested.join("untracked.txt"), "untracked\n")
            .expect("untracked file should write");
        let storage = Storage::open(base.join("halv.sqlite")).expect("storage should open");
        let project = storage
            .upsert_project("workspace", &workspace.to_string_lossy())
            .expect("project should insert");

        let resolved = resolve_review_root(&storage, None, None, Some(&nested.to_string_lossy()))
            .expect("nested repository should resolve");
        assert_eq!(resolved.project_id, project.id);
        assert_eq!(resolved.root, nested);
        assert_eq!(resolved.git, ReviewGit::Sandboxed);

        let sentinel = base.join("escaped-program-ran");
        let script = base.join("escaped-program");
        exec::write_sentinel_program(&script, &sentinel, "cat");
        // This entry acts as a filter that the overrides did not see. For
        // example, a FIFO can give the config query a different config.
        let _injected = InjectedReviewGitConfig::set(vec![(
            "filter.escaped.clean".to_string(),
            format!("'{}'", script.display()),
        )]);

        if !git_sandbox_available() {
            let error = review_snapshot(
                resolved.project_id,
                &resolved.root,
                resolved.git,
                ProjectReviewScope::WorkingTree,
                None,
                None,
            )
            .expect_err("a nested review must not start git without the sandbox");
            assert!(error.contains("sandbox"), "unexpected error: {error}");
            assert!(!sentinel.exists());
            drop(storage);
            let _ = fs::remove_dir_all(base);
            return;
        }

        for scope in [
            ProjectReviewScope::WorkingTree,
            ProjectReviewScope::Unstaged,
            ProjectReviewScope::Staged,
        ] {
            let snapshot = review_snapshot(
                resolved.project_id,
                &resolved.root,
                resolved.git,
                scope,
                None,
                None,
            )
            .expect("sandboxed review diff should load");
            if scope != ProjectReviewScope::Staged {
                assert!(snapshot.diff.contains("+changed"), "{}", snapshot.diff);
                assert!(snapshot.diff.contains("+untracked"), "{}", snapshot.diff);
            }
            for path in ["tracked.txt", "untracked.txt"] {
                review_file_snapshot(
                    resolved.project_id,
                    &resolved.root,
                    resolved.git,
                    path,
                    scope,
                    None,
                    3,
                )
                .expect("sandboxed review file diff should load");
            }
        }
        let commit = review_snapshot(
            resolved.project_id,
            &resolved.root,
            resolved.git,
            ProjectReviewScope::Commit,
            Some("HEAD"),
            None,
        )
        .expect("sandboxed commit review should load");
        assert!(commit.diff.contains(".gitattributes"), "{}", commit.diff);
        assert!(
            !sentinel.exists(),
            "a program in the nested review must not write files"
        );

        review_snapshot(
            resolved.project_id,
            &resolved.root,
            ReviewGit::Managed,
            ProjectReviewScope::WorkingTree,
            None,
            None,
        )
        .expect("managed review diff should load");
        assert!(
            sentinel.exists(),
            "the program should write its file outside the sandbox"
        );

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn review_diffs_do_not_start_git_diff_in_submodules() {
        let base = test_path("submodule-diff-programs");
        let sentinel = base.join("submodule-diff-program-ran");
        let root = initialize_review_test_repo(&base.join("project"));
        exec::plant_submodule_diff_program(&root, &sentinel);

        let review = |scope: ProjectReviewScope, reference: Option<&str>| {
            let snapshot = review_snapshot(1, &root, ReviewGit::Managed, scope, reference, None)
                .expect("review diff should load");
            review_file_snapshot(1, &root, ReviewGit::Managed, "sub", scope, reference, 3)
                .expect("review file diff should load");
            assert!(
                !sentinel.exists(),
                "the {} review must not start git diff in a submodule",
                scope.as_str()
            );
            assert!(
                snapshot.diff.contains("+Subproject commit "),
                "the {} review should show the gitlink: {}",
                scope.as_str(),
                snapshot.diff
            );
        };
        review(ProjectReviewScope::WorkingTree, None);
        review(ProjectReviewScope::Unstaged, None);
        assert_test_git(&root, &["add", "sub"]);
        review(ProjectReviewScope::Staged, None);
        assert_test_git(&root, &["commit", "-qm", "move submodule"]);
        review(ProjectReviewScope::Commit, Some("HEAD"));

        assert_test_git(&root, &["show", "HEAD"]);
        assert!(
            sentinel.exists(),
            "plain Git should run the submodule diff program"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_lists_a_repository_with_a_required_filter() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-required-filter");
        let root = initialize_review_test_repo(&base.join("nested"));
        // In the sandbox, a filter such as git-lfs cannot write its files.
        // The overrides also clear a filter from the repository config. In
        // both cases a required filter must not stop the status.
        for (key, value) in [
            ("filter.locked.clean", "cat"),
            ("filter.locked.required", "true"),
        ] {
            assert_test_git(&root, &["config", key, value]);
        }
        fs::write(root.join(".gitattributes"), "*.txt filter=locked\n")
            .expect("attributes should write");
        make_stat_dirty(&root.join("tracked.txt"));

        assert_eq!(
            git_repos_under_path(&base),
            vec![root.to_string_lossy().into_owned()]
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_returns_empty_inside_a_repo() {
        let base = test_path("git-repos-inside-repo");
        let root = initialize_review_test_repo(&base.join("project"));
        let nested = root.join("src/nested");
        fs::create_dir_all(&nested).expect("nested directory should create");

        assert!(git_repos_under_path(&nested).is_empty());

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn nested_repo_scan_command_scans_the_canonical_folder() {
        let base = test_path("nested-repo-command");
        let folder = base.join("halv");
        fs::create_dir_all(folder.join("web/.git")).expect("web marker should create");
        fs::create_dir_all(folder.join("app/.git")).expect("app marker should create");
        let canonical = folder.canonicalize().expect("folder should canonicalize");

        let scan = tauri::async_runtime::block_on(project_scan_nested_repos(format!(
            "  {}  ",
            folder.join("app").join("..").display()
        )))
        .expect("scan should run");
        assert_eq!(scan.path, canonical.to_string_lossy());
        assert!(!scan.git_repo);
        assert_eq!(
            scan.repos,
            [
                canonical.join("app").to_string_lossy().into_owned(),
                canonical.join("web").to_string_lossy().into_owned(),
            ]
        );
        assert!(tauri::async_runtime::block_on(project_scan_nested_repos(
            base.join("missing").to_string_lossy().into_owned()
        ))
        .is_err());

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_stops_after_twenty_returned_repositories() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-scan-limit");
        fs::create_dir_all(&base).expect("scan root should create");
        for index in 0..=GIT_REPO_SCAN_LIMIT {
            let root = base.join(format!("repo-{index:02}"));
            fs::create_dir_all(&root).expect("repository root should create");
            assert_test_git(&root, &["init", "-q"]);
            fs::write(root.join("untracked.txt"), "changed\n")
                .expect("untracked file should write");
        }

        let repos = git_repos_under_path(&base);
        assert_eq!(repos.len(), GIT_REPO_SCAN_LIMIT);
        let expected_last = base
            .join("repo-19")
            .canonicalize()
            .expect("last returned repository should canonicalize")
            .to_string_lossy()
            .into_owned();
        assert_eq!(repos.last(), Some(&expected_last));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_returns_clean_repo_ahead_of_upstream() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-ahead");
        let root = initialize_review_test_repo(&base.join("project"));
        let remote = base.join("upstream.git");
        fs::create_dir_all(&remote).expect("upstream root should create");
        assert_test_git(&remote, &["init", "--bare", "-q"]);
        let remote_path = remote.to_string_lossy().into_owned();
        assert_test_git(&root, &["remote", "add", "origin", &remote_path]);
        let branch = run_test_git(&root, &["branch", "--show-current"]);
        let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
        assert!(!branch.is_empty());
        assert_test_git(&root, &["push", "-q", "origin", &branch]);
        let upstream = format!("origin/{branch}");
        assert_test_git(&root, &["branch", "--set-upstream-to", &upstream]);
        fs::write(root.join("tracked.txt"), "ahead\n").expect("ahead file should write");
        assert_test_git(&root, &["add", "tracked.txt"]);
        assert_test_git(&root, &["commit", "-qm", "ahead"]);

        assert_eq!(
            git_repos_under_path(&base),
            vec![root.to_string_lossy().into_owned()]
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_omits_clean_repo_without_upstream() {
        let base = test_path("git-repos-clean-no-upstream");
        let root = initialize_review_test_repo(&base.join("project"));

        assert!(git_repos_under_path(&base).is_empty());
        assert!(!git_repo_has_changes(&root));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn git_repo_scan_inspects_past_clean_repo_return_limit() {
        if !git_sandbox_available() {
            return;
        }
        let base = test_path("git-repos-clean-before-dirty");
        fs::create_dir_all(&base).expect("scan root should create");
        for index in 0..=GIT_REPO_SCAN_LIMIT {
            initialize_review_test_repo(&base.join(format!("clean-{index:02}")));
        }
        let dirty = initialize_review_test_repo(&base.join("z-dirty"));
        fs::write(dirty.join("tracked.txt"), "changed\n").expect("dirty file should write");

        assert_eq!(
            git_repos_under_path(&base),
            vec![dirty.to_string_lossy().into_owned()]
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn review_root_resolves_from_a_registered_project_subdirectory() {
        let base = test_path("resolve-project-subdirectory");
        let root = initialize_review_test_repo(&base.join("project"));
        let subdirectory = root.join("src/nested");
        fs::create_dir_all(&subdirectory).expect("repository subdirectory should create");
        let storage = Storage::open(base.join("halv.sqlite")).expect("storage should open");
        let project = storage
            .upsert_project("repo", &root.to_string_lossy())
            .expect("project should insert");

        let resolved = resolve_review_root(
            &storage,
            Some(project.id + 1),
            Some("missing-workspace"),
            Some(&subdirectory.to_string_lossy()),
        )
        .expect("cwd should override the project and workspace key");

        assert_eq!(resolved.project_id, project.id);
        assert_eq!(resolved.root, root);
        assert_eq!(resolved.git, ReviewGit::Managed);
        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn review_root_resolves_from_a_registered_worktree() {
        let base = test_path("resolve-worktree");
        let root = initialize_review_test_repo(&base.join("project"));
        let worktree_path = base.join("worktree");
        let worktree_arg = worktree_path.to_string_lossy().to_string();
        assert_test_git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "review-worktree",
                &worktree_arg,
            ],
        );
        let worktree = worktree_path
            .canonicalize()
            .expect("worktree should canonicalize");
        let subdirectory = worktree.join("src");
        fs::create_dir_all(&subdirectory).expect("worktree subdirectory should create");
        let storage = Storage::open(base.join("halv.sqlite")).expect("storage should open");
        let project = storage
            .upsert_project("repo", &root.to_string_lossy())
            .expect("project should insert");
        storage
            .insert_managed_workspace(NewWorkspace {
                project_id: project.id,
                name: "review-worktree",
                root_path: &worktree.to_string_lossy(),
                git_common_dir: None,
                base_ref: Some("HEAD"),
                base_commit: None,
                branch: Some("review-worktree"),
                local_environment_config_path: None,
            })
            .expect("workspace should insert");

        let resolved =
            resolve_review_root(&storage, None, None, Some(&subdirectory.to_string_lossy()))
                .expect("registered worktree should resolve");

        assert_eq!(resolved.project_id, project.id);
        assert_eq!(resolved.root, worktree);
        assert_eq!(resolved.git, ReviewGit::Managed);
        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn review_root_allows_an_unregistered_repository() {
        let base = test_path("allow-unregistered-review-root");
        let registered = initialize_review_test_repo(&base.join("registered"));
        let outside = initialize_review_test_repo(&base.join("outside"));
        let storage = Storage::open(base.join("halv.sqlite")).expect("storage should open");
        storage
            .upsert_project("registered", &registered.to_string_lossy())
            .expect("project should insert");

        let resolved = resolve_review_root(&storage, None, None, Some(&outside.to_string_lossy()))
            .expect("unregistered repository should resolve");

        assert_eq!(resolved.project_id, 0);
        assert_eq!(resolved.root, outside);
        assert_eq!(resolved.git, ReviewGit::Sandboxed);
        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn review_file_snapshot_uses_the_requested_context() {
        let base = test_path("review-file-context");
        let root = initialize_review_test_repo(&base.join("project"));
        let original = (1..=12)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(root.join("tracked.txt"), &original).expect("tracked file should update");
        assert_test_git(&root, &["add", "tracked.txt"]);
        assert_test_git(&root, &["commit", "-qm", "add context lines"]);
        let changed = original.replace("line 6\n", "changed line 6\n");
        fs::write(root.join("tracked.txt"), changed).expect("tracked file should change");

        let snapshot = review_file_snapshot(
            7,
            &root,
            ReviewGit::Managed,
            "tracked.txt",
            ProjectReviewScope::Unstaged,
            None,
            1,
        )
        .expect("file diff should build");

        assert_eq!(snapshot.project_id, 7);
        assert!(snapshot.diff.contains("@@ -5,3 +5,3 @@"));
        assert!(snapshot.diff.contains(" line 5"));
        assert!(snapshot.diff.contains("-line 6"));
        assert!(snapshot.diff.contains("+changed line 6"));
        assert!(snapshot.diff.contains(" line 7"));
        assert!(!snapshot.diff.contains("\n line 4\n"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn review_snapshot_includes_tracked_and_untracked_changes() {
        let root = test_path("review-root");
        fs::create_dir_all(&root).expect("review root should create");
        let run_git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "halv@example.invalid"]);
        run_git(&["config", "user.name", "Halv Tests"]);
        fs::write(root.join("tracked.txt"), "before\n").expect("tracked file should write");
        run_git(&["add", "tracked.txt"]);
        run_git(&["commit", "-qm", "initial"]);
        fs::write(root.join("tracked.txt"), "after\n").expect("tracked file should update");
        fs::write(root.join("untracked.txt"), "new\n").expect("untracked file should write");

        let snapshot = review_snapshot(
            7,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::WorkingTree,
            None,
            None,
        )
        .expect("review snapshot should build");
        assert_eq!(snapshot.project_id, 7);
        assert!(!snapshot.truncated);
        assert!(snapshot.head.is_some());
        assert!(snapshot.diff.contains("tracked.txt"));
        assert!(snapshot.diff.contains("untracked.txt"));
        assert!(snapshot.diff.contains("-before"));
        assert!(snapshot.diff.contains("+after"));
        assert!(snapshot.diff.contains("+new"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_scopes_separate_staged_unstaged_branch_and_commit_diffs() {
        let root = test_path("review-scopes");
        fs::create_dir_all(&root).expect("review root should create");
        let run_git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "halv@example.invalid"]);
        run_git(&["config", "user.name", "Halv Tests"]);
        fs::write(root.join("tracked.txt"), "base\n").expect("base should write");
        run_git(&["add", "tracked.txt"]);
        run_git(&["commit", "-qm", "initial"]);
        run_git(&["branch", "baseline"]);

        fs::write(root.join("tracked.txt"), "staged\n").expect("staged should write");
        run_git(&["add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "working\n").expect("working should write");

        let staged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Staged,
            None,
            None,
        )
        .expect("staged snapshot should build");
        assert!(staged.diff.contains("+staged"));
        assert!(!staged.diff.contains("+working"));

        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("unstaged snapshot should build");
        assert!(unstaged.diff.contains("-staged"));
        assert!(unstaged.diff.contains("+working"));

        run_git(&["restore", "--worktree", "tracked.txt"]);
        run_git(&["commit", "-qm", "feature change"]);
        let branch = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Branch,
            Some("baseline"),
            None,
        )
        .expect("branch snapshot should build");
        assert!(branch.diff.contains("+staged"));

        let commit = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Commit,
            Some("HEAD"),
            None,
        )
        .expect("commit snapshot should build");
        assert!(commit.diff.contains("+staged"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_last_turn_accepts_a_bound_managed_worktree_and_rejects_other_roots() {
        let base = test_path("last-turn-worktree");
        let primary = base.join("primary");
        let worktree = base.join("managed");
        fs::create_dir_all(&primary).expect("primary root should create");
        fs::create_dir_all(&worktree).expect("managed root should create");
        let storage = Storage::open(base.join("halv.sqlite")).expect("storage should open");
        let project = storage
            .upsert_project("repo", &primary.to_string_lossy())
            .expect("project should insert");
        let workspace = storage
            .insert_managed_workspace(NewWorkspace {
                project_id: project.id,
                name: "review-worktree",
                root_path: &worktree.to_string_lossy(),
                git_common_dir: None,
                base_ref: Some("HEAD"),
                base_commit: Some("0123456789abcdef"),
                branch: Some("review-worktree"),
                local_environment_config_path: None,
            })
            .expect("workspace should insert");
        let conversation_id = storage
            .ensure_conversation_in_workspace(
                None,
                "Review the worktree turn",
                "gpt-test",
                Some(project.id),
                Some(&workspace.id),
            )
            .expect("conversation should insert");
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        storage
            .upsert_turn_diff(&conversation_id, "turn-a", &worktree, diff, true)
            .expect("turn diff should insert");

        let review = last_turn_review_diff(&storage, project.id, &worktree, Some(&conversation_id))
            .expect("managed worktree turn should load");
        assert_eq!(review.identity.as_deref(), Some("turn-a"));
        assert_eq!(review.diff, diff.as_bytes());

        storage
            .upsert_turn_diff(&conversation_id, "turn-z", &primary, diff, true)
            .expect("outside turn diff should insert");
        let error = last_turn_review_diff(&storage, project.id, &worktree, Some(&conversation_id))
            .expect_err("a turn from another root must be rejected");
        assert!(error.contains("different workspace"));

        drop(storage);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn review_actions_are_snapshot_guarded_and_apply_individual_hunks() {
        let root = test_path("review-actions");
        fs::create_dir_all(&root).expect("review root should create");
        let run_git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "halv@example.invalid"]);
        run_git(&["config", "user.name", "Halv Tests"]);
        let original = (1..=24)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(root.join("tracked.txt"), &original).expect("base should write");
        run_git(&["add", "tracked.txt"]);
        run_git(&["commit", "-qm", "initial"]);

        let changed = original
            .replace("line 2\n", "line 2 changed\n")
            .replace("line 22\n", "line 22 changed\n");
        fs::write(root.join("tracked.txt"), &changed).expect("changes should write");
        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("unstaged snapshot should build");
        let file_patch =
            review_file_patch(&unstaged.diff, "tracked.txt").expect("tracked patch should exist");
        assert!(review_hunk_patch(file_patch, 0).is_some());
        assert!(review_hunk_patch(file_patch, 1).is_some());

        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            Some("tracked.txt"),
            Some(0),
            ProjectReviewAction::Stage,
        )
        .expect("first hunk should stage");
        let cached = String::from_utf8(
            git_diff_output(&root, ReviewGit::Managed, ["diff", "--cached", "--"])
                .expect("cached diff should load"),
        )
        .expect("cached diff should be utf8");
        assert!(cached.contains("line 2 changed"));
        assert!(!cached.contains("line 22 changed"));

        let stale_error = apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            Some("tracked.txt"),
            Some(0),
            ProjectReviewAction::Stage,
        )
        .expect_err("stale snapshots must not mutate git state");
        assert!(stale_error.contains("diff changed"));

        let staged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Staged,
            None,
            None,
        )
        .expect("staged snapshot should build");
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &staged.id,
            ProjectReviewScope::Staged,
            None,
            Some("tracked.txt"),
            Some(0),
            ProjectReviewAction::Unstage,
        )
        .expect("staged hunk should unstage");

        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("unstaged snapshot should refresh");
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            Some("tracked.txt"),
            Some(0),
            ProjectReviewAction::Revert,
        )
        .expect("first hunk should revert");
        let remaining =
            fs::read_to_string(root.join("tracked.txt")).expect("remaining file should read");
        assert!(remaining.contains("line 2\n"));
        assert!(remaining.contains("line 22 changed\n"));

        fs::write(root.join("untracked.txt"), "remove me\n").expect("untracked should write");
        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("untracked snapshot should build");
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            Some("untracked.txt"),
            None,
            ProjectReviewAction::Stage,
        )
        .expect("untracked file should stage");
        let staged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Staged,
            None,
            None,
        )
        .expect("staged file snapshot should build");
        assert!(staged.diff.contains("untracked.txt"));
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &staged.id,
            ProjectReviewScope::Staged,
            None,
            Some("untracked.txt"),
            None,
            ProjectReviewAction::Unstage,
        )
        .expect("untracked file should unstage");
        assert!(root.join("untracked.txt").exists());

        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("untracked snapshot should refresh");
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            Some("untracked.txt"),
            None,
            ProjectReviewAction::Revert,
        )
        .expect("untracked file should revert");
        assert!(!root.join("untracked.txt").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_actions_apply_to_the_entire_snapshot() {
        let root = test_path("review-actions-all");
        fs::create_dir_all(&root).expect("review root should create");
        let run_git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "halv@example.invalid"]);
        run_git(&["config", "user.name", "Halv Tests"]);
        fs::write(root.join("first.txt"), "first\n").expect("first base should write");
        fs::write(root.join("second.txt"), "second\n").expect("second base should write");
        run_git(&["add", "first.txt", "second.txt"]);
        run_git(&["commit", "-qm", "initial"]);

        fs::write(root.join("first.txt"), "first changed\n").expect("first should change");
        fs::write(root.join("second.txt"), "second changed\n").expect("second should change");
        fs::write(root.join("new.txt"), "new\n").expect("untracked file should write");

        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("unstaged snapshot should build");
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            None,
            None,
            ProjectReviewAction::Stage,
        )
        .expect("whole snapshot should stage");
        let staged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Staged,
            None,
            None,
        )
        .expect("staged snapshot should build");
        assert!(staged.diff.contains("first.txt"));
        assert!(staged.diff.contains("second.txt"));
        assert!(staged.diff.contains("new.txt"));
        assert!(review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None
        )
        .expect("unstaged snapshot should refresh")
        .diff
        .is_empty());

        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &staged.id,
            ProjectReviewScope::Staged,
            None,
            None,
            None,
            ProjectReviewAction::Unstage,
        )
        .expect("whole snapshot should unstage");
        let unstaged = review_snapshot(
            1,
            &root,
            ReviewGit::Managed,
            ProjectReviewScope::Unstaged,
            None,
            None,
        )
        .expect("unstaged snapshot should return");
        apply_review_action(
            1,
            &root,
            ReviewGit::Managed,
            &unstaged.id,
            ProjectReviewScope::Unstaged,
            None,
            None,
            None,
            ProjectReviewAction::Revert,
        )
        .expect("whole snapshot should revert");
        assert_eq!(
            fs::read_to_string(root.join("first.txt")).expect("first should read"),
            "first\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("second.txt")).expect("second should read"),
            "second\n"
        );
        assert!(!root.join("new.txt").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_paths_reject_parent_and_absolute_components() {
        assert!(validate_review_path("src/main.rs").is_ok());
        assert!(validate_review_path("../outside.txt").is_err());
        assert!(validate_review_path("/tmp/outside.txt").is_err());
        assert!(validate_review_path("src\nmain.rs").is_err());
    }

    #[test]
    fn review_files_resolve_only_inside_the_project() {
        let base = test_path("review-open-file");
        let root = base.join("project");
        let outside = base.join("outside.txt");
        fs::create_dir_all(root.join("src")).expect("project should create");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("file should write");
        fs::write(&outside, "outside\n").expect("outside file should write");

        assert_eq!(
            resolve_review_file(&root, "src/main.rs").expect("project file should resolve"),
            root.join("src/main.rs")
                .canonicalize()
                .expect("project file should canonicalize")
        );
        assert!(resolve_review_file(&root, "src/missing.rs").is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("src/escape.rs"))
                .expect("escape symlink should create");
            assert!(resolve_review_file(&root, "src/escape.rs").is_err());
        }

        let _ = fs::remove_dir_all(base);
    }
}
