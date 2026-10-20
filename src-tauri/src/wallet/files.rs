//! File helpers for managed account folders.
//!
//! Credential folders get owner-only permissions on unix. Links share the
//! user's CLI resources and history with each managed account folder.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Creates a directory and its parents, then restricts the leaf to its owner.
pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    restrict_dir(path)
}

#[cfg(unix)]
pub(super) fn restrict_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(super) fn restrict_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn restrict_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub(super) fn restrict_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Writes a file through an owner-only temporary file and a rename.
///
/// The function refuses a link at `target`, because a link can point out of
/// the account folder.
pub(super) fn write_private_file(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the file has no parent folder")
    })?;
    if fs::symlink_metadata(target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to replace a link",
        ));
    }
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let temporary = parent.join(format!(".{name}.halv-{:016x}.tmp", rand::random::<u64>()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, target)?;
        restrict_file(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkOutcome {
    Linked,
    AlreadyLinked,
    /// The link path holds the user's own file or folder. Halv keeps it.
    Kept,
    SourceMissing,
}

/// Links `link` to the folder `source` when `link` is free.
///
/// An empty real folder at `link` is replaced, because the CLI can create one
/// before Halv links it. A folder with content stays in place.
pub(super) fn link_directory(source: &Path, link: &Path) -> io::Result<LinkOutcome> {
    if !source.is_dir() {
        return Ok(LinkOutcome::SourceMissing);
    }
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(link_target_outcome(source, link)),
        Ok(metadata) if metadata.is_dir() => {
            if fs::read_dir(link)?.next().is_some() {
                return Ok(LinkOutcome::Kept);
            }
            fs::remove_dir(link)?;
            create_directory_link(source, link)?;
            Ok(LinkOutcome::Linked)
        }
        Ok(_) => Ok(LinkOutcome::Kept),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_directory_link(source, link)?;
            Ok(LinkOutcome::Linked)
        }
        Err(error) => Err(error),
    }
}

/// Links `link` to the file `source` when `link` is free.
///
/// Windows without symlink privilege gets a copy instead of a link.
pub(super) fn link_file(source: &Path, link: &Path) -> io::Result<LinkOutcome> {
    if !source.is_file() {
        return Ok(LinkOutcome::SourceMissing);
    }
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(link_target_outcome(source, link)),
        Ok(_) => Ok(LinkOutcome::Kept),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_file_link(source, link)?;
            Ok(LinkOutcome::Linked)
        }
        Err(error) => Err(error),
    }
}

fn link_target_outcome(source: &Path, link: &Path) -> LinkOutcome {
    match (fs::canonicalize(source), fs::canonicalize(link)) {
        (Ok(source), Ok(target)) if source == target => LinkOutcome::AlreadyLinked,
        _ => LinkOutcome::Kept,
    }
}

#[cfg(unix)]
fn create_directory_link(source: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

#[cfg(windows)]
fn create_directory_link(source: &Path, link: &Path) -> io::Result<()> {
    if std::os::windows::fs::symlink_dir(source, link).is_ok() {
        return Ok(());
    }
    // A junction needs no Windows privilege, unlike a directory symlink.
    let mut command = std::process::Command::new("cmd");
    command
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(source)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = crate::process::hide_console(&mut command).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("mklink /J failed"))
    }
}

#[cfg(unix)]
fn create_file_link(source: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

#[cfg(windows)]
fn create_file_link(source: &Path, link: &Path) -> io::Result<()> {
    if std::os::windows::fs::symlink_file(source, link).is_ok() {
        return Ok(());
    }
    fs::copy(source, link).map(|_| ())
}

/// Removes a link without following it.
fn remove_link(path: &Path) -> io::Result<()> {
    // A Windows directory link or junction is removed as a directory.
    fs::remove_file(path).or_else(|_| fs::remove_dir(path))
}

/// Removes every link below `directory` first, then removes the folder.
///
/// The CLI history folders are links into the user's own CLI folder. The
/// first pass makes sure the delete can never reach that history.
pub(super) fn remove_tree(directory: &Path) -> io::Result<()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => return remove_link(directory),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    unlink_links(directory)?;
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn unlink_links(directory: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            remove_link(&path)?;
        } else if metadata.is_dir() {
            unlink_links(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "halv-wallet-files-{name}-{:016x}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&path).expect("scratch folder");
        path
    }

    #[test]
    fn remove_tree_keeps_the_link_targets() {
        let root = scratch("remove");
        let shared = root.join("shared");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("history.jsonl"), "keep").unwrap();
        let account = root.join("account");
        let home = account.join("home");
        fs::create_dir_all(&home).unwrap();
        assert_eq!(
            link_directory(&shared, &home.join("projects")).unwrap(),
            LinkOutcome::Linked
        );
        fs::write(home.join("secret"), "drop").unwrap();

        remove_tree(&account).unwrap();

        assert!(!account.exists());
        assert_eq!(
            fs::read_to_string(shared.join("history.jsonl")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn link_directory_replaces_an_empty_folder_and_keeps_a_full_one() {
        let root = scratch("link");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        let full = root.join("full");
        fs::create_dir_all(&full).unwrap();
        fs::write(full.join("file"), "user data").unwrap();

        assert_eq!(
            link_directory(&source, &empty).unwrap(),
            LinkOutcome::Linked
        );
        assert_eq!(
            link_directory(&source, &empty).unwrap(),
            LinkOutcome::AlreadyLinked
        );
        assert_eq!(link_directory(&source, &full).unwrap(), LinkOutcome::Kept);
        assert_eq!(
            link_directory(&root.join("missing"), &root.join("other")).unwrap(),
            LinkOutcome::SourceMissing
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_files_and_folders_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("private");
        let folder = root.join("account");
        create_private_dir(&folder).unwrap();
        let file = folder.join("registry.json");
        write_private_file(&file, b"{}").unwrap();

        let folder_mode = fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(folder_mode, 0o700);
        assert_eq!(file_mode, 0o600);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_private_file_refuses_a_link() {
        let root = scratch("refuse");
        let outside = root.join("outside");
        fs::write(&outside, "outside").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        assert!(write_private_file(&link, b"new").is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
        fs::remove_dir_all(root).unwrap();
    }
}
