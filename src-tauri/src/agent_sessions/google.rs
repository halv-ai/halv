use super::{
    adapter::{
        AdapterProcess, AdapterState, AgentAdapter, AgentProbe, CanonicalMode,
        InteractiveLaunchContext, InteractiveTranscript, InteractiveTranscriptDiscovery,
        InteractiveTranscriptQuery, LaunchContext, ParsedTurn, PermissionFlags, TurnKind,
    },
    cli_binary::{self, HostOs},
    AgentEvent, AgentKind, LaunchSpec, SessionLane, TurnUsage,
};
use crate::host_env;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::SystemTime,
};

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

const GEMINI_BINARY_NAME: &str = "gemini";
const ANTIGRAVITY_BINARY_NAME: &str = "agy";
const GEMINI_JSONL_METADATA_MAX_BYTES: u64 = 64 * 1024;
const GEMINI_LEGACY_JSON_MAX_BYTES: u64 = 8 * 1024 * 1024;
const ANTIGRAVITY_DISCOVERY_LIMITATION: &str = "Antigravity only documents a latest-conversation ID per workspace. Its cache has no per-entry start time or transcript path, so Halv cannot prove which pane owns the ID.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoogleCli {
    Gemini,
    Antigravity,
}

impl GoogleCli {
    fn agent_kind(self) -> AgentKind {
        match self {
            Self::Gemini => AgentKind::Gemini,
            Self::Antigravity => AgentKind::Antigravity,
        }
    }

    fn binary_name(self) -> &'static str {
        match self {
            Self::Gemini => GEMINI_BINARY_NAME,
            Self::Antigravity => ANTIGRAVITY_BINARY_NAME,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Gemini => "Gemini CLI",
            Self::Antigravity => "Antigravity CLI",
        }
    }
}

pub(super) struct GoogleAdapter {
    cli: GoogleCli,
}

pub(super) static GEMINI_ADAPTER: GoogleAdapter = GoogleAdapter {
    cli: GoogleCli::Gemini,
};
pub(super) static ANTIGRAVITY_ADAPTER: GoogleAdapter = GoogleAdapter {
    cli: GoogleCli::Antigravity,
};

pub(super) struct GoogleAdapterState {
    cli: GoogleCli,
    parser: GoogleEventParser,
    launch_config: Option<GoogleLaunchConfig>,
}

struct GoogleLaunchConfig {
    cli: GoogleCli,
    binary: PathBuf,
    cwd: PathBuf,
    permission_mode: CanonicalMode,
    model: Option<String>,
    effort: Option<String>,
}

impl AgentAdapter for GoogleAdapter {
    fn kind(&self) -> AgentKind {
        self.cli.agent_kind()
    }

    fn probe(&self) -> AgentProbe {
        probe(self.cli)
    }

    fn binary(&self) -> Option<PathBuf> {
        find_binary(self.cli)
    }

    fn permission_flags(&self, mode: CanonicalMode) -> Result<PermissionFlags, String> {
        Ok(permission_flags(self.cli, mode))
    }

    fn login_launch_spec(&self) -> Result<LaunchSpec, String> {
        let program = find_binary(self.cli).ok_or_else(|| {
            format!(
                "{} was not found in PATH or a supported install location",
                self.cli.display_name()
            )
        })?;
        Ok(LaunchSpec {
            program,
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
        })
    }

    fn interactive_launch_spec(
        &self,
        ctx: &InteractiveLaunchContext<'_>,
    ) -> Result<LaunchSpec, String> {
        validate_lane(self.cli, ctx.lane)?;
        let program = find_binary(self.cli).ok_or_else(|| {
            format!(
                "{} was not found in PATH or a supported install location",
                self.cli.display_name()
            )
        })?;
        Ok(interactive_launch_spec_with_program(
            self.cli,
            program,
            ctx.permission_mode,
            ctx.resume_cli_session_id,
            ctx.model,
            None,
        ))
    }

    fn start(
        &self,
        ctx: &LaunchContext<'_>,
        state: &mut AdapterState,
    ) -> Result<AdapterProcess, String> {
        let AdapterState::Google(state) = state else {
            return Err(format!(
                "{} received incompatible session state",
                self.cli.display_name()
            ));
        };
        validate_lane(self.cli, ctx.lane)?;
        let binary = find_binary(self.cli).ok_or_else(|| {
            format!(
                "{} was not found in PATH or a supported install location",
                self.cli.display_name()
            )
        })?;
        state.launch_config = Some(GoogleLaunchConfig {
            cli: self.cli,
            binary,
            cwd: ctx.cwd.to_path_buf(),
            permission_mode: ctx.permission_mode,
            model: trimmed_owned(ctx.model),
            effort: trimmed_owned(ctx.effort),
        });
        Ok(AdapterProcess::deferred())
    }

    fn new_state(
        &self,
        resumed: bool,
        _permission_mode: CanonicalMode,
        _started_at: std::time::Instant,
        _initializing: bool,
    ) -> AdapterState {
        AdapterState::Google(GoogleAdapterState {
            cli: self.cli,
            parser: GoogleEventParser::new(self.cli, resumed),
            launch_config: None,
        })
    }

    fn parse_line(&self, state: &mut AdapterState, line: &str) -> ParsedTurn {
        let AdapterState::Google(state) = state else {
            return ParsedTurn {
                events: vec![AgentEvent::ProtocolError {
                    message: format!(
                        "{} received incompatible parser state",
                        self.cli.display_name()
                    ),
                }],
                ..ParsedTurn::default()
            };
        };
        if state.cli != self.cli {
            return ParsedTurn {
                events: vec![AgentEvent::ProtocolError {
                    message: "Google CLI adapter state used the wrong provider".to_string(),
                }],
                ..ParsedTurn::default()
            };
        }
        ParsedTurn {
            events: state.parser.parse_line(line),
            ..ParsedTurn::default()
        }
    }

