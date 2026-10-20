use super::ShellTurnDiff;
use crate::exec::{self, apply_patch::FileFingerprint, FileSnapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;
use std::process::Stdio;

const MAX_GIT_PATH_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

const MAX_SHELL_PREIMAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct GitShellPreimages {
    pre_head: Option<String>,
    pre_status: BTreeMap<String, String>,
    paths: BTreeMap<String, GitShellPathPreimage>,
    snapshot_budget: exec::apply_patch::SnapshotBudget,
    materialized_bytes: usize,
}

#[derive(Debug)]
struct GitShellPathPreimage {
    absolute_path: Option<PathBuf>,
    snapshot: Option<FileSnapshot>,
    fingerprint: Result<FileFingerprint, String>,
    skip_reason: Option<String>,
}

#[derive(Debug)]
struct GitTreeEntry {
    mode: String,
    object: String,
}

enum GitPreimageResolution {
    Unchanged,
    Preimage(Option<Vec<u8>>),
}

pub(super) fn snapshot_git_shell_preimages(default_cwd: &str) -> Result<GitShellPreimages, String> {
    let cwd = Path::new(default_cwd);
    let pre_head = git_head(cwd)?;
    let pre_status = git_status(cwd)?;
    let mut paths = BTreeMap::new();
    let mut snapshot_budget = exec::apply_patch::SnapshotBudget::default();
    let mut materialized_bytes = 0_usize;

    for (index, path) in pre_status.keys().enumerate() {
        if index >= exec::apply_patch::MAX_SNAPSHOT_FILES {
            paths.insert(
                path.clone(),
                GitShellPathPreimage {
                    absolute_path: None,
                    snapshot: None,
                    fingerprint: Err("path was not inspected".to_string()),
                    skip_reason: Some(format!(
                        "dirty path capture exceeded the {}-file snapshot safety limit",
                        exec::apply_patch::MAX_SNAPSHOT_FILES
                    )),
                },
            );
            continue;
        }
        let absolute_path = match exec::resolve_workspace_path(cwd, path) {
            Ok(path) => path,
            Err(error) => {
                paths.insert(
                    path.clone(),
                    GitShellPathPreimage {
                        absolute_path: None,
                        snapshot: None,
                        fingerprint: Err(error.clone()),
                        skip_reason: Some(format!(
                            "failed to normalize dirty path '{}': {error}",
                            path
                        )),
                    },
                );
                continue;
            }
        };
        let fingerprint = exec::apply_patch::fingerprint_path(&absolute_path);
        let mut snapshot = None;
        let mut skip_reason = None;

        match &fingerprint {
            Ok(FileFingerprint::Missing) => {
                snapshot = Some(FileSnapshot {
                    display_path: path.clone(),
                    absolute_path: absolute_path.clone(),
                    before: None,
                    after_exists: None,
                    after_sha256: None,
                });
            }
            Ok(FileFingerprint::Regular { size, .. })
                if *size > exec::apply_patch::MAX_SNAPSHOT_FILE_BYTES as u64 =>
            {
                let size = usize::try_from(*size).unwrap_or(usize::MAX);
                if materialized_bytes.saturating_add(size) > MAX_SHELL_PREIMAGE_BYTES {
                    skip_reason = Some(format!(
                        "dirty preimage '{}' exceeds the 64 MiB shell preimage ceiling",
                        path
                    ));
                } else {
                    match exec::apply_patch::read_regular_file_bounded(
                        &absolute_path,
                        MAX_SHELL_PREIMAGE_BYTES,
                    ) {
                        Ok(Some(bytes)) => {
                            materialized_bytes = materialized_bytes.saturating_add(bytes.len());
                            snapshot = Some(FileSnapshot {
                                display_path: path.clone(),
                                absolute_path: absolute_path.clone(),
                                before: Some(bytes),
                                after_exists: None,
                                after_sha256: None,
                            });
                        }
                        Ok(None) => {
                            snapshot = Some(FileSnapshot {
                                display_path: path.clone(),
                                absolute_path: absolute_path.clone(),
                                before: None,
                                after_exists: None,
                                after_sha256: None,
                            });
                        }
                        Err(error) => {
                            skip_reason = Some(format!(
                                "failed to capture dirty preimage '{}': {error}",
                                path
                            ));
                        }
                    }
                }
            }
            Ok(FileFingerprint::Regular { .. }) => match snapshot_budget.read(&absolute_path) {
                Ok(before) => {
                    let bytes = before.as_ref().map_or(0, Vec::len);
                    if materialized_bytes.saturating_add(bytes) > MAX_SHELL_PREIMAGE_BYTES {
                        skip_reason = Some(format!(
                            "dirty preimage '{}' exceeds the 64 MiB shell preimage ceiling",
                            path
                        ));
                    } else {
                        materialized_bytes = materialized_bytes.saturating_add(bytes);
                        snapshot = Some(FileSnapshot {
                            display_path: path.clone(),
                            absolute_path: absolute_path.clone(),
                            before,
                            after_exists: None,
                            after_sha256: None,
                        });
                    }
                }
                Err(error) => {
                    skip_reason = Some(format!(
                        "failed to capture dirty preimage '{}': {error}",
                        path
                    ));
                }
            },
            Ok(FileFingerprint::Symlink { .. }) => {
                skip_reason = Some(format!(
                    "dirty path '{}' is a symbolic link and cannot use byte restore",
                    path
                ));
            }
            Ok(FileFingerprint::Other { kind, .. }) => {
                skip_reason = Some(format!(
                    "dirty path '{}' is a {kind} and cannot use byte restore",
                    path
                ));
            }
            Err(error) => {
                skip_reason = Some(format!(
                    "failed to inspect dirty preimage '{}': {error}",
                    path
                ));
            }
        }

        paths.insert(
            path.clone(),
            GitShellPathPreimage {
                absolute_path: Some(absolute_path),
                snapshot,
                fingerprint,
                skip_reason,
            },
        );
    }

    Ok(GitShellPreimages {
        pre_head,
        pre_status,
        paths,
        snapshot_budget,
        materialized_bytes,
    })
}

pub(super) fn shell_turn_diff(
    default_cwd: &str,
    preimages: Result<GitShellPreimages, String>,
) -> Result<Option<ShellTurnDiff>, String> {
    let cwd = Path::new(default_cwd);
    let mut preimages = match preimages {
        Ok(preimages) => preimages,
        Err(error) => {
            if is_not_git_repository_error(&error) {
                return Ok(None);
            }
            let changed = git_status(cwd)
                .map(|status| status.into_keys().collect::<Vec<_>>())
                .unwrap_or_default();
            return Ok(Some(ShellTurnDiff {
                snapshots: Vec::new(),
                diff: bounded_nonrevertible_diff(
                    &changed,
                    &format!("Halv could not capture the pre-turn Git state: {error}"),
                ),
                revertible: false,
            }));
        }
    };
    let post_status = match git_status(cwd) {
        Ok(status) => status,
        Err(error) => {
            return Ok(Some(ShellTurnDiff {
                snapshots: Vec::new(),
                diff: bounded_nonrevertible_diff(
                    &[],
                    &format!("Halv could not read the post-turn Git state: {error}"),
                ),
                revertible: false,
            }));
        }
    };
    let post_head = match git_head(cwd) {
        Ok(head) => head,
        Err(error) => {
            let changed = post_status.keys().cloned().collect::<Vec<_>>();
            return Ok(Some(ShellTurnDiff {
                snapshots: Vec::new(),
                diff: bounded_nonrevertible_diff(
                    &changed,
                    &format!("Halv could not read the post-turn Git HEAD: {error}"),
                ),
                revertible: false,
            }));
        }
    };
    let mut candidates = BTreeSet::new();
    for path in preimages.pre_status.keys().chain(post_status.keys()) {
        if preimages.pre_status.get(path) != post_status.get(path) {
            candidates.insert(path.clone());
        }
    }
    match git_head_changed_paths(cwd, preimages.pre_head.as_deref(), post_head.as_deref()) {
        Ok(paths) => candidates.extend(paths),
        Err(error) => {
            let changed = candidates.into_iter().collect::<Vec<_>>();
            return Ok(Some(ShellTurnDiff {
                snapshots: Vec::new(),
                diff: bounded_nonrevertible_diff(&changed, &error),
                revertible: false,
            }));
        }
    }

    for (path, preimage) in &preimages.paths {
        let Some(absolute_path) = preimage.absolute_path.as_deref() else {
            continue;
        };
        if let (Ok(before), Ok(after)) = (
            preimage.fingerprint.as_ref(),
            exec::apply_patch::fingerprint_path(absolute_path),
        ) {
            if before != &after {
                candidates.insert(path.clone());
            }
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }

    let mut snapshots = Vec::new();
    let mut diff = String::new();
    let mut changed_paths = Vec::new();
    let mut failures = Vec::new();

    for path in candidates {
        let absolute_path = match exec::resolve_workspace_path(cwd, &path) {
            Ok(path) => path,
            Err(error) => {
                changed_paths.push(path.clone());
                failures.push(format!(
                    "failed to normalize changed path '{}': {error}",
                    path
                ));
                continue;
            }
        };
        let post_fingerprint = match exec::apply_patch::fingerprint_path(&absolute_path) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                changed_paths.push(path.clone());
                failures.push(format!(
                    "failed to inspect changed path '{}': {error}",
                    path
                ));
                continue;
            }
        };

        let before =
            if let Some(preimage) = preimages.paths.remove(&path) {
                if let Some(snapshot) = preimage.snapshot {
                    if preimage_matches_postimage(&snapshot.before, &post_fingerprint) {
                        continue;
                    }
                    snapshot.before
                } else {
                    if preimage
                        .fingerprint
                        .as_ref()
                        .is_ok_and(|before| before == &post_fingerprint)
                    {
                        continue;
                    }
                    changed_paths.push(path.clone());
                    failures.push(preimage.skip_reason.unwrap_or_else(|| {
                        format!("dirty preimage '{}' was not recoverable", path)
                    }));
                    continue;
                }
            } else {
                match resolve_git_preimage(
                    cwd,
                    preimages.pre_head.as_deref(),
                    &path,
                    &absolute_path,
                    &post_fingerprint,
                    &mut preimages.snapshot_budget,
                    &mut preimages.materialized_bytes,
                ) {
                    Ok(GitPreimageResolution::Unchanged) => continue,
                    Ok(GitPreimageResolution::Preimage(before)) => before,
                    Err(error) => {
                        changed_paths.push(path.clone());
                        failures.push(error);
                        continue;
                    }
                }
            };

        changed_paths.push(path.clone());
        let (after_exists, after_sha256) = match &post_fingerprint {
            FileFingerprint::Missing => (false, None),
            FileFingerprint::Regular { sha256, .. } => (true, Some(sha256.clone())),
            FileFingerprint::Symlink { .. } => {
                failures.push(format!(
                    "changed path '{}' is a symbolic link and cannot use byte restore",
                    path
                ));
                continue;
            }
            FileFingerprint::Other { kind, .. } => {
                failures.push(format!(
                    "changed path '{}' is a {kind} and cannot use byte restore",
                    path
                ));
                continue;
            }
        };
        let snapshot = FileSnapshot {
            display_path: path,
            absolute_path,
            before,
            after_exists: Some(after_exists),
            after_sha256,
        };
        append_shell_file_diff(cwd, &snapshot, &post_fingerprint, &mut diff);
        snapshots.push(snapshot);
    }

    if changed_paths.is_empty() {
        return Ok(None);
    }
    if !failures.is_empty() {
        return Ok(Some(ShellTurnDiff {
            snapshots: Vec::new(),
            diff: bounded_nonrevertible_diff(&changed_paths, &bounded_failure_reason(&failures)),
            revertible: false,
        }));
    }
    Ok(Some(ShellTurnDiff {
        snapshots,
        diff,
        revertible: true,
    }))
}

