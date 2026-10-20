//! Tracks the processes of one sandboxed shell command on macOS.
//!
//! Seatbelt has no equivalent of the bubblewrap `--die-with-parent` and
//! `--unshare-pid` options. A child can start a new session and then lose its
//! parent. Then launchd becomes its parent and the child has its own process
//! group. A walk from the shell at exit and a kill of the shell process group
//! do not find such a child, and it keeps the Seatbelt policy of the command.
//!
//! This module finds such processes in two ways:
//!
//! - A sampler thread walks the process tree while the command runs. It keeps
//!   each process that it sees, also after the process leaves the tree.
//! - The Seatbelt policy of the command allows one mach-lookup name that no
//!   other policy allows. At cleanup, a scan asks the sandbox of each process
//!   whether it allows that name. Every descendant keeps the policy.
//!
//! Cleanup sends a signal to a process only after it proves the identity of
//! the process. A recorded process must still have its recorded start time. A
//! scanned process must have the policy of the command.

use std::{
    collections::HashMap,
    ffi::{c_char, c_int, c_void, CString},
    io, mem,
    sync::{mpsc, Mutex, OnceLock},
    thread,
    time::Duration,
};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
const MAX_STOP_ROUNDS: usize = 16;
const MAX_LISTED_PIDS: usize = 1 << 16;
const SANDBOX_FILTER_GLOBAL_NAME: c_int = 2;
const TAG_NAME_PREFIX: &str = "ai.halv.process-family";

/// Names one process. macOS gives PIDs in sequence, so a PID comes back only
/// after the counter wraps. The start time tells the two processes apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessIdentity {
    pid: i32,
    start: (u64, u64),
}

impl ProcessIdentity {
    /// Reports whether the PID still names the process that this identity
    /// recorded. macOS gives no snapshot of a zombie, thus a zombie does not
    /// count. A zombie cannot run or fork.
    fn is_current(&self) -> bool {
        process_snapshot(self.pid).is_some_and(|snapshot| snapshot.identity == *self)
    }
}

#[derive(Clone, Copy, Debug)]
struct ProcessSnapshot {
    identity: ProcessIdentity,
    ppid: i32,
    pgid: i32,
    uid: u32,
}

fn process_snapshot(pid: i32) -> Option<ProcessSnapshot> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: proc_bsdinfo is plain data, and proc_pidinfo writes at most
    // `size` bytes into it.
    let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let size = mem::size_of::<libc::proc_bsdinfo>() as c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast::<c_void>(),
            size,
        )
    };
    if written != size || info.pbi_pid as i32 != pid {
        return None;
    }
    Some(ProcessSnapshot {
        identity: ProcessIdentity {
            pid,
            start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
        },
        ppid: info.pbi_ppid as i32,
        pgid: info.pbi_pgid as i32,
        uid: info.pbi_uid,
    })
}

/// Sends `signal` only when the PID still names the recorded process. The
/// check and the signal are not atomic. A PID comes back only after the PID
/// counter wraps, which cannot occur in that short interval.
fn signal_if_current(identity: ProcessIdentity, signal: c_int) -> bool {
    if !identity.is_current() {
        return false;
    }
    unsafe { libc::kill(identity.pid, signal) == 0 }
}

fn list_pids(list: impl Fn(*mut c_void, c_int) -> c_int) -> Vec<i32> {
    let mut capacity = 256;
    loop {
        let mut pids = vec![0_i32; capacity];
        let bytes = (capacity * mem::size_of::<i32>()) as c_int;
        let count = list(pids.as_mut_ptr().cast::<c_void>(), bytes);
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        if count < capacity || capacity >= MAX_LISTED_PIDS {
            pids.truncate(count.min(capacity));
            pids.retain(|pid| *pid > 0);
            return pids;
        }
        capacity *= 4;
    }
}

fn child_pids(parent: i32) -> Vec<i32> {
    list_pids(|buffer, size| unsafe { libc::proc_listchildpids(parent, buffer, size) })
}

fn all_pids() -> Vec<i32> {
    list_pids(|buffer, size| unsafe { libc::proc_listallpids(buffer, size) })
}