    fn encode_user_message(
        &self,
        _state: &mut AdapterState,
        _text: &str,
        turn: TurnKind<'_>,
    ) -> Result<Option<String>, String> {
        match turn {
            TurnKind::Start { .. } => Ok(None),
            TurnKind::Steer { .. } | TurnKind::Queued { .. } => Err(format!(
                "{} does not support steering an active turn",
                self.cli.display_name()
            )),
        }
    }

    fn start_turn(
        &self,
        state: &mut AdapterState,
        text: &str,
        resume_cli_session_id: Option<&str>,
    ) -> Result<AdapterProcess, String> {
        let AdapterState::Google(state) = state else {
            return Err(format!(
                "{} received incompatible session state",
                self.cli.display_name()
            ));
        };
        let config = state.launch_config.as_ref().ok_or_else(|| {
            format!(
                "the {} launch configuration is unavailable",
                self.cli.display_name()
            )
        })?;
        AdapterProcess::per_turn(
            config.spawn_turn(text, resume_cli_session_id)?,
            self.cli.display_name(),
        )
    }

    fn is_per_turn(&self) -> bool {
        true
    }

    fn assigned_interactive_session_id(&self, _resume_cli_session_id: Option<&str>) -> bool {
        false
    }

    fn accepts_initial_prompt(&self) -> bool {
        self.cli == GoogleCli::Gemini
    }

    fn initial_prompt_args(&self, prompt: &str) -> Vec<OsString> {
        match self.cli {
            GoogleCli::Gemini => vec!["--prompt-interactive".into(), prompt.into()],
            GoogleCli::Antigravity => Vec::new(),
        }
    }

    fn discover_interactive_transcripts(
        &self,
        query: InteractiveTranscriptQuery<'_>,
    ) -> Result<InteractiveTranscriptDiscovery, String> {
        match self.cli {
            GoogleCli::Gemini => Ok(InteractiveTranscriptDiscovery::Found(
                discover_gemini_transcripts(query),
            )),
            GoogleCli::Antigravity => Ok(InteractiveTranscriptDiscovery::Unsupported(
                ANTIGRAVITY_DISCOVERY_LIMITATION,
            )),
        }
    }
}

impl GoogleLaunchConfig {
    fn spawn_turn(
        &self,
        prompt: &str,
        resume_cli_session_id: Option<&str>,
    ) -> Result<Child, String> {
        let mut command = headless_command(
            self.cli,
            &self.binary,
            &self.cwd,
            self.permission_mode,
            prompt,
            resume_cli_session_id,
            self.model.as_deref(),
            self.effort.as_deref(),
        );
        command
            .spawn()
            .map_err(|error| format!("failed to start {}: {error}", self.cli.display_name()))
    }
}

fn trimmed_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_lane(cli: GoogleCli, lane: &SessionLane) -> Result<(), String> {
    if matches!(lane, SessionLane::Custom { env } if env.is_empty()) {
        return Ok(());
    }
    Err(format!(
        "the {} provider requires its built-in account lane",
        cli.display_name()
    ))
}

fn permission_flags(cli: GoogleCli, mode: CanonicalMode) -> PermissionFlags {
    let mut flags = PermissionFlags::empty();
    match (cli, mode) {
        (GoogleCli::Gemini, CanonicalMode::ReadOnly | CanonicalMode::Never) => {
            flags.args.extend(["--approval-mode".into(), "plan".into()]);
        }
        (GoogleCli::Gemini, CanonicalMode::SkipAll) => {
            flags.args.extend(["--approval-mode".into(), "yolo".into()]);
        }
        (GoogleCli::Gemini, CanonicalMode::OnFailure) => {
            flags
                .args
                .extend(["--approval-mode".into(), "auto_edit".into()]);
        }
        (GoogleCli::Antigravity, CanonicalMode::ReadOnly | CanonicalMode::Never) => {
            flags.args.push("--mode=plan".into());
        }
        (GoogleCli::Antigravity, CanonicalMode::SkipAll) => {
            flags.args.push("--dangerously-skip-permissions".into());
        }
        (GoogleCli::Antigravity, CanonicalMode::OnFailure) => {
            flags.args.push("--mode=accept-edits".into());
        }
        (_, CanonicalMode::Ask | CanonicalMode::OnRequest) => {}
    }
    flags
}

