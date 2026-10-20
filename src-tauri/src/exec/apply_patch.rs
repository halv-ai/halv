use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

const BEGIN_MARKER: &str = "*** Begin Patch";
const END_MARKER: &str = "*** End Patch";
const ADD_FILE: &str = "*** Add File: ";
const DELETE_FILE: &str = "*** Delete File: ";
const UPDATE_FILE: &str = "*** Update File: ";
const MOVE_TO: &str = "*** Move to: ";
pub(crate) const MAX_SNAPSHOT_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SNAPSHOT_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_SNAPSHOT_FILES: usize = 10_000;
pub(crate) const MAX_INLINE_DIFF_INPUT_BYTES: usize = 512 * 1024;
pub(crate) const MAX_RENDERED_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct SnapshotBudget {
    files: usize,
    total_bytes: usize,
    max_files: usize,
    max_total_bytes: usize,
    max_file_bytes: usize,
}

impl Default for SnapshotBudget {
    fn default() -> Self {
        Self {
            files: 0,
            total_bytes: 0,
            max_files: MAX_SNAPSHOT_FILES,
            max_total_bytes: MAX_SNAPSHOT_TOTAL_BYTES,
            max_file_bytes: MAX_SNAPSHOT_FILE_BYTES,
        }
    }
}