/// Adds every current descendant of `root` and of the known processes to
/// `known`. Returns the processes that it adds.
fn add_descendants(
    known: &mut HashMap<i32, ProcessIdentity>,
    root: ProcessIdentity,
) -> Vec<ProcessIdentity> {
    let mut added = Vec::new();
    let mut parents: Vec<ProcessIdentity> = known.values().copied().collect();
    parents.push(root);
    while let Some(parent) = parents.pop() {
        if !parent.is_current() {
            continue;
        }
        for pid in child_pids(parent.pid) {
            if known.contains_key(&pid) {
                continue;
            }
            let Some(child) = process_snapshot(pid) else {
                continue;
            };
            if child.ppid != parent.pid {
                continue;
            }
            known.insert(pid, child.identity);
            added.push(child.identity);
            parents.push(child.identity);
        }
    }
    added
}

type SandboxCheck = unsafe extern "C" fn(libc::pid_t, *const c_char, c_int, ...) -> c_int;

struct SandboxApi {
    check: SandboxCheck,
    no_report: c_int,
}

/// Loads `sandbox_check` at run time. It is not public API. When a macOS
/// release removes it, the scan finds nothing and the sampler still works.
fn sandbox_api() -> Option<&'static SandboxApi> {
    static API: OnceLock<Option<SandboxApi>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let check = libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr());
        let no_report = libc::dlsym(libc::RTLD_DEFAULT, c"SANDBOX_CHECK_NO_REPORT".as_ptr());
        if check.is_null() || no_report.is_null() {
            return None;
        }
        Some(SandboxApi {
            check: mem::transmute::<*mut c_void, SandboxCheck>(check),
            no_report: *no_report.cast::<c_int>(),
        })
    })
    .as_ref()
}

/// Marks the Seatbelt policy of one command. The policy allows the lookup of
/// a random mach service name. No service has that name. The policy denies
/// every other lookup, also the lookup of the probe name.
pub(super) struct SandboxTag {
    allowed: CString,
    probe: CString,
}

impl SandboxTag {
    pub(super) fn new() -> Self {
        let token: String = rand::random::<[u8; 16]>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let allowed = format!("{TAG_NAME_PREFIX}.{token}");
        let probe = format!("{allowed}.probe");
        Self {
            allowed: CString::new(allowed).expect("tag name has no NUL byte"),
            probe: CString::new(probe).expect("tag name has no NUL byte"),
        }
    }

    pub(super) fn seatbelt_rule(&self) -> String {
        format!(
            "(allow mach-lookup (global-name \"{}\"))",
            self.allowed.to_string_lossy()
        )
    }

    /// Reports whether the sandbox of `pid` allows the tag name and denies
    /// the probe name. An unsandboxed process allows both names. Another
    /// deny-by-default policy denies both names.
    fn is_carried_by(&self, pid: i32) -> bool {
        let Some(api) = sandbox_api() else {
            return false;
        };
        let filter = SANDBOX_FILTER_GLOBAL_NAME | api.no_report;
        let operation = c"mach-lookup".as_ptr();
        unsafe {
            (api.check)(pid, operation, filter, self.probe.as_ptr()) == 1
                && (api.check)(pid, operation, filter, self.allowed.as_ptr()) == 0
        }
    }
}

/// Finds the processes of this user that started after the root and carry
/// the tag. They can have any parent and any process group.
fn tagged_processes(root: ProcessIdentity, tag: &SandboxTag) -> Vec<ProcessIdentity> {
    let uid = unsafe { libc::geteuid() };
    let own_pid = std::process::id() as i32;
    all_pids()
        .into_iter()
        .filter(|pid| *pid > 1 && *pid != own_pid)
        .filter_map(process_snapshot)
        .filter(|snapshot| snapshot.uid == uid && snapshot.identity.start >= root.start)
        .filter(|snapshot| tag.is_carried_by(snapshot.identity.pid))
        .map(|snapshot| snapshot.identity)
        .collect()
}

