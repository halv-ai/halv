use crate::{
    exec::managed_git_command,
    storage::{new_uuid_v4, Storage, StorageError},
};
use chrono::Utc;
use serde::Serialize;
use std::{
    ffi::OsStr,
    fmt, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Mutex,
};
use tauri::{AppHandle, Manager};

const CHECKPOINT_REF_PREFIX: &str = "refs/halv/checkpoints";
const CHECKPOINT_RETENTION: usize = 50;
static RESTORE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub repo_root: String,
    pub commit_sha: String,
    pub tree_sha: String,
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub kind: String,
    pub label: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointPreview {
    pub checkpoint_id: String,
    pub modified: Vec<String>,
    pub created: Vec<String>,
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRestoreResult {
    pub checkpoint_id: String,
    pub safety_checkpoint_id: String,
    pub modified: Vec<String>,
    pub created: Vec<String>,
    pub deleted: Vec<String>,
}

#[derive(Debug)]
enum CheckpointError {
    Io(std::io::Error),
    Storage(StorageError),
    Git { operation: String, stderr: String },
    Invalid(String),
    NotFound(String),
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "checkpoint I/O error: {error}"),
            Self::Storage(error) => write!(formatter, "checkpoint storage error: {error}"),
            Self::Git { operation, stderr } => {
                write!(formatter, "checkpoint Git operation `{operation}` failed")?;
                if !stderr.is_empty() {
                    write!(formatter, ": {stderr}")?;
                }
                Ok(())
            }
            Self::Invalid(message) => formatter.write_str(message),
            Self::NotFound(id) => write!(formatter, "checkpoint `{id}` was not found"),
        }
    }
}

impl std::error::Error for CheckpointError {}

impl From<std::io::Error> for CheckpointError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<StorageError> for CheckpointError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

type CheckpointResult<T> = Result<T, CheckpointError>;

#[derive(Debug)]
struct Repository {
    root: PathBuf,
    index_path: PathBuf,
    head_sha: Option<String>,
    branch: Option<String>,
}

struct TemporaryIndex {
    path: PathBuf,
}

impl TemporaryIndex {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "halv-checkpoint-index-{}-{}",
                std::process::id(),
                new_uuid_v4()
            )),
        }
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "failed to remove temporary checkpoint index `{}`: {error}",
                self.path.display()
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct ChangeSet {
    modified: Vec<String>,
    created: Vec<String>,
    deleted: Vec<String>,
}

impl ChangeSet {
    fn preview(&self, checkpoint_id: String) -> CheckpointPreview {
        CheckpointPreview {
            checkpoint_id,
            modified: self.modified.clone(),
            created: self.created.clone(),
            deleted: self.deleted.clone(),
        }
    }
}

pub(crate) fn spawn_turn_checkpoint(
    app: &AppHandle,
    session_id: String,
    cwd: String,
    turn_id: Option<String>,
) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let storage = app.state::<Storage>();
        if let Err(error) = create_checkpoint(
            &storage,
            &session_id,
            Path::new(&cwd),
            turn_id,
            "turn",
            "Before turn".to_string(),
            true,
        ) {
            eprintln!("failed to create agent session checkpoint: {error}");
        }
    });
}

#[tauri::command]
pub fn agent_session_checkpoints(
    storage: tauri::State<'_, Storage>,
    session_id: String,
) -> Result<Vec<Checkpoint>, String> {
    storage
        .session_checkpoints(&session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn agent_session_checkpoint_preview(
    app: AppHandle,
    checkpoint_id: String,
) -> Result<CheckpointPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let storage = app.state::<Storage>();
        preview_checkpoint(&storage, &checkpoint_id).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("checkpoint preview task failed: {error}"))?
}

#[tauri::command]
pub async fn agent_session_checkpoint_restore(
    app: AppHandle,
    checkpoint_id: String,
) -> Result<CheckpointRestoreResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let storage = app.state::<Storage>();
        restore_checkpoint(&storage, &checkpoint_id).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("checkpoint restore task failed: {error}"))?
}

fn create_checkpoint(
    storage: &Storage,
    session_id: &str,
    cwd: &Path,
    turn_id: Option<String>,
    kind: &str,
    label: String,
    prune: bool,
) -> CheckpointResult<Option<Checkpoint>> {
    let Some(repository) = resolve_repository(cwd)? else {
        return Ok(None);
    };
    let tree_sha = build_tree(&repository)?;
    let commit_sha = commit_tree(&repository, &tree_sha)?;
    let id = new_uuid_v4();
    let checkpoint_ref = checkpoint_ref(&id)?;
    run_git(
        &repository.root,
        ["update-ref", checkpoint_ref.as_str(), commit_sha.as_str()],
        None,
    )?;

    let checkpoint = Checkpoint {
        id,
        session_id: session_id.to_string(),
        turn_id,
        repo_root: repository.root.to_string_lossy().into_owned(),
        commit_sha,
        tree_sha,
        head_sha: repository.head_sha,
        branch: repository.branch,
        kind: kind.to_string(),
        label,
        created_at: Utc::now().to_rfc3339(),
    };
    if let Err(error) = storage.insert_session_checkpoint(&checkpoint) {
        let _ = run_git(
            &repository.root,
            [
                "update-ref",
                "-d",
                checkpoint_ref.as_str(),
                checkpoint.commit_sha.as_str(),
            ],
            None,
        );
        return Err(error.into());
    }
    if prune {
        prune_checkpoints(storage, &checkpoint.session_id, &[])?;
    }
    Ok(Some(checkpoint))
}

