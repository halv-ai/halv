#![cfg(unix)]

use halv_supervisor::{
    server::InProcessSupervisor, AttachEvents, Event, OpenRequest, SessionStatus, Snapshot,
    SupervisorClient,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static NEXT_TEST: AtomicU64 = AtomicU64::new(1);
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

struct TestServer {
    backend: Option<Backend>,
    directory: PathBuf,
    socket: PathBuf,
    log: PathBuf,
    session_pids: Vec<u32>,
}

enum Backend {
    Process(Child),
    InProcess(InProcessSupervisor),
}

impl TestServer {
    fn start() -> Self {
        Self::start_with_env(&[])
    }

    /// Starts a server process with extra variables in its environment.
    fn start_with_env(extra_env: &[(&str, &str)]) -> Self {
        let unique = NEXT_TEST.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            PathBuf::from("/tmp").join(format!("halv-sup-{}-{unique}-{nanos}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let socket = directory.join("supervisor.sock");
        let log = directory.join("supervisor.log");
        let probe = directory.join("probe.sock");
        let backend = match UnixListener::bind(&probe) {
            Ok(listener) => {
                drop(listener);
                fs::remove_file(&probe).unwrap();
                Backend::Process(
                    Command::new(env!("CARGO_BIN_EXE_halv-supervisor"))
                        .arg("--socket")
                        .arg(&socket)
                        .arg("--log")
                        .arg(&log)
                        .arg("--idle-timeout-secs")
                        .arg("60")
                        .envs(extra_env.iter().copied())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap(),
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Backend::InProcess(InProcessSupervisor::new())
            }
            Err(error) => panic!("failed to probe Unix socket support: {error}"),
        };
        let mut server = Self {
            backend: Some(backend),
            directory,
            socket,
            log,
            session_pids: Vec::new(),
        };
        if matches!(server.backend, Some(Backend::Process(_))) {
            server.wait_until_ready();
        }
        server
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            if SupervisorClient::connect(&self.socket, "integration-test").is_ok() {
                return;
            }
            let Backend::Process(child) = self.backend.as_mut().unwrap() else {
                unreachable!();
            };
            if let Some(status) = child.try_wait().unwrap() {
                let log = fs::read_to_string(&self.log).unwrap_or_default();
                panic!("supervisor exited during startup with {status}: {log}");
            }
            assert!(
                Instant::now() < deadline,
                "supervisor did not create its socket"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn connect(&self) -> SupervisorClient {
        match self.backend.as_ref().unwrap() {
            Backend::Process(_) => {
                SupervisorClient::connect(&self.socket, "integration-test").unwrap()
            }
            Backend::InProcess(supervisor) => supervisor.connect("integration-test").unwrap(),
        }
    }

    fn attach(&self, id: &str) -> (Snapshot, AttachEvents) {
        match self.backend.as_ref().unwrap() {
            Backend::Process(_) => {
                SupervisorClient::attach(&self.socket, "integration-test", id).unwrap()
            }
            Backend::InProcess(supervisor) => supervisor.attach("integration-test", id).unwrap(),
        }
    }

    fn raw_connection(&self) -> UnixStream {
        match self.backend.as_ref().unwrap() {
            Backend::Process(_) => UnixStream::connect(&self.socket).unwrap(),
            Backend::InProcess(supervisor) => supervisor.raw_connection().unwrap(),
        }
    }

    fn open_shell(&mut self, client: &mut SupervisorClient, id: &str) {
        let session = client
            .open(OpenRequest {
                id: id.to_string(),
                program: "/bin/sh".to_string(),
                args: Vec::new(),
                cwd: "/tmp".to_string(),
                env: Vec::new(),
                env_remove: Vec::new(),
                cols: 80,
                rows: 24,
                meta: json!({"test": id}),
            })
            .unwrap();
        assert_eq!(session.status, SessionStatus::Running);
        self.session_pids.push(session.pid);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Ok(mut client) = match self.backend.as_ref() {
            Some(Backend::Process(_)) => {
                SupervisorClient::connect(&self.socket, "integration-test")
            }
            Some(Backend::InProcess(supervisor)) => supervisor.connect("integration-test"),
            None => return,
        } {
            let _ = client.shutdown(true);
        }
        if let Some(Backend::Process(child)) = self.backend.as_mut() {
            let deadline = Instant::now() + TEST_TIMEOUT;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ => thread::sleep(Duration::from_millis(25)),
                }
            }
        }
        for pid in &self.session_pids {
            unsafe {
                libc::kill(-(*pid as libc::pid_t), libc::SIGHUP);
                libc::kill(-(*pid as libc::pid_t), libc::SIGKILL);
            }
        }
        if let Some(Backend::Process(mut child)) = self.backend.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn open_write_attach_close_and_remove() {
    let mut server = TestServer::start();
    let mut client = server.connect();
    server.open_shell(&mut client, "shell-main");
    client
        .write("shell-main", b"printf 'HALV_OK\\n'\n")
        .unwrap();

    let (snapshot, events) = server.attach("shell-main");
    if !snapshot.scrollback.contains("HALV_OK") {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for event in events {
                match event {
                    Ok(Event::Data { data, .. }) if data.contains("HALV_OK") => {
                        let _ = sender.send(());
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => panic!("attach stream failed: {error}"),
                }
            }
        });
        receiver
            .recv_timeout(TEST_TIMEOUT)
            .expect("attach stream did not contain HALV_OK");
    }

    let closed = client.close("shell-main").unwrap();
    assert_eq!(closed.status, SessionStatus::Exited);
    client.remove("shell-main").unwrap();
    assert!(client.list().unwrap().is_empty());
}

#[test]
fn session_and_output_survive_control_client_disconnect() {
    let mut server = TestServer::start();
    let mut client = server.connect();
    server.open_shell(&mut client, "shell-disconnect");
    client.write("shell-disconnect", b"stty -echo\n").unwrap();
    thread::sleep(Duration::from_millis(100));
    client
        .write("shell-disconnect", b"sleep 0.3; printf 'OFFLINE_OK\\n'\n")
        .unwrap();
    drop(client);
    thread::sleep(Duration::from_millis(700));

    let mut reconnected = server.connect();
    let sessions = reconnected.list().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "shell-disconnect");
    assert_eq!(sessions[0].status, SessionStatus::Running);
    let (snapshot, _events) = server.attach("shell-disconnect");
    assert!(
        snapshot.scrollback.contains("OFFLINE_OK"),
        "offline output was missing from {:?}",
        snapshot.scrollback
    );
    reconnected.close("shell-disconnect").unwrap();
    reconnected.remove("shell-disconnect").unwrap();
}

#[test]
fn hello_protocol_mismatch_returns_an_error_frame() {
    let server = TestServer::start();
    let mut stream = server.raw_connection();
    stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    stream
        .write_all(b"{\"type\":\"hello\",\"proto\":999,\"app_version\":\"test\"}\n")
        .unwrap();
    stream.flush().unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["type"], "error");
    assert!(frame["message"].as_str().unwrap().contains("protocol"));
}

fn send_line(stream: &mut UnixStream, line: &str) {
    stream.write_all(line.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
}

fn read_frame(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap() == 0 {
        return None;
    }
    Some(serde_json::from_str(&line).unwrap())
}

#[test]
fn hello_from_a_protocol_1_app_negotiates_the_baseline() {
    // Halv 0.7.1 sends this hello. It has no max_proto field.
    let server = TestServer::start();
    let mut stream = server.raw_connection();
    stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send_line(
        &mut stream,
        r#"{"type":"hello","proto":1,"app_version":"0.7.1"}"#,
    );
    let hello = read_frame(&mut reader).expect("the supervisor should answer the hello");
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["proto"], 1);
    assert_eq!(hello["max_proto"], halv_supervisor::MAX_PROTOCOL_VERSION);

    send_line(&mut stream, r#"{"type":"list"}"#);
    let list = read_frame(&mut reader).expect("the connection should stay open");
    assert_eq!(list["type"], "list");
}

#[test]
fn a_new_client_negotiates_with_the_supervisor() {
    let server = TestServer::start();
    let client = server.connect();
    assert_eq!(
        client.negotiated_protocol(),
        halv_supervisor::MAX_PROTOCOL_VERSION
    );
}

#[test]
fn an_unknown_request_closes_only_its_own_connection() {
    let server = TestServer::start();
    let mut stream = server.raw_connection();
    stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send_line(
        &mut stream,
        r#"{"type":"hello","proto":1,"app_version":"0.7.1","max_proto":1}"#,
    );
    assert_eq!(read_frame(&mut reader).unwrap()["type"], "hello");
    send_line(&mut stream, r#"{"type":"future_request","id":"t1"}"#);
    let error = read_frame(&mut reader).expect("the supervisor should report the error");
    assert_eq!(error["type"], "error");
    assert!(
        read_frame(&mut reader).is_none(),
        "the connection should close"
    );

    let mut other = server.connect();
    assert!(other.list().unwrap().is_empty());
}

/// Collects the output of a session until it contains `marker`.
fn output_until(server: &TestServer, id: &str, marker: &str) -> String {
    let (snapshot, events) = server.attach(id);
    let mut output = snapshot.scrollback;
    let deadline = Instant::now() + TEST_TIMEOUT;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for event in events {
            match event {
                Ok(Event::Data { data, .. }) => {
                    if sender.send(data).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
    while !output.contains(marker) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(data) => output.push_str(&data),
            Err(_) => panic!("session output did not contain {marker}: {output:?}"),
        }
    }
    output
}

#[test]
fn sessions_do_not_inherit_orca_variables() {
    let mut server = TestServer::start_with_env(&[
        ("ORCA_PANE_KEY", "tab-1:leaf-1"),
        ("ORCA_AGENT_HOOK_PORT", "4242"),
        ("HALV_SUPERVISOR_TEST_MARK", "kept"),
    ]);
    if !matches!(server.backend, Some(Backend::Process(_))) {
        // The in-process fallback shares the environment of the test runner.
        return;
    }
    let mut client = server.connect();
    let session = client
        .open(OpenRequest {
            id: "env-check".to_string(),
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "env; printf 'ENV_%s\\n' DONE; sleep 5".to_string(),
            ],
            cwd: "/tmp".to_string(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cols: 200,
            rows: 50,
            meta: json!({"test": "env-check"}),
        })
        .unwrap();
    server.session_pids.push(session.pid);
    let output = output_until(&server, "env-check", "ENV_DONE");
    assert!(
        output.contains("HALV_SUPERVISOR_TEST_MARK=kept"),
        "{output}"
    );
    assert!(!output.contains("ORCA_"), "{output}");
    client.close("env-check").unwrap();
}

#[test]
fn the_agent_hook_helper_prints_nothing_but_an_empty_object() {
    let unique = NEXT_TEST.fetch_add(1, Ordering::SeqCst);
    let directory = std::env::temp_dir().join(format!(
        "halv-agent-hook-{}-{unique}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pane = "0f9e8d7c-6b5a-4f3e-8d2c-1b0a9f8e7d6c";
    let run = |env: &[(&str, &std::ffi::OsStr)], payload: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_halv-supervisor"))
            .args(["agent-hook", "claude"])
            .env_clear()
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let payload = r#"{"session_id":"cli-1","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"cargo test"}}"#;

    let output = run(
        &[
            ("HALV_AGENT_SESSION_ID", pane.as_ref()),
            ("HALV_AGENT_STATUS_DIR", directory.as_os_str()),
        ],
        payload,
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    assert!(output.stderr.is_empty());
    let journal = fs::read_to_string(directory.join(format!("{pane}.jsonl"))).unwrap();
    let lines = journal.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let record = serde_json::from_str::<Value>(lines[0]).unwrap();
    assert_eq!(record["event"], "PermissionRequest");
    assert_eq!(record["pane"], pane);
    assert_eq!(record["command"], "cargo test");

    // Without the pane variables the helper writes nothing.
    let output = run(&[], payload);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    let output = run(
        &[
            ("HALV_AGENT_SESSION_ID", pane.as_ref()),
            ("HALV_AGENT_STATUS_DIR", directory.as_os_str()),
        ],
        "not json",
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    let journal = fs::read_to_string(directory.join(format!("{pane}.jsonl"))).unwrap();
    assert_eq!(journal.lines().count(), 1);
    let _ = fs::remove_dir_all(&directory);
}