/// Stops each process of the command, then kills it. A stopped process
/// cannot fork or exit, so its PID stays valid until the kill. Each round
/// stops the processes that the previous round found, then looks again for
/// children and tagged processes.
fn stop_and_kill(
    root: ProcessIdentity,
    mut members: HashMap<i32, ProcessIdentity>,
    tag: &SandboxTag,
) {
    if root.is_current() {
        members.insert(root.pid, root);
    }
    let mut pending: Vec<ProcessIdentity> = members.values().copied().collect();
    for _ in 0..MAX_STOP_ROUNDS {
        for member in pending.drain(..) {
            signal_if_current(member, libc::SIGSTOP);
        }
        let mut found = add_descendants(&mut members, root);
        for identity in tagged_processes(root, tag) {
            if members.insert(identity.pid, identity).is_none() {
                found.push(identity);
            }
        }
        if found.is_empty() {
            break;
        }
        pending = found;
    }

    for member in members.values() {
        let Some(snapshot) = process_snapshot(member.pid) else {
            continue;
        };
        if snapshot.identity != *member {
            continue;
        }
        unsafe {
            // A member that leads its own group made that group. The group
            // ID cannot name another group while the leader lives.
            if snapshot.pgid == member.pid {
                let _ = libc::kill(-member.pid, libc::SIGKILL);
            }
            let _ = libc::kill(member.pid, libc::SIGKILL);
        }
    }
}

struct Sampler {
    stop: mpsc::Sender<()>,
    thread: thread::JoinHandle<HashMap<i32, ProcessIdentity>>,
}

impl Sampler {
    fn start(root: ProcessIdentity) -> io::Result<Self> {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name("halv-process-tree".to_string())
            .spawn(move || {
                let mut records = HashMap::new();
                loop {
                    // Drop the records of ended processes. Their PIDs can
                    // name other processes later.
                    records.retain(|_, identity: &mut ProcessIdentity| identity.is_current());
                    add_descendants(&mut records, root);
                    match stopped.recv_timeout(SAMPLE_INTERVAL) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return records,
                    }
                }
            })?;
        Ok(Self { stop, thread })
    }

    fn finish(self) -> HashMap<i32, ProcessIdentity> {
        drop(self.stop);
        self.thread.join().unwrap_or_default()
    }
}

/// Follows the processes of one sandboxed command from spawn to cleanup.
pub(super) struct ProcessTree {
    root: ProcessIdentity,
    tag: SandboxTag,
    sampler: Mutex<Option<Sampler>>,
}

impl ProcessTree {
    pub(super) fn track(root_pid: u32, tag: SandboxTag) -> io::Result<Self> {
        let pid = i32::try_from(root_pid)
            .map_err(|_| io::Error::other("the command has no valid process ID"))?;
        // A command that already exited has no snapshot. Then the identity
        // has no start time and matches no process, and the scan checks
        // every process of this user.
        let root = process_snapshot(pid)
            .map_or(ProcessIdentity { pid, start: (0, 0) }, |snapshot| {
                snapshot.identity
            });
        let sampler = Sampler::start(root)?;
        Ok(Self {
            root,
            tag,
            sampler: Mutex::new(Some(sampler)),
        })
    }

