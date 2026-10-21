pub mod navigation_guard;

use std::path::{Path, PathBuf};

use url::Url;

use crate::terminal_links::{is_remote_path, normalize_lexically};

#[tauri::command]
pub fn app_open_external_url(url: String) -> Result<(), String> {
    let url = normalize_external_url(&url)?;
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|error| format!("Could not open link: {error}"))
}

/// Opens a file or a directory that exists. A directory opens in the file
/// manager. A file opens in the default app of the system, so an HTML file
/// opens in the browser. A launchable file or bundle never opens: the file
/// manager shows it instead.
#[tauri::command]
pub async fn app_open_path(path: String) -> Result<(), String> {
    let path = existing_absolute_path(&path)?;
    let metadata =
        std::fs::metadata(&path).map_err(|error| format!("Could not open the path: {error}"))?;
    if is_launchable(&path, &metadata) {
        return reveal(&path);
    }
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|error| format!("Could not open the path: {error}"))
}

/// Shows a file or a directory that exists in the file manager.
#[tauri::command]
pub async fn app_reveal_path(path: String) -> Result<(), String> {
    let path = existing_absolute_path(&path)?;
    reveal(&path)
}

fn reveal(path: &Path) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path)
        .map_err(|error| format!("Could not show the path: {error}"))
}

/// The longest path that the open and reveal commands accept.
const MAX_PATH_CHARS: usize = 4096;

fn existing_absolute_path(raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.chars().count() > MAX_PATH_CHARS || raw.chars().any(char::is_control) {
        return Err("Invalid path".to_string());
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err("Only absolute paths can be opened".to_string());
    }
    if is_remote_path(path) {
        return Err("Network paths cannot be opened".to_string());
    }
    let path = normalize_lexically(path).ok_or_else(|| "Invalid path".to_string())?;
    if is_remote_path(&path) {
        return Err("Network paths cannot be opened".to_string());
    }
    if !path.exists() {
        return Err("The path does not exist".to_string());
    }
    Ok(path)
}

/// The operating system families that have their own launchable types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// File extensions that can run code on every platform. The list has the
/// scripts and programs of all platforms, because a runtime for a foreign
/// type can be present: Wine, WSL, Java, Python or PowerShell. The list
/// compares in lower case.
const SHARED_FILE_EXTENSIONS: &[&str] = &[
    // Shell scripts
    "sh", "bash", "zsh", "csh", "ksh", "fish", // Python, Java and PowerShell
    "py", "pyw", "pyc", "pyz", "pyzw", "jar", "class", "jnlp", "ps1", "psm1",
    // Windows programs, installers and shortcuts
    "exe", "msi", "msp", "bat", "cmd", "com", "scr", "pif", "lnk", // Linux
    "appimage", "run", "desktop",
];

/// File extensions that the macOS system opens with code, an installer, a
/// disk image mount or a network connection.
const MACOS_FILE_EXTENSIONS: &[&str] = &[
    "app",
    "command",
    "tool",
    "workflow",
    "scpt",
    "scptd",
    "applescript",
    "pkg",
    "mpkg",
    "dmg",
    "dmgpart",
    "udif",
    "iso",
    "img",
    "cdr",
    "sparseimage",
    "toast",
    "terminal",
    "webloc",
    "inetloc",
    "fileloc",
    "ftploc",
    "afploc",
    "atloc",
    "vncloc",
    "shortcut",
    "mobileconfig",
    "configprofile",
    "provisionprofile",
    "ipa",
    "prefpane",
    "tcl",
];

/// File extensions that only Windows opens with code or a network
/// connection. On other platforms these types are plain files. For example,
/// a ".js" file from a Node stack trace opens in an editor.
const WINDOWS_FILE_EXTENSIONS: &[&str] = &[
    // Scripts and shell commands
    "msc",
    "cpl",
    "vbs",
    "vbe",
    "vb",
    "js",
    "jse",
    "wsf",
    "wsh",
    "ws",
    "wsc",
    "sct",
    "hta",
    "reg",
    "inf",
    "psc1",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msh1xml",
    "msh2xml",
    // Shell links, search connectors and network connections
    "scf",
    "url",
    "appref-ms",
    "application",
    "settingcontent-ms",
    "library-ms",
    "searchconnector-ms",
    "website",
    "rdp",
    "theme",
    "themepack",
    "deskthemepack",
    "shs",
    "shb",
    // Help files, installers, packages and add-ins
    "chm",
    "appinstaller",
    "msix",
    "appx",
    "msixbundle",
    "appxbundle",
    "msu",
    "xll",
    "diagcab",
    "gadget",
    "xbap",
];