impl SnapshotBudget {
    pub(crate) fn read(&mut self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        self.files = self
            .files
            .checked_add(1)
            .ok_or_else(|| "snapshot file count overflowed".to_string())?;
        if self.files > self.max_files {
            return Err(format!(
                "snapshot exceeds the {MAX_SNAPSHOT_FILES}-file safety limit"
            ));
        }

        let bytes = read_regular_file_bounded(path, self.max_file_bytes)?;
        if let Some(bytes) = bytes.as_ref() {
            self.total_bytes = self
                .total_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| "snapshot byte count overflowed".to_string())?;
            if self.total_bytes > self.max_total_bytes {
                return Err(format!(
                    "snapshot exceeds the {} MiB aggregate safety limit",
                    self.max_total_bytes / (1024 * 1024)
                ));
            }
        }
        Ok(bytes)
    }

    pub(crate) fn account_materialized(&mut self, bytes: usize) -> Result<(), String> {
        self.files = self
            .files
            .checked_add(1)
            .ok_or_else(|| "snapshot file count overflowed".to_string())?;
        if self.files > self.max_files {
            return Err(format!(
                "snapshot exceeds {MAX_SNAPSHOT_FILES}-file safety limit"
            ));
        }
        if bytes > self.max_file_bytes {
            return Err(format!(
                "preimage exceeds {} MiB per-file snapshot safety limit",
                self.max_file_bytes / (1024 * 1024)
            ));
        }
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "snapshot byte count overflowed".to_string())?;
        if self.total_bytes > self.max_total_bytes {
            return Err(format!(
                "snapshot exceeds {} MiB aggregate safety limit",
                self.max_total_bytes / (1024 * 1024)
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn with_limits(max_files: usize, max_total_bytes: usize, max_file_bytes: usize) -> Self {
        Self {
            files: 0,
            total_bytes: 0,
            max_files,
            max_total_bytes,
            max_file_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPatch {
    operations: Vec<PatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchOperation {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchHunk {
    lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HunkLine {
    Context(String),
    Delete(String),
    Add(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PatchFileChange {
    pub path: String,
    pub change_type: PatchChangeType,
    pub requires_approval: bool,
    pub approved: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatchChangeType {
    Add,
    Delete,
    Update,
    MoveSource,
    MoveTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub display_path: String,
    pub absolute_path: PathBuf,
    pub before: Option<Vec<u8>>,
    #[serde(default)]
    pub after_exists: Option<bool>,
    #[serde(default)]
    pub after_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileFingerprint {
    Missing,
    Regular {
        size: u64,
        sha256: String,
    },
    Symlink {
        target: PathBuf,
    },
    Other {
        kind: &'static str,
        size: u64,
        modified_nanos: Option<u128>,
    },
}

impl FileSnapshot {
    pub(crate) fn capture_postimage(&mut self, after: Option<&[u8]>) {
        self.after_exists = Some(after.is_some());
        self.after_sha256 = after.map(sha256_hex);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPatchSummary {
    pub files_changed: usize,
    pub message: String,
}

pub fn apply_patch_tool_schema() -> Value {
    json!({
        "type": "custom",
        "name": "apply_patch",
        "description": "Apply a patch to files. Input must use the Codex apply_patch envelope.",
        "format": {
            "type": "grammar",
            "syntax": "lark",
            "definition": concat!(
                "start: begin_patch hunk+ end_patch\n",
                "begin_patch: \"*** Begin Patch\" LF\n",
                "end_patch: \"*** End Patch\" LF?\n",
                "hunk: add_hunk | delete_hunk | update_hunk\n",
                "add_hunk: \"*** Add File: \" /.+/ LF add_line+\n",
                "add_line: \"+\" /.*?/ LF\n",
                "delete_hunk: \"*** Delete File: \" /.+/ LF\n",
                "update_hunk: \"*** Update File: \" /.+/ LF move? change?\n",
                "move: \"*** Move to: \" /.+/ LF\n",
                "change: (\"@@\" /.*?/ LF)? ((\" \" | \"+\" | \"-\") /.*?/ LF)+\n",
                "LF: /\\n/"
            )
        }
    })
}

pub fn parse_patch(input: &str) -> Result<ParsedPatch, String> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized.split('\n').collect::<Vec<_>>();
    let first = lines
        .first()
        .copied()
        .ok_or_else(|| "apply_patch input is empty".to_string())?;
    if first != BEGIN_MARKER {
        return Err("apply_patch input must begin with `*** Begin Patch`".to_string());
    }

    let mut index = 1;
    let mut operations = Vec::new();
    while index < lines.len() {
        let line = lines[index];
        if line == END_MARKER {
            let trailing = &lines[index + 1..];
            if trailing.iter().any(|line| !line.is_empty()) {
                return Err("apply_patch input has content after `*** End Patch`".to_string());
            }
            if operations.is_empty() {
                return Err("apply_patch input has no file operations".to_string());
            }
            return Ok(ParsedPatch { operations });
        }

        if let Some(path) = line.strip_prefix(ADD_FILE) {
            validate_patch_path(path)?;
            index += 1;
            let mut added = Vec::new();
            while index < lines.len() && !is_operation_or_end(lines[index]) {
                let line = lines[index];
                let Some(content) = line.strip_prefix('+') else {
                    return Err(format!(
                        "add file `{path}` contains a non-add line: `{line}`"
                    ));
                };
                added.push(content.to_string());
                index += 1;
            }
            if added.is_empty() {
                return Err(format!(
                    "add file `{path}` must contain at least one `+` line"
                ));
            }
            operations.push(PatchOperation::Add {
                path: path.to_string(),
                lines: added,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix(DELETE_FILE) {
            validate_patch_path(path)?;
            index += 1;
            operations.push(PatchOperation::Delete {
                path: path.to_string(),
            });
            continue;
        }

        if let Some(path) = line.strip_prefix(UPDATE_FILE) {
            validate_patch_path(path)?;
            index += 1;
            let mut move_to = None;
            let mut hunks = Vec::new();
            let mut current = Vec::new();

            while index < lines.len() && !is_operation_or_end(lines[index]) {
                let line = lines[index];
                if let Some(target) = line.strip_prefix(MOVE_TO) {
                    validate_patch_path(target)?;
                    if move_to.replace(target.to_string()).is_some() {
                        return Err(format!(
                            "update file `{path}` has more than one move target"
                        ));
                    }
                    index += 1;
                    continue;
                }
                if line.starts_with("@@") {
                    if !current.is_empty() {
                        hunks.push(PatchHunk { lines: current });
                        current = Vec::new();
                    }
                    index += 1;
                    continue;
                }
                let Some((prefix, content)) = split_hunk_line(line) else {
                    return Err(format!(
                        "update file `{path}` contains an invalid hunk line"
                    ));
                };
                current.push(match prefix {
                    ' ' => HunkLine::Context(content.to_string()),
                    '-' => HunkLine::Delete(content.to_string()),
                    '+' => HunkLine::Add(content.to_string()),
                    _ => unreachable!("split_hunk_line only returns known prefixes"),
                });
                index += 1;
            }

            if !current.is_empty() {
                hunks.push(PatchHunk { lines: current });
            }
            if hunks.is_empty() && move_to.is_none() {
                return Err(format!("update file `{path}` has no hunks or move target"));
            }
            operations.push(PatchOperation::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }

        if line.trim().is_empty() {
            index += 1;
            continue;
        }

        return Err(format!("unknown apply_patch directive: `{line}`"));
    }

    Err("apply_patch input must end with `*** End Patch`".to_string())
}

pub fn file_changes(parsed: &ParsedPatch) -> Vec<PatchFileChange> {
    parsed
        .operations
        .iter()
        .flat_map(|operation| match operation {
            PatchOperation::Add { path, .. } => vec![change(path, PatchChangeType::Add)],
            PatchOperation::Delete { path } => vec![change(path, PatchChangeType::Delete)],
            PatchOperation::Update { path, move_to, .. } => {
                let mut changes = vec![change(
                    path,
                    if move_to.is_some() {
                        PatchChangeType::MoveSource
                    } else {
                        PatchChangeType::Update
                    },
                )];
                if let Some(target) = move_to {
                    changes.push(change(target, PatchChangeType::MoveTarget));
                }
                changes
            }
        })
        .collect()
}

pub fn touched_paths(parsed: &ParsedPatch) -> Vec<String> {
    file_changes(parsed)
        .into_iter()
        .map(|change| change.path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn snapshot_paths(cwd: &Path, paths: &[String]) -> Result<Vec<FileSnapshot>, String> {
    let mut budget = SnapshotBudget::default();
    let mut snapshots = Vec::with_capacity(paths.len().min(MAX_SNAPSHOT_FILES));
    for path in paths {
        let absolute_path = super::resolve_workspace_path(cwd, path)?;
        let before = budget.read(&absolute_path).map_err(|error| {
            format!(
                "failed to capture preimage for `{}`: {error}",
                absolute_path.display()
            )
        })?;
        snapshots.push(FileSnapshot {
            display_path: path.clone(),
            absolute_path,
            before,
            after_exists: None,
            after_sha256: None,
        });
    }
    Ok(snapshots)
}

pub(crate) fn read_regular_file_bounded(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to inspect `{}`: {error}", path.display())),
    };
    if path_metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to snapshot symbolic link `{}`",
            path.display()
        ));
    }
    if !path_metadata.is_file() {
        return Err(format!(
            "refusing to snapshot non-regular file `{}`",
            path.display()
        ));
    }
    ensure_file_size_within_limit(path, path_metadata.len(), max_bytes)?;

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to open `{}`: {error}", path.display())),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect open file `{}`: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "refusing to snapshot non-regular file `{}`",
            path.display()
        ));
    }
    ensure_file_size_within_limit(path, metadata.len(), max_bytes)?;

    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(max_bytes));
    file.take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "`{}` exceeds the {} MiB per-file snapshot safety limit",
            path.display(),
            max_bytes / (1024 * 1024)
        ));
    }
    Ok(Some(bytes))
}

pub(crate) fn fingerprint_path(path: &Path) -> Result<FileFingerprint, String> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(FileFingerprint::Missing);
        }
        Err(error) => return Err(format!("failed to inspect `{}`: {error}", path.display())),
    };
    if path_metadata.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(|error| {
            format!(
                "failed to inspect symbolic link `{}`: {error}",
                path.display()
            )
        })?;
        return Ok(FileFingerprint::Symlink { target });
    }
    if !path_metadata.is_file() {
        let kind = if path_metadata.is_dir() {
            "directory"
        } else {
            "non-regular file"
        };
        let modified_nanos = path_metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());
        return Ok(FileFingerprint::Other {
            kind,
            size: path_metadata.len(),
            modified_nanos,
        });
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(FileFingerprint::Missing);
        }
        Err(error) => return Err(format!("failed to open `{}`: {error}", path.display())),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect open file `{}`: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "refusing to fingerprint non-regular file `{}`",
            path.display()
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(FileFingerprint::Regular {
        size: metadata.len(),
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn ensure_file_size_within_limit(path: &Path, size: u64, max_bytes: usize) -> Result<(), String> {
    if size > max_bytes as u64 {
        return Err(format!(
            "`{}` exceeds the {} MiB per-file snapshot safety limit",
            path.display(),
            max_bytes / (1024 * 1024)
        ));
    }
    Ok(())
}

pub fn restore_snapshots(
    _cwd: &Path,
    snapshots: &[FileSnapshot],
    roots: &super::WorkspaceRoots,
) -> Result<(), String> {
    let mut validated = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let target = require_writable_path(roots, &snapshot.absolute_path).map_err(|error| {
            format!(
                "refusing to restore `{}` outside the workspace-write roots: {error}",
                snapshot.display_path
            )
        })?;
        verify_snapshot_postcondition(snapshot, &target)?;
        validated.push((snapshot, target));
    }

    for (snapshot, target) in validated {
        match &snapshot.before {
            Some(bytes) => {
                atomic_write_file(&target, bytes).map_err(|error| {
                    format!("failed to restore `{}`: {error}", snapshot.display_path)
                })?;
            }
            None => match fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "failed to remove added file `{}`: {error}",
                        snapshot.display_path
                    ));
                }
            },
        }
    }
    Ok(())
}

fn verify_snapshot_postcondition(snapshot: &FileSnapshot, target: &Path) -> Result<(), String> {
    let Some(expected_exists) = snapshot.after_exists else {
        return Err(format!(
            "refusing to revert `{}` because its postimage was not recorded",
            snapshot.display_path
        ));
    };
    let current = fingerprint_path(target).map_err(|error| {
        format!(
            "refusing to revert `{}` because its current state is unsafe: {error}",
            snapshot.display_path
        )
    })?;
    match (expected_exists, snapshot.after_sha256.as_deref(), current) {
        (false, None, FileFingerprint::Missing) => Ok(()),
        (true, Some(expected), FileFingerprint::Regular { sha256, .. }) if sha256 == expected => {
            Ok(())
        }
        (false, Some(_), _) | (true, None, _) => Err(format!(
            "refusing to revert `{}` because its stored postcondition is invalid",
            snapshot.display_path
        )),
        _ => Err(format!(
            "refusing to revert `{}` because it changed after the turn completed",
            snapshot.display_path
        )),
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn apply_parsed_patch(
    cwd: &Path,
    parsed: &ParsedPatch,
    roots: &super::WorkspaceRoots,
) -> Result<ApplyPatchSummary, String> {
    for operation in &parsed.operations {
        apply_operation(cwd, roots, operation)?;
    }
    let count = touched_paths(parsed).len();
    Ok(ApplyPatchSummary {
        files_changed: count,
        message: format!("Applied patch to {count} file(s)."),
    })
}

pub fn unified_diff(cwd: &Path, snapshots: &mut [FileSnapshot]) -> Result<String, String> {
    let mut budget = SnapshotBudget::default();
    let mut output = String::new();
    for snapshot in snapshots {
        let after = budget.read(&snapshot.absolute_path).map_err(|error| {
            format!(
                "failed to capture postimage for `{}`: {error}",
                snapshot.display_path
            )
        })?;
        snapshot.capture_postimage(after.as_deref());
        if snapshot.before == after {
            continue;
        }
        let file_diff = simple_file_diff(cwd, snapshot, after.as_deref(), after.is_some(), false);
        if output.len().saturating_add(file_diff.len()) > MAX_RENDERED_DIFF_BYTES {
            return Err(format!(
                "rendered diff exceeds the {} MiB safety limit",
                MAX_RENDERED_DIFF_BYTES / (1024 * 1024)
            ));
        }
        output.push_str(&file_diff);
    }
    Ok(output)
}

fn apply_operation(
    cwd: &Path,
    roots: &super::WorkspaceRoots,
    operation: &PatchOperation,
) -> Result<(), String> {
    match operation {
        PatchOperation::Add { path, lines } => {
            let target = super::resolve_workspace_path(cwd, path)?;
            let target = require_writable_path(roots, &target)?;
            if target.exists() {
                return Err(format!("cannot add `{path}` because it already exists"));
            }
            write_text_file(roots, &target, &join_patch_lines(lines))
                .map(|_| ())
                .map_err(|error| format!("failed to add `{path}`: {error}"))
        }
        PatchOperation::Delete { path } => {
            let target = super::resolve_workspace_path(cwd, path)?;
            let target = require_writable_path(roots, &target)?;
            fs::remove_file(&target).map_err(|error| format!("failed to delete `{path}`: {error}"))
        }
        PatchOperation::Update {
            path,
            move_to,
            hunks,
        } => {
            let source = super::resolve_workspace_path(cwd, path)?;
            let source = require_writable_path(roots, &source)?;
            let original = fs::read_to_string(&source)
                .map_err(|error| format!("failed to read `{path}`: {error}"))?;
            let (mut lines, final_newline) = split_text_lines(&original);
            let mut cursor = 0_usize;

            for hunk in hunks {
                let old = hunk_old_lines(hunk);
                let new = hunk_new_lines(hunk);
                let Some(position) = seek_sequence(&lines, &old, cursor) else {
                    return Err(format!("failed to match patch hunk in `{path}`"));
                };
                lines.splice(position..position + old.len(), new);
                cursor = position.saturating_add(hunk_new_lines(hunk).len());
            }

            let updated = join_text_lines(&lines, final_newline || !lines.is_empty());
            let target = move_to
                .as_deref()
                .map(|path| super::resolve_workspace_path(cwd, path))
                .transpose()?
                .unwrap_or_else(|| source.clone());
            let target = write_text_file(roots, &target, &updated)
                .map_err(|error| format!("failed to write `{}`: {error}", target.display()))?;
            if target != source {
                fs::remove_file(&source)
                    .map_err(|error| format!("failed to remove moved source `{path}`: {error}"))?;
            }
            Ok(())
        }
    }
}

pub fn seek_sequence(haystack: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(haystack.len()));
    }
    for strategy in [
        MatchStrategy::Exact,
        MatchStrategy::Whitespace,
        MatchStrategy::Unicode,
    ] {
        let upper = haystack.len().checked_sub(needle.len())?;
        for index in start.min(upper)..=upper {
            if sequence_matches(&haystack[index..index + needle.len()], needle, strategy) {
                return Some(index);
            }
        }
        for index in 0..start.min(haystack.len()) {
            if index + needle.len() <= haystack.len()
                && sequence_matches(&haystack[index..index + needle.len()], needle, strategy)
            {
                return Some(index);
            }
        }
    }
    None
}

fn sequence_matches(left: &[String], right: &[String], strategy: MatchStrategy) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| line_matches(left, right, strategy))
}