fn prune_checkpoints(
    storage: &Storage,
    session_id: &str,
    protected_ids: &[&str],
) -> CheckpointResult<()> {
    let checkpoints = if protected_ids.is_empty() {
        storage.session_checkpoints_beyond(session_id, CHECKPOINT_RETENTION)?
    } else {
        let mut retained_unprotected = 0;
        let unprotected_limit = CHECKPOINT_RETENTION.saturating_sub(protected_ids.len());
        storage
            .session_checkpoints(session_id)?
            .into_iter()
            .filter(|checkpoint| {
                if protected_ids.contains(&checkpoint.id.as_str()) {
                    return false;
                }
                retained_unprotected += 1;
                retained_unprotected > unprotected_limit
            })
            .collect()
    };
    for checkpoint in checkpoints {
        let root = PathBuf::from(&checkpoint.repo_root);
        let checkpoint_ref = checkpoint_ref(&checkpoint.id)?;
        run_git(
            &root,
            [
                "update-ref",
                "-d",
                checkpoint_ref.as_str(),
                checkpoint.commit_sha.as_str(),
            ],
            None,
        )?;
        if let Err(error) = storage.delete_session_checkpoint(&checkpoint.id) {
            let restore_ref = run_git(
                &root,
                [
                    "update-ref",
                    checkpoint_ref.as_str(),
                    checkpoint.commit_sha.as_str(),
                ],
                None,
            );
            if let Err(restore_error) = restore_ref {
                return Err(CheckpointError::Invalid(format!(
                    "failed to restore checkpoint ref after storage error `{error}`: {restore_error}"
                )));
            }
            return Err(error.into());
        }
    }
    Ok(())
}

fn preview_checkpoint(
    storage: &Storage,
    checkpoint_id: &str,
) -> CheckpointResult<CheckpointPreview> {
    let checkpoint = load_checkpoint(storage, checkpoint_id)?;
    let repository = validate_checkpoint_repository(&checkpoint)?;
    let current_tree = build_tree(&repository)?;
    let changes = diff_trees(&repository.root, &current_tree, &checkpoint.tree_sha)?;
    Ok(changes.preview(checkpoint.id))
}

fn restore_checkpoint(
    storage: &Storage,
    checkpoint_id: &str,
) -> CheckpointResult<CheckpointRestoreResult> {
    let _restore_guard = RESTORE_LOCK
        .lock()
        .map_err(|_| CheckpointError::Invalid("checkpoint restore lock is poisoned".to_string()))?;
    let checkpoint = load_checkpoint(storage, checkpoint_id)?;
    let repository = validate_checkpoint_repository(&checkpoint)?;
    let safety = create_checkpoint(
        storage,
        &checkpoint.session_id,
        &repository.root,
        None,
        "safety",
        format!("Before restoring {}", checkpoint.id),
        false,
    )?
    .ok_or_else(|| {
        CheckpointError::Invalid("checkpoint repository is no longer available".to_string())
    })?;
    prune_checkpoints(
        storage,
        &checkpoint.session_id,
        &[checkpoint.id.as_str(), safety.id.as_str()],
    )?;
    let changes = diff_trees(&repository.root, &safety.tree_sha, &checkpoint.tree_sha)?;

    if let Err(error) = apply_tree(&repository, &checkpoint.tree_sha, &changes) {
        let rollback = rollback_to_safety(&repository, &safety.tree_sha);
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(CheckpointError::Invalid(format!(
                "checkpoint restore failed: {error}; safety rollback failed: {rollback_error}"
            ))),
        };
    }

    Ok(CheckpointRestoreResult {
        checkpoint_id: checkpoint.id,
        safety_checkpoint_id: safety.id,
        modified: changes.modified,
        created: changes.created,
        deleted: changes.deleted,
    })
}

fn rollback_to_safety(repository: &Repository, safety_tree: &str) -> CheckpointResult<()> {
    let current_tree = build_tree(repository)?;
    let changes = diff_trees(&repository.root, &current_tree, safety_tree)?;
    apply_tree(repository, safety_tree, &changes)
}

fn load_checkpoint(storage: &Storage, checkpoint_id: &str) -> CheckpointResult<Checkpoint> {
    storage
        .session_checkpoint(checkpoint_id)?
        .ok_or_else(|| CheckpointError::NotFound(checkpoint_id.to_string()))
}