fn interactive_launch_spec_with_program(
    cli: GoogleCli,
    program: PathBuf,
    permission_mode: CanonicalMode,
    resume_cli_session_id: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> LaunchSpec {
    let mut args = permission_flags(cli, permission_mode).args;
    add_resume_args(&mut args, cli, resume_cli_session_id);
    add_model_args(&mut args, cli, model, effort);
    LaunchSpec {
        program,
        args,
        env: Vec::new(),
        env_remove: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn headless_command(
    cli: GoogleCli,
    binary: &Path,
    cwd: &Path,
    permission_mode: CanonicalMode,
    prompt: &str,
    resume_cli_session_id: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Command {
    let mut command = Command::new(binary);
    command.args(permission_flags(cli, permission_mode).args);
    let mut args = Vec::new();
    add_resume_args(&mut args, cli, resume_cli_session_id);
    add_model_args(&mut args, cli, model, effort);
    command
        .args(args)
        .arg("--prompt")
        .arg(prompt)
        .arg("--output-format")
        .arg("stream-json")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    command
}

fn add_resume_args(args: &mut Vec<OsString>, cli: GoogleCli, session_id: Option<&str>) {
    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    match cli {
        GoogleCli::Gemini => args.extend(["--resume".into(), session_id.into()]),
        GoogleCli::Antigravity => {
            args.extend(["--conversation".into(), session_id.into()]);
        }
    }
}

fn add_model_args(
    args: &mut Vec<OsString>,
    cli: GoogleCli,
    model: Option<&str>,
    effort: Option<&str>,
) {
    if let Some(model) = google_model(cli, model) {
        args.extend(["--model".into(), model.into()]);
    }
    if cli == GoogleCli::Antigravity {
        if let Some(effort) = effort
            .map(str::trim)
            .filter(|value| matches!(*value, "low" | "medium" | "high"))
        {
            args.extend(["--effort".into(), effort.into()]);
        }
    }
}

fn google_model(cli: GoogleCli, model: Option<&str>) -> Option<&str> {
    let model = model.map(str::trim).filter(|value| !value.is_empty())?;
    let model = match cli {
        GoogleCli::Gemini => model
            .strip_prefix("gemini-cli/")
            .or_else(|| model.strip_prefix("gemini/"))
            .unwrap_or(model),
        GoogleCli::Antigravity => model
            .strip_prefix("antigravity/")
            .or_else(|| model.strip_prefix("agy/"))
            .unwrap_or(model),
    };
    match cli {
        GoogleCli::Gemini if model == "auto" || model.starts_with("gemini-") => Some(model),
        GoogleCli::Antigravity
            if model.starts_with("gemini-")
                || model.starts_with("claude-")
                || model.starts_with("gpt-oss-") =>
        {
            Some(model)
        }
        _ => None,
    }
}

fn discover_gemini_transcripts(
    query: InteractiveTranscriptQuery<'_>,
) -> Vec<InteractiveTranscript> {
    let Some(runtime_directory) = gemini_runtime_directory() else {
        return Vec::new();
    };
    discover_gemini_transcripts_in(&runtime_directory, query)
}

fn discover_gemini_transcripts_in(
    runtime_directory: &Path,
    query: InteractiveTranscriptQuery<'_>,
) -> Vec<InteractiveTranscript> {
    let Some(project_hash) = gemini_project_hash(query.cwd) else {
        return Vec::new();
    };
    let Ok(projects) = fs::read_dir(runtime_directory.join("tmp")) else {
        return Vec::new();
    };
    let mut transcripts = Vec::new();
    for project in projects.flatten() {
        let Ok(entries) = fs::read_dir(project.path().join("chats")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("json" | "jsonl")
            ) {
                continue;
            }
            let Some(transcript) = read_gemini_transcript(&path, &project_hash) else {
                continue;
            };
            if transcript.started_at >= query.started_at {
                transcripts.push(transcript);
            }
        }
    }
    transcripts.sort_by_key(|transcript| transcript.started_at);
    transcripts.dedup_by(|left, right| left.cli_session_id == right.cli_session_id);
    transcripts
}

fn read_gemini_transcript(path: &Path, project_hash: &str) -> Option<InteractiveTranscript> {
    let metadata = read_gemini_metadata(path)?;
    if metadata.get("projectHash").and_then(Value::as_str) != Some(project_hash) {
        return None;
    }
    let cli_session_id = metadata
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let started_at = metadata
        .get("startTime")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_system_time)?;
    let modified_at = metadata
        .get("lastUpdated")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_system_time)
        .or_else(|| fs::metadata(path).ok()?.modified().ok())?;
    Some(InteractiveTranscript {
        cli_session_id,
        path: path.to_path_buf(),
        started_at,
        modified_at,
    })
}

fn read_gemini_metadata(path: &Path) -> Option<Value> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("jsonl") => {
            let file = fs::File::open(path).ok()?;
            let mut first_line = String::new();
            let bytes_read = BufReader::new(file)
                .take(GEMINI_JSONL_METADATA_MAX_BYTES + 1)
                .read_line(&mut first_line)
                .ok()?;
            if bytes_read == 0 || bytes_read as u64 > GEMINI_JSONL_METADATA_MAX_BYTES {
                return None;
            }
            serde_json::from_str(&first_line).ok()
        }
        Some("json") => {
            let file = fs::File::open(path).ok()?;
            let mut document = Vec::new();
            file.take(GEMINI_LEGACY_JSON_MAX_BYTES + 1)
                .read_to_end(&mut document)
                .ok()?;
            if document.is_empty() || document.len() as u64 > GEMINI_LEGACY_JSON_MAX_BYTES {
                return None;
            }
            serde_json::from_slice(&document).ok()
        }
        _ => None,
    }
}

pub(super) fn gemini_project_hash(cwd: &Path) -> Option<String> {
    let cwd = cwd.to_str()?;
    let mut hasher = Sha256::new();
    hasher.update(cwd.as_bytes());
    Some(crate::lower_hex::lower_hex(&hasher.finalize()))
}

fn gemini_runtime_directory() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(runtime_directory) = test_gemini_runtime_directory() {
        return Some(runtime_directory);
    }
    let home = env::var_os("GEMINI_CLI_HOME")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)?;
    if env::var("SANDBOX").ok().as_deref() == Some("sandbox-exec") {
        Some(home.join(".cache/.gemini"))
    } else {
        Some(home.join(".gemini"))
    }
}

#[cfg(test)]
static TEST_GEMINI_RUNTIME_DIRECTORY: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
#[cfg(test)]
static TEST_GEMINI_RUNTIME_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
fn test_gemini_runtime_directory() -> Option<PathBuf> {
    TEST_GEMINI_RUNTIME_DIRECTORY
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .clone()
}