fn line_matches(left: &str, right: &str, strategy: MatchStrategy) -> bool {
    match strategy {
        MatchStrategy::Exact => left == right,
        MatchStrategy::Whitespace => whitespace_key(left) == whitespace_key(right),
        MatchStrategy::Unicode => unicode_key(left) == unicode_key(right),
    }
}

#[derive(Debug, Clone, Copy)]
enum MatchStrategy {
    Exact,
    Whitespace,
    Unicode,
}

fn whitespace_key(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unicode_key(line: &str) -> String {
    whitespace_key(
        &line
            .chars()
            .filter_map(fold_unicode_char)
            .collect::<String>(),
    )
}

fn fold_unicode_char(ch: char) -> Option<char> {
    if ('\u{0300}'..='\u{036f}').contains(&ch) {
        return None;
    }
    Some(match ch {
        '\u{00a0}' | '\u{2007}' | '\u{202f}' => ' ',
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' | 'Ā' | 'Ă' | 'Ą' => 'A',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => 'E',
        'í' | 'ì' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' | 'Ī' | 'Ĭ' | 'Į' => 'I',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ō' | 'ŏ' | 'ő' => 'o',
        'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' | 'Ō' | 'Ŏ' | 'Ő' => 'O',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => 'U',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => 'C',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' => 'N',
        _ => ch,
    })
}

fn hunk_old_lines(hunk: &PatchHunk) -> Vec<String> {
    hunk.lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(text) | HunkLine::Delete(text) => Some(text.clone()),
            HunkLine::Add(_) => None,
        })
        .collect()
}