fn resolve_repository(cwd: &Path) -> CheckpointResult<Option<Repository>> {
    let output = git_output(cwd, ["rev-parse", "--show-toplevel"], None)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") || stderr.contains("must be run in a work tree")
        {
            return Ok(None);
        }
        return Err(git_failure("rev-parse --show-toplevel", &output));
    }
    let root = PathBuf::from(stdout_text("rev-parse --show-toplevel", &output)?);
    let root = fs::canonicalize(root)?;
    let canonical_cwd = fs::canonicalize(cwd)?;
    if !canonical_cwd.starts_with(&root) {
        return Err(CheckpointError::Invalid(format!(
            "session path `{}` is outside repository `{}`",
            cwd.display(),
            root.display()
        )));
    }

    let branch_output = git_output(&root, ["symbolic-ref", "--quiet", "--short", "HEAD"], None)?;
    let branch = if branch_output.status.success() {
        Some(stdout_text("symbolic-ref --short HEAD", &branch_output)?)
    } else if branch_output.status.code() == Some(1) {
        None
    } else {
        return Err(git_failure("symbolic-ref --short HEAD", &branch_output));
    };

    let head_output = git_output(&root, ["rev-parse", "--verify", "HEAD^{commit}"], None)?;
    let head_sha = if head_output.status.success() {
        Some(stdout_text("rev-parse --verify HEAD", &head_output)?)
    } else if branch.is_some() {
        None
    } else {
        return Err(git_failure("rev-parse --verify HEAD", &head_output));
    };

    let index_output = run_git(&root, ["rev-parse", "--git-path", "index"], None)?;
    let index_path = PathBuf::from(stdout_text("rev-parse --git-path index", &index_output)?);
    let index_path = if index_path.is_absolute() {
        index_path
    } else {
        root.join(index_path)
    };

    Ok(Some(Repository {
        root,
        index_path,
        head_sha,
        branch,
    }))
}

fn validate_checkpoint_repository(checkpoint: &Checkpoint) -> CheckpointResult<Repository> {
    validate_object_id("commit", &checkpoint.commit_sha)?;
    validate_object_id("tree", &checkpoint.tree_sha)?;
    let stored_root = PathBuf::from(&checkpoint.repo_root);
    let repository = resolve_repository(&stored_root)?.ok_or_else(|| {
        CheckpointError::Invalid(format!(
            "checkpoint repository `{}` is not a Git worktree",
            stored_root.display()
        ))
    })?;
    let canonical_stored_root = fs::canonicalize(&stored_root)?;
    if repository.root != canonical_stored_root {
        return Err(CheckpointError::Invalid(format!(
            "checkpoint repository changed from `{}` to `{}`",
            canonical_stored_root.display(),
            repository.root.display()
        )));
    }

    let checkpoint_ref = format!("{}^{{commit}}", checkpoint_ref(&checkpoint.id)?);
    let ref_output = run_git(
        &repository.root,
        ["rev-parse", "--verify", checkpoint_ref.as_str()],
        None,
    )?;
    let ref_commit = stdout_text("verify checkpoint ref", &ref_output)?;
    if ref_commit != checkpoint.commit_sha {
        return Err(CheckpointError::Invalid(format!(
            "checkpoint ref `{}` does not match its stored commit",
            checkpoint.id
        )));
    }
    let tree_expression = format!("{}^{{tree}}", checkpoint.commit_sha);
    let tree_output = run_git(
        &repository.root,
        ["rev-parse", "--verify", tree_expression.as_str()],
        None,
    )?;
    let tree_sha = stdout_text("verify checkpoint tree", &tree_output)?;
    if tree_sha != checkpoint.tree_sha {
        return Err(CheckpointError::Invalid(format!(
            "checkpoint tree `{}` does not match its stored commit",
            checkpoint.id
        )));
    }
    Ok(repository)
}

fn build_tree(repository: &Repository) -> CheckpointResult<String> {
    let temporary_index = TemporaryIndex::new();
    if repository.index_path.exists() {
        fs::copy(&repository.index_path, &temporary_index.path)?;
    }
    // To check a submodule for changes, `git add` starts a git status in the
    // submodule. That process reads the submodule config, which the
    // overrides do not cover. Thus `git add` gets every path except the
    // submodules. Then update-index records the commit of each submodule. It
    // reads only the submodule HEAD.
    let gitlinks = gitlink_paths(&repository.root, &temporary_index.path)?;
    let mut pathspecs = b".\0".to_vec();
    for path in &gitlinks {
        pathspecs.extend_from_slice(b":(exclude,literal)");
        pathspecs.extend_from_slice(path);
        pathspecs.push(0);
    }
    run_git_with_input(
        &repository.root,
        ["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"],
        Some(&temporary_index.path),
        &pathspecs,
    )?;
    if !gitlinks.is_empty() {
        let paths = gitlinks
            .iter()
            .flat_map(|path| path.iter().copied().chain([0]))
            .collect::<Vec<u8>>();
        run_git_with_input(
            &repository.root,
            ["update-index", "--add", "--remove", "-z", "--stdin"],
            Some(&temporary_index.path),
            &paths,
        )?;
    }
    let output = run_git(
        &repository.root,
        ["write-tree"],
        Some(&temporary_index.path),
    )?;
    stdout_text("write-tree", &output)
}