#[cfg(test)]
pub(super) fn with_test_gemini_runtime_directory<T>(
    runtime_directory: PathBuf,
    operation: impl FnOnce() -> T,
) -> T {
    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            if let Ok(mut value) = TEST_GEMINI_RUNTIME_DIRECTORY
                .get_or_init(Default::default)
                .lock()
            {
                *value = None;
            }
        }
    }

    let _serial = TEST_GEMINI_RUNTIME_SERIAL
        .get_or_init(Default::default)
        .lock()
        .expect("Gemini runtime override should serialize");
    {
        let mut value = TEST_GEMINI_RUNTIME_DIRECTORY
            .get_or_init(Default::default)
            .lock()
            .expect("Gemini runtime override should lock");
        assert!(value.is_none(), "Gemini runtime override is already active");
        *value = Some(runtime_directory);
    }
    let reset = Reset;
    let result = operation();
    drop(reset);
    result
}

fn parse_rfc3339_system_time(value: &str) -> Option<SystemTime> {
    let parsed = DateTime::parse_from_rfc3339(value).ok()?;
    Some(parsed.with_timezone(&Utc).into())
}

fn probe(cli: GoogleCli) -> AgentProbe {
    let Some(binary) = find_binary(cli) else {
        return AgentProbe {
            found: false,
            binary: None,
            version: None,
            error: None,
        };
    };
    let binary_text = binary.to_string_lossy().into_owned();
    let mut command = Command::new(&binary);
    command.arg("--version");
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    match command.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let version = if stdout.is_empty() {
                (!stderr.is_empty()).then_some(stderr.clone())
            } else {
                Some(stdout)
            };
            AgentProbe {
                found: true,
                binary: Some(binary_text),
                version,
                error: (!output.status.success()).then(|| {
                    if stderr.is_empty() {
                        format!(
                            "{} --version exited with {}",
                            cli.binary_name(),
                            output.status
                        )
                    } else {
                        stderr
                    }
                }),
            }
        }
        Err(error) => {
            let error = format!("failed to run {binary_text} --version: {error}");
            AgentProbe {
                found: true,
                binary: Some(binary_text),
                version: None,
                error: Some(error),
            }
        }
    }
}

pub(super) fn gemini_login_status() -> Option<bool> {
    login_status(GoogleCli::Gemini)
}

pub(super) fn antigravity_login_status() -> Option<bool> {
    login_status(GoogleCli::Antigravity)
}

fn login_status(cli: GoogleCli) -> Option<bool> {
    find_binary(cli)?;
    let home = home_directory()?;
    match cli {
        GoogleCli::Gemini => gemini_login_status_from_home(&home),
        GoogleCli::Antigravity => {
            Some(home.join(".gemini/antigravity-cli/settings.json").is_file())
        }
    }
}

fn gemini_login_status_from_home(home: &Path) -> Option<bool> {
    let settings = read_json(&home.join(".gemini/settings.json"));
    let selected_type = settings
        .as_ref()
        .and_then(|value| value.pointer("/security/auth/selectedType"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            env::var("GEMINI_DEFAULT_AUTH_TYPE")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
    let has_api_key = ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
        .iter()
        .any(|name| env::var(name).is_ok_and(|value| !value.trim().is_empty()));
    let oauth_credentials = read_json(&home.join(".gemini/oauth_creds.json"));
    let has_oauth_credentials = oauth_credentials
        .as_ref()
        .is_some_and(gemini_oauth_credentials_ready);
    let has_application_credentials = env::var("GOOGLE_APPLICATION_CREDENTIALS")
        .ok()
        .map(|value| PathBuf::from(value.trim()))
        .is_some_and(|path| path.is_file())
        || home
            .join(".config/gcloud/application_default_credentials.json")
            .is_file();
    gemini_auth_ready(
        selected_type.as_deref(),
        has_api_key,
        has_oauth_credentials,
        has_application_credentials,
    )
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
}

fn gemini_oauth_credentials_ready(value: &Value) -> bool {
    ["refresh_token", "access_token"].iter().any(|name| {
        value
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())
    })
}

fn gemini_auth_ready(
    selected_type: Option<&str>,
    has_api_key: bool,
    has_oauth_credentials: bool,
    has_application_credentials: bool,
) -> Option<bool> {
    match selected_type {
        Some("oauth-personal") => Some(has_oauth_credentials),
        Some("gemini-api-key") => Some(has_api_key),
        Some("vertex-ai" | "USE_VERTEX_AI" | "COMPUTE_ADC") => Some(has_application_credentials),
        Some("cloud-shell") => None,
        Some(_) => None,
        None => Some(false),
    }
}

fn find_binary(cli: GoogleCli) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            add_binary_candidate(&mut candidates, &directory, cli.binary_name());
        }
    }
    if let Some(home) = home_directory() {
        for directory in [
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join(".volta/bin"),
            home.join(".local/share/mise/shims"),
            home.join(".asdf/shims"),
            home.join(".bun/bin"),
            home.join("Library/pnpm"),
        ] {
            add_binary_candidate(&mut candidates, &directory, cli.binary_name());
        }
        super::claude::add_versioned_binary_candidates(
            &mut candidates,
            &home.join(".nvm/versions/node"),
            Path::new(&format!("bin/{}", cli.binary_name())),
        );
        super::claude::add_versioned_binary_candidates(
            &mut candidates,
            &home.join(".local/share/fnm/node-versions"),
            Path::new(&format!("installation/bin/{}", cli.binary_name())),
        );
    }
    for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        add_binary_candidate(&mut candidates, Path::new(directory), cli.binary_name());
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .find(|candidate| seen.insert(candidate.clone()) && cli_binary::is_executable(candidate))
}