/// macOS bundle extensions. The system launches or installs such a
/// directory when it opens it. Other platforms have no launchable directory
/// type.
const MACOS_DIRECTORY_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "prefpane",
    "saver",
    "qlgenerator",
    "mdimporter",
    "kext",
    "dext",
    "systemextension",
    "xpc",
    "bundle",
    "plugin",
    "framework",
    "workflow",
    "action",
    "service",
    "pkg",
    "mpkg",
    "scptd",
    "sparsebundle",
];

/// True when the system can run code to open the path. The check reads the
/// extension of the path and of its symlink target, the executable bits of a
/// file and, on macOS, the structure and the bindings of the path.
fn is_launchable(path: &Path, metadata: &std::fs::Metadata) -> bool {
    is_launchable_on(Platform::current(), path, metadata)
}

fn is_launchable_on(platform: Platform, path: &Path, metadata: &std::fs::Metadata) -> bool {
    let target = std::fs::canonicalize(path).ok();
    let has_launchable_extension = std::iter::once(path)
        .chain(target.as_deref())
        .any(|candidate| has_launchable_extension(platform, candidate, metadata.is_dir()));
    has_launchable_extension
        || (metadata.is_file() && is_executable(metadata))
        || (platform == Platform::MacOs && has_macos_launch_binding(path, metadata))
}