/// Returns the paths of the submodule (gitlink) entries in an index.
fn gitlink_paths(root: &Path, index: &Path) -> CheckpointResult<Vec<Vec<u8>>> {
    let output = run_git(root, ["ls-files", "--stage", "-z"], Some(index))?;
    let mut paths = Vec::<Vec<u8>>::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let path = &record[tab + 1..];
        // A conflict gives one record for each stage of the same path.
        if record.starts_with(b"160000 ") && paths.last().map(Vec::as_slice) != Some(path) {
            paths.push(path.to_vec());
        }
    }
    Ok(paths)
}

fn commit_tree(repository: &Repository, tree_sha: &str) -> CheckpointResult<String> {
    let mut arguments = vec!["commit-tree".to_string(), tree_sha.to_string()];
    if let Some(head_sha) = &repository.head_sha {
        arguments.push("-p".to_string());
        arguments.push(head_sha.clone());
    }
    arguments.push("-m".to_string());
    arguments.push("halv checkpoint".to_string());

    let mut command = checkpoint_git_command(&repository.root, &arguments, None)?;
    command
        .env("GIT_AUTHOR_NAME", "Halv")
        .env("GIT_AUTHOR_EMAIL", "checkpoint@halv.local")
        .env("GIT_COMMITTER_NAME", "Halv")
        .env("GIT_COMMITTER_EMAIL", "checkpoint@halv.local");
    let output = command.output()?;
    if !output.status.success() {
        return Err(git_failure("commit-tree", &output));
    }
    stdout_text("commit-tree", &output)
}

fn diff_trees(root: &Path, current_tree: &str, target_tree: &str) -> CheckpointResult<ChangeSet> {
    let output = run_git(
        root,
        [
            "diff-tree",
            "--no-ext-diff",
            "--no-textconv",
            "--no-commit-id",
            "--no-renames",
            "--name-status",
            "-r",
            "-z",
            current_tree,
            target_tree,
        ],
        None,
    )?;
    parse_changes(&output.stdout)
}

fn parse_changes(output: &[u8]) -> CheckpointResult<ChangeSet> {
    let fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() % 2 != 0 {
        return Err(CheckpointError::Invalid(
            "Git returned an incomplete checkpoint diff".to_string(),
        ));
    }
    let mut changes = ChangeSet {
        modified: Vec::new(),
        created: Vec::new(),
        deleted: Vec::new(),
    };
    for pair in fields.chunks_exact(2) {
        let status = std::str::from_utf8(pair[0]).map_err(|error| {
            CheckpointError::Invalid(format!("Git returned an invalid change status: {error}"))
        })?;
        let path = String::from_utf8(pair[1].to_vec()).map_err(|_| {
            CheckpointError::Invalid(
                "checkpoint paths must use valid UTF-8 before restore".to_string(),
            )
        })?;
        validate_relative_path(&path)?;
        match status.as_bytes().first().copied() {
            Some(b'A') => changes.created.push(path),
            Some(b'D') => changes.deleted.push(path),
            Some(b'M' | b'T' | b'U' | b'X' | b'B') => changes.modified.push(path),
            _ => {
                return Err(CheckpointError::Invalid(format!(
                    "Git returned unsupported checkpoint status `{status}`"
                )))
            }
        }
    }
    changes.modified.sort();
    changes.created.sort();
    changes.deleted.sort();
    Ok(changes)
}

fn apply_tree(
    repository: &Repository,
    target_tree: &str,
    changes: &ChangeSet,
) -> CheckpointResult<()> {
    preflight_changes(repository, changes)?;
    run_git(
        &repository.root,
        ["read-tree", "-u", "--reset", target_tree],
        None,
    )?;

    for relative in &changes.deleted {
        let target = repository.root.join(relative_path(relative)?);
        let metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            return Err(CheckpointError::Invalid(format!(
                "checkpoint restore refuses to remove directory `{relative}`"
            )));
        }
        fs::remove_file(target)?;
    }
    Ok(())
}

