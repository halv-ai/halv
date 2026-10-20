use crate::{
    client::{decode_base64, encode_base64},
    negotiated_protocol, OpenRequest, RequestFrame, ResponseFrame, ScrollbackRing, SessionStatus,
    SessionSummary, SupervisorClient, MAX_PROTOCOL_VERSION, PROTOCOL_VERSION,
    SUPERVISOR_FINGERPRINT, SUPERVISOR_VERSION,
};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_SESSIONS: usize = 32;
const MAX_WRITE_BYTES: usize = 64 * 1024;
const MAX_SCROLLBACK_BYTES: usize = 256 * 1024;
const MIN_COLS: u16 = 20;
const MAX_COLS: u16 = 500;
const MIN_ROWS: u16 = 5;
const MAX_ROWS: u16 = 300;
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_millis(25);
const ATTACH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(4);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

struct Config {
    socket_path: PathBuf,
    log_path: Option<PathBuf>,
    idle_timeout: Duration,
}

pub fn run_from_env() -> Result<(), String> {
    let config = parse_args(env::args_os().skip(1))?;
    let listener = match bind_single_instance(&config.socket_path)? {
        BindResult::Existing => return Ok(()),
        BindResult::Listener(listener) => listener,
    };
    let socket_guard = SocketGuard::new(&config.socket_path).inspect_err(|_error| {
        let _ = fs::remove_file(&config.socket_path);
    })?;
    detach(config.log_path.as_deref())?;
    serve(listener, config.idle_timeout)?;
    drop(socket_guard);
    Ok(())
}

enum BindResult {
    Existing,
    Listener(UnixListener),
}

fn parse_args(arguments: impl Iterator<Item = OsString>) -> Result<Config, String> {
    let mut socket_path = None;
    let mut log_path = None;
    let mut idle_timeout = Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS);
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--socket") => {
                let value = arguments.next().ok_or_else(|| {
                    "failed to parse arguments: --socket needs a path".to_string()
                })?;
                socket_path = Some(PathBuf::from(value));
            }
            Some("--log") => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "failed to parse arguments: --log needs a path".to_string())?;
                log_path = Some(PathBuf::from(value));
            }
            Some("--idle-timeout-secs") => {
                let value = arguments.next().ok_or_else(|| {
                    "failed to parse arguments: --idle-timeout-secs needs a number".to_string()
                })?;
                let value = value.to_str().ok_or_else(|| {
                    "failed to parse arguments: idle timeout is not valid UTF-8".to_string()
                })?;
                let seconds = value
                    .parse::<u64>()
                    .map_err(|error| format!("failed to parse idle timeout `{value}`: {error}"))?;
                idle_timeout = Duration::from_secs(seconds);
            }
            Some(value) => {
                return Err(format!(
                    "failed to parse arguments: unknown option `{value}`"
                ));
            }
            None => {
                return Err("failed to parse arguments: option is not valid UTF-8".to_string());
            }
        }
    }
    let socket_path =
        socket_path.ok_or_else(|| "failed to parse arguments: --socket is required".to_string())?;
    Ok(Config {
        socket_path,
        log_path,
        idle_timeout,
    })
}