fn preimage_matches_postimage(before: &Option<Vec<u8>>, postimage: &FileFingerprint) -> bool {
    match (before, postimage) {
        (None, FileFingerprint::Missing) => true,
        (Some(before), FileFingerprint::Regular { sha256, .. }) => {
            exec::apply_patch::sha256_hex(before) == *sha256
        }
        _ => false,
    }
}

fn resolve_git_preimage(
    cwd: &Path,
    pre_head: Option<&str>,
    path: &str,
    absolute_path: &Path,
    postimage: &FileFingerprint,
    snapshot_budget: &mut exec::apply_patch::SnapshotBudget,
    materialized_bytes: &mut usize,
) -> Result<GitPreimageResolution, String> {
    let Some(pre_head) = pre_head else {
        return Ok(GitPreimageResolution::Preimage(None));
    };
    let Some(entry) = git_tree_entry(cwd, pre_head, path)? else {
        return Ok(GitPreimageResolution::Preimage(None));
    };
    if entry.mode != "100644" && entry.mode != "100755" {
        return Err(format!(
            "changed path '{}' had Git mode {} before the turn and cannot use byte restore",
            path, entry.mode
        ));
    }
    if matches!(postimage, FileFingerprint::Regular { .. })
        && git_hash_worktree_file(cwd, path).ok().as_deref() == Some(entry.object.as_str())
    {
        return Ok(GitPreimageResolution::Unchanged);
    }

    let size = git_blob_size(cwd, &entry.object)?;
    if materialized_bytes.saturating_add(size) > MAX_SHELL_PREIMAGE_BYTES {
        return Err(format!(
            "preimage for changed path '{}' exceeds the 64 MiB shell preimage ceiling",
            path
        ));
    }
    let bytes = git_blob_bytes(cwd, &entry.object, size)?;
    if size <= exec::apply_patch::MAX_SNAPSHOT_FILE_BYTES {
        snapshot_budget
            .account_materialized(bytes.len())
            .map_err(|error| format!("preimage for changed path '{}': {error}", path))?;
    }
    *materialized_bytes = materialized_bytes.saturating_add(bytes.len());
    if preimage_matches_postimage(&Some(bytes.clone()), postimage) {
        return Ok(GitPreimageResolution::Unchanged);
    }

    let resolved = exec::resolve_workspace_path(cwd, path)?;
    if resolved != absolute_path {
        return Err(format!(
            "changed path '{}' resolved inconsistently while its preimage was captured",
            path
        ));
    }
    Ok(GitPreimageResolution::Preimage(Some(bytes)))
}

