#[cfg(any(target_os = "linux", test))]
use std::{path::PathBuf, process::Command};

#[cfg(any(target_os = "linux", test))]
use super::{normalize_path_lexically, path_is_within, SandboxMode, WorkspaceRoots};

pub(super) const BWRAP_PATH: &str = "bwrap";

#[cfg(any(target_os = "linux", test))]
pub(super) fn bubblewrap_args(
    policy: SandboxMode,
    workspace_roots: &WorkspaceRoots,
    protected_paths: &[PathBuf],
    shared_rtk_state_dir: Option<&std::path::Path>,
) -> Vec<String> {
    if matches!(policy, SandboxMode::DangerFullAccess) {
        return Vec::new();
    }

    let mut args = vec![
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "--tmpfs".to_string(),
        "/var/tmp".to_string(),
        "--tmpfs".to_string(),
        "/run".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--unshare-net".to_string(),
        "--unshare-ipc".to_string(),
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
        "--unshare-pid".to_string(),
    ];

    if matches!(policy, SandboxMode::WorkspaceWrite) {
        for root in workspace_roots.roots() {
            if is_private_scratch_root(root) {
                continue;
            }
            let root = path_arg(root);
            args.push("--bind".to_string());
            args.push(root.clone());
            args.push(root);
        }

        for protected_path in protected_paths {
            let protected_path = normalize_path_lexically(protected_path);
            if workspace_roots
                .roots()
                .iter()
                .any(|root| path_is_within(&protected_path, &normalize_path_lexically(root)))
            {
                let protected_path = path_arg(&protected_path);
                args.push("--ro-bind".to_string());
                args.push(protected_path.clone());
                args.push(protected_path);
            }
        }
    }

    // RTK records the amount it removed in a tiny SQLite database. Bind only
    // that app-owned directory back out of bwrap's private /tmp so the managed
    // Headroom process can observe the same counters. The rest of the scratch
    // filesystem stays private.
    if let Some(state_dir) = shared_rtk_state_dir.filter(|path| path.is_dir()) {
        let state_dir = path_arg(state_dir);
        args.push("--bind".to_string());
        args.push(state_dir.clone());
        args.push(state_dir);
    }

    args
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn harden_command_environment(command: &mut Command) {
    for key in [
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "DOCKER_HOST",
        "GPG_AGENT_INFO",
        "KUBECONFIG",
        "PULSE_SERVER",
        "SSH_AGENT_PID",
        "SSH_AUTH_SOCK",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
    ] {
        command.env_remove(key);
    }
    command.env("TMPDIR", "/tmp");
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn is_private_scratch_root(root: &std::path::Path) -> bool {
    let mut candidates = vec![
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/run"),
    ];
    if let Some(tmpdir) = std::env::var_os("TMPDIR") {
        candidates.push(PathBuf::from(tmpdir));
    }
    candidates.into_iter().any(|candidate| {
        let candidate = candidate.canonicalize().unwrap_or(candidate);
        normalize_path_lexically(root) == normalize_path_lexically(&candidate)
    })
}

#[cfg(any(target_os = "linux", test))]
fn path_arg(path: &std::path::Path) -> String {
    path.to_string_lossy().to_string()
}