fn bind_single_instance(socket_path: &Path) -> Result<BindResult, String> {
    if let Ok(metadata) = fs::symlink_metadata(socket_path) {
        if !metadata.file_type().is_socket() {
            return Err(format!(
                "failed to bind supervisor socket `{}`: path is not a socket",
                socket_path.display()
            ));
        }
        match SupervisorClient::connect(socket_path, SUPERVISOR_VERSION) {
            Ok(_) => return Ok(BindResult::Existing),
            Err(hello_error) => match UnixStream::connect(socket_path) {
                Ok(_) => {
                    return Err(format!(
                        "failed to verify existing supervisor at `{}`: {hello_error}",
                        socket_path.display()
                    ));
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    fs::remove_file(socket_path).map_err(|remove_error| {
                        format!(
                            "failed to remove stale supervisor socket `{}`: {remove_error}",
                            socket_path.display()
                        )
                    })?;
                }
                Err(error) => {
                    return Err(format!(
                        "failed to inspect supervisor socket `{}`: {error}",
                        socket_path.display()
                    ));
                }
            },
        }
    }

    let listener = match UnixListener::bind(socket_path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if SupervisorClient::connect(socket_path, SUPERVISOR_VERSION).is_ok() {
                return Ok(BindResult::Existing);
            }
            return Err(format!(
                "failed to bind supervisor socket `{}`: {error}",
                socket_path.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "failed to bind supervisor socket `{}`: {error}",
                socket_path.display()
            ));
        }
    };
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        let _ = fs::remove_file(socket_path);
        format!(
            "failed to protect supervisor socket `{}`: {error}",
            socket_path.display()
        )
    })?;
    Ok(BindResult::Listener(listener))
}

fn detach(log_path: Option<&Path>) -> Result<(), String> {
    let session_id = unsafe { libc::setsid() };
    if session_id == -1 {
        let error = std::io::Error::last_os_error();
        let already_leader = error.raw_os_error() == Some(libc::EPERM)
            && unsafe { libc::getpgrp() } == unsafe { libc::getpid() };
        if !already_leader {
            return Err(format!("failed to detach supervisor: {error}"));
        }
    }
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let output = open_log(log_path)?;
    redirect_fd(&output, libc::STDOUT_FILENO)?;
    redirect_fd(&output, libc::STDERR_FILENO)?;
    Ok(())
}

fn open_log(log_path: Option<&Path>) -> Result<File, String> {
    match log_path {
        Some(path) => OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| {
                format!(
                    "failed to open supervisor log `{}`: {error}",
                    path.display()
                )
            }),
        None => OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .map_err(|error| format!("failed to open /dev/null: {error}")),
    }
}

fn redirect_fd(file: &File, target: RawFd) -> Result<(), String> {
    if unsafe { libc::dup2(file.as_raw_fd(), target) } == -1 {
        return Err(format!(
            "failed to redirect supervisor output: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketGuard {
    fn new(path: &Path) -> Result<Self, String> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!(
                "failed to inspect supervisor socket `{}`: {error}",
                path.display()
            )
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct SupervisorState {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    clients: AtomicUsize,
    stopping: AtomicBool,
    next_subscriber: AtomicU64,
}

impl SupervisorState {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            clients: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            next_subscriber: AtomicU64::new(1),
        }
    }

    fn session(&self, id: &str) -> Result<Arc<Session>, String> {
        self.sessions
            .lock()
            .expect("supervisor sessions lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| format!("terminal session `{id}` was not found"))
    }

    fn list(&self) -> Vec<SessionSummary> {
        let sessions = self
            .sessions
            .lock()
            .expect("supervisor sessions lock poisoned");
        let mut list = sessions
            .values()
            .map(|session| session.summary())
            .collect::<Vec<_>>();
        list.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        list
    }

    fn is_idle(&self) -> bool {
        self.clients.load(Ordering::SeqCst) == 0
            && self
                .sessions
                .lock()
                .expect("supervisor sessions lock poisoned")
                .is_empty()
    }
}

struct ClientGuard {
    state: Arc<SupervisorState>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.state.clients.fetch_sub(1, Ordering::SeqCst);
    }
}

fn serve(listener: UnixListener, idle_timeout: Duration) -> Result<(), String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("failed to configure supervisor listener: {error}"))?;
    let state = Arc::new(SupervisorState::new());
    let mut idle_since = None;
    while !state.stopping.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                idle_since = None;
                state.clients.fetch_add(1, Ordering::SeqCst);
                let client_state = state.clone();
                if let Err(error) = thread::Builder::new()
                    .name("halv-supervisor-client".to_string())
                    .spawn(move || {
                        let _guard = ClientGuard {
                            state: client_state.clone(),
                        };
                        if let Err(error) = handle_client(stream, &client_state) {
                            eprintln!("{error}");
                        }
                    })
                {
                    state.clients.fetch_sub(1, Ordering::SeqCst);
                    eprintln!("failed to start supervisor client thread: {error}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("failed to accept supervisor client: {error}")),
        }

        if state.is_idle() {
            let started = idle_since.get_or_insert_with(Instant::now);
            if started.elapsed() >= idle_timeout {
                break;
            }
        } else {
            idle_since = None;
        }
        thread::sleep(HOUSEKEEPING_INTERVAL);
    }
    Ok(())
}