fn git_head(cwd: &Path) -> Result<Option<String>, String> {
    let output = exec::managed_git_command(cwd)?
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|error| format!("failed to read Git HEAD: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let head = std::str::from_utf8(&output.stdout)
        .map_err(|_| "Git HEAD was not UTF-8".to_string())?
        .trim();
    if head.is_empty() {
        Ok(None)
    } else {
        Ok(Some(head.to_string()))
    }
}

fn git_status(cwd: &Path) -> Result<BTreeMap<String, String>, String> {
    let bytes = git_output_bounded(
        cwd,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
        ],
        MAX_GIT_PATH_OUTPUT_BYTES,
        "Git status",
    )?;
    if !bytes.is_empty() && !bytes.ends_with(&[0]) {
        return Err("Git status returned an incomplete record".to_string());
    }
    let mut records = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut status = BTreeMap::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            return Err("Git status returned an invalid porcelain record".to_string());
        }
        let code = std::str::from_utf8(&record[..2])
            .map_err(|_| "Git status returned an invalid status code".to_string())?;
        let path = std::str::from_utf8(&record[3..])
            .map_err(|_| "Git returned a non-UTF-8 path".to_string())?
            .to_string();
        if code
            .as_bytes()
            .iter()
            .any(|byte| *byte == b'R' || *byte == b'C')
        {
            let original = records
                .next()
                .ok_or_else(|| "Git status omitted a rename source".to_string())?;
            let original = std::str::from_utf8(original)
                .map_err(|_| "Git returned a non-UTF-8 path".to_string())?
                .to_string();
            status.insert(path.clone(), format!("{code}:from:{original}"));
            status.insert(original, format!("{code}:to:{path}"));
        } else {
            status.insert(path, code.to_string());
        }
    }
    Ok(status)
}