fn preflight_changes(repository: &Repository, changes: &ChangeSet) -> CheckpointResult<()> {
    for relative in changes
        .modified
        .iter()
        .chain(changes.created.iter())
        .chain(changes.deleted.iter())
    {
        let relative_path = relative_path(relative)?;
        let target = repository.root.join(&relative_path);
        validate_target_inside_repository(&repository.root, &target)?;
        if is_ignored(&repository.root, relative)? {
            return Err(CheckpointError::Invalid(format!(
                "checkpoint restore refuses to modify ignored path `{relative}`"
            )));
        }
        if changes.created.contains(relative) {
            if let Ok(metadata) = fs::symlink_metadata(&target) {
                if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                    return Err(CheckpointError::Invalid(format!(
                        "checkpoint restore refuses to replace directory `{relative}`"
                    )));
                }
            }
        }
        if changes.deleted.contains(relative) {
            if let Ok(metadata) = fs::symlink_metadata(&target) {
                if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                    return Err(CheckpointError::Invalid(format!(
                        "checkpoint restore refuses to remove directory `{relative}`"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_target_inside_repository(root: &Path, target: &Path) -> CheckpointResult<()> {
    let canonical_root = fs::canonicalize(root)?;
    let parent = target.parent().ok_or_else(|| {
        CheckpointError::Invalid(format!(
            "checkpoint path `{}` has no parent",
            target.display()
        ))
    })?;
    let existing_parent = parent
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| {
            CheckpointError::Invalid(format!(
                "checkpoint path `{}` has no existing parent",
                target.display()
            ))
        })?;
    let canonical_parent = fs::canonicalize(existing_parent)?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(CheckpointError::Invalid(format!(
            "checkpoint path `{}` resolves outside repository `{}`",
            target.display(),
            root.display()
        )));
    }
    Ok(())
}

fn is_ignored(root: &Path, relative: &str) -> CheckpointResult<bool> {
    let output = git_output(
        root,
        ["check-ignore", "--no-index", "--quiet", "--", relative],
        None,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_failure("check-ignore", &output)),
    }
}

fn relative_path(value: &str) -> CheckpointResult<PathBuf> {
    validate_relative_path(value)?;
    Ok(PathBuf::from(value))
}

fn validate_relative_path(value: &str) -> CheckpointResult<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == OsStr::new(".git"))
    {
        return Err(CheckpointError::Invalid(format!(
            "checkpoint path `{value}` is not a safe repository-relative path"
        )));
    }
    Ok(())
}

fn checkpoint_ref(checkpoint_id: &str) -> CheckpointResult<String> {
    let bytes = checkpoint_id.as_bytes();
    let valid = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !valid {
        return Err(CheckpointError::Invalid(format!(
            "checkpoint id `{checkpoint_id}` is invalid"
        )));
    }
    Ok(format!("{CHECKPOINT_REF_PREFIX}/{checkpoint_id}"))
}

fn validate_object_id(kind: &str, object_id: &str) -> CheckpointResult<()> {
    if matches!(object_id.len(), 40 | 64) && object_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(());
    }
    Err(CheckpointError::Invalid(format!(
        "checkpoint {kind} object id is invalid"
    )))
}

fn run_git<I, S>(cwd: &Path, arguments: I, index: Option<&Path>) -> CheckpointResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let operation = arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let output = checkpoint_git_command(cwd, &arguments, index)?.output()?;
    if !output.status.success() {
        return Err(git_failure(&operation, &output));
    }
    Ok(output)
}

/// Runs git like `run_git`, and gives `input` to git on its standard input.
fn run_git_with_input<I, S>(
    cwd: &Path,
    arguments: I,
    index: Option<&Path>,
    input: &[u8],
) -> CheckpointResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let operation = arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let mut command = checkpoint_git_command(cwd, &arguments, index)?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        CheckpointError::Invalid(format!("Git operation `{operation}` has no input pipe"))
    })?;
    let input = input.to_vec();
    // Another thread writes the input, so that a full output pipe cannot
    // stop git while Halv writes.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    let written = writer.join();
    if !output.status.success() {
        return Err(git_failure(&operation, &output));
    }
    match written {
        Ok(Ok(())) => Ok(output),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Err(CheckpointError::Invalid(format!(
            "failed to give input to Git operation `{operation}`"
        ))),
    }
}

fn git_output<I, S>(cwd: &Path, arguments: I, index: Option<&Path>) -> CheckpointResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_os_string())
        .collect::<Vec<_>>();
    checkpoint_git_command(cwd, &arguments, index)?
        .output()
        .map_err(CheckpointError::from)
}

fn checkpoint_git_command<I, S>(
    cwd: &Path,
    arguments: I,
    index: Option<&Path>,
) -> CheckpointResult<Command>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = managed_git_command(cwd).map_err(CheckpointError::Invalid)?;
    command.args(arguments);
    command.env("LC_ALL", "C");
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    Ok(command)
}

fn stdout_text(operation: &str, output: &Output) -> CheckpointResult<String> {
    let value = String::from_utf8(output.stdout.clone()).map_err(|error| {
        CheckpointError::Invalid(format!(
            "Git operation `{operation}` returned invalid UTF-8: {error}"
        ))
    })?;
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() {
        return Err(CheckpointError::Invalid(format!(
            "Git operation `{operation}` returned no value"
        )));
    }
    Ok(value.to_string())
}