#[doc(hidden)]
pub struct InProcessSupervisor {
    state: Arc<SupervisorState>,
}

impl InProcessSupervisor {
    pub fn new() -> Self {
        Self {
            state: Arc::new(SupervisorState::new()),
        }
    }

    pub fn connect(&self, app_version: &str) -> Result<SupervisorClient, String> {
        SupervisorClient::from_stream(self.raw_connection()?, app_version)
    }

    pub fn attach(
        &self,
        app_version: &str,
        id: &str,
    ) -> Result<(crate::Snapshot, crate::AttachEvents), String> {
        SupervisorClient::attach_stream(self.raw_connection()?, app_version, id, None)
    }

    pub fn raw_connection(&self) -> Result<UnixStream, String> {
        let (client, server) = UnixStream::pair()
            .map_err(|error| format!("failed to create supervisor test socket: {error}"))?;
        self.state.clients.fetch_add(1, Ordering::SeqCst);
        let client_state = self.state.clone();
        if let Err(error) = thread::Builder::new()
            .name("halv-supervisor-in-process-client".to_string())
            .spawn(move || {
                let _guard = ClientGuard {
                    state: client_state.clone(),
                };
                if let Err(error) = handle_client(server, &client_state) {
                    eprintln!("{error}");
                }
            })
        {
            self.state.clients.fetch_sub(1, Ordering::SeqCst);
            return Err(format!(
                "failed to start in-process supervisor client: {error}"
            ));
        }
        Ok(client)
    }
}

impl Default for InProcessSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

struct Session {
    inner: Mutex<SessionInner>,
    exited: Condvar,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
}

struct SessionInner {
    summary: SessionSummary,
    output: ScrollbackRing,
    subscribers: Vec<Subscriber>,
}

struct Subscriber {
    id: u64,
    sender: Sender<ResponseFrame>,
}

impl Session {
    fn summary(&self) -> SessionSummary {
        self.inner
            .lock()
            .expect("terminal session lock poisoned")
            .summary
            .clone()
    }

    fn emit_data(&self, data: &[u8]) {
        let mut inner = self.inner.lock().expect("terminal session lock poisoned");
        let Some((sequence, data)) = inner.output.append(data) else {
            return;
        };
        let frame = ResponseFrame::Data {
            sequence,
            data: encode_base64(data.as_bytes()),
        };
        send_to_subscribers(&mut inner.subscribers, &frame);
    }

    fn finish(&self, exit_code: Option<i32>, signal: Option<String>) {
        self.writer
            .lock()
            .expect("terminal writer lock poisoned")
            .take();
        self.killer
            .lock()
            .expect("terminal killer lock poisoned")
            .take();
        let mut inner = self.inner.lock().expect("terminal session lock poisoned");
        if inner.summary.status == SessionStatus::Exited {
            return;
        }
        inner.summary.status = SessionStatus::Exited;
        inner.summary.exited_at = Some(rfc3339_now());
        inner.summary.exit_code = exit_code;
        inner.summary.signal.clone_from(&signal);
        let sequence = inner.output.next_sequence();
        let frame = ResponseFrame::Exit {
            sequence,
            exit_code,
            signal,
        };
        send_to_subscribers(&mut inner.subscribers, &frame);
        inner.subscribers.clear();
        self.exited.notify_all();
    }
}

