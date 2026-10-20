use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

pub const APPIMAGE_RUNTIME_VARS: &[&str] = &[
    "APPDIR",
    "APPIMAGE",
    "OWD",
    "ARGV0",
    "LD_LIBRARY_PATH",
    "LD_LIBRARY_PATH_ORIG",
    "LD_PRELOAD",
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONDONTWRITEBYTECODE",
    "GIO_MODULE_DIR",
    "GIO_EXTRA_MODULES",
    "GIO_LAUNCHED_DESKTOP_FILE_PID",
    "GDK_PIXBUF_MODULE_FILE",
    "GDK_PIXBUF_MODULEDIR",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_SCANNER",
    "GST_REGISTRY",
    "GSETTINGS_SCHEMA_DIR",
    "GTK_PATH",
    "GTK_EXE_PREFIX",
    "GTK_DATA_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GI_TYPELIB_PATH",
    "QT_PLUGIN_PATH",
    "PERLLIB",
];

const FALLBACK_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

pub fn running_from_appimage() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn scrubbed_path(path: &OsStr, appdir: &Path) -> OsString {
    let entries = std::env::split_paths(path)
        .filter(|entry| !entry.starts_with(appdir))
        .collect::<Vec<_>>();
    joined_paths(entries).unwrap_or_else(|| OsString::from(FALLBACK_PATH))
}

/// Returns the absolute entries of a PATH value. A child process resolves a
/// relative or empty entry from its working directory.
pub fn absolute_path_entries(path: &OsStr) -> OsString {
    let entries = std::env::split_paths(path)
        .filter(|entry| entry.is_absolute())
        .collect::<Vec<_>>();
    joined_paths(entries).unwrap_or_else(|| OsString::from(FALLBACK_PATH))
}

pub fn scrubbed_var(name: &str, value: &OsStr, appdir: &Path) -> Option<OsString> {
    if name != "XDG_DATA_DIRS" {
        return Some(value.to_os_string());
    }
    let entries = std::env::split_paths(value)
        .filter(|entry| !entry.starts_with(appdir))
        .collect::<Vec<_>>();
    joined_paths(entries)
}