fn add_binary_candidate(candidates: &mut Vec<PathBuf>, directory: &Path, name: &str) {
    cli_binary::push_candidates(candidates, directory, name, HostOs::current());
}

fn home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let value: Option<OsString> = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value: Option<OsString> = env::var_os("HOME");
    value.map(PathBuf::from)
}

#[derive(Default)]
struct GoogleEventParser {
    cli: Option<GoogleCli>,
    session_initialized: bool,
    emitted_text: bool,
    previous_usage: TurnUsage,
    tool_names: HashMap<String, String>,
}

impl GoogleEventParser {
    fn new(cli: GoogleCli, session_initialized: bool) -> Self {
        Self {
            cli: Some(cli),
            session_initialized,
            ..Self::default()
        }
    }

    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                let name = self.cli.map_or("Google CLI", GoogleCli::display_name);
                return vec![AgentEvent::ProtocolError {
                    message: format!("invalid {name} stream JSON: {error}"),
                }];
            }
        };
        match self.cli {
            Some(GoogleCli::Gemini) => self.parse_gemini(&value),
            Some(GoogleCli::Antigravity) => self.parse_antigravity(&value),
            None => Vec::new(),
        }
    }

    fn parse_gemini(&mut self, value: &Value) -> Vec<AgentEvent> {
        match value.get("type").and_then(Value::as_str) {
            Some("init") => self.session_init(
                value.get("session_id").and_then(Value::as_str),
                value.get("model").and_then(Value::as_str),
            ),
            Some("message") if value.get("role").and_then(Value::as_str) == Some("assistant") => {
                self.text_event(value.get("content").and_then(Value::as_str))
            }
            Some("tool_use") => {
                let name = value
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let id = value
                    .get("tool_id")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .to_string();
                self.tool_names.insert(id, name.clone());
                vec![AgentEvent::ToolCallStarted {
                    name,
                    summary_json: json_summary(
                        value.get("parameters").or_else(|| value.get("args")),
                    ),
                }]
            }
            Some("tool_result") => {
                let id = value
                    .get("tool_id")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let name = self
                    .tool_names
                    .remove(id)
                    .or_else(|| {
                        value
                            .get("tool_name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "tool".to_string());
                vec![AgentEvent::ToolCallCompleted { name }]
            }
            Some("error") => vec![AgentEvent::ProtocolError {
                message: error_message(value).unwrap_or_else(|| "Gemini CLI error".to_string()),
            }],
            Some("result") => self.finish_turn(value.get("stats"), value.get("response")),
            _ => Vec::new(),
        }
    }

    fn parse_antigravity(&mut self, value: &Value) -> Vec<AgentEvent> {
        match value.get("event").and_then(Value::as_str) {
            Some("init") => self.session_init(
                value.get("conversation_id").and_then(Value::as_str),
                value
                    .get("init")
                    .and_then(|init| init.get("model"))
                    .and_then(Value::as_str),
            ),
            Some("step_update") => {
                let step = value.get("step_update").unwrap_or(&Value::Null);
                match step.get("step_type").and_then(Value::as_str) {
                    Some("agent_response") => {
                        self.text_event(step.get("text_delta").and_then(Value::as_str))
                    }
                    Some("tool") => self.antigravity_tool_events(step),
                    _ => Vec::new(),
                }
            }
            Some("result") => {
                let result = value.get("result").unwrap_or(&Value::Null);
                let mut events = Vec::new();
                if result.get("status").and_then(Value::as_str) != Some("SUCCESS") {
                    events.push(AgentEvent::ProtocolError {
                        message: error_message(result)
                            .unwrap_or_else(|| "Antigravity CLI turn failed".to_string()),
                    });
                }
                events.extend(self.finish_turn(result.get("usage"), result.get("response")));
                events
            }
            _ => Vec::new(),
        }
    }

    fn session_init(&mut self, session_id: Option<&str>, model: Option<&str>) -> Vec<AgentEvent> {
        if self.session_initialized {
            return Vec::new();
        }
        let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) else {
            return Vec::new();
        };
        self.session_initialized = true;
        vec![AgentEvent::SessionInit {
            cli_session_id: session_id.to_string(),
            model: model.unwrap_or("Gemini").to_string(),
        }]
    }

    fn text_event(&mut self, text: Option<&str>) -> Vec<AgentEvent> {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return Vec::new();
        };
        self.emitted_text = true;
        vec![AgentEvent::TextDelta {
            text: text.to_string(),
        }]
    }

    fn antigravity_tool_events(&mut self, step: &Value) -> Vec<AgentEvent> {
        let name = step
            .get("tool_name")
            .or_else(|| step.get("tool_info").and_then(|info| info.get("name")))
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let id = step
            .get("step_index")
            .and_then(Value::as_u64)
            .map(|value| value.to_string())
            .unwrap_or_else(|| name.clone());
        let state = step.get("state").and_then(Value::as_str);
        let mut events = Vec::new();
        if !self.tool_names.contains_key(&id) {
            self.tool_names.insert(id.clone(), name.clone());
            events.push(AgentEvent::ToolCallStarted {
                name: name.clone(),
                summary_json: json_summary(
                    step.get("tool_info")
                        .and_then(|info| info.get("parameters")),
                ),
            });
        }
        if state == Some("DONE") {
            let name = self.tool_names.remove(&id).unwrap_or(name);
            events.push(AgentEvent::ToolCallCompleted { name });
        }
        events
    }

    fn finish_turn(&mut self, usage: Option<&Value>, response: Option<&Value>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if !self.emitted_text {
            if let Some(response) = response
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                events.push(AgentEvent::TextDelta {
                    text: response.to_string(),
                });
            }
        }
        let total = parse_usage(usage);
        let usage = usage_delta(&total, &self.previous_usage);
        self.previous_usage = total;
        self.emitted_text = false;
        self.tool_names.clear();
        events.push(AgentEvent::TurnCompleted { usage });
        events
    }
}