fn send_to_subscribers(subscribers: &mut Vec<Subscriber>, frame: &ResponseFrame) {
    // PTY programs such as Codex redraw their full-screen UI in short bursts.
    // A depth-limited queue turned those harmless bursts into an attach error,
    // forcing the app through a reconnect loop even though both the PTY and the
    // supervisor were healthy. The socket writer still has a hard timeout, so
    // a genuinely abandoned reader is removed without allowing this queue to
    // live indefinitely.
    subscribers.retain(|subscriber| subscriber.sender.send(frame.clone()).is_ok());
}

fn handle_client(stream: UnixStream, state: &Arc<SupervisorState>) -> Result<(), String> {
    stream
        .set_nonblocking(false)
        .map_err(|error| format!("failed to configure supervisor client socket: {error}"))?;
    stream
        .set_write_timeout(Some(SOCKET_WRITE_TIMEOUT))
        .map_err(|error| format!("failed to set supervisor client timeout: {error}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|error| format!("failed to clone supervisor client socket: {error}"))?;
    let mut reader = BufReader::new(reader_stream);
    let hello = match read_request(&mut reader) {
        Ok(frame) => frame,
        Err(error) => {
            let _ = send_error(&stream, &error);
            return Ok(());
        }
    };
    // Protocol 1 is the frozen baseline. A newer revision is negotiated
    // through `max_proto`, so every older app still connects.
    let protocol = match hello {
        RequestFrame::Hello {
            proto, max_proto, ..
        } if proto == PROTOCOL_VERSION => {
            send_response(
                &stream,
                &ResponseFrame::Hello {
                    proto: PROTOCOL_VERSION,
                    supervisor_version: SUPERVISOR_VERSION.to_string(),
                    supervisor_fingerprint: Some(SUPERVISOR_FINGERPRINT.to_string()),
                    pid: std::process::id(),
                    max_proto: Some(MAX_PROTOCOL_VERSION),
                },
            )?;
            negotiated_protocol(MAX_PROTOCOL_VERSION, max_proto)
        }
        RequestFrame::Hello { proto, .. } => {
            send_error(
                &stream,
                &format!(
                    "failed to connect to supervisor: protocol {proto} does not match {PROTOCOL_VERSION}"
                ),
            )?;
            return Ok(());
        }
        _ => {
            send_error(
                &stream,
                "failed to connect to supervisor: hello must be the first request",
            )?;
            return Ok(());
        }
    };

    loop {
        let request = match read_request_optional(&mut reader) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(()),
            Err(error) => {
                send_error(&stream, &error)?;
                return Ok(());
            }
        };
        if request.min_protocol() > protocol {
            send_error(
                &stream,
                "failed to handle the request: the client did not negotiate it",
            )?;
            continue;
        }
        if let RequestFrame::Attach { id, from_sequence } = request {
            handle_attach(&stream, state, &id, from_sequence)?;
            return Ok(());
        }
        let (response, stop) = dispatch_control(request, state);
        send_response(&stream, &response)?;
        if stop {
            state.stopping.store(true, Ordering::SeqCst);
            return Ok(());
        }
    }
}

fn dispatch_control(request: RequestFrame, state: &Arc<SupervisorState>) -> (ResponseFrame, bool) {
    let result = match request {
        RequestFrame::Open {
            id,
            program,
            args,
            cwd,
            env,
            env_remove,
            cols,
            rows,
            meta,
        } => open_session(
            state,
            OpenRequest {
                id,
                program,
                args,
                cwd,
                env,
                env_remove,
                cols,
                rows,
                meta,
            },
        )
        .map(|session| ResponseFrame::Session { session }),
        RequestFrame::Write { id, data } => {
            write_session(state, &id, &data).map(|()| ResponseFrame::Ok)
        }
        RequestFrame::Resize { id, cols, rows } => {
            resize_session(state, &id, cols, rows).map(|session| ResponseFrame::Session { session })
        }
        RequestFrame::Close { id } => {
            close_session(state, &id).map(|session| ResponseFrame::Session { session })
        }
        RequestFrame::Remove { id } => remove_session(state, &id).map(|()| ResponseFrame::Ok),
        RequestFrame::List => Ok(ResponseFrame::List {
            sessions: state.list(),
        }),
        RequestFrame::UpdateMeta { id, meta } => {
            update_meta(state, &id, meta).map(|session| ResponseFrame::Session { session })
        }
        RequestFrame::Shutdown { kill_sessions } => {
            return match shutdown_sessions(state, kill_sessions) {
                Ok(()) => (ResponseFrame::Ok, true),
                Err(message) => (ResponseFrame::Error { message }, false),
            };
        }
        RequestFrame::Hello { .. } => {
            Err("failed to handle supervisor request: hello was already completed".to_string())
        }
        RequestFrame::Attach { .. } => unreachable!("attach is handled before control dispatch"),
    };
    match result {
        Ok(response) => (response, false),
        Err(message) => (ResponseFrame::Error { message }, false),
    }
}

fn open_session(
    state: &Arc<SupervisorState>,
    request: OpenRequest,
) -> Result<SessionSummary, String> {
    if !Path::new(&request.cwd).is_dir() {
        return Err(format!("terminal cwd `{}` is not a directory", request.cwd));
    }
    if request.program.is_empty() {
        return Err("failed to start terminal program: program cannot be empty".to_string());
    }
    let mut sessions = state
        .sessions
        .lock()
        .expect("supervisor sessions lock poisoned");
    if sessions.contains_key(&request.id) {
        return Err(format!("terminal session `{}` already exists", request.id));
    }
    let running = sessions
        .values()
        .filter(|session| session.summary().status == SessionStatus::Running)
        .count();
    if running >= MAX_SESSIONS {
        return Err(format!(
            "at most {MAX_SESSIONS} terminal sessions can run at once"
        ));
    }

    let size = terminal_size(request.cols, request.rows);
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(size)
        .map_err(|error| format!("failed to open terminal: {error}"))?;
    let mut command = CommandBuilder::new(&request.program);
    for argument in &request.args {
        command.arg(argument);
    }
    for name in env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| is_orca_variable(name))
    {
        command.env_remove(name);
    }
    for [name, value] in &request.env {
        command.env(name, value);
    }
    for name in &request.env_remove {
        command.env_remove(name);
    }
    command.cwd(&request.cwd);
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("failed to start terminal program: {error}"))?;
    let pid = child.process_id().unwrap_or(0);
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("failed to attach terminal output: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("failed to attach terminal input: {error}"))?;
    drop(pair.slave);

    let summary = SessionSummary {
        id: request.id.clone(),
        pid,
        cwd: request.cwd,
        program: request.program,
        status: SessionStatus::Running,
        cols: size.cols,
        rows: size.rows,
        started_at: rfc3339_now(),
        exited_at: None,
        exit_code: None,
        signal: None,
        meta: request.meta,
    };
    let session = Arc::new(Session {
        inner: Mutex::new(SessionInner {
            summary: summary.clone(),
            output: ScrollbackRing::new(MAX_SCROLLBACK_BYTES),
            subscribers: Vec::new(),
        }),
        exited: Condvar::new(),
        master: Mutex::new(pair.master),
        writer: Mutex::new(Some(writer)),
        killer: Mutex::new(Some(killer)),
    });
    sessions.insert(request.id, session.clone());
    drop(sessions);

    let (reader_done_tx, reader_done_rx) = mpsc::channel();
    let reader_session = session.clone();
    thread::Builder::new()
        .name("halv-supervisor-pty-reader".to_string())
        .spawn(move || {
            read_pty(reader_session, reader);
            let _ = reader_done_tx.send(());
        })
        .map_err(|error| format!("failed to start terminal output reader: {error}"))?;
    thread::Builder::new()
        .name("halv-supervisor-pty-wait".to_string())
        .spawn(move || {
            let result = child.wait();
            let _ = reader_done_rx.recv_timeout(Duration::from_millis(100));
            match result {
                Ok(exit) => session.finish(
                    Some(exit.exit_code().min(i32::MAX as u32) as i32),
                    exit.signal().map(ToString::to_string),
                ),
                Err(error) => {
                    eprintln!("failed to wait for terminal program: {error}");
                    session.finish(None, None);
                }
            }
        })
        .map_err(|error| format!("failed to start terminal wait thread: {error}"))?;
    Ok(summary)
}