fn git_head_changed_paths(
    cwd: &Path,
    pre_head: Option<&str>,
    post_head: Option<&str>,
) -> Result<Vec<String>, String> {
    match (pre_head, post_head) {
        (Some(pre_head), Some(post_head)) if pre_head != post_head => git_paths(
            cwd,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--name-only",
                "-z",
                "--no-renames",
                pre_head,
                post_head,
                "--",
            ],
        )
        .map_err(|error| format!("failed to compare pre-turn and post-turn HEAD: {error}")),
        (None, Some(post_head)) => git_paths(
            cwd,
            &[
                "diff-tree",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                post_head,
                "--",
            ],
        )
        .map_err(|error| format!("failed to compare unborn HEAD with post-turn HEAD: {error}")),
        (Some(_), None) => Err(
            "Git HEAD became unborn during the turn, so Halv could not recover clean preimages"
                .to_string(),
        ),
        _ => Ok(Vec::new()),
    }
}

fn git_tree_entry(cwd: &Path, head: &str, path: &str) -> Result<Option<GitTreeEntry>, String> {
    let bytes = git_output_bounded(
        cwd,
        &["ls-tree", "-z", head, "--", path],
        MAX_GIT_PATH_OUTPUT_BYTES,
        "Git tree lookup",
    )?;
    let Some(record) = bytes
        .split(|byte| *byte == 0)
        .find(|record| !record.is_empty())
    else {
        return Ok(None);
    };
    let metadata = record
        .split(|byte| *byte == b'\t')
        .next()
        .ok_or_else(|| format!("Git tree entry for '{}' was invalid", path))?;
    let metadata = std::str::from_utf8(metadata)
        .map_err(|_| format!("Git tree entry for '{}' was not UTF-8", path))?;
    let mut fields = metadata.split_whitespace();
    let mode = fields
        .next()
        .ok_or_else(|| format!("Git tree entry for '{}' omitted its mode", path))?;
    let kind = fields
        .next()
        .ok_or_else(|| format!("Git tree entry for '{}' omitted its kind", path))?;
    let object = fields
        .next()
        .ok_or_else(|| format!("Git tree entry for '{}' omitted its object", path))?;
    if kind != "blob" {
        return Err(format!(
            "Git preimage for changed path '{}' is a {kind}, not a blob",
            path
        ));
    }
    Ok(Some(GitTreeEntry {
        mode: mode.to_string(),
        object: object.to_string(),
    }))
}

fn git_hash_worktree_file(cwd: &Path, path: &str) -> Result<String, String> {
    let bytes = git_output_bounded(
        cwd,
        &["hash-object", "--no-filters", "--", path],
        256,
        "Git worktree hash",
    )?;
    let hash = std::str::from_utf8(&bytes)
        .map_err(|_| format!("Git worktree hash for '{}' was not UTF-8", path))?
        .trim();
    if hash.is_empty() {
        Err(format!("Git worktree hash for '{}' was empty", path))
    } else {
        Ok(hash.to_string())
    }
}

fn git_blob_size(cwd: &Path, object: &str) -> Result<usize, String> {
    let bytes = git_output_bounded(cwd, &["cat-file", "-s", object], 256, "Git blob size")?;
    let size = std::str::from_utf8(&bytes)
        .map_err(|_| "Git blob size was not UTF-8".to_string())?
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("Git blob size was invalid: {error}"))?;
    usize::try_from(size).map_err(|_| "Git blob size exceeded this platform".to_string())
}

fn git_blob_bytes(cwd: &Path, object: &str, size: usize) -> Result<Vec<u8>, String> {
    let bytes = git_output_bounded(cwd, &["cat-file", "blob", object], size, "Git blob")?;
    if bytes.len() != size {
        return Err(format!(
            "Git blob '{}' changed size while Halv read it",
            object
        ));
    }
    Ok(bytes)
}