fn json_summary(value: Option<&Value>) -> String {
    value
        .map(Value::to_string)
        .unwrap_or_else(|| "{}".to_string())
}

fn error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| match error {
            Value::String(message) => Some(message.clone()),
            Value::Object(_) => error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn parse_usage(value: Option<&Value>) -> TurnUsage {
    let value = value.unwrap_or(&Value::Null);
    let models = value.get("models").and_then(Value::as_object);
    let model_total = |names: &[&str]| {
        models
            .map(|models| {
                models
                    .values()
                    .map(|model| token_value(model, names))
                    .sum::<u64>()
            })
            .unwrap_or_default()
    };
    let input_tokens = token_value(value, &["input_tokens", "prompt_tokens", "input"]);
    let output_tokens = token_value(value, &["output_tokens", "completion_tokens", "output"]);
    let cache_read_tokens = token_value(
        value,
        &[
            "cache_read_tokens",
            "cached_tokens",
            "cached_content_tokens",
        ],
    );
    TurnUsage {
        input_tokens: input_tokens.max(model_total(&[
            "input_tokens",
            "prompt_tokens",
            "prompt",
            "input",
        ])),
        output_tokens: output_tokens.max(model_total(&[
            "output_tokens",
            "completion_tokens",
            "candidates",
            "output",
        ])),
        cache_read_tokens: cache_read_tokens.max(model_total(&[
            "cache_read_tokens",
            "cached_tokens",
            "cached",
        ])),
        cache_creation_tokens: 0,
        total_cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
    }
}

fn token_value(value: &Value, names: &[&str]) -> u64 {
    names
        .iter()
        .find_map(|name| {
            value.get(name).and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.get("total").and_then(Value::as_u64))
            })
        })
        .unwrap_or_default()
}