fn git_failure(operation: &str, output: &Output) -> CheckpointError {
    CheckpointError::Git {
        operation: operation.to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct TestRepository {
        root: PathBuf,
    }

    impl TestRepository {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("halv-checkpoint-{name}-{}", new_uuid_v4()));
            fs::create_dir_all(&root).expect("create test repository");
            git(&root, &["init"]);
            git(&root, &["config", "user.name", "Halv Test"]);
            git(&root, &["config", "user.email", "halv@example.com"]);
            fs::write(root.join("tracked.txt"), "initial\n").expect("write tracked file");
            fs::write(root.join(".gitignore"), "*.ignored\n").expect("write ignore file");
            git(&root, &["add", "."]);
            git(&root, &["commit", "-m", "initial"]);
            Self { root }
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct TestStorage {
        storage: Storage,
        path: PathBuf,
    }

    impl TestStorage {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("halv-checkpoint-{name}-{}.sqlite3", new_uuid_v4()));
            let storage = Storage::open(&path).expect("open test storage");
            Self { storage, path }
        }
    }

    impl Drop for TestStorage {
        fn drop(&mut self) {
            for suffix in ["", "-shm", "-wal"] {
                let _ = fs::remove_file(format!("{}{}", self.path.display(), suffix));
            }
        }
    }

    fn git(root: &Path, arguments: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(root)
            .args(arguments)
            .output()
            .expect("run test Git command");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn checkpoint(storage: &Storage, repository: &TestRepository, session_id: &str) -> Checkpoint {
        create_checkpoint(
            storage,
            session_id,
            &repository.root,
            None,
            "turn",
            "Before turn".to_string(),
            true,
        )
        .expect("create checkpoint")
        .expect("Git repository checkpoint")
    }

    fn status(root: &Path) -> Vec<u8> {
        git(root, &["status", "--porcelain=v1", "-z"])
    }

    #[test]
    fn snapshot_includes_untracked_files_and_excludes_ignored_files() {
        let repository = TestRepository::new("untracked");
        let storage = TestStorage::new("untracked");
        fs::write(repository.root.join("draft.txt"), "draft\n").expect("write draft");
        fs::write(repository.root.join("private.ignored"), "secret\n").expect("write ignored file");

        let checkpoint = checkpoint(&storage.storage, &repository, "session-untracked");
        let names = String::from_utf8(git(
            &repository.root,
            &["ls-tree", "-r", "--name-only", &checkpoint.commit_sha],
        ))
        .expect("tree names use UTF-8");
        assert!(names.lines().any(|name| name == "draft.txt"));
        assert!(!names.lines().any(|name| name == "private.ignored"));
    }

    #[test]
    fn snapshot_does_not_modify_the_worktree_or_real_index() {
        let repository = TestRepository::new("unchanged-index");
        let storage = TestStorage::new("unchanged-index");
        fs::write(repository.root.join("tracked.txt"), "staged\n").expect("write staged state");
        git(&repository.root, &["add", "tracked.txt"]);
        fs::write(repository.root.join("tracked.txt"), "worktree\n").expect("write worktree state");
        fs::write(repository.root.join("draft.txt"), "draft\n").expect("write draft");
        let before = status(&repository.root);

        checkpoint(&storage.storage, &repository, "session-index");

        assert_eq!(status(&repository.root), before);
        assert_eq!(
            fs::read_to_string(repository.root.join("tracked.txt")).expect("read tracked file"),
            "worktree\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_does_not_run_repository_programs() {
        let repository = TestRepository::new("programs");
        let storage = TestStorage::new("programs");
        let sentinel = repository.root.join(".git/halv-program-ran");
        crate::exec::plant_repository_programs(&repository.root, &sentinel);
        fs::write(repository.root.join("draft.txt"), "draft\n").expect("write draft");

        checkpoint(&storage.storage, &repository, "session-programs");
        assert!(
            !sentinel.exists(),
            "checkpoints must not run programs from the repository config"
        );

        git(&repository.root, &["add", "-A"]);
        assert!(
            sentinel.exists(),
            "the planted programs should run in plain Git"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_keeps_a_standard_repository_lfs_filter() {
        let repository = TestRepository::new("lfs");
        let storage = TestStorage::new("lfs");
        let lfs = crate::exec::FakeGitLfs::new(&repository.root.join(".git"));
        crate::exec::plant_standard_lfs_filter(&repository.root, "*.bin");
        fs::write(repository.root.join("large.bin"), "large content\n").expect("write file");

        let snapshot = {
            let _environment = lfs.activate();
            checkpoint(&storage.storage, &repository, "session-lfs")
        };
        assert!(
            !lfs.runs().is_empty(),
            "checkpoints must keep a standard git-lfs filter from the repository config"
        );
        assert_eq!(
            git(
                &repository.root,
                &[
                    "cat-file",
                    "-p",
                    &format!("{}:large.bin", snapshot.commit_sha)
                ],
            ),
            crate::exec::FakeGitLfs::POINTER.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_does_not_start_git_lfs_when_the_repository_config_hides_keys() {
        let repository = TestRepository::new("lfs-hidden");
        let storage = TestStorage::new("lfs-hidden");
        let lfs = crate::exec::FakeGitLfs::new(&repository.root.join(".git"));
        crate::exec::plant_standard_lfs_filter(&repository.root, "*.bin");
        // git-lfs reads the second line of the value as an lfs.storage key.
        let hidden_storage = repository.root.join(".git/halv-hidden-storage");
        lfs.git(
            &repository.root,
            &[
                "config",
                "halvtest.note",
                &format!("first\nlfs.storage={}", hidden_storage.display()),
            ],
        );
        fs::write(repository.root.join("large.bin"), "large content\n").expect("write file");

        {
            let _environment = lfs.activate();
            // The required lfs filter has no program, thus git add can fail.
            let _ = create_checkpoint(
                &storage.storage,
                "session-lfs-hidden",
                &repository.root,
                None,
                "turn",
                "Before turn".to_string(),
                true,
            );
        }
        assert!(
            lfs.runs().is_empty(),
            "checkpoints must not start git-lfs when the repository config hides keys"
        );
        assert!(!hidden_storage.exists());

        lfs.git(&repository.root, &["add", "-A"]);
        assert!(
            hidden_storage.join("objects").is_dir(),
            "plain Git should give the hidden lfs.storage key to git-lfs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_records_submodules_without_starting_git_in_them() {
        let repository = TestRepository::new("submodule");
        let storage = TestStorage::new("submodule");
        let sentinel = repository.root.join(".git/halv-submodule-program-ran");
        let submodule = crate::exec::plant_submodule_program(&repository.root, &sentinel);
        fs::write(repository.root.join("draft.txt"), "draft\n").expect("write draft");
        let gitlink = |commit: &str| {
            let entry = String::from_utf8(git(&repository.root, &["ls-tree", commit, "sub"]))
                .expect("tree entry uses UTF-8");
            let (metadata, _) = entry.split_once('\t').expect("tree entry has a path");
            metadata.to_string()
        };
        let head = |root: &Path| {
            String::from_utf8(git(root, &["rev-parse", "HEAD"]))
                .expect("commit id uses UTF-8")
                .trim()
                .to_string()
        };
        let initial = head(&submodule);

        let first = checkpoint(&storage.storage, &repository, "session-submodule");
        assert!(
            !sentinel.exists(),
            "checkpoints must not start git in a submodule"
        );
        assert_eq!(
            gitlink(&first.commit_sha),
            format!("160000 commit {initial}")
        );
        let names = String::from_utf8(git(
            &repository.root,
            &["ls-tree", "-r", "--name-only", &first.commit_sha],
        ))
        .expect("tree names use UTF-8");
        assert!(names.lines().any(|name| name == "draft.txt"));

        // Move the submodule HEAD without a checkout, so that git does not
        // read the submodule files.
        let moved = String::from_utf8(git(
            &submodule,
            &[
                "-c",
                "user.name=Halv Test",
                "-c",
                "user.email=halv@example.com",
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "HEAD",
                "-m",
                "moved",
            ],
        ))
        .expect("commit id uses UTF-8")
        .trim()
        .to_string();
        git(&submodule, &["update-ref", "HEAD", &moved]);
        let second = checkpoint(&storage.storage, &repository, "session-submodule");
        assert!(
            !sentinel.exists(),
            "checkpoints must not start git in a submodule"
        );
        assert_eq!(
            gitlink(&second.commit_sha),
            format!("160000 commit {moved}")
        );

        git(&submodule, &["update-ref", "HEAD", &initial]);
        git(&repository.root, &["add", "-A"]);
        assert!(
            sentinel.exists(),
            "plain Git should run the submodule filter"
        );
    }

    #[test]
    fn restore_returns_to_snapshot_and_preserves_ignored_files() {
        let repository = TestRepository::new("restore");
        let storage = TestStorage::new("restore");
        fs::write(repository.root.join("tracked.txt"), "checkpoint\n")
            .expect("write checkpoint state");
        fs::write(repository.root.join("included.txt"), "included\n").expect("write included file");
        let checkpoint = checkpoint(&storage.storage, &repository, "session-restore");

        fs::write(repository.root.join("tracked.txt"), "later\n").expect("write later state");
        fs::remove_file(repository.root.join("included.txt")).expect("remove included file");
        fs::write(repository.root.join("created-later.txt"), "later\n").expect("write later file");
        fs::write(repository.root.join("cache.ignored"), "keep\n").expect("write ignored file");

        let result =
            restore_checkpoint(&storage.storage, &checkpoint.id).expect("restore checkpoint");
        assert!(result.modified.contains(&"tracked.txt".to_string()));
        assert!(result.created.contains(&"included.txt".to_string()));
        assert!(result.deleted.contains(&"created-later.txt".to_string()));
        assert_eq!(
            fs::read_to_string(repository.root.join("tracked.txt")).expect("read tracked file"),
            "checkpoint\n"
        );
        assert_eq!(
            fs::read_to_string(repository.root.join("included.txt")).expect("read included file"),
            "included\n"
        );
        assert!(!repository.root.join("created-later.txt").exists());
        assert_eq!(
            fs::read_to_string(repository.root.join("cache.ignored")).expect("read ignored file"),
            "keep\n"
        );
    }

    #[test]
    fn safety_checkpoint_restores_the_pre_restore_state() {
        let repository = TestRepository::new("safety");
        let storage = TestStorage::new("safety");
        fs::write(repository.root.join("tracked.txt"), "checkpoint\n")
            .expect("write checkpoint state");
        let checkpoint = checkpoint(&storage.storage, &repository, "session-safety");
        fs::write(repository.root.join("tracked.txt"), "before restore\n")
            .expect("write pre-restore state");
        fs::write(repository.root.join("pre-restore.txt"), "recover me\n")
            .expect("write pre-restore file");

        let restored =
            restore_checkpoint(&storage.storage, &checkpoint.id).expect("restore checkpoint");
        let safety = storage
            .storage
            .session_checkpoint(&restored.safety_checkpoint_id)
            .expect("load safety checkpoint")
            .expect("safety checkpoint should exist");
        assert_eq!(safety.kind, "safety");
        assert!(safety.label.contains(&checkpoint.id));
        restore_checkpoint(&storage.storage, &restored.safety_checkpoint_id)
            .expect("restore safety checkpoint");

        assert_eq!(
            fs::read_to_string(repository.root.join("tracked.txt")).expect("read tracked file"),
            "before restore\n"
        );
        assert_eq!(
            fs::read_to_string(repository.root.join("pre-restore.txt"))
                .expect("read pre-restore file"),
            "recover me\n"
        );
    }

    #[test]
    fn preview_changes_nothing_on_disk() {
        let repository = TestRepository::new("preview");
        let storage = TestStorage::new("preview");
        let checkpoint = checkpoint(&storage.storage, &repository, "session-preview");
        fs::write(repository.root.join("tracked.txt"), "later\n").expect("write later state");
        fs::write(repository.root.join("later.txt"), "later\n").expect("write later file");
        let before_status = status(&repository.root);
        let before_contents =
            fs::read_to_string(repository.root.join("tracked.txt")).expect("read tracked file");
        let before_rows = storage
            .storage
            .session_checkpoints("session-preview")
            .expect("list checkpoints");

        let preview =
            preview_checkpoint(&storage.storage, &checkpoint.id).expect("preview checkpoint");

        assert!(preview.modified.contains(&"tracked.txt".to_string()));
        assert!(preview.deleted.contains(&"later.txt".to_string()));
        assert_eq!(status(&repository.root), before_status);
        assert_eq!(
            fs::read_to_string(repository.root.join("tracked.txt")).expect("read tracked file"),
            before_contents
        );
        assert_eq!(
            storage
                .storage
                .session_checkpoints("session-preview")
                .expect("list checkpoints"),
            before_rows
        );
    }

    #[test]
    fn non_git_session_records_no_checkpoint_and_lists_empty() {
        let root = std::env::temp_dir().join(format!("halv-checkpoint-nongit-{}", new_uuid_v4()));
        fs::create_dir_all(&root).expect("create non-Git directory");
        let storage = TestStorage::new("nongit");

        let result = create_checkpoint(
            &storage.storage,
            "session-nongit",
            &root,
            None,
            "turn",
            "Before turn".to_string(),
            true,
        )
        .expect("non-Git checkpoint should not fail");

        assert!(result.is_none());
        assert!(storage
            .storage
            .session_checkpoints("session-nongit")
            .expect("list checkpoints")
            .is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retention_keeps_fifty_checkpoints_and_deletes_the_oldest_ref() {
        let repository = TestRepository::new("retention");
        let storage = TestStorage::new("retention");
        let oldest = checkpoint(&storage.storage, &repository, "session-retention");
        for index in 1..=50 {
            fs::write(
                repository.root.join("tracked.txt"),
                format!("checkpoint {index}\n"),
            )
            .expect("write checkpoint state");
            checkpoint(&storage.storage, &repository, "session-retention");
        }

        let checkpoints = storage
            .storage
            .session_checkpoints("session-retention")
            .expect("list checkpoints");
        assert_eq!(checkpoints.len(), CHECKPOINT_RETENTION);
        assert!(!checkpoints
            .iter()
            .any(|checkpoint| checkpoint.id == oldest.id));
        let oldest_ref = checkpoint_ref(&oldest.id).expect("valid oldest checkpoint ref");
        let status = Command::new("git")
            .current_dir(&repository.root)
            .args(["show-ref", "--verify", "--quiet", &oldest_ref])
            .status()
            .expect("check oldest checkpoint ref");
        assert!(!status.success());
    }
}