fn has_launchable_extension(platform: Platform, path: &Path, is_dir: bool) -> bool {
    let lists: &[&[&str]] = match (platform, is_dir) {
        (Platform::MacOs, true) => &[MACOS_DIRECTORY_EXTENSIONS],
        (_, true) => &[],
        (Platform::MacOs, false) => &[SHARED_FILE_EXTENSIONS, MACOS_FILE_EXTENSIONS],
        (Platform::Windows, false) => &[SHARED_FILE_EXTENSIONS, WINDOWS_FILE_EXTENSIONS],
        (Platform::Linux, false) => &[SHARED_FILE_EXTENSIONS],
    };
    lists.iter().any(|list| has_extension(path, list))
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// True when LaunchServices can open a macOS path with code that the name of
/// the path does not show:
/// - A directory with "Contents/Info.plist" is a bundle. LaunchServices
///   treats some bundle types as an application, whatever the extension is.
/// - A Finder alias opens its target. A bookmark file keeps the target in
///   the data of the file.
/// - The "OpenWith" attribute of a file chooses the application for that
///   file only. An archive can carry the attribute.
fn has_macos_launch_binding(path: &Path, metadata: &std::fs::Metadata) -> bool {
    if metadata.is_dir() {
        return is_bundle_directory(path) || macos::has_open_with_binding(path);
    }
    // The checks below read file data, so they accept regular files only. A
    // FIFO or a device could block the read.
    metadata.is_file() && (macos::has_open_with_binding(path) || macos::is_alias_file(path))
}

fn is_bundle_directory(path: &Path) -> bool {
    path.join("Contents").join("Info.plist").is_file()
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CString;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const OPEN_WITH: &str = "com.apple.LaunchServices.OpenWith";
    const FINDER_INFO: &str = "com.apple.FinderInfo";
    /// The size of the FinderInfo attribute.
    const FINDER_INFO_SIZE: usize = 32;
    /// The Finder flag of an alias file. It is in bytes 8 and 9.
    const FINDER_FLAG_IS_ALIAS: u16 = 0x8000;
    /// A bookmark file starts with "book" and has "mark" at offset 8.
    const BOOKMARK_MAGIC: &[u8; 4] = b"book";
    const BOOKMARK_MARK: &[u8; 4] = b"mark";

    /// Reads an extended attribute of the path, or of the symlink itself.
    /// Returns the size of the value, or None when the attribute is absent.
    fn read_attribute(
        path: &Path,
        name: &str,
        follow_symlink: bool,
        buffer: &mut [u8],
    ) -> Option<usize> {
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let name = CString::new(name).ok()?;
        let options = if follow_symlink {
            0
        } else {
            libc::XATTR_NOFOLLOW
        };
        // SAFETY: path and name are NUL terminated. The pointer and the size
        // of the buffer match, and a null pointer with size 0 is allowed.
        let size = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                if buffer.is_empty() {
                    std::ptr::null_mut()
                } else {
                    buffer.as_mut_ptr().cast()
                },
                buffer.len(),
                0,
                options,
            )
        };
        usize::try_from(size).ok()
    }

    pub fn has_open_with_binding(path: &Path) -> bool {
        [true, false]
            .into_iter()
            .any(|follow| read_attribute(path, OPEN_WITH, follow, &mut []).is_some())
    }

    pub fn is_alias_file(path: &Path) -> bool {
        let mut info = [0u8; FINDER_INFO_SIZE];
        let has_alias_flag = [true, false].into_iter().any(|follow| {
            read_attribute(path, FINDER_INFO, follow, &mut info).is_some_and(|size| {
                size >= 10
                    && (&info[..4] == b"alis"
                        || u16::from_be_bytes([info[8], info[9]]) & FINDER_FLAG_IS_ALIAS != 0)
            })
        });
        has_alias_flag || starts_with_bookmark_magic(path)
    }

    fn starts_with_bookmark_magic(path: &Path) -> bool {
        let mut head = [0u8; 12];
        std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut head))
            .is_ok()
            && &head[..4] == BOOKMARK_MAGIC
            && &head[8..] == BOOKMARK_MARK
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use std::path::Path;

    pub fn has_open_with_binding(_path: &Path) -> bool {
        false
    }

    pub fn is_alias_file(_path: &Path) -> bool {
        false
    }
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn normalize_external_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let parsed = Url::parse(trimmed).map_err(|error| format!("Invalid link: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Only http:// and https:// links can be opened".to_string());
    }
    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "halv-open-path-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn launchable(path: &Path) -> bool {
        is_launchable(path, &fs::metadata(path).expect("metadata"))
    }

    #[test]
    fn open_path_accepts_only_existing_absolute_paths() {
        let root = temp_root("accept");
        let file = root.join("page.html");
        fs::write(&file, "<p>ok</p>").expect("write file");
        let dotted = root.join("x").join("..").join("page.html");
        fs::create_dir_all(root.join("x")).expect("create dir");

        assert_eq!(
            existing_absolute_path(&file.to_string_lossy()),
            Ok(file.clone())
        );
        assert_eq!(
            existing_absolute_path(&dotted.to_string_lossy()),
            Ok(file.clone())
        );
        assert!(existing_absolute_path("page.html").is_err());
        assert!(existing_absolute_path("").is_err());
        assert!(existing_absolute_path(&root.join("missing").to_string_lossy()).is_err());
        assert!(existing_absolute_path(&format!("{}\n", file.display())).is_err());
        #[cfg(not(windows))]
        assert!(existing_absolute_path("/net/example.com/share").is_err());
        #[cfg(windows)]
        assert!(existing_absolute_path(r"\\example.com\share\x").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn launchable_files_and_bundles_are_revealed() {
        let root = temp_root("launchable");
        for name in [
            "run.command",
            "SETUP.EXE",
            "tool.AppImage",
            "page.html",
            "notes.txt",
        ] {
            fs::write(root.join(name), "x").expect("write file");
        }
        fs::create_dir_all(root.join("Tool.app")).expect("create bundle");
        fs::create_dir_all(root.join("docs")).expect("create dir");

        // The extension lists differ per platform, so the test names the
        // platform. The CI runs on Linux, macOS and Windows.
        assert!(launchable_on(Platform::MacOs, &root.join("run.command")));
        assert!(!launchable_on(Platform::Linux, &root.join("run.command")));
        assert!(launchable_on(Platform::Windows, &root.join("SETUP.EXE")));
        assert!(launchable_on(Platform::MacOs, &root.join("SETUP.EXE")));
        assert!(launchable(&root.join("tool.AppImage")));
        assert!(launchable_on(Platform::MacOs, &root.join("Tool.app")));
        assert!(!launchable_on(Platform::Linux, &root.join("Tool.app")));
        assert!(!launchable(&root.join("page.html")));
        assert!(!launchable(&root.join("notes.txt")));
        assert!(!launchable(&root.join("docs")));

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let script = root.join("build");
            fs::write(&script, "#!/bin/sh").expect("write file");
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
            assert!(launchable(&script));

            // A harmless name does not hide a launchable symlink target.
            // ".sh" is in the shared list, so the check holds on every platform.
            let target = root.join("payload.sh");
            fs::write(&target, "x").expect("write file");
            let link = root.join("readme.html");
            symlink(&target, &link).expect("symlink");
            assert!(launchable(&link));
        }
        let _ = fs::remove_dir_all(&root);
    }

    fn launchable_on(platform: Platform, path: &Path) -> bool {
        is_launchable_on(platform, path, &fs::metadata(path).expect("metadata"))
    }

    fn write_files(root: &Path, names: &[&str]) {
        for name in names {
            fs::write(root.join(name), "x").expect("write file");
        }
    }

    #[test]
    fn windows_types_that_run_code_or_connect_are_revealed_on_windows() {
        let root = temp_root("windows-types");
        let names = [
            "help.chm",
            "Help.CHM",
            "tool.pyz",
            "tool.pyzw",
            "view.library-ms",
            "share.searchConnector-ms",
            "look.theme",
            "look.themepack",
            "look.deskthemepack",
            "host.rdp",
            "app.appinstaller",
            "app.msix",
            "app.appx",
            "app.msixbundle",
            "app.appxbundle",
            "addin.xll",
            "fix.diagcab",
            "site.website",
            "run.jnlp",
            "run.wsc",
            "run.sct",
            "page.html",
            "notes.txt",
        ];
        write_files(&root, &names);
        for name in &names[..names.len() - 2] {
            assert!(
                launchable_on(Platform::Windows, &root.join(name)),
                "{name} must be revealed on Windows"
            );
        }
        assert!(!launchable_on(Platform::Windows, &root.join("page.html")));
        assert!(!launchable_on(Platform::Windows, &root.join("notes.txt")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn windows_only_types_open_on_other_platforms() {
        let root = temp_root("platform-gate");
        let windows_only = [
            "vite.config.js",
            "run.vbs",
            "setup.reg",
            "help.chm",
            "view.library-ms",
            "host.rdp",
            "app.msix",
        ];
        write_files(&root, &windows_only);
        for name in windows_only {
            assert!(
                launchable_on(Platform::Windows, &root.join(name)),
                "{name} must be revealed on Windows"
            );
            assert!(
                !launchable_on(Platform::MacOs, &root.join(name)),
                "{name} must open on macOS"
            );
            assert!(
                !launchable_on(Platform::Linux, &root.join(name)),
                "{name} must open on Linux"
            );
        }
        // The shared list holds for every platform.
        write_files(&root, &["run.sh", "tool.exe", "app.jar"]);
        for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            for name in ["run.sh", "tool.exe", "app.jar"] {
                assert!(launchable_on(platform, &root.join(name)));
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn macos_types_share_the_handler_of_a_listed_type() {
        let root = temp_root("macos-types");
        let names = [
            "Probe.class",
            "run.jnlp",
            "site.ftploc",
            "site.afploc",
            "site.atloc",
            "host.vncloc",
            "disk.iso",
            "disk.img",
            "disk.cdr",
            "disk.sparseimage",
            "disk.toast",
            "disk.dmgpart",
            "disk.udif",
            "profile.configprofile",
            "profile.provisionprofile",
            "app.ipa",
            "script.tcl",
            "page.html",
        ];
        write_files(&root, &names);
        for name in &names[..names.len() - 1] {
            assert!(
                launchable_on(Platform::MacOs, &root.join(name)),
                "{name} must be revealed on macOS"
            );
        }
        assert!(!launchable_on(Platform::MacOs, &root.join("page.html")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn macos_application_directories_are_revealed() {
        let root = temp_root("macos-dirs");
        for name in ["Notes.service", "Disk.sparsebundle", "Tool.APP", "docs"] {
            fs::create_dir_all(root.join(name)).expect("create dir");
        }
        for name in ["Notes.service", "Disk.sparsebundle", "Tool.APP"] {
            assert!(launchable_on(Platform::MacOs, &root.join(name)), "{name}");
        }
        assert!(!launchable_on(Platform::MacOs, &root.join("docs")));
        // No other platform has a launchable directory type.
        assert!(!launchable_on(Platform::Windows, &root.join("Tool.APP")));
        assert!(!launchable_on(Platform::Linux, &root.join("Tool.APP")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn macos_bundle_structure_is_revealed_whatever_the_name_is() {
        let root = temp_root("macos-bundle");
        let bundle = root.join("tools").join("Helper.widget");
        fs::create_dir_all(bundle.join("Contents").join("MacOS")).expect("create bundle");
        fs::write(bundle.join("Contents").join("Info.plist"), "<plist/>").expect("write plist");
        fs::create_dir_all(root.join("plain").join("Contents")).expect("create dir");

        assert!(launchable_on(Platform::MacOs, &bundle));
        // A "Contents" directory without an Info.plist is a plain directory.
        assert!(!launchable_on(Platform::MacOs, &root.join("plain")));
        assert!(!launchable_on(Platform::Linux, &bundle));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "macos")]
    fn set_attribute(path: &Path, name: &str, value: &[u8]) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(path.as_os_str().as_bytes()).expect("path");
        let name = CString::new(name).expect("name");
        // SAFETY: the strings are NUL terminated and the value pointer has
        // the given size.
        let result = unsafe {
            libc::setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        assert_eq!(result, 0, "setxattr failed");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_alias_files_are_revealed_whatever_the_name_is() {
        let root = temp_root("macos-alias");
        let finder_alias = root.join("guide.html");
        let bookmark = root.join("notes.txt");
        let plain = root.join("plain.txt");
        fs::write(&finder_alias, "x").expect("write file");
        fs::write(&plain, "x").expect("write file");
        // The Finder marks an alias file with the type "alis" and the alias
        // flag in the FinderInfo attribute.
        let mut info = [0u8; 32];
        info[..8].copy_from_slice(b"alisMACS");
        info[8] = 0x80;
        set_attribute(&finder_alias, "com.apple.FinderInfo", &info);
        // A bookmark file has "book" at offset 0 and "mark" at offset 8.
        let mut data = b"book\0\0\0\0mark\0\0\0\0".to_vec();
        data.extend_from_slice(&[0u8; 32]);
        fs::write(&bookmark, data).expect("write bookmark");

        assert!(launchable_on(Platform::MacOs, &finder_alias));
        assert!(launchable_on(Platform::MacOs, &bookmark));
        assert!(!launchable_on(Platform::MacOs, &plain));
        // The alias check belongs to macOS.
        assert!(!launchable_on(Platform::Linux, &finder_alias));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_open_with_binding_is_revealed() {
        let root = temp_root("macos-open-with");
        let bound = root.join("readme.txt");
        let bound_dir = root.join("data");
        let plain = root.join("other.txt");
        fs::write(&bound, "x").expect("write file");
        fs::write(&plain, "x").expect("write file");
        fs::create_dir_all(&bound_dir).expect("create dir");
        set_attribute(&bound, "com.apple.LaunchServices.OpenWith", b"bplist00");
        set_attribute(&bound_dir, "com.apple.LaunchServices.OpenWith", b"bplist00");

        assert!(launchable_on(Platform::MacOs, &bound));
        assert!(launchable_on(Platform::MacOs, &bound_dir));
        assert!(!launchable_on(Platform::MacOs, &plain));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn external_links_allow_only_absolute_http_urls() {
        assert_eq!(
            normalize_external_url(" https://example.com/docs ").as_deref(),
            Ok("https://example.com/docs")
        );
        assert_eq!(
            normalize_external_url("HTTPS:attacker.example/x").as_deref(),
            Ok("https://attacker.example/x")
        );
        assert!(normalize_external_url("javascript:alert(1)").is_err());
        assert!(normalize_external_url("/relative").is_err());
        assert!(normalize_external_url("https://").is_err());
    }
}