fn read_pty(session: Arc<Session>, mut reader: Box<dyn Read + Send>) {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => session.emit_data(&buffer[..read]),
            Err(error) => {
                eprintln!("terminal output failed: {error}");
                return;
            }
        }
    }
}

fn write_session(state: &SupervisorState, id: &str, data: &str) -> Result<(), String> {
    if data.len() > (MAX_WRITE_BYTES.div_ceil(3) * 4) {
        return Err(format!(
            "terminal input exceeds the {MAX_WRITE_BYTES}-byte limit"
        ));
    }
    let decoded =
        decode_base64(data).map_err(|error| format!("failed to decode terminal input: {error}"))?;
    if decoded.len() > MAX_WRITE_BYTES {
        return Err(format!(
            "terminal input exceeds the {MAX_WRITE_BYTES}-byte limit"
        ));
    }
    let session = state.session(id)?;
    if session.summary().status != SessionStatus::Running {
        return Err("terminal session is not running".to_string());
    }
    let mut writer = session
        .writer
        .lock()
        .expect("terminal writer lock poisoned");
    let writer = writer
        .as_mut()
        .ok_or_else(|| "terminal input is closed".to_string())?;
    writer
        .write_all(&decoded)
        .and_then(|()| writer.flush())
        .map_err(|error| format!("failed to write terminal input: {error}"))
}