fn hunk_new_lines(hunk: &PatchHunk) -> Vec<String> {
    hunk.lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(text) | HunkLine::Add(text) => Some(text.clone()),
            HunkLine::Delete(_) => None,
        })
        .collect()
}

fn require_writable_path(roots: &super::WorkspaceRoots, path: &Path) -> Result<PathBuf, String> {
    let resolved = roots.resolve_write_path(path)?;
    if resolved.is_protected {
        return Err(format!(
            "`{}` is under a protected top-level directory",
            resolved.absolute_path.display()
        ));
    }
    if !resolved.is_writable {
        return Err(format!(
            "`{}` resolves outside the workspace-write roots",
            resolved.absolute_path.display()
        ));
    }
    Ok(resolved.real_path)
}

fn write_text_file(
    roots: &super::WorkspaceRoots,
    path: &Path,
    text: &str,
) -> Result<PathBuf, String> {
    let target = require_writable_path(roots, path)?;
    atomic_write_file(&target, text.as_bytes())?;
    Ok(target)
}

pub(crate) fn atomic_write_file(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("`{}` has no parent directory", target.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create parent `{}`: {error}", parent.display()))?;

    let permissions = match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "refusing to replace symbolic link `{}`",
                target.display()
            ));
        }
        Ok(metadata) if metadata.is_file() => Some(metadata.permissions()),
        Ok(_) => {
            return Err(format!(
                "refusing to replace non-regular file `{}`",
                target.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!("failed to inspect `{}`: {error}", target.display()));
        }
    };

    let (temporary_path, mut temporary) = (0..8)
        .find_map(|_| {
            let path = parent.join(format!(".halv-write-{}.tmp", crate::storage::new_uuid_v4()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => Some(Ok((path, file))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(format!(
                    "failed to create temporary file in `{}`: {error}",
                    parent.display()
                ))),
            }
        })
        .transpose()?
        .ok_or_else(|| {
            format!(
                "failed to reserve a temporary file in `{}`",
                parent.display()
            )
        })?;

    let result = (|| -> Result<(), String> {
        temporary
            .write_all(bytes)
            .map_err(|error| format!("failed to write `{}`: {error}", target.display()))?;
        if let Some(permissions) = permissions {
            temporary.set_permissions(permissions).map_err(|error| {
                format!(
                    "failed to preserve permissions for `{}`: {error}",
                    target.display()
                )
            })?;
        }
        temporary
            .sync_all()
            .map_err(|error| format!("failed to sync `{}`: {error}", target.display()))?;
        drop(temporary);
        replace_file(&temporary_path, target)
            .map_err(|error| format!("failed to replace `{}`: {error}", target.display()))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn split_text_lines(text: &str) -> (Vec<String>, bool) {
    let final_newline = text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(text);
    if body.is_empty() {
        (Vec::new(), final_newline)
    } else {
        (
            body.split('\n').map(ToString::to_string).collect(),
            final_newline,
        )
    }
}

fn join_text_lines(lines: &[String], final_newline: bool) -> String {
    let mut text = lines.join("\n");
    if final_newline {
        text.push('\n');
    }
    text
}

fn join_patch_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

pub(crate) fn render_file_diff(
    cwd: &Path,
    snapshot: &FileSnapshot,
    after: Option<&[u8]>,
    after_exists: bool,
    force_omitted: bool,
) -> String {
    simple_file_diff(cwd, snapshot, after, after_exists, force_omitted)
}

fn simple_file_diff(
    cwd: &Path,
    snapshot: &FileSnapshot,
    after: Option<&[u8]>,
    after_exists: bool,
    force_omitted: bool,
) -> String {
    let path = snapshot
        .absolute_path
        .strip_prefix(cwd)
        .ok()
        .and_then(|path| path.to_str())
        .unwrap_or(&snapshot.display_path)
        .replace('\\', "/");
    let mut output = format!(
        "--- {}\n+++ {}\n",
        if snapshot.before.is_some() {
            format!("a/{path}")
        } else {
            "/dev/null".to_string()
        },
        if after_exists {
            format!("b/{path}")
        } else {
            "/dev/null".to_string()
        }
    );
    let inline_bytes = snapshot
        .before
        .as_ref()
        .map_or(0, Vec::len)
        .saturating_add(after.map_or(0, <[u8]>::len));
    let binary = snapshot
        .before
        .as_deref()
        .is_some_and(|bytes| std::str::from_utf8(bytes).is_err())
        || after.is_some_and(|bytes| std::str::from_utf8(bytes).is_err());
    if force_omitted || binary || inline_bytes > MAX_INLINE_DIFF_INPUT_BYTES {
        output.push_str("@@ -0,0 +0,0 @@\n+Halv omitted a binary or oversized file diff.\n");
        return output;
    }

    let before_text = snapshot
        .before
        .as_deref()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let after_text = after.map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let before_lines = before_text
        .as_deref()
        .map(|text| split_text_lines(text).0)
        .unwrap_or_default();
    let after_lines = after_text
        .as_deref()
        .map(|text| split_text_lines(text).0)
        .unwrap_or_default();
    output.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        before_lines.len(),
        after_lines.len()
    ));
    for line in before_lines {
        output.push('-');
        output.push_str(&line);
        output.push('\n');
    }
    for line in after_lines {
        output.push('+');
        output.push_str(&line);
        output.push('\n');
    }
    output
}

fn change(path: &str, change_type: PatchChangeType) -> PatchFileChange {
    PatchFileChange {
        path: path.to_string(),
        change_type,
        requires_approval: false,
        approved: false,
    }
}

fn split_hunk_line(line: &str) -> Option<(char, &str)> {
    let mut chars = line.chars();
    let prefix = chars.next()?;
    matches!(prefix, ' ' | '-' | '+').then_some((prefix, chars.as_str()))
}

fn is_operation_or_end(line: &str) -> bool {
    line == END_MARKER
        || line.starts_with(ADD_FILE)
        || line.starts_with(DELETE_FILE)
        || line.starts_with(UPDATE_FILE)
}

fn validate_patch_path(path: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("patch path cannot be empty".to_string());
    }
    if path.contains('\0') {
        return Err("patch path cannot contain NUL bytes".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::new_uuid_v4;

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-patch-{name}-{}", new_uuid_v4()))
    }

    #[test]
    fn tool_schema_grammar_uses_real_rule_separators() {
        let schema = apply_patch_tool_schema();
        let definition = schema["format"]["definition"]
            .as_str()
            .expect("apply_patch grammar definition should be a string");

        assert!(definition.matches('\n').count() >= 8);
        assert!(!definition.as_bytes().windows(3).any(|window| {
            window[0] == b'\\' && window[1] == b'n' && window[2].is_ascii_lowercase()
        }));
        assert!(
            definition.lines().any(|line| line == r"LF: /\n/"),
            "LF must remain a literal backslash-n regex terminal"
        );
    }

    #[test]
    fn tool_schema_grammar_defines_every_referenced_rule() {
        let schema = apply_patch_tool_schema();
        let definition = schema["format"]["definition"]
            .as_str()
            .expect("apply_patch grammar definition should be a string");

        for rule in [
            "begin_patch",
            "end_patch",
            "hunk",
            "add_hunk",
            "delete_hunk",
            "update_hunk",
            "move",
            "change",
            "add_line",
            "LF",
        ] {
            assert!(
                definition
                    .lines()
                    .any(|line| line.starts_with(&format!("{rule}:"))),
                "missing definition for grammar rule `{rule}`"
            );
        }
    }

    #[test]
    fn parses_and_applies_add_update_delete_and_move_round_trip() {
        let dir = test_dir("round-trip");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("existing.txt"), "alpha\nbeta\ngamma\n").unwrap();
        fs::write(dir.join("delete.txt"), "remove me\n").unwrap();
        let patch = r#"*** Begin Patch
*** Add File: added.txt
+new
+file
*** Update File: existing.txt
@@
 alpha
-beta
+BETA
 gamma
*** Update File: existing.txt
*** Move to: moved.txt
*** Delete File: delete.txt
*** End Patch
"#;
        let parsed = parse_patch(patch).unwrap();
        let mut snapshots = snapshot_paths(&dir, &touched_paths(&parsed)).unwrap();
        let roots = crate::exec::WorkspaceRoots::for_cwd(&dir);
        let summary = apply_parsed_patch(&dir, &parsed, &roots).unwrap();
        let diff = unified_diff(&dir, &mut snapshots).unwrap();

        assert_eq!(summary.files_changed, 4);
        assert_eq!(
            fs::read_to_string(dir.join("added.txt")).unwrap(),
            "new\nfile\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("moved.txt")).unwrap(),
            "alpha\nBETA\ngamma\n"
        );
        assert!(!dir.join("existing.txt").exists());
        assert!(!dir.join("delete.txt").exists());
        assert!(diff.contains("--- /dev/null\n+++ b/added.txt"));
        assert!(diff.contains("--- a/delete.txt\n+++ /dev/null"));

        restore_snapshots(&dir, &snapshots, &roots).unwrap();
        assert!(!dir.join("added.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.join("existing.txt")).unwrap(),
            "alpha\nbeta\ngamma\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("delete.txt")).unwrap(),
            "remove me\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_rejects_symlinks_and_special_files_without_reading_them() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};

        let dir = test_dir("unsafe-files");
        fs::create_dir_all(&dir).unwrap();
        let symlink = dir.join("zero-link");
        std::os::unix::fs::symlink("/dev/zero", &symlink).unwrap();
        let symlink_error = SnapshotBudget::default().read(&symlink).unwrap_err();
        assert!(symlink_error.contains("symbolic link"));

        let fifo = dir.join("pipe");
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        let fifo_error = SnapshotBudget::default().read(&fifo).unwrap_err();
        assert!(fifo_error.contains("non-regular"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_rejects_sparse_files_above_the_per_file_limit() {
        let dir = test_dir("sparse-limit");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large.bin");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_SNAPSHOT_FILE_BYTES as u64 + 1)
            .unwrap();

        let error = SnapshotBudget::default().read(&path).unwrap_err();

        assert!(error.contains("per-file snapshot safety limit"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_budget_enforces_an_aggregate_limit() {
        let dir = test_dir("aggregate-limit");
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        fs::write(&first, b"123456").unwrap();
        fs::write(&second, b"abcdef").unwrap();
        let mut budget = SnapshotBudget::with_limits(2, 10, 10);

        assert_eq!(budget.read(&first).unwrap().unwrap(), b"123456");
        let error = budget.read(&second).unwrap_err();

        assert!(error.contains("aggregate safety limit"));
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn patch_update_replaces_a_hardlink_without_mutating_the_external_inode() {
        use std::os::unix::fs::MetadataExt;

        let dir = test_dir("hardlink-update");
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let external = dir.parent().unwrap().join(format!(
            "halv-hardlink-external-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::write(&external, "before\n").unwrap();
        let workspace = dir.join("linked.txt");
        fs::hard_link(&external, &workspace).unwrap();
        let original_inode = fs::metadata(&external).unwrap().ino();
        let parsed = parse_patch(
            "*** Begin Patch\n*** Update File: linked.txt\n@@\n-before\n+after\n*** End Patch\n",
        )
        .unwrap();
        let roots = crate::exec::WorkspaceRoots::for_cwd(&dir);

        apply_parsed_patch(&dir, &parsed, &roots).unwrap();

        assert_eq!(fs::read_to_string(&external).unwrap(), "before\n");
        assert_eq!(fs::read_to_string(&workspace).unwrap(), "after\n");
        assert_eq!(fs::metadata(&external).unwrap().ino(), original_inode);
        assert_ne!(fs::metadata(&workspace).unwrap().ino(), original_inode);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_file(external);
    }

    #[cfg(unix)]
    #[test]
    fn revert_replaces_a_hardlink_and_preserves_later_external_content() {
        use std::os::unix::fs::MetadataExt;

        let dir = test_dir("hardlink-revert");
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let external = dir.parent().unwrap().join(format!(
            "halv-revert-external-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::write(&external, "turn content\n").unwrap();
        let workspace = dir.join("linked.txt");
        fs::hard_link(&external, &workspace).unwrap();
        let external_inode = fs::metadata(&external).unwrap().ino();
        let mut snapshot = FileSnapshot {
            display_path: "linked.txt".to_string(),
            absolute_path: workspace.clone(),
            before: Some(b"before turn\n".to_vec()),
            after_exists: None,
            after_sha256: None,
        };
        snapshot.capture_postimage(Some(b"turn content\n"));
        let roots = crate::exec::WorkspaceRoots::for_cwd(&dir);

        restore_snapshots(&dir, &[snapshot], &roots).unwrap();

        assert_eq!(fs::read_to_string(&external).unwrap(), "turn content\n");
        assert_eq!(fs::read_to_string(&workspace).unwrap(), "before turn\n");
        assert_eq!(fs::metadata(&external).unwrap().ino(), external_inode);
        assert_ne!(fs::metadata(&workspace).unwrap().ino(), external_inode);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_file(external);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_patch_write_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("permissions");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("script.sh");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
        let roots = crate::exec::WorkspaceRoots::for_cwd(&dir);
        let parsed = parse_patch(
            "*** Begin Patch\n*** Update File: script.sh\n@@\n-old\n+new\n*** End Patch\n",
        )
        .unwrap();

        apply_parsed_patch(&dir, &parsed, &roots).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o751
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fuzzy_matcher_tolerates_whitespace_and_unicode_normalization() {
        let lines = vec![
            "fn main() {".to_string(),
            "    let label = \"café\";".to_string(),
            "}".to_string(),
        ];
        let whitespace = vec!["let label = \"café\";".to_string()];
        assert_eq!(seek_sequence(&lines, &whitespace, 0), Some(1));

        let unicode = vec!["    let label = \"cafe\u{0301}\";".to_string()];
        assert_eq!(seek_sequence(&lines, &unicode, 0), Some(1));
    }
}