fn is_not_git_repository_error(error: &str) -> bool {
    error.to_ascii_lowercase().contains("not a git repository")
}

fn git_output_bounded(
    cwd: &Path,
    args: &[&str],
    max_bytes: usize,
    description: &str,
) -> Result<Vec<u8>, String> {
    const MAX_GIT_STDERR_BYTES: usize = 4 * 1024;
    let mut child = exec::managed_git_command(cwd)?
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run {description}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("failed to capture {description} output"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("failed to capture {description} error output"))?;
    let stderr_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut bounded = (&mut stderr).take(MAX_GIT_STDERR_BYTES as u64);
        bounded.read_to_end(&mut bytes)?;
        std::io::copy(&mut stderr, &mut std::io::sink())?;
        Ok(bytes)
    });
    let mut bytes = Vec::new();
    let read_result = stdout
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes);
    if let Err(error) = read_result {
        let _ = child.kill();
        let _ = child.wait();
        let _ = stderr_reader.join();
        return Err(format!("failed to read {description}: {error}"));
    }
    if bytes.len() > max_bytes {
        let _ = child.kill();
        let _ = child.wait();
        let _ = stderr_reader.join();
        return Err(format!(
            "{description} exceeded the {} MiB safety limit",
            max_bytes / (1024 * 1024)
        ));
    }
    let wait_result = child.wait();
    let stderr = stderr_reader
        .join()
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or_default();
    let status =
        wait_result.map_err(|error| format!("failed to wait for {description}: {error}"))?;
    if !status.success() {
        let first_line = String::from_utf8_lossy(&stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string);
        return Err(match first_line {
            Some(line) => format!("{description} failed: {line}"),
            None => format!("{description} failed"),
        });
    }
    Ok(bytes)
}

fn append_shell_file_diff(
    cwd: &Path,
    snapshot: &FileSnapshot,
    postimage: &FileFingerprint,
    output: &mut String,
) {
    const ADDITIONAL_OMITTED: &str =
        "Halv omitted additional file diffs because the rendered diff reached its safety limit.\n";
    let (after, after_exists, force_omitted) = match postimage {
        FileFingerprint::Missing => (None, false, false),
        FileFingerprint::Regular { size, .. }
            if *size <= exec::apply_patch::MAX_INLINE_DIFF_INPUT_BYTES as u64 =>
        {
            match exec::apply_patch::read_regular_file_bounded(
                &snapshot.absolute_path,
                exec::apply_patch::MAX_INLINE_DIFF_INPUT_BYTES,
            ) {
                Ok(after) => (after, true, false),
                Err(_) => (None, true, true),
            }
        }
        FileFingerprint::Regular { .. } => (None, true, true),
        FileFingerprint::Symlink { .. } | FileFingerprint::Other { .. } => (None, true, true),
    };
    let file_diff = exec::apply_patch::render_file_diff(
        cwd,
        snapshot,
        after.as_deref(),
        after_exists,
        force_omitted,
    );
    if output.len().saturating_add(file_diff.len()) <= exec::apply_patch::MAX_RENDERED_DIFF_BYTES {
        output.push_str(&file_diff);
        return;
    }
    let placeholder = exec::apply_patch::render_file_diff(cwd, snapshot, None, after_exists, true);
    if output.len().saturating_add(placeholder.len()) <= exec::apply_patch::MAX_RENDERED_DIFF_BYTES
    {
        output.push_str(&placeholder);
    } else if !output.ends_with(ADDITIONAL_OMITTED)
        && output.len().saturating_add(ADDITIONAL_OMITTED.len())
            <= exec::apply_patch::MAX_RENDERED_DIFF_BYTES
    {
        output.push_str(ADDITIONAL_OMITTED);
    }
}

fn bounded_failure_reason(failures: &[String]) -> String {
    const MAX_REASON_CHARS: usize = 2_000;
    let mut reason = failures
        .iter()
        .map(|failure| failure.replace(['\r', '\n'], " "))
        .collect::<Vec<_>>()
        .join("; ");
    if reason.chars().count() > MAX_REASON_CHARS {
        reason = reason.chars().take(MAX_REASON_CHARS).collect();
        reason.push_str("...");
    }
    reason
}

pub(super) fn changed_file_snapshots(
    snapshots: Vec<FileSnapshot>,
) -> Result<Vec<FileSnapshot>, String> {
    let mut budget = exec::apply_patch::SnapshotBudget::default();
    let mut changed = Vec::new();
    for mut snapshot in snapshots {
        let after = budget.read(&snapshot.absolute_path).map_err(|error| {
            format!(
                "failed to capture postimage for `{}`: {error}",
                snapshot.display_path
            )
        })?;
        if snapshot.before != after {
            snapshot.capture_postimage(after.as_deref());
            changed.push(snapshot);
        }
    }
    Ok(changed)
}