fn resize_session(
    state: &SupervisorState,
    id: &str,
    cols: u16,
    rows: u16,
) -> Result<SessionSummary, String> {
    let session = state.session(id)?;
    let size = terminal_size(cols, rows);
    session
        .master
        .lock()
        .expect("terminal master lock poisoned")
        .resize(size)
        .map_err(|error| format!("failed to resize terminal: {error}"))?;
    let mut inner = session
        .inner
        .lock()
        .expect("terminal session lock poisoned");
    inner.summary.cols = size.cols;
    inner.summary.rows = size.rows;
    Ok(inner.summary.clone())
}

fn close_session(state: &SupervisorState, id: &str) -> Result<SessionSummary, String> {
    let session = state.session(id)?;
    if session.summary().status == SessionStatus::Running {
        if let Some(killer) = session
            .killer
            .lock()
            .expect("terminal killer lock poisoned")
            .as_mut()
        {
            killer
                .kill()
                .map_err(|error| format!("failed to close terminal: {error}"))?;
        }
        let inner = session
            .inner
            .lock()
            .expect("terminal session lock poisoned");
        let (inner, timeout) = session
            .exited
            .wait_timeout_while(inner, CLOSE_TIMEOUT, |inner| {
                inner.summary.status == SessionStatus::Running
            })
            .expect("terminal session lock poisoned while closing");
        if timeout.timed_out() && inner.summary.status == SessionStatus::Running {
            return Err("failed to close terminal: timed out waiting for exit".to_string());
        }
        return Ok(inner.summary.clone());
    }
    Ok(session.summary())
}

fn remove_session(state: &SupervisorState, id: &str) -> Result<(), String> {
    let mut sessions = state
        .sessions
        .lock()
        .expect("supervisor sessions lock poisoned");
    let session = sessions
        .get(id)
        .ok_or_else(|| format!("terminal session `{id}` was not found"))?;
    if session.summary().status == SessionStatus::Running {
        return Err(format!("terminal session `{id}` is still running"));
    }
    sessions.remove(id);
    Ok(())
}

fn update_meta(
    state: &SupervisorState,
    id: &str,
    meta: serde_json::Value,
) -> Result<SessionSummary, String> {
    let session = state.session(id)?;
    let mut inner = session
        .inner
        .lock()
        .expect("terminal session lock poisoned");
    inner.summary.meta = meta;
    Ok(inner.summary.clone())
}