    /// Kills every process of the command whose identity it can prove: the
    /// recorded processes, their current descendants, and the processes
    /// that carry the tag.
    pub(super) fn terminate(&self) {
        let records = self
            .sampler
            .lock()
            .ok()
            .and_then(|mut sampler| sampler.take())
            .map(Sampler::finish)
            .unwrap_or_default();
        stop_and_kill(self.root, records, &self.tag);
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if let Some(sampler) = self.sampler.get_mut().ok().and_then(Option::take) {
            sampler.finish();
        }
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use std::{
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    /// Starts a daemon that leaves the process tree. The first child starts a
    /// new session and closes the output pipes of the command, as a real
    /// daemon does. It waits `before_fork` seconds, forks the daemon, waits
    /// `after_fork` seconds, and exits. The parent waits `parent_hold`
    /// seconds, or for the first child when `parent_hold` is 0. Then it waits
    /// for the daemon PID file and exits. The daemon appends to `marker`.
    const DAEMON_SCRIPT: &str = r#"
use strict;
use POSIX ();
my ($dir, $before_fork, $after_fork, $parent_hold) = @ARGV;
my $first = fork();
die "fork failed: $!" unless defined $first;
if ($first == 0) {
    POSIX::setsid();
    open(STDIN, '<', '/dev/null');
    open(STDOUT, '>', '/dev/null');
    open(STDERR, '>', '/dev/null');
    select(undef, undef, undef, $before_fork);
    my $daemon = fork();
    if (defined $daemon && $daemon == 0) {
        open(my $pid_file, '>', "$dir/daemon.pid.tmp") or POSIX::_exit(1);
        print $pid_file $$;
        close $pid_file;
        rename("$dir/daemon.pid.tmp", "$dir/daemon.pid");
        while (1) {
            open(my $marker, '>>', "$dir/marker") or POSIX::_exit(1);
            print $marker "x";
            close $marker;
            select(undef, undef, undef, 0.02);
        }
    }
    select(undef, undef, undef, $after_fork);
    POSIX::_exit(0);
}
if ($parent_hold > 0) {
    select(undef, undef, undef, $parent_hold);
} else {
    waitpid($first, 0);
}
for (1 .. 200) {
    last if -s "$dir/daemon.pid";
    select(undef, undef, undef, 0.025);
}
exit 0;
"#;

    pub(in crate::exec) const PERL_PATH: &str = "/usr/bin/perl";

    pub(in crate::exec) fn daemon_directory(label: &str) -> PathBuf {
        let token: String = rand::random::<[u8; 8]>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let directory = std::env::temp_dir().join(format!("halv-process-tree-{label}-{token}"));
        std::fs::create_dir_all(&directory).expect("daemon directory should create");
        let directory = directory
            .canonicalize()
            .expect("daemon directory should resolve");
        std::fs::write(directory.join("daemon.pl"), DAEMON_SCRIPT)
            .expect("daemon script should write");
        directory
    }

    /// Asserts that the daemon is gone and that the marker stops growing.
    /// Kills a surviving daemon before the assertion fails, so a failed test
    /// leaves no process behind.
    pub(in crate::exec) fn assert_daemon_gone(directory: &Path) {
        let pid = std::fs::read_to_string(directory.join("daemon.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<i32>().ok())
            .expect("the daemon should write its PID before cleanup");
        let alive = || unsafe { libc::kill(pid, 0) == 0 };
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let survived = alive();
        if survived {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        let marker = directory.join("marker");
        let marker_len = || std::fs::metadata(&marker).map(|metadata| metadata.len());
        let before = marker_len().unwrap_or(0);
        thread::sleep(Duration::from_millis(300));
        let after = marker_len().unwrap_or(0);
        let _ = std::fs::remove_dir_all(directory);
        assert!(!survived, "detached daemon {pid} survived cleanup");
        assert_eq!(before, after, "the marker kept growing after cleanup");
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    use std::{
        os::unix::process::{CommandExt, ExitStatusExt},
        path::Path,
        process::{Child, Command, Stdio},
    };

    fn spawn_daemon(launcher: &mut Command, directory: &Path, timing: [&str; 3]) -> Child {
        launcher
            .arg(PERL_PATH)
            .arg(directory.join("daemon.pl"))
            .arg(directory)
            .args(timing)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("daemon launcher should start")
    }

    fn identity_of(child: &Child) -> ProcessIdentity {
        process_snapshot(child.id() as i32)
            .expect("a live child should have a snapshot")
            .identity
    }

    fn is_stopped(child: &Child) -> bool {
        let mut status = 0;
        let changed = unsafe {
            libc::waitpid(
                child.id() as i32,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED,
            )
        };
        changed == child.id() as i32 && libc::WIFSTOPPED(status)
    }

    fn seatbelt_launcher(policy: &str) -> Command {
        let mut launcher = Command::new("/usr/bin/sandbox-exec");
        launcher.arg("-p").arg(policy).arg("--");
        launcher
    }

    fn sandbox_blocked(status: std::process::ExitStatus) -> bool {
        if status.code() == Some(71) {
            eprintln!("sandbox-exec is blocked by the outer test environment; assertion skipped");
            return true;
        }
        false
    }

    #[test]
    fn identity_guard_refuses_a_pid_with_another_start_time() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("sleep should start");
        let identity = identity_of(&child);
        assert!(identity.is_current());

        let reused = ProcessIdentity {
            pid: identity.pid,
            start: (identity.start.0, identity.start.1 + 1),
        };
        assert!(!reused.is_current());
        assert!(!signal_if_current(reused, libc::SIGKILL));
        thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().expect("sleep should poll").is_none(),
            "a guarded signal reached a process with another start time"
        );

        assert!(signal_if_current(identity, libc::SIGKILL));
        let status = child.wait().expect("sleep should exit");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert!(!identity.is_current(), "a reaped PID still matched");
    }

    #[test]
    fn cleanup_leaves_a_process_whose_identity_does_not_match_the_record() {
        let mut root = Command::new("/usr/bin/true")
            .spawn()
            .expect("true should start");
        let root_identity = identity_of(&root);
        root.wait().expect("true should exit");

        let mut unrelated = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("sleep should start");
        let actual = identity_of(&unrelated);
        let stale = ProcessIdentity {
            pid: actual.pid,
            start: (actual.start.0.saturating_sub(1), actual.start.1),
        };
        stop_and_kill(
            root_identity,
            HashMap::from([(stale.pid, stale)]),
            &SandboxTag::new(),
        );

        let stopped = is_stopped(&unrelated);
        let running = unrelated.try_wait().expect("sleep should poll").is_none();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        assert!(
            !stopped,
            "cleanup stopped a process with another start time"
        );
        assert!(running, "cleanup killed a process with another start time");
    }

    #[test]
    fn sampler_follows_a_detached_child_and_its_later_children() {
        let directory = daemon_directory("sampler");
        // The first child detaches at once. It forks the daemon after 0.4 s
        // and exits after 1.0 s. The root exits after 1.5 s. At cleanup the
        // daemon has launchd as its parent and the group of the first child.
        let mut root = spawn_daemon(
            &mut Command::new("/usr/bin/env"),
            &directory,
            ["0.4", "0.6", "1.5"],
        );
        let tree = ProcessTree::track(root.id(), SandboxTag::new()).expect("tracking should start");
        root.wait().expect("root should exit");
        tree.terminate();
        assert_daemon_gone(&directory);
    }

    #[test]
    fn tag_marks_only_processes_with_the_policy_of_the_command() {
        if sandbox_api().is_none() {
            eprintln!("sandbox_check is unavailable; tag assertion skipped");
            return;
        }
        let tag = SandboxTag::new();
        let other_tag = SandboxTag::new();
        let base = "(version 1)\n(deny default)\n(allow process-exec)\n(allow file-read*)";
        let spawn_sleep = |mut launcher: Command| {
            launcher
                .arg("/bin/sleep")
                .arg("30")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sleep should start")
        };
        let mut tagged = spawn_sleep(seatbelt_launcher(&format!(
            "{base}\n{}",
            tag.seatbelt_rule()
        )));
        let mut other = spawn_sleep(seatbelt_launcher(&format!(
            "{base}\n{}",
            other_tag.seatbelt_rule()
        )));
        let mut permissive = spawn_sleep(seatbelt_launcher("(version 1)\n(allow default)"));
        let mut unsandboxed = spawn_sleep(Command::new("/usr/bin/env"));
        thread::sleep(Duration::from_millis(300));

        let blocked = tagged
            .try_wait()
            .ok()
            .flatten()
            .is_some_and(sandbox_blocked);
        let results = [
            tag.is_carried_by(tagged.id() as i32),
            tag.is_carried_by(other.id() as i32),
            tag.is_carried_by(permissive.id() as i32),
            tag.is_carried_by(unsandboxed.id() as i32),
        ];
        for child in [&mut tagged, &mut other, &mut permissive, &mut unsandboxed] {
            let _ = child.kill();
            let _ = child.wait();
        }
        if blocked {
            return;
        }
        assert_eq!(results, [true, false, false, false]);
    }

    #[test]
    fn tag_finds_a_detached_daemon_that_no_sample_saw() {
        if sandbox_api().is_none() {
            eprintln!("sandbox_check is unavailable; tag assertion skipped");
            return;
        }
        let directory = daemon_directory("tag");
        let tag = SandboxTag::new();
        let roots = super::super::WorkspaceRoots::for_cwd(&directory);
        let policy = format!(
            "{}\n{}",
            super::super::workspace_write_seatbelt_policy(&roots),
            tag.seatbelt_rule()
        );
        // The first child detaches, forks the daemon, and exits at once.
        let mut root = spawn_daemon(&mut seatbelt_launcher(&policy), &directory, ["0", "0", "0"]);
        let root_identity = identity_of(&root);
        let status = root.wait().expect("root should exit");
        if sandbox_blocked(status) {
            let _ = std::fs::remove_dir_all(&directory);
            return;
        }
        // No sampler ran, thus only the tag can find the daemon.
        stop_and_kill(root_identity, HashMap::new(), &tag);
        assert_daemon_gone(&directory);
    }
}