pub(super) fn bounded_nonrevertible_diff(changed_paths: &[String], reason: &str) -> String {
    let reason = bounded_failure_reason(&[reason.to_string()]);
    let reason = if reason.is_empty() {
        "Halv could not safely recover one or more changed preimages".to_string()
    } else {
        reason
    };
    let mut output = format!("This turn cannot be reverted: {reason}.\n");
    const OMITTED: &str = "- \"additional paths omitted\"\n";
    for path in changed_paths {
        let line = format!(
            "- {}\n",
            serde_json::to_string(path).unwrap_or_else(|_| "\"?\"".to_string())
        );
        if output.len().saturating_add(line.len()) > exec::apply_patch::MAX_RENDERED_DIFF_BYTES {
            if output.len().saturating_add(OMITTED.len())
                <= exec::apply_patch::MAX_RENDERED_DIFF_BYTES
            {
                output.push_str(OMITTED);
            }
            break;
        }
        output.push_str(&line);
    }
    output
}

#[cfg(test)]
fn git_shell_diff(cwd: &Path, changed_paths: &[String]) -> Result<String, String> {
    let mut tracked_paths = Vec::new();
    let mut untracked_paths = Vec::new();
    for path in changed_paths {
        if git_path_is_tracked(cwd, path) {
            tracked_paths.push(path.clone());
        } else {
            untracked_paths.push(path.clone());
        }
    }

    let mut diff = String::new();
    if !tracked_paths.is_empty() {
        let output = exec::managed_git_command(cwd)?
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--",
            ])
            .args(&tracked_paths)
            .output()
            .map_err(|error| format!("failed to run git diff: {error}"))?;
        if output.status.success() {
            if output.stdout.len() > exec::apply_patch::MAX_RENDERED_DIFF_BYTES {
                return Err("Git diff exceeded the rendered diff safety limit".to_string());
            }
            diff.push_str(&String::from_utf8_lossy(&output.stdout));
        }
    }

    for path in untracked_paths {
        let absolute_path = exec::resolve_workspace_path(cwd, &path)?;
        let snapshot = FileSnapshot {
            display_path: path,
            absolute_path,
            before: None,
            after_exists: None,
            after_sha256: None,
        };
        let file_diff = exec::patch_diff(cwd, &mut [snapshot])?;
        if diff.len().saturating_add(file_diff.len()) > exec::apply_patch::MAX_RENDERED_DIFF_BYTES {
            return Err("Git diff exceeded the rendered diff safety limit".to_string());
        }
        diff.push_str(&file_diff);
    }

    Ok(diff)
}

#[cfg(test)]
fn git_path_is_tracked(cwd: &Path, path: &str) -> bool {
    let Ok(mut command) = exec::managed_git_command(cwd) else {
        return false;
    };
    command
        .args(["ls-files", "--error-unmatch", "--", path])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn git_paths(cwd: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let mut command = exec::managed_git_command(cwd)?;
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run Git: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture Git paths".to_string())?;
    let mut bytes = Vec::new();
    if let Err(error) = stdout
        .take(MAX_GIT_PATH_OUTPUT_BYTES.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("failed to read Git paths: {error}"));
    }
    if bytes.len() > MAX_GIT_PATH_OUTPUT_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Git path enumeration exceeded the safety limit".to_string());
    }
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for Git: {error}"))?;
    if !status.success() {
        return Err("Git path enumeration failed".to_string());
    }
    if !bytes.is_empty() && !bytes.ends_with(&[0]) {
        return Err("Git path enumeration returned an incomplete record".to_string());
    }

    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(ToString::to_string)
                .map_err(|_| "Git returned a non-UTF-8 path".to_string())
        })
        .collect()
}

#[cfg(test)]
pub(super) fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(test)]
pub(super) fn init_git_repo(root: &Path) {
    std::fs::create_dir_all(root).expect("git root should create");
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "halv@example.test"]);
    run_git(root, &["config", "user.name", "Halv Tests"]);
}

#[cfg(test)]
mod tests {
    use super::super::{record_turn_diff, DEFAULT_MODEL};
    use super::*;
    use crate::{
        exec::WorkspaceRoots,
        storage::{self, Storage},
    };
    use std::io::Write as _;