fn shutdown_sessions(state: &SupervisorState, kill_sessions: bool) -> Result<(), String> {
    let sessions = state
        .sessions
        .lock()
        .expect("supervisor sessions lock poisoned")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let running = sessions
        .iter()
        .filter(|session| session.summary().status == SessionStatus::Running)
        .collect::<Vec<_>>();
    if !kill_sessions && !running.is_empty() {
        return Err("terminal sessions are still running".to_string());
    }
    for session in running {
        close_session_by_arc(session)?;
    }
    Ok(())
}

fn close_session_by_arc(session: &Arc<Session>) -> Result<(), String> {
    if let Some(killer) = session
        .killer
        .lock()
        .expect("terminal killer lock poisoned")
        .as_mut()
    {
        killer
            .kill()
            .map_err(|error| format!("failed to close terminal: {error}"))?;
    }
    let inner = session
        .inner
        .lock()
        .expect("terminal session lock poisoned");
    let (inner, timeout) = session
        .exited
        .wait_timeout_while(inner, CLOSE_TIMEOUT, |inner| {
            inner.summary.status == SessionStatus::Running
        })
        .expect("terminal session lock poisoned while closing");
    if timeout.timed_out() && inner.summary.status == SessionStatus::Running {
        return Err("failed to close terminal: timed out waiting for exit".to_string());
    }
    Ok(())
}

/// Orca marks its panes with `ORCA_*` variables, and its agent hooks report
/// to the pane that they name. A supervisor that starts in an Orca pane
/// inherits them. A Halv pane must not report to Orca.
fn is_orca_variable(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with("ORCA_"))
}

fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.clamp(MIN_COLS, MAX_COLS),
        rows: rows.clamp(MIN_ROWS, MAX_ROWS),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn handle_attach(
    stream: &UnixStream,
    state: &SupervisorState,
    id: &str,
    from_sequence: Option<u64>,
) -> Result<(), String> {
    let session = match state.session(id) {
        Ok(session) => session,
        Err(error) => {
            send_error(stream, &error)?;
            return Ok(());
        }
    };
    let subscriber_id = state.next_subscriber.fetch_add(1, Ordering::SeqCst);
    let (sender, receiver) = mpsc::channel();
    let (snapshot, exit, snapshot_sequence) = {
        let mut inner = session
            .inner
            .lock()
            .expect("terminal session lock poisoned");
        let (sequence, scrollback) = inner.output.snapshot(from_sequence);
        let snapshot = ResponseFrame::Snapshot {
            session: inner.summary.clone(),
            sequence,
            scrollback: encode_base64(scrollback.as_bytes()),
        };
        let exit = if inner.summary.status == SessionStatus::Exited {
            Some(ResponseFrame::Exit {
                sequence,
                exit_code: inner.summary.exit_code,
                signal: inner.summary.signal.clone(),
            })
        } else {
            inner.subscribers.push(Subscriber {
                id: subscriber_id,
                sender,
            });
            None
        };
        (snapshot, exit, sequence)
    };

    if send_response(stream, &snapshot).is_err() {
        remove_subscriber(&session, subscriber_id);
        return Ok(());
    }
    if let Some(exit) = exit {
        let _ = send_response(stream, &exit);
        return Ok(());
    }
    let _subscriber_guard = SubscriberGuard {
        session: session.clone(),
        id: subscriber_id,
    };
    attach_event_loop(stream, receiver, id, snapshot_sequence)
}

struct SubscriberGuard {
    session: Arc<Session>,
    id: u64,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        remove_subscriber(&self.session, self.id);
    }
}

fn remove_subscriber(session: &Session, id: u64) {
    session
        .inner
        .lock()
        .expect("terminal session lock poisoned")
        .subscribers
        .retain(|subscriber| subscriber.id != id);
}

