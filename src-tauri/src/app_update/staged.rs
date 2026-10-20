//! The staged update archive. The download writes it, and the install reads
//! it back. The plugin checks the release signature during the download, so
//! the install only compares the SHA-256 digest that the download kept.

use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const UPDATES_DIRECTORY: &str = "updates";
const FILE_PREFIX: &str = "halv-";
const ARCHIVE_SUFFIX: &str = ".archive";
const PARTIAL_SUFFIX: &str = ".archive.partial";
const MAX_VERSION_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagedUpdate {
    pub version: String,
    pub path: PathBuf,
    pub sha256: [u8; 32],
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StagedReadError {
    /// The file is missing or unreadable.
    Unreadable(String),
    /// The file changed after the download.
    Damaged,
}

pub(crate) fn updates_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(UPDATES_DIRECTORY)
}

/// Writes the archive with mode 0600 and returns its digest.
pub(crate) fn write(dir: &Path, version: &str, bytes: &[u8]) -> io::Result<StagedUpdate> {
    validate_version(version)?;
    create_private_dir(dir)?;
    let path = dir.join(format!("{FILE_PREFIX}{version}{ARCHIVE_SUFFIX}"));
    let partial = dir.join(format!("{FILE_PREFIX}{version}{PARTIAL_SUFFIX}"));
    let result = (|| {
        remove_if_present(&partial)?;
        let mut file = create_private_file(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&partial, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result?;
    Ok(StagedUpdate {
        version: version.to_string(),
        path,
        sha256: digest(bytes),
        size: bytes.len() as u64,
    })
}

/// Reads the archive and compares it with the digest from the download.
pub(crate) fn read_verified(staged: &StagedUpdate) -> Result<Vec<u8>, StagedReadError> {
    let bytes = fs::read(&staged.path).map_err(|error| {
        StagedReadError::Unreadable(format!("failed to read the downloaded update: {error}"))
    })?;
    if bytes.len() as u64 != staged.size || digest(&bytes) != staged.sha256 {
        return Err(StagedReadError::Damaged);
    }
    Ok(bytes)
}

pub(crate) fn remove(staged: &StagedUpdate) {
    if let Err(error) = remove_if_present(&staged.path) {
        eprintln!(
            "failed to remove the staged update `{}`: {error}",
            staged.path.display()
        );
    }
}

/// Removes the archives of earlier runs. A staged archive is valid only for
/// the process that downloaded it, because only that process holds its digest.
pub(crate) fn remove_leftovers(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let staged_name = name.starts_with(FILE_PREFIX)
            && (name.ends_with(ARCHIVE_SUFFIX) || name.ends_with(PARTIAL_SUFFIX));
        if staged_name && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

// The version becomes part of a file name, so it must not carry a path.
fn validate_version(version: &str) -> io::Result<()> {
    let valid = !version.is_empty()
        && version.len() <= MAX_VERSION_LENGTH
        && !version.starts_with('.')
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the update version `{version}` is not valid"),
        ))
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-staged-update-{name}-{}",
                rand::random::<u64>()
            ));
            fs::create_dir_all(&path).expect("create the staged update test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn staged_archive_round_trips_with_its_digest() {
        let directory = TestDirectory::new("round-trip");
        let dir = updates_dir(&directory.0);
        let staged = write(&dir, "0.7.2", b"signed archive").expect("write the archive");

        assert_eq!(staged.version, "0.7.2");
        assert_eq!(staged.path, dir.join("halv-0.7.2.archive"));
        assert_eq!(staged.size, 14);
        assert_eq!(
            read_verified(&staged).expect("read the archive"),
            b"signed archive"
        );
        assert!(!dir.join("halv-0.7.2.archive.partial").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staged_archive_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new("private");
        let dir = updates_dir(&directory.0);
        let staged = write(&dir, "0.7.2", b"signed archive").expect("write the archive");

        let file_mode = fs::metadata(&staged.path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn a_changed_archive_is_refused() {
        let directory = TestDirectory::new("changed");
        let dir = updates_dir(&directory.0);
        let staged = write(&dir, "0.7.2", b"signed archive").expect("write the archive");

        fs::write(&staged.path, b"signed archivE").expect("change the archive");
        assert_eq!(read_verified(&staged), Err(StagedReadError::Damaged));

        fs::write(&staged.path, b"signed archive and more").expect("grow the archive");
        assert_eq!(read_verified(&staged), Err(StagedReadError::Damaged));

        fs::remove_file(&staged.path).expect("remove the archive");
        assert!(matches!(
            read_verified(&staged),
            Err(StagedReadError::Unreadable(_))
        ));
    }

    #[test]
    fn leftovers_from_earlier_runs_are_removed() {
        let directory = TestDirectory::new("leftovers");
        let dir = updates_dir(&directory.0);
        write(&dir, "0.7.1", b"old").expect("write the old archive");
        fs::write(dir.join("halv-0.7.2.archive.partial"), b"partial").unwrap();
        fs::write(dir.join("notes.txt"), b"keep").unwrap();

        assert_eq!(remove_leftovers(&dir).expect("remove leftovers"), 2);
        assert!(!dir.join("halv-0.7.1.archive").exists());
        assert!(!dir.join("halv-0.7.2.archive.partial").exists());
        assert!(dir.join("notes.txt").exists());
        assert_eq!(
            remove_leftovers(&directory.0.join("missing")).expect("a missing directory"),
            0
        );
    }

    #[test]
    fn a_version_cannot_leave_the_updates_directory() {
        let directory = TestDirectory::new("version");
        let dir = updates_dir(&directory.0);
        for version in ["", "../0.7.2", "0.7.2/evil", ".hidden", "0.7.2\0"] {
            assert!(
                write(&dir, version, b"archive").is_err(),
                "`{version}` must be refused"
            );
        }
        assert!(write(&dir, "0.8.0-beta.1+build.7", b"archive").is_ok());
    }
}