    #[test]
    fn shell_turn_diff_persists_only_changed_snapshots() {
        let root =
            std::env::temp_dir().join(format!("halv-shell-changed-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        std::fs::write(root.join("changed.txt"), "before\n").expect("changed should write");
        std::fs::write(root.join("unchanged.txt"), "same\n").expect("unchanged should write");
        std::fs::write(root.join("untracked-before.txt"), "same\n")
            .expect("untracked should write");
        run_git(&root, &["add", "changed.txt", "unchanged.txt"]);

        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);
        std::fs::write(root.join("changed.txt"), "after\n").expect("changed should update");
        std::fs::write(root.join("new.txt"), "new\n").expect("new should write");

        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");
        let paths = turn_diff
            .snapshots
            .iter()
            .map(|snapshot| snapshot.display_path.as_str())
            .collect::<BTreeSet<_>>();

        assert!(turn_diff.revertible);
        assert_eq!(paths, BTreeSet::from(["changed.txt", "new.txt"]));
        assert!(turn_diff.diff.contains("changed.txt"));
        assert!(turn_diff.diff.contains("new.txt"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn git_path_enumeration_preserves_whitespace_and_newlines() {
        let root = std::env::temp_dir().join(format!("halv-git-paths-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        let unusual = " leading and trailing \nname.txt ";
        std::fs::write(root.join(unusual), "content").expect("unusual path should write");

        let paths = git_paths(&root, &["ls-files", "-z", "-co", "--exclude-standard"])
            .expect("Git paths should enumerate");

        assert!(paths.iter().any(|path| path == unusual));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn git_output_bounded_surfaces_first_stderr_line() {
        let root = std::env::temp_dir().join(format!("halv-git-stderr-{}", storage::new_uuid_v4()));
        init_git_repo(&root);

        let error = git_output_bounded(
            &root,
            &["rev-parse", "--verify", "halv-missing-revision"],
            256,
            "Git failure probe",
        )
        .expect_err("missing revision should fail");

        assert!(error.starts_with("Git failure probe failed: fatal:"));
        assert_eq!(error.lines().count(), 1);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_preimage_error_records_non_revertible_diff_without_snapshots() {
        let root = std::env::temp_dir().join(format!(
            "halv-shell-unrevertible-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        std::fs::write(root.join("tracked.txt"), "before\n").expect("tracked should write");
        run_git(&root, &["add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "after\n").expect("tracked should update");

        let root_string = root.to_string_lossy().to_string();
        let turn_diff = shell_turn_diff(
            &root_string,
            Err("Git status failed: fatal: transient status failure".to_string()),
        )
        .expect("shell diff should build")
        .expect("shell diff should exist");
        assert!(!turn_diff.revertible);
        assert!(turn_diff.snapshots.is_empty());
        assert!(turn_diff.diff.starts_with(
            "This turn cannot be reverted: Halv could not capture the pre-turn Git state: Git status failed: fatal: transient status failure.\n"
        ));

        let db_path = std::env::temp_dir().join(format!(
            "halv-shell-unrevertible-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Track shell failure", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        record_turn_diff(
            &storage,
            &conversation_id,
            "turn-1",
            &root,
            turn_diff.snapshots,
            turn_diff.diff,
            turn_diff.revertible,
        )
        .expect("turn diff should record");

        let diffs = storage
            .list_turn_diffs(&conversation_id)
            .expect("diffs should list");
        assert_eq!(diffs.len(), 1);
        assert!(!diffs[0].revertible);
        assert!(storage
            .get_turn_snapshots(&conversation_id, "turn-1")
            .expect("snapshots should list")
            .is_empty());

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn shell_turn_diff_recovers_clean_tracked_preimage_from_git() {
        let root = std::env::temp_dir().join(format!(
            "halv-shell-clean-tracked-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        let tracked_path = root.join("tracked.txt");
        std::fs::write(&tracked_path, b"before\n").expect("tracked file should write");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-m", "initial"]);
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);

        std::fs::write(&tracked_path, b"after\n").expect("tracked file should update");
        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(turn_diff.revertible);
        assert_eq!(turn_diff.snapshots.len(), 1);
        assert_eq!(
            turn_diff.snapshots[0].before.as_deref(),
            Some(b"before\n".as_slice())
        );
        let roots = WorkspaceRoots::for_cwd(&root);
        exec::apply_patch::restore_snapshots(&root, &turn_diff.snapshots, &roots)
            .expect("clean tracked file should restore");
        assert_eq!(std::fs::read(&tracked_path).unwrap(), b"before\n");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_turn_diff_recovers_clean_preimage_when_head_moves() {
        let root =
            std::env::temp_dir().join(format!("halv-shell-head-moved-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        let tracked_path = root.join("tracked.txt");
        std::fs::write(&tracked_path, b"before\n").expect("tracked file should write");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-m", "initial"]);
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);

        std::fs::write(&tracked_path, b"after\n").expect("tracked file should update");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-m", "turn commit"]);
        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(turn_diff.revertible);
        assert_eq!(turn_diff.snapshots.len(), 1);
        let roots = WorkspaceRoots::for_cwd(&root);
        exec::apply_patch::restore_snapshots(&root, &turn_diff.snapshots, &roots)
            .expect("pre-commit bytes should restore");
        assert_eq!(std::fs::read(&tracked_path).unwrap(), b"before\n");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_turn_diff_skips_non_git_directory() {
        let root =
            std::env::temp_dir().join(format!("halv-shell-non-git-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("non-Git root should create");
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);

        let turn_diff =
            shell_turn_diff(&root_string, preimages).expect("non-Git shell diff should build");

        assert!(turn_diff.is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_turn_diff_deletes_file_created_by_turn() {
        let root =
            std::env::temp_dir().join(format!("halv-shell-created-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);
        let created_path = root.join("created.txt");
        std::fs::write(&created_path, b"created\n").expect("created file should write");

        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(turn_diff.revertible);
        assert_eq!(turn_diff.snapshots.len(), 1);
        assert!(turn_diff.snapshots[0].before.is_none());
        let roots = WorkspaceRoots::for_cwd(&root);
        exec::apply_patch::restore_snapshots(&root, &turn_diff.snapshots, &roots)
            .expect("created file should delete");
        assert!(!created_path.exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn shell_turn_diff_ignores_untouched_tracked_symlink() {
        let root = std::env::temp_dir().join(format!(
            "halv-shell-untouched-symlink-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        std::fs::write(root.join("target.txt"), b"target\n").expect("target should write");
        std::fs::write(root.join("tracked.txt"), b"before\n").expect("tracked should write");
        std::os::unix::fs::symlink("target.txt", root.join("link.txt"))
            .expect("tracked symlink should create");
        run_git(&root, &["add", "target.txt", "tracked.txt", "link.txt"]);
        run_git(&root, &["commit", "-m", "initial"]);
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);

        std::fs::write(root.join("tracked.txt"), b"after\n").expect("tracked should update");
        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(turn_diff.revertible);
        assert_eq!(turn_diff.snapshots.len(), 1);
        assert_eq!(turn_diff.snapshots[0].display_path, "tracked.txt");
        assert!(root.join("link.txt").is_symlink());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_turn_diff_restores_dirty_untracked_preimage() {
        let root = std::env::temp_dir().join(format!(
            "halv-shell-dirty-untracked-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        let dirty_path = root.join("dirty.txt");
        std::fs::write(&dirty_path, b"before\n").expect("dirty file should write");
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);

        std::fs::write(&dirty_path, b"after\n").expect("dirty file should update");
        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(turn_diff.revertible);
        let roots = WorkspaceRoots::for_cwd(&root);
        exec::apply_patch::restore_snapshots(&root, &turn_diff.snapshots, &roots)
            .expect("dirty untracked file should restore");
        assert_eq!(std::fs::read(&dirty_path).unwrap(), b"before\n");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_turn_diff_restores_dirty_oversized_preimage_below_ceiling() {
        let root = std::env::temp_dir().join(format!(
            "halv-shell-dirty-oversized-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        let dirty_path = root.join("dirty.bin");
        let mut dirty_file = std::fs::File::create(&dirty_path).expect("dirty file should create");
        dirty_file
            .set_len(9 * 1024 * 1024)
            .expect("dirty file should grow");
        dirty_file
            .write_all(b"before")
            .expect("dirty prefix should write");
        drop(dirty_file);
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);

        let mut dirty_file = std::fs::OpenOptions::new()
            .write(true)
            .open(&dirty_path)
            .expect("dirty file should reopen");
        dirty_file
            .write_all(b"after!")
            .expect("dirty prefix should update");
        drop(dirty_file);
        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(turn_diff.revertible);
        let roots = WorkspaceRoots::for_cwd(&root);
        exec::apply_patch::restore_snapshots(&root, &turn_diff.snapshots, &roots)
            .expect("oversized dirty file should restore");
        let mut prefix = [0_u8; 6];
        std::fs::File::open(&dirty_path)
            .unwrap()
            .read_exact(&mut prefix)
            .unwrap();
        assert_eq!(&prefix, b"before");

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn shell_postimage_symlink_is_never_read_or_persisted() {
        let root = std::env::temp_dir().join(format!(
            "halv-shell-symlink-postimage-{}",
            storage::new_uuid_v4()
        ));
        let outside =
            std::env::temp_dir().join(format!("halv-shell-secret-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        std::fs::write(&outside, "host-secret-must-not-appear").expect("secret should write");
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);
        std::os::unix::fs::symlink(&outside, root.join("leak.txt")).expect("symlink should create");

        let turn_diff = shell_turn_diff(&root_string, preimages)
            .expect("shell diff should build")
            .expect("shell diff should exist");

        assert!(!turn_diff.revertible);
        assert!(turn_diff.snapshots.is_empty());
        assert!(turn_diff.diff.contains("leak.txt"));
        assert!(turn_diff.diff.contains("symbolic link"));
        assert!(!turn_diff.diff.contains("host-secret-must-not-appear"));

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn fallback_git_diff_disables_external_diff_commands() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "halv-shell-external-diff-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        std::fs::write(root.join("tracked.txt"), "before\n").expect("tracked should write");
        run_git(&root, &["add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "after\n").expect("tracked should update");

        let sentinel = root.join("external-diff-ran");
        let external_diff = root.join("external-diff.sh");
        std::fs::write(
            &external_diff,
            format!("#!/bin/sh\n: > '{}'\n", sentinel.display()),
        )
        .expect("external diff should write");
        let mut permissions = std::fs::metadata(&external_diff).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&external_diff, permissions).unwrap();
        run_git(
            &root,
            &[
                "config",
                "diff.external",
                external_diff.to_string_lossy().as_ref(),
            ],
        );

        let diff = git_shell_diff(&root, &["tracked.txt".to_string()])
            .expect("fallback diff should render");

        assert!(diff.contains("tracked.txt"));
        assert!(diff.contains("after"));
        assert!(
            !sentinel.exists(),
            "managed Git must disable external diff commands"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