fn usage_delta(total: &TurnUsage, previous: &TurnUsage) -> TurnUsage {
    TurnUsage {
        input_tokens: total.input_tokens.saturating_sub(previous.input_tokens),
        output_tokens: total.output_tokens.saturating_sub(previous.output_tokens),
        cache_read_tokens: total
            .cache_read_tokens
            .saturating_sub(previous.cache_read_tokens),
        cache_creation_tokens: total
            .cache_creation_tokens
            .saturating_sub(previous.cache_creation_tokens),
        total_cost_usd: total.total_cost_usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Gemini CLI names its project folders by this hash, so the text must
    // stay the same across sha2 versions.
    #[test]
    fn gemini_project_hash_is_the_lowercase_sha256_of_the_path() {
        assert_eq!(
            gemini_project_hash(Path::new("/Users/halv/project")).as_deref(),
            Some("9ab77df2d34ed610a6324a8a421f3cd172dc555cfcdeebf0f095f5d36650cf8f")
        );
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-google-{name}-{:032x}",
                rand::random::<u128>()
            ));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn at(value: &str) -> SystemTime {
        parse_rfc3339_system_time(value).expect("test timestamp should be valid")
    }

    fn write_gemini_transcript(
        runtime: &Path,
        project_directory: &str,
        cwd: &Path,
        filename: &str,
        session_id: &str,
        start_time: &str,
        last_updated: &str,
    ) -> PathBuf {
        let chats = runtime.join("tmp").join(project_directory).join("chats");
        fs::create_dir_all(&chats).expect("chat directory should be created");
        let path = chats.join(filename);
        let metadata = serde_json::json!({
            "sessionId": session_id,
            "projectHash": gemini_project_hash(cwd).expect("cwd should be UTF-8"),
            "startTime": start_time,
            "lastUpdated": last_updated,
        });
        fs::write(&path, format!("{metadata}\n{{\"id\":\"message-1\"}}\n"))
            .expect("transcript should be written");
        path
    }

    fn args(spec: &LaunchSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn gemini_interactive_launch_maps_permissions_resume_and_model() {
        let spec = interactive_launch_spec_with_program(
            GoogleCli::Gemini,
            PathBuf::from("/tmp/gemini"),
            CanonicalMode::ReadOnly,
            Some("gemini-session"),
            Some("gemini-cli/gemini-3-pro-preview"),
            None,
        );
        assert_eq!(
            args(&spec),
            [
                "--approval-mode",
                "plan",
                "--resume",
                "gemini-session",
                "--model",
                "gemini-3-pro-preview",
            ]
        );
    }

    #[test]
    fn google_launch_ignores_a_parent_codex_model() {
        let gemini = interactive_launch_spec_with_program(
            GoogleCli::Gemini,
            PathBuf::from("/tmp/gemini"),
            CanonicalMode::Ask,
            None,
            Some("gpt-5.5"),
            None,
        );
        let antigravity = interactive_launch_spec_with_program(
            GoogleCli::Antigravity,
            PathBuf::from("/tmp/agy"),
            CanonicalMode::Ask,
            None,
            Some("codex-cli/gpt-5.5"),
            None,
        );
        assert!(args(&gemini).is_empty());
        assert!(args(&antigravity).is_empty());
    }

    #[test]
    fn antigravity_strips_its_conversation_model_prefix() {
        let spec = interactive_launch_spec_with_program(
            GoogleCli::Antigravity,
            PathBuf::from("/tmp/agy"),
            CanonicalMode::Ask,
            None,
            Some("antigravity/claude-sonnet-4-6"),
            None,
        );
        assert_eq!(args(&spec), ["--model", "claude-sonnet-4-6"]);
    }

    #[test]
    fn gemini_uses_the_documented_interactive_prompt_flag() {
        assert_eq!(
            GEMINI_ADAPTER
                .initial_prompt_args("inspect this project")
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>(),
            ["--prompt-interactive", "inspect this project"]
        );
        assert!(ANTIGRAVITY_ADAPTER
            .initial_prompt_args("inspect this project")
            .is_empty());
    }

    #[test]
    fn antigravity_interactive_launch_maps_full_access_and_resume() {
        let spec = interactive_launch_spec_with_program(
            GoogleCli::Antigravity,
            PathBuf::from("/tmp/agy"),
            CanonicalMode::SkipAll,
            Some("agy-session"),
            Some("default"),
            Some("high"),
        );
        assert_eq!(
            args(&spec),
            [
                "--dangerously-skip-permissions",
                "--conversation",
                "agy-session",
                "--effort",
                "high",
            ]
        );
    }

    #[test]
    fn google_clis_map_auto_edit_permissions() {
        assert_eq!(
            permission_flags(GoogleCli::Gemini, CanonicalMode::OnFailure).args,
            ["--approval-mode", "auto_edit"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            permission_flags(GoogleCli::Antigravity, CanonicalMode::OnFailure).args,
            [OsString::from("--mode=accept-edits")]
        );
    }

    #[test]
    fn gemini_login_requires_selected_auth_and_matching_credentials() {
        assert_eq!(gemini_auth_ready(None, true, true, true), Some(false));
        assert_eq!(
            gemini_auth_ready(Some("oauth-personal"), false, false, false),
            Some(false)
        );
        assert_eq!(
            gemini_auth_ready(Some("oauth-personal"), false, true, false),
            Some(true)
        );
        assert_eq!(
            gemini_auth_ready(Some("gemini-api-key"), true, false, false),
            Some(true)
        );
        assert_eq!(
            gemini_auth_ready(Some("vertex-ai"), false, false, true),
            Some(true)
        );
        assert_eq!(
            gemini_auth_ready(Some("future-auth"), false, false, false),
            None
        );
    }

    #[test]
    fn gemini_oauth_credentials_require_a_non_empty_token() {
        assert!(!gemini_oauth_credentials_ready(&serde_json::json!({})));
        assert!(!gemini_oauth_credentials_ready(&serde_json::json!({
            "refresh_token": "  "
        })));
        assert!(gemini_oauth_credentials_ready(&serde_json::json!({
            "refresh_token": "refresh-token"
        })));
    }

    #[test]
    fn gemini_discovery_uses_cwd_start_time_and_full_resume_id() {
        let directory = TestDirectory::new("transcript-discovery");
        let runtime = directory.path.join(".gemini");
        let cwd = directory.path.join("workspace");
        let other_cwd = directory.path.join("other-workspace");
        fs::create_dir_all(&cwd).expect("workspace should be created");
        fs::create_dir_all(&other_cwd).expect("other workspace should be created");

        let resume_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        let expected_path = write_gemini_transcript(
            &runtime,
            "workspace",
            &cwd,
            "session-2026-09-30T12-00-a1b2c3d4.jsonl",
            resume_id,
            "2026-09-30T12:00:02Z",
            "2026-09-30T12:01:00Z",
        );
        write_gemini_transcript(
            &runtime,
            "other-workspace",
            &other_cwd,
            "session-2026-09-30T12-00-bbbbbbbb.jsonl",
            "bbbbbbbb-1111-2222-3333-444444444444",
            "2026-09-30T12:00:03Z",
            "2026-09-30T12:01:00Z",
        );
        write_gemini_transcript(
            &runtime,
            "legacy-workspace-hash",
            &cwd,
            "session-2026-09-30T11-59-cccccccc.jsonl",
            "cccccccc-1111-2222-3333-444444444444",
            "2026-09-30T11:59:40Z",
            "2026-09-30T12:01:00Z",
        );

        let transcripts = discover_gemini_transcripts_in(
            &runtime,
            InteractiveTranscriptQuery {
                cwd: &cwd,
                started_at: at("2026-09-30T12:00:00Z"),
            },
        );

        assert_eq!(
            transcripts,
            [InteractiveTranscript {
                cli_session_id: resume_id.to_string(),
                path: expected_path,
                started_at: at("2026-09-30T12:00:02Z"),
                modified_at: at("2026-09-30T12:01:00Z"),
            }]
        );
    }

    #[test]
    fn gemini_discovery_reads_a_pretty_printed_legacy_json_document() {
        let directory = TestDirectory::new("legacy-json-transcript");
        let runtime = directory.path.join(".gemini");
        let cwd = directory.path.join("workspace");
        fs::create_dir_all(&cwd).expect("workspace should be created");
        let chats = runtime.join("tmp/workspace/chats");
        fs::create_dir_all(&chats).expect("chat directory should be created");
        let path = chats.join("session-2026-09-30T12-00-dddddddd.json");
        let document = serde_json::json!({
            "sessionId": "dddddddd-1111-2222-3333-444444444444",
            "projectHash": gemini_project_hash(&cwd).expect("cwd should be UTF-8"),
            "startTime": "2026-09-30T12:00:02Z",
            "lastUpdated": "2026-09-30T12:00:10Z",
            "messages": [{"id": "message-1", "type": "user", "content": "hello"}],
        });
        fs::write(
            &path,
            serde_json::to_string_pretty(&document).expect("document should serialize"),
        )
        .expect("legacy transcript should be written");

        let transcripts = discover_gemini_transcripts_in(
            &runtime,
            InteractiveTranscriptQuery {
                cwd: &cwd,
                started_at: at("2026-09-30T12:00:00Z"),
            },
        );

        assert_eq!(
            transcripts,
            [InteractiveTranscript {
                cli_session_id: "dddddddd-1111-2222-3333-444444444444".to_string(),
                path,
                started_at: at("2026-09-30T12:00:02Z"),
                modified_at: at("2026-09-30T12:00:10Z"),
            }]
        );
    }

    #[test]
    fn gemini_legacy_json_reader_enforces_its_document_limit() {
        let directory = TestDirectory::new("legacy-json-limit");
        let path = directory.path.join("oversized.json");
        fs::write(&path, vec![b' '; GEMINI_LEGACY_JSON_MAX_BYTES as usize + 1])
            .expect("oversized fixture should be written");

        assert_eq!(read_gemini_metadata(&path), None);
    }

    #[test]
    fn antigravity_reports_the_safe_discovery_limit() {
        let cwd = Path::new("/tmp/workspace");
        let result = ANTIGRAVITY_ADAPTER
            .discover_interactive_transcripts(InteractiveTranscriptQuery {
                cwd,
                started_at: SystemTime::UNIX_EPOCH,
            })
            .expect("discovery status should be available");
        assert_eq!(
            result,
            InteractiveTranscriptDiscovery::Unsupported(ANTIGRAVITY_DISCOVERY_LIMITATION)
        );
    }

    #[test]
    fn gemini_stream_maps_session_text_tools_and_usage() {
        let mut parser = GoogleEventParser::new(GoogleCli::Gemini, false);
        assert_eq!(
            parser.parse_line(r#"{"type":"init","session_id":"gemini-1","model":"gemini-3-pro"}"#,),
            [AgentEvent::SessionInit {
                cli_session_id: "gemini-1".to_string(),
                model: "gemini-3-pro".to_string(),
            }]
        );
        assert_eq!(
            parser.parse_line(
                r#"{"type":"message","role":"assistant","content":"hello","delta":true}"#,
            ),
            [AgentEvent::TextDelta {
                text: "hello".to_string(),
            }]
        );
        assert_eq!(
            parser.parse_line(
                r#"{"type":"tool_use","tool_id":"tool-1","tool_name":"read_file","parameters":{"path":"README.md"}}"#,
            ),
            [AgentEvent::ToolCallStarted {
                name: "read_file".to_string(),
                summary_json: r#"{"path":"README.md"}"#.to_string(),
            }]
        );
        assert_eq!(
            parser.parse_line(r#"{"type":"tool_result","tool_id":"tool-1"}"#),
            [AgentEvent::ToolCallCompleted {
                name: "read_file".to_string(),
            }]
        );
        assert_eq!(
            parser.parse_line(
                r#"{"type":"result","stats":{"input_tokens":120,"output_tokens":30,"cached_tokens":40}}"#,
            ),
            [AgentEvent::TurnCompleted {
                usage: TurnUsage {
                    input_tokens: 120,
                    output_tokens: 30,
                    cache_read_tokens: 40,
                    ..TurnUsage::default()
                },
            }]
        );
    }

    #[test]
    fn antigravity_stream_maps_response_tool_and_cumulative_usage() {
        let mut parser = GoogleEventParser::new(GoogleCli::Antigravity, false);
        assert_eq!(
            parser.parse_line(
                r#"{"event":"init","conversation_id":"agy-1","init":{"model":"gemini-pro"}}"#,
            ),
            [AgentEvent::SessionInit {
                cli_session_id: "agy-1".to_string(),
                model: "gemini-pro".to_string(),
            }]
        );
        assert_eq!(
            parser.parse_line(r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"parameters":{"CommandLine":"pwd"}}}}"#),
            [
                AgentEvent::ToolCallStarted {
                    name: "run_command".to_string(),
                    summary_json: r#"{"CommandLine":"pwd"}"#.to_string(),
                },
                AgentEvent::ToolCallCompleted {
                    name: "run_command".to_string(),
                },
            ]
        );
        assert_eq!(
            parser.parse_line(r#"{"event":"step_update","step_update":{"state":"ACTIVE","step_type":"agent_response","text_delta":"done"}}"#),
            [AgentEvent::TextDelta {
                text: "done".to_string(),
            }]
        );
        assert_eq!(
            parser.parse_line(r#"{"event":"result","result":{"status":"SUCCESS","response":"done","usage":{"input_tokens":100,"output_tokens":20,"cache_read_tokens":10}}}"#),
            [AgentEvent::TurnCompleted {
                usage: TurnUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_read_tokens: 10,
                    ..TurnUsage::default()
                },
            }]
        );
        assert_eq!(
            parser.parse_line(r#"{"event":"result","result":{"status":"SUCCESS","response":"next","usage":{"input_tokens":150,"output_tokens":35,"cache_read_tokens":25}}}"#),
            [
                AgentEvent::TextDelta {
                    text: "next".to_string(),
                },
                AgentEvent::TurnCompleted {
                    usage: TurnUsage {
                        input_tokens: 50,
                        output_tokens: 15,
                        cache_read_tokens: 15,
                        ..TurnUsage::default()
                    },
                },
            ]
        );
    }

    #[test]
    fn only_empty_custom_lane_is_accepted() {
        assert!(validate_lane(GoogleCli::Gemini, &SessionLane::Custom { env: Vec::new() }).is_ok());
        assert!(validate_lane(
            GoogleCli::Antigravity,
            &SessionLane::Custom {
                env: vec![("TOKEN".to_string(), "secret".to_string())],
            }
        )
        .is_err());
    }
}