pub fn apply_to_command(command: &mut Command) {
    #[cfg(target_os = "linux")]
    {
        if !running_from_appimage() {
            return;
        }
        let path = command_environment_value(command, "PATH")
            .unwrap_or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let xdg_data_dirs = command_environment_value(command, "XDG_DATA_DIRS")
            .unwrap_or_else(|| std::env::var_os("XDG_DATA_DIRS"));
        for name in APPIMAGE_RUNTIME_VARS {
            command.env_remove(name);
        }
        command.env("PATH", scrubbed_appimage_path(&path));
        match xdg_data_dirs
            .as_deref()
            .and_then(|value| scrubbed_appimage_var("XDG_DATA_DIRS", value))
        {
            Some(value) => {
                command.env("XDG_DATA_DIRS", value);
            }
            None => {
                command.env_remove("XDG_DATA_DIRS");
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = command;
}

pub fn user_home_cwd() -> PathBuf {
    let owd = std::env::var_os("OWD");
    let owd_is_directory = owd.as_deref().is_some_and(|path| Path::new(path).is_dir());
    select_user_home_cwd(
        owd.as_deref(),
        owd_is_directory,
        std::env::var_os("HOME").as_deref(),
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn scrubbed_appimage_path(path: &OsStr) -> OsString {
    match std::env::var_os("APPDIR") {
        Some(appdir) => scrubbed_path(path, Path::new(&appdir)),
        None if path.is_empty() => OsString::from(FALLBACK_PATH),
        None => path.to_os_string(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn scrubbed_appimage_var(name: &str, value: &OsStr) -> Option<OsString> {
    match std::env::var_os("APPDIR") {
        Some(appdir) => scrubbed_var(name, value, Path::new(&appdir)),
        None if value.is_empty() => None,
        None => Some(value.to_os_string()),
    }
}

fn joined_paths(paths: Vec<PathBuf>) -> Option<OsString> {
    let value = std::env::join_paths(paths).ok()?;
    (!value.is_empty()).then_some(value)
}

#[cfg(target_os = "linux")]
fn command_environment_value(command: &Command, name: &str) -> Option<Option<OsString>> {
    command.get_envs().find_map(|(candidate, value)| {
        (candidate == OsStr::new(name)).then(|| value.map(OsStr::to_os_string))
    })
}

fn select_user_home_cwd(
    owd: Option<&OsStr>,
    owd_is_directory: bool,
    home: Option<&OsStr>,
) -> PathBuf {
    if owd_is_directory {
        if let Some(owd) = owd {
            return PathBuf::from(owd);
        }
    }
    home.map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_value(entries: &[&str]) -> OsString {
        std::env::join_paths(entries.iter().map(PathBuf::from)).expect("paths should join")
    }

    fn path_entries(value: &OsStr) -> Vec<PathBuf> {
        std::env::split_paths(value).collect()
    }

    #[test]
    fn appimage_runtime_vars_cover_the_real_environment_dump() {
        let dump_vars = [
            "APPDIR",
            "APPIMAGE",
            "GIO_EXTRA_MODULES",
            "GIO_LAUNCHED_DESKTOP_FILE_PID",
            "GST_PLUGIN_SYSTEM_PATH",
            "GST_PLUGIN_SYSTEM_PATH_1_0",
            "LD_LIBRARY_PATH",
            "LD_LIBRARY_PATH_ORIG",
            "OWD",
            "PYTHONDONTWRITEBYTECODE",
            "PYTHONHOME",
            "PYTHONPATH",
        ];

        for name in dump_vars {
            assert!(
                APPIMAGE_RUNTIME_VARS.contains(&name),
                "missing AppImage runtime variable: {name}"
            );
        }
    }

    #[test]
    fn scrubbed_path_removes_appimage_entries_and_preserves_other_entries() {
        let appdir = Path::new("/tmp/.mount_Halv");
        let path = path_value(&[
            "/tmp/.mount_Halv/usr/bin",
            "/usr/local/bin",
            "/usr/local/bin",
            "/tmp/.mount_Halv/usr/libexec",
            "/usr/bin",
        ]);

        assert_eq!(
            path_entries(&scrubbed_path(&path, appdir)),
            [
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/bin"),
            ]
        );
    }

    #[test]
    fn scrubbed_path_uses_fallback_when_all_entries_are_removed() {
        let appdir = Path::new("/tmp/.mount_Halv");
        let path = path_value(&["/tmp/.mount_Halv/usr/bin", "/tmp/.mount_Halv/usr/sbin"]);

        assert_eq!(scrubbed_path(&path, appdir), OsString::from(FALLBACK_PATH));
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_entries_drop_relative_and_empty_entries() {
        let path = OsString::from("/opt/bin:node_modules/.bin::./bin:/usr/bin:");

        assert_eq!(
            path_entries(&absolute_path_entries(&path)),
            [PathBuf::from("/opt/bin"), PathBuf::from("/usr/bin")]
        );
        assert_eq!(
            absolute_path_entries(OsStr::new("bin:.")),
            OsString::from(FALLBACK_PATH)
        );
    }

    #[test]
    fn scrubbed_var_removes_appimage_data_directories() {
        let appdir = Path::new("/tmp/.mount_Halv");
        let value = path_value(&[
            "/tmp/.mount_Halv/usr/share",
            "/usr/local/share",
            "/usr/share",
        ]);

        assert_eq!(
            scrubbed_var("XDG_DATA_DIRS", &value, appdir)
                .as_deref()
                .map(path_entries),
            Some(vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ])
        );
    }

    #[test]
    fn scrubbed_var_drops_an_empty_data_directory_list() {
        let appdir = Path::new("/tmp/.mount_Halv");
        let value = path_value(&["/tmp/.mount_Halv/usr/share"]);

        assert_eq!(scrubbed_var("XDG_DATA_DIRS", &value, appdir), None);
    }

    #[test]
    fn user_home_cwd_prefers_a_valid_original_directory() {
        assert_eq!(
            select_user_home_cwd(
                Some(OsStr::new("/work/project")),
                true,
                Some(OsStr::new("/home/user")),
            ),
            PathBuf::from("/work/project")
        );
    }

    #[test]
    fn user_home_cwd_falls_back_to_home_and_root() {
        assert_eq!(
            select_user_home_cwd(
                Some(OsStr::new("/missing")),
                false,
                Some(OsStr::new("/home/user")),
            ),
            PathBuf::from("/home/user")
        );
        assert_eq!(select_user_home_cwd(None, false, None), PathBuf::from("/"));
    }
}