fn attach_event_loop(
    stream: &UnixStream,
    receiver: Receiver<ResponseFrame>,
    session_id: &str,
    mut last_delivered_sequence: u64,
) -> Result<(), String> {
    loop {
        match receiver.recv_timeout(ATTACH_POLL_INTERVAL) {
            Ok(frame) => {
                let is_exit = matches!(frame, ResponseFrame::Exit { .. });
                let sequence = match &frame {
                    ResponseFrame::Data { sequence, .. } | ResponseFrame::Exit { sequence, .. } => {
                        Some(*sequence)
                    }
                    _ => None,
                };
                if let Err(error) = send_response(stream, &frame) {
                    log_attach_write_error(session_id, &error, last_delivered_sequence);
                    return Ok(());
                }
                if let Some(sequence) = sequence {
                    last_delivered_sequence = sequence;
                }
                if is_exit {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => match peer_state(stream)? {
                PeerState::Open => {}
                PeerState::Closed => return Ok(()),
                PeerState::SentData => {
                    let _ = send_error(
                        stream,
                        "failed to handle attach request: control requests are not accepted",
                    );
                    return Ok(());
                }
            },
        }
    }
}

fn log_attach_write_error(session_id: &str, error: &str, last_delivered_sequence: u64) {
    eprintln!(
        "supervisor: failed to send attach response for session `{session_id}`: {error}; last delivered sequence {last_delivered_sequence}"
    );
}

enum PeerState {
    Open,
    Closed,
    SentData,
}

fn peer_state(stream: &UnixStream) -> Result<PeerState, String> {
    let mut byte = 0_u8;
    let result = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            (&raw mut byte).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if result == 0 {
        return Ok(PeerState::Closed);
    }
    if result > 0 {
        return Ok(PeerState::SentData);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    ) {
        Ok(PeerState::Open)
    } else {
        Err(format!("failed to inspect terminal attach client: {error}"))
    }
}

fn read_request(reader: &mut BufReader<UnixStream>) -> Result<RequestFrame, String> {
    read_request_optional(reader)?
        .ok_or_else(|| "failed to read supervisor request: connection closed".to_string())
}

fn read_request_optional(
    reader: &mut BufReader<UnixStream>,
) -> Result<Option<RequestFrame>, String> {
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|error| format!("failed to read supervisor request: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|error| format!("failed to decode supervisor request: {error}"))
}

fn send_error(stream: &UnixStream, message: &str) -> Result<(), String> {
    send_response(
        stream,
        &ResponseFrame::Error {
            message: message.to_string(),
        },
    )
}

fn send_response(mut stream: &UnixStream, frame: &ResponseFrame) -> Result<(), String> {
    serde_json::to_writer(&mut stream, frame)
        .map_err(|error| format!("failed to encode supervisor response: {error}"))?;
    stream
        .write_all(b"\n")
        .and_then(|()| stream.flush())
        .map_err(|error| format!("failed to send supervisor response: {error}"))
}

fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_seconds = duration.as_secs() as i64;
    let days = total_seconds.div_euclid(86_400);
    let day_seconds = total_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_size_is_bounded() {
        assert_eq!(terminal_size(1, 1).cols, 20);
        assert_eq!(terminal_size(1, 1).rows, 5);
        assert_eq!(terminal_size(u16::MAX, u16::MAX).cols, 500);
        assert_eq!(terminal_size(u16::MAX, u16::MAX).rows, 300);
    }

    #[test]
    fn bursty_subscriber_preserves_every_frame() {
        let (sender, receiver) = mpsc::channel();
        let mut subscribers = vec![Subscriber { id: 1, sender }];

        for sequence in 1..=1_024 {
            send_to_subscribers(
                &mut subscribers,
                &ResponseFrame::Data {
                    sequence,
                    data: format!("frame-{sequence}"),
                },
            );
        }

        assert_eq!(subscribers.len(), 1);
        for expected in 1..=1_024 {
            assert!(matches!(
                receiver.try_recv(),
                Ok(ResponseFrame::Data { sequence, data })
                    if sequence == expected && data == format!("frame-{expected}")
            ));
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn unix_epoch_formats_as_rfc3339() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_324), (2025, 8, 24));
    }
}
