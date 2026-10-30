//! Orca's account store, shared with Halv.
//!
//! Orca (stablyai/orca, MIT) keeps Claude and Codex accounts in the settings
//! of its active profile. When Orca is installed, Halv shows those accounts
//! and uses the account that is active in Orca. An account that the user adds
//! in Orca is then also in Halv, and the other way around.
//!
//! Orca is the single writer. Halv never writes, locks, or migrates Orca's
//! files or keychain items. Halv sends each change to the running Orca
//! through Orca's local runtime socket, and starts Orca when it is closed.
//! Only a change starts Orca. A read never does.
//!
//! Halv reads Orca's accounts from three sources, in this order:
//! 1. A live mirror. A background thread asks the running Orca for its
//!    accounts: a stream when Orca can stream them on its local socket, else
//!    one `accounts.list` request every 5 seconds.
//! 2. Orca's SQLite profile state (`profile-state.db`), read-only, when Orca
//!    is closed or does not answer. Until a running Orca has not answered
//!    for some seconds, Halv keeps the last mirror, because Orca closes its
//!    database while it quits.
//! 3. The JSON data file of Orca builds before SQLite. Halv reads it only
//!    when the profile has no SQLite state, because Orca stops writing it
//!    when it moves to SQLite.
//!
//! Older Orca builds switch Claude for the whole user account: they copy the
//! selected login into the default Claude Code credential slots. Newer Orca
//! builds give each account its own `CLAUDE_CONFIG_DIR` and write the folder
//! of the selected account into a pointer file. Codex is per launch: a CLI
//! gets `CODEX_HOME` set to the folder of the selected account.

use super::{claude, AccountIdentity, WalletProvider};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

const PROFILE_INDEX_FILE: &str = "orca-profile-index.json";
/// Orca reads this copy when the index itself is torn.
const PROFILE_INDEX_BACKUP_FILE: &str = "orca-profile-index.json.bak";
const DATA_FILE: &str = "orca-data.json";
const DATABASE_FILE: &str = "profile-state.db";
/// SQLite keeps these files beside a database. Any of them means that SQLite
/// owns the profile, also while the database file itself is missing.
const DATABASE_SIDE_FILES: [&str; 3] = ["-wal", "-shm", "-journal"];
/// The SQLite document that holds Orca's settings.
const SETTINGS_DOMAIN: &str = "settings";
const RUNTIME_FILE: &str = "orca-runtime.json";
const PROFILES_DIR: &str = "profiles";
const CLAUDE_ACCOUNTS_DIR: &str = "claude-accounts";
const CODEX_ACCOUNTS_DIR: &str = "codex-accounts";
/// Newer Orca keeps one Claude config folder per account here.
const CLAUDE_PROFILES_DIR: &str = "claude-profiles";
/// Orca writes the folder of the selected Claude account into this file.
const CLAUDE_POINTER_FILE: &str = "selected-host";
/// Orca writes this file last when it sets up an account folder.
const CLAUDE_PROFILE_MARKER: &str = "profile.json";
#[cfg(target_os = "macos")]
const APP_BUNDLE_ID: &str = "com.stablyai.orca";
/// Orca sets this variable beside each `CLAUDE_CONFIG_DIR` that it injects,
/// so its own tools never read that folder as the user's own folder.
pub(super) const CLAUDE_INJECTED_CONFIG_DIR_ENV: &str = "ORCA_CLAUDE_INJECTED_CONFIG_DIR";
/// Orca also removes this login variable from its Claude launches.
const CLAUDE_BEDROCK_TOKEN_ENV: &str = "AWS_BEARER_TOKEN_BEDROCK";
/// Orca removes these headers when they carry a credential.
const CLAUDE_CUSTOM_HEADERS_ENV: &str = "ANTHROPIC_CUSTOM_HEADERS";
/// The answer when Orca does not take Claude logins from Halv.
pub(super) const CLAUDE_ADD_IN_ORCA: &str =
    "Orca now adds Claude accounts itself. Add the account in Orca, and Halv shows it here.";

/// A short read gives up fast, so a hung Orca cannot stall the UI.
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// A switch writes keychain items and can wait for a running sign-in.
pub(super) const CHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Orca needs some seconds to start and open its runtime socket.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const START_POLL: Duration = Duration::from_millis(250);
/// How long a read of Orca's database waits while Orca holds a lock.
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_millis(500);
/// How often the mirror asks Orca for its accounts when Orca cannot stream.
const MIRROR_POLL: Duration = Duration::from_secs(5);
/// The mirror answers reads for this long after Orca's last answer.
const MIRROR_LIFETIME: Duration = Duration::from_secs(15);
/// After the mirror stops, Halv keeps its store for this long while the Orca
/// process that sent it still runs. See `held_store`.
const OFFLINE_HOLD: Duration = Duration::from_secs(30);
/// A connected stream keeps the mirror live for this long.
const STREAM_LIFETIME: Duration = Duration::from_secs(30);
/// How often the watcher looks at Orca while the mirror is down.
const WATCH_TICK: Duration = Duration::from_secs(2);
/// The first wait after a failed request. Each failure doubles it.
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_LIMIT: Duration = Duration::from_secs(30);
/// Orca closes a socket after 30 seconds without traffic. A heartbeat keeps
/// a quiet stream open.
#[cfg(unix)]
const STREAM_HEARTBEAT: Duration = Duration::from_secs(10);
/// A stream that ends sooner than this counts as unstable.
#[cfg(unix)]
const STREAM_STABLE: Duration = Duration::from_secs(60);
/// After this many unstable streams in a row, the watcher polls instead.
/// Why: each new stream makes Orca fetch the usage of every account.
#[cfg(unix)]
const STREAM_UNSTABLE_LIMIT: u32 = 3;
/// Halv checks for the Orca app again after this time.
#[cfg(target_os = "macos")]
const APP_CHECK_LIFETIME: Duration = Duration::from_secs(60);

const UNAVAILABLE: &str = "Halv could not reach Orca. Open Orca and try again.";
const NO_ANSWER: &str = "Orca did not answer in time. Try again.";

/// Parsed stores of Orca's files, keyed by file path. See `read_saved_store`.
static STORE_CACHE: Mutex<Option<HashMap<PathBuf, CachedStore>>> = Mutex::new(None);
/// The live mirror of Orca's accounts, keyed by Orca's data folder.
static MIRROR: Mutex<Option<HashMap<PathBuf, MirrorSlot>>> = Mutex::new(None);
#[cfg(target_os = "macos")]
static APP_CHECK: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
/// True after Halv saw a running Orca in this session.
#[cfg(target_os = "macos")]
static APP_SEEN_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The answer of the Spotlight lookup for an Orca app outside the
/// Applications folders. Empty until the lookup is done.
#[cfg(target_os = "macos")]
static APP_ELSEWHERE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OrcaAccount {
    pub id: String,
    pub email: String,
    /// Claude organization UUID. Codex accounts have none.
    pub organization_id: Option<String>,
    /// Claude organization name, or ChatGPT workspace label.
    pub organization_name: Option<String>,
    /// Claude: the managed auth folder. Codex: the managed `CODEX_HOME`.
    pub path: PathBuf,
    /// Unix milliseconds.
    pub created_at: i64,
    /// The account folder holds no login. Orca refuses to select the account
    /// until the user signs in to it again in Orca.
    pub needs_sign_in: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct OrcaProvider {
    pub active_id: Option<String>,
    pub accounts: Vec<OrcaAccount>,
}

impl OrcaProvider {
    pub(super) fn find(&self, id: &str) -> Option<&OrcaAccount> {
        self.accounts.iter().find(|account| account.id == id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct OrcaStore {
    pub claude: OrcaProvider,
    pub codex: OrcaProvider,
}

impl OrcaStore {
    pub(super) fn provider(&self, provider: WalletProvider) -> &OrcaProvider {
        match provider {
            WalletProvider::Claude => &self.claude,
            WalletProvider::Codex => &self.codex,
        }
    }

    fn provider_mut(&mut self, provider: WalletProvider) -> &mut OrcaProvider {
        match provider {
            WalletProvider::Claude => &mut self.claude,
            WalletProvider::Codex => &mut self.codex,
        }
    }
}

/// The usage of one window of an Orca account.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OrcaUsageWindow {
    pub kind: &'static str,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OrcaAccountUsage {
    pub provider: WalletProvider,
    pub account_id: String,
    pub windows: Vec<OrcaUsageWindow>,
    pub updated_at: i64,
}

/// An Orca request that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OrcaError {
    /// Orca's error code, such as `method_not_found`. `None` when Orca did
    /// not answer.
    pub code: Option<String>,
    pub message: String,
}

impl OrcaError {
    fn local(message: &str) -> Self {
        Self {
            code: None,
            message: message.to_string(),
        }
    }

    /// True when this Orca does not know the method, or cannot run it on
    /// its local socket.
    pub(super) fn unknown_method(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some("method_not_found" | "method_not_supported")
        )
    }
}

impl From<OrcaError> for String {
    fn from(error: OrcaError) -> Self {
        error.message
    }
}

/// Where Orca keeps the settings of its active profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StoreFile {
    /// Orca's SQLite profile state. Once it exists, it is the only truth.
    Database(PathBuf),
    /// The JSON data file of Orca builds before SQLite.
    Legacy(PathBuf),
}

/// Where Orca sends Claude launches on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClaudeRoute {
    /// The default Claude folder. Orca without per-account folders copies
    /// the selected login into it. Newer Orca uses it for System default.
    System,
    /// The folder of the selected account, exactly as Orca wrote it.
    Profile(PathBuf),
    /// The folder of the selected account. Orca has not set it up yet.
    NotSetUp(PathBuf),
    /// A pointer file that Halv cannot read, or that names no absolute
    /// folder. Orca never writes such a pointer.
    Unreadable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    length: u64,
}

struct CachedStore {
    /// The stamps of the store files at the last read.
    stamps: Vec<Option<FileStamp>>,
    /// The revision and content hash of the SQLite settings document.
    version: Option<(i64, String)>,
    /// `None` until a read succeeds.
    store: Option<OrcaStore>,
    /// The error of the last read. Orca writes often, so Halv logs each
    /// error once.
    error: Option<String>,
}

#[derive(Default)]
struct MirrorSlot {
    /// Increments with each update. An answer that Orca made before an
    /// update must not replace it.
    generation: u64,
    mirror: Option<Mirror>,
}

struct Mirror {
    store: OrcaStore,
    usage: Vec<OrcaAccountUsage>,
    live_until: Instant,
    /// The Orca process that sent the store.
    pid: Option<i64>,
}

/// Orca's data folder. `HALV_ORCA_DATA_DIR` replaces it, for tests and for a
/// custom Orca location.
pub(crate) fn data_root() -> Option<PathBuf> {
    if let Some(custom) = env::var_os("HALV_ORCA_DATA_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(custom));
    }
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|home| home.join("Library/Application Support/orca"));
    }
    if cfg!(windows) {
        return env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(|app_data| PathBuf::from(app_data).join("orca"));
    }
    env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")))
        .map(|config| config.join("orca"))
}

/// The folder of the active Orca profile. Orca before profiles kept its
/// files in the data folder.
fn profile_dir(root: &Path) -> PathBuf {
    match active_profile_id(root) {
        Some(profile) => root.join(PROFILES_DIR).join(profile),
        None => root.to_path_buf(),
    }
}

fn active_profile_id(root: &Path) -> Option<String> {
    [PROFILE_INDEX_FILE, PROFILE_INDEX_BACKUP_FILE]
        .iter()
        .find_map(|name| {
            let bytes = fs::read(root.join(name)).ok()?;
            let index = serde_json::from_slice::<Value>(&bytes).ok()?;
            index
                .get("activeProfileId")
                .and_then(Value::as_str)
                .filter(|id| safe_segment(id))
                .map(str::to_string)
        })
}

/// The store of the active Orca profile. `None` means that Orca keeps no
/// data in this folder.
pub(super) fn store_file(root: &Path) -> Option<StoreFile> {
    let profile = profile_dir(root);
    let database = profile.join(DATABASE_FILE);
    if database.exists()
        || DATABASE_SIDE_FILES
            .iter()
            .any(|suffix| side_file(&database, suffix).exists())
    {
        return Some(StoreFile::Database(database));
    }
    let data = profile.join(DATA_FILE);
    if data.is_file() {
        return Some(StoreFile::Legacy(data));
    }
    let legacy = root.join(DATA_FILE);
    legacy.is_file().then_some(StoreFile::Legacy(legacy))
}

fn side_file(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && value.len() <= 128
}

fn stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: metadata.modified().ok(),
        length: metadata.len(),
    })
}

/// True when the Orca app is on this computer: an app with Orca's bundle id
/// in an Applications folder or in a place that Spotlight knows, or an Orca
/// that runs or ran in this session. Other systems keep the earlier rule:
/// Orca's data alone turns the Orca store on.
///
/// Why the session rule: `open -b` starts Orca from any place, so an Orca
/// that the user quit stays the owner of the accounts.
pub(super) fn installed(root: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::Ordering;
        if app_bundle_present() {
            return true;
        }
        if running(root) {
            APP_SEEN_RUNNING.store(true, Ordering::Relaxed);
            return true;
        }
        APP_SEEN_RUNNING.load(Ordering::Relaxed) || app_found_elsewhere()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = root;
        true
    }
}

/// Looks for the Orca app bundle. The answer is kept for a minute, because
/// the wallet asks on each launch and each state read.
#[cfg(target_os = "macos")]
fn app_bundle_present() -> bool {
    let mut guard = APP_CHECK.lock().expect("orca app check lock poisoned");
    if let Some((checked, found)) = *guard {
        if checked.elapsed() < APP_CHECK_LIFETIME {
            return found;
        }
    }
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let found = [
        Some(PathBuf::from("/Applications")),
        home.map(|home| home.join("Applications")),
    ]
    .into_iter()
    .flatten()
    .any(|folder| bundle_has_orca_id(&folder.join("Orca.app")));
    *guard = Some((Instant::now(), found));
    found
}

#[cfg(target_os = "macos")]
fn bundle_has_orca_id(app: &Path) -> bool {
    fs::read(app.join("Contents/Info.plist")).is_ok_and(|plist| {
        plist
            .windows(APP_BUNDLE_ID.len())
            .any(|window| window == APP_BUNDLE_ID.as_bytes())
    })
}

/// True when Spotlight knows an app with Orca's bundle id. The lookup runs
/// once per session on its own thread, because it can take some time and
/// the wallet asks while it holds its lock. Until the lookup is done, the
/// answer is false.
#[cfg(target_os = "macos")]
fn app_found_elsewhere() -> bool {
    static LOOKUP: std::sync::Once = std::sync::Once::new();
    LOOKUP.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("halv-orca-app".to_string())
            .spawn(|| {
                let _ = APP_ELSEWHERE.set(spotlight_finds_app());
            });
        if let Err(error) = spawned {
            eprintln!("wallet: could not look for the Orca app: {error}");
        }
    });
    APP_ELSEWHERE.get().copied().unwrap_or(false)
}

#[cfg(all(target_os = "macos", not(test)))]
fn spotlight_finds_app() -> bool {
    let Ok(output) = std::process::Command::new("/usr/bin/mdfind")
        .arg(format!("kMDItemCFBundleIdentifier == '{APP_BUNDLE_ID}'"))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|app| bundle_has_orca_id(Path::new(app)))
}

/// Tests never ask Spotlight.
#[cfg(all(target_os = "macos", test))]
fn spotlight_finds_app() -> bool {
    false
}

/// Reads Orca's accounts: the live mirror, else the last mirror of an Orca
/// that stopped answering but still runs, else Orca's saved store. `None`
/// means that Halv cannot read Orca's accounts now.
pub(super) fn read_store(root: &Path) -> Option<OrcaStore> {
    mirrored_store(root)
        .or_else(|| held_store(root))
        .or_else(|| read_saved_store(root).map(|store| with_claude_sign_ins(root, store)))
}

/// Marks the saved Claude accounts that need a sign-in, as Orca's account
/// list does. Orca with per-account Claude folders works out the mark on
/// each list and never saves it.
fn with_claude_sign_ins(root: &Path, store: OrcaStore) -> OrcaStore {
    if !claude_profiles_in_use(root) {
        return store;
    }
    let system = claude::system_state_file().and_then(|path| super::cached_identity(&path));
    mark_claude_sign_ins(root, store, system.as_ref())
}

/// An account folder without a login needs a sign-in, unless System default
/// holds the same login. Orca also shows each account with the login of its
/// folder, so Halv does the same.
fn mark_claude_sign_ins(
    root: &Path,
    mut store: OrcaStore,
    system: Option<&AccountIdentity>,
) -> OrcaStore {
    for account in &mut store.claude.accounts {
        let home = claude_profile_home(root, &account.id);
        match claude::state_file(Some(&home)).and_then(|path| super::cached_identity(&path)) {
            Some(login) => {
                account.email = login.email;
                account.organization_id = login.organization_id;
                account.organization_name = login.organization_name;
            }
            None => {
                account.needs_sign_in =
                    !system.is_some_and(|system| same_claude_login(account, system));
            }
        }
    }
    store
}

/// Orca's rule: the same email, and the same organization when both logins
/// name one.
fn same_claude_login(account: &OrcaAccount, login: &AccountIdentity) -> bool {
    account
        .email
        .trim()
        .eq_ignore_ascii_case(login.email.trim())
        && match (&account.organization_id, &login.organization_id) {
            (Some(saved), Some(login)) => saved == login,
            _ => true,
        }
}

/// Reads Orca's saved store. A read that fails keeps the last good store.
///
/// Orca rewrites its database every few seconds for many reasons, so Halv
/// parses the settings again only when their revision or hash changed.
fn read_saved_store(root: &Path) -> Option<OrcaStore> {
    let file = store_file(root)?;
    let (path, stamps) = match &file {
        StoreFile::Database(path) => (path, vec![stamp(path), stamp(&side_file(path, "-wal"))]),
        StoreFile::Legacy(path) => (path, vec![stamp(path)]),
    };
    let mut guard = STORE_CACHE.lock().expect("orca store cache lock poisoned");
    let cache = guard.get_or_insert_with(HashMap::new);
    let cached = cache.get(path);
    if let Some(cached) = cached.filter(|cached| cached.stamps == stamps) {
        return cached.store.clone();
    }
    let read = match &file {
        StoreFile::Database(path) => read_database(path, root, cached),
        StoreFile::Legacy(path) => fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string())
            })
            .map(|data| (None, parse_store(&data, root))),
    };
    let entry = match read {
        Ok((version, store)) => CachedStore {
            stamps,
            version,
            store: Some(store),
            error: None,
        },
        Err(error) => {
            // Why keep the stamps: Halv tries again only after Orca writes.
            if cached.and_then(|cached| cached.error.as_deref()) != Some(error.as_str()) {
                eprintln!("wallet: could not read Orca's accounts: {error}");
            }
            CachedStore {
                stamps,
                version: cached.and_then(|cached| cached.version.clone()),
                store: cached.and_then(|cached| cached.store.clone()),
                error: Some(error),
            }
        }
    };
    let store = entry.store.clone();
    cache.insert(path.clone(), entry);
    store
}

/// Reads the settings document of Orca's SQLite profile state.
///
/// The payload can be large, so the query returns it only when the
/// revision or the content hash differs from the cached version. One
/// statement keeps the version and the payload from the same snapshot.
fn read_database(
    database: &Path,
    root: &Path,
    cached: Option<&CachedStore>,
) -> Result<(Option<(i64, String)>, OrcaStore), String> {
    use rusqlite::OptionalExtension;
    let known = cached
        .filter(|cached| cached.store.is_some())
        .and_then(|cached| cached.version.clone());
    let connection = open_database(database)?;
    let row = connection
        .query_row(
            "SELECT revision, content_hash,
               CASE WHEN revision = ?2 AND content_hash = ?3 THEN NULL ELSE payload END
             FROM profile_state_documents WHERE domain = ?1",
            rusqlite::params![
                SETTINGS_DOMAIN,
                known.as_ref().map(|(revision, _)| *revision),
                known.as_ref().map(|(_, hash)| hash.as_str()),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    // Why: without a settings document, Orca runs with default settings.
    let Some((revision, hash, payload)) = row else {
        return Ok((None, OrcaStore::default()));
    };
    let version = Some((revision, hash));
    let Some(payload) = payload else {
        let store = cached
            .and_then(|cached| cached.store.clone())
            .unwrap_or_default();
        return Ok((version, store));
    };
    let settings = serde_json::from_str::<Value>(&payload).map_err(|error| error.to_string())?;
    // Why: Orca saves its whole settings object, which always holds both
    // account lists. Without them, Halv does not know the format. Empty
    // lists would send Codex launches to the System default account.
    let known_format = ["claudeManagedAccounts", "codexManagedAccounts"]
        .iter()
        .all(|key| settings.get(key).is_some_and(Value::is_array));
    if !known_format {
        return Err("Orca's settings have a format that Halv does not know.".to_string());
    }
    Ok((version, parse_settings(&settings, root)))
}

/// Opens Orca's database read-only for one read.
///
/// Without a `-wal` file, all data of a WAL database is in the database
/// file and Orca is not writing, so Halv opens it as immutable. Why: a
/// read-only open of a WAL database creates the missing `-wal` and `-shm`
/// files in Orca's folder. For the same reason, Halv does not open a
/// database whose `-wal` file has no `-shm` file beside it. A database in
/// rollback mode is immutable only without a `-journal` file.
///
/// Orca can delete its `-wal` file after these checks, when it closes the
/// database. `held_store` keeps Halv from reading while Orca quits.
fn open_database(database: &Path) -> Result<rusqlite::Connection, String> {
    use rusqlite::{config::DbConfig, Connection, OpenFlags};
    let wal = side_file(database, "-wal").exists();
    if wal && !side_file(database, "-shm").exists() {
        return Err("Orca's database has a write-ahead log without its index.".to_string());
    }
    let immutable = if wal_mode(database)? {
        !wal
    } else {
        !wal && !side_file(database, "-journal").exists()
    };
    let connection = Connection::open_with_flags(
        database_uri(database, immutable),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())?;
    connection
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|error| error.to_string())?;
    // Why: the last connection to close can checkpoint a WAL database. A
    // read-only connection cannot, and this setting makes sure of it.
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

/// True when the header of the database names WAL mode. A plain file read
/// takes no SQLite lock. Why an error for a short file: SQLite deletes the
/// `-wal` file of an empty database, also on a read-only connection.
fn wal_mode(database: &Path) -> Result<bool, String> {
    use std::io::Read;
    let mut header = [0_u8; 20];
    fs::File::open(database)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("Halv could not read the header of Orca's database: {error}"))?;
    // Bytes 18 and 19 are the write and read versions of the file format.
    // Version 2 is WAL.
    Ok(header[18] == 2 || header[19] == 2)
}

/// A read-only `file:` URI for SQLite. Orca's folder name holds a space.
fn database_uri(database: &Path, immutable: bool) -> String {
    let text = database.to_string_lossy().replace('\\', "/");
    let mut uri = String::from(if text.starts_with('/') {
        "file:"
    } else {
        "file:///"
    });
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                uri.push(char::from(byte));
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str(if immutable {
        "?mode=ro&immutable=1"
    } else {
        "?mode=ro"
    });
    uri
}

/// Parses the account part of an Orca data file.
pub(super) fn parse_store(data: &Value, root: &Path) -> OrcaStore {
    parse_settings(data.get("settings").unwrap_or(&Value::Null), root)
}

/// Parses Orca's settings. Only accounts of the host runtime count; WSL
/// accounts belong to Windows.
fn parse_settings(settings: &Value, root: &Path) -> OrcaStore {
    OrcaStore {
        claude: OrcaProvider {
            active_id: host_selection(
                settings,
                "activeClaudeManagedAccountIdsByRuntime",
                "activeClaudeManagedAccountId",
            ),
            accounts: parse_accounts(
                WalletProvider::Claude,
                settings.get("claudeManagedAccounts"),
                Some("managedAuthPath"),
                root,
            ),
        },
        codex: OrcaProvider {
            active_id: host_selection(
                settings,
                "activeCodexManagedAccountIdsByRuntime",
                "activeCodexManagedAccountId",
            ),
            accounts: parse_accounts(
                WalletProvider::Codex,
                settings.get("codexManagedAccounts"),
                Some("managedHomePath"),
                root,
            ),
        },
    }
}

/// The host slot wins over the older single field, also when the slot is
/// null, because null there means System default.
fn host_selection(settings: &Value, by_runtime: &str, legacy: &str) -> Option<String> {
    let selection = match settings.get(by_runtime).and_then(|slots| slots.get("host")) {
        Some(host) => host,
        None => settings.get(legacy).unwrap_or(&Value::Null),
    };
    selection
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn parse_accounts(
    provider: WalletProvider,
    list: Option<&Value>,
    path_key: Option<&str>,
    root: &Path,
) -> Vec<OrcaAccount> {
    let (runtime_key, organization_key) = match provider {
        WalletProvider::Claude => ("managedAuthRuntime", "organizationName"),
        WalletProvider::Codex => ("managedHomeRuntime", "workspaceLabel"),
    };
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut accounts = list
        .iter()
        .filter(|entry| {
            entry
                .get(runtime_key)
                .and_then(Value::as_str)
                .is_none_or(|runtime| runtime == "host")
        })
        .filter_map(|entry| {
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            let id = text("id").filter(|id| safe_segment(id))?;
            let path = path_key
                .and_then(text)
                .map(PathBuf::from)
                .unwrap_or_else(|| default_account_path(provider, root, &id));
            Some(OrcaAccount {
                email: text("email").unwrap_or_default(),
                organization_id: match provider {
                    WalletProvider::Claude => text("organizationUuid"),
                    WalletProvider::Codex => None,
                },
                organization_name: text(organization_key),
                created_at: entry.get("createdAt").and_then(Value::as_i64).unwrap_or(0),
                needs_sign_in: entry.get("needsSignIn").and_then(Value::as_bool) == Some(true),
                path,
                id,
            })
        })
        .collect::<Vec<_>>();
    // Why a fixed order: Orca's answers list the most recently used account
    // first, and Orca's settings keep the saved order. With one order, the
    // mirror and the saved store give equal accounts, so a change of source
    // is no account change. The oldest account comes first, as in Halv's
    // own wallet.
    accounts.sort_by(|left, right| (left.created_at, &left.id).cmp(&(right.created_at, &right.id)));
    accounts
}

/// The config folder that newer Orca gives a Claude account.
fn claude_profile_home(root: &Path, id: &str) -> PathBuf {
    root.join(CLAUDE_PROFILES_DIR).join(id).join("home")
}

/// Orca's folder of an account whose entry names no folder.
fn default_account_path(provider: WalletProvider, root: &Path, id: &str) -> PathBuf {
    match provider {
        WalletProvider::Claude => {
            // Newer Orca keeps the login in the account's own config folder.
            let profile = claude_profile_home(root, id);
            if profile.is_dir() {
                profile
            } else {
                root.join(CLAUDE_ACCOUNTS_DIR).join(id).join("auth")
            }
        }
        WalletProvider::Codex => root.join(CODEX_ACCOUNTS_DIR).join(id).join("home"),
    }
}

/// Parses the account state of one provider that Orca returns. The answer
/// has no folder paths, so Halv takes them from the known accounts or uses
/// Orca's default layout.
fn parse_provider_answer(
    provider: WalletProvider,
    answer: &Value,
    known: &OrcaProvider,
    root: &Path,
) -> Option<OrcaProvider> {
    let list = answer.get("accounts").filter(|list| list.is_array())?;
    let mut accounts = parse_accounts(provider, Some(list), None, root);
    for account in &mut accounts {
        if let Some(existing) = known.find(&account.id) {
            account.path = existing.path.clone();
        }
    }
    let active_id = match answer
        .get("activeAccountIdsByRuntime")
        .and_then(|slots| slots.get("host"))
    {
        Some(host) => host.as_str().map(str::to_string),
        None => answer
            .get("activeAccountId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
    .filter(|id| !id.is_empty());
    Some(OrcaProvider {
        active_id,
        accounts,
    })
}

/// Parses a snapshot of `accounts.list` or `accounts.subscribe`.
fn parse_snapshot(
    snapshot: &Value,
    known: &OrcaStore,
    root: &Path,
) -> Option<(OrcaStore, Vec<OrcaAccountUsage>)> {
    let store = OrcaStore {
        claude: parse_provider_answer(
            WalletProvider::Claude,
            snapshot.get("claude")?,
            &known.claude,
            root,
        )?,
        codex: parse_provider_answer(
            WalletProvider::Codex,
            snapshot.get("codex")?,
            &known.codex,
            root,
        )?,
    };
    Some((store, parse_inactive_usage(snapshot)))
}

fn with_mirror<T>(root: &Path, action: impl FnOnce(&mut MirrorSlot) -> T) -> T {
    let mut guard = MIRROR.lock().expect("orca mirror lock poisoned");
    let slots = guard.get_or_insert_with(HashMap::new);
    action(slots.entry(root.to_path_buf()).or_default())
}

fn live_mirror(slot: &MirrorSlot) -> Option<&Mirror> {
    slot.mirror
        .as_ref()
        .filter(|mirror| Instant::now() < mirror.live_until)
}

fn mirrored_store(root: &Path) -> Option<OrcaStore> {
    with_mirror(root, |slot| {
        live_mirror(slot).map(|mirror| mirror.store.clone())
    })
}

/// The last store of an Orca that stopped answering but still runs.
///
/// Why: Orca stops its socket while it quits, and closes its database at
/// the same time. A read at that moment can make SQLite create new `-wal`
/// and `-shm` files in Orca's folder. So Halv keeps the last store until
/// that Orca process ends, for at most `OFFLINE_HOLD`. A short failure of
/// a poll also keeps the store, so the source of the accounts stays the
/// same.
fn held_store(root: &Path) -> Option<OrcaStore> {
    with_mirror(root, |slot| {
        let mirror = slot.mirror.as_ref()?;
        let held = Instant::now() < mirror.live_until + OFFLINE_HOLD
            && mirror.pid.is_some_and(process_alive);
        held.then(|| mirror.store.clone())
    })
}

fn mirror_generation(root: &Path) -> u64 {
    with_mirror(root, |slot| slot.generation)
}

/// Stops the mirror. Its store stays for `held_store`.
fn mark_offline(root: &Path) {
    with_mirror(root, |slot| {
        if let Some(mirror) = slot.mirror.as_mut() {
            mirror.live_until = mirror.live_until.min(Instant::now());
        }
    });
}

/// The accounts that Halv knows without a read of Orca's database: the last
/// mirror, else the last saved store that Halv read.
fn known_store(root: &Path) -> OrcaStore {
    with_mirror(root, |slot| {
        slot.mirror.as_ref().map(|mirror| mirror.store.clone())
    })
    .or_else(|| cached_saved_store(root))
    .unwrap_or_default()
}

fn cached_saved_store(root: &Path) -> Option<OrcaStore> {
    let (StoreFile::Database(path) | StoreFile::Legacy(path)) = store_file(root)?;
    let guard = STORE_CACHE.lock().expect("orca store cache lock poisoned");
    guard.as_ref()?.get(&path)?.store.clone()
}

#[cfg(unix)]
fn extend_mirror(root: &Path, lifetime: Duration) {
    with_mirror(root, |slot| {
        if let Some(mirror) = slot.mirror.as_mut() {
            mirror.live_until = mirror.live_until.max(Instant::now() + lifetime);
        }
    });
}

/// Replaces the mirror with a snapshot of Orca. Returns `None` for a snapshot
/// that Halv cannot read, and `Some(false)` when a newer change won.
fn update_mirror(
    root: &Path,
    generation: u64,
    snapshot: &Value,
    lifetime: Duration,
) -> Option<bool> {
    // Why no database read for each answer: the answer holds the accounts,
    // and Orca writes its database all the time. Halv reads the database
    // only for the folder of an account that it does not know. The read
    // happens outside the mirror lock.
    let known = known_store(root);
    let (mut store, mut usage) = parse_snapshot(snapshot, &known, root)?;
    let new_account = WalletProvider::ALL.iter().any(|&provider| {
        store
            .provider(provider)
            .accounts
            .iter()
            .any(|account| known.provider(provider).find(&account.id).is_none())
    });
    if new_account {
        if let Some(saved) = read_saved_store(root) {
            (store, usage) = parse_snapshot(snapshot, &saved, root)?;
        }
    }
    let pid = endpoint(root).and_then(|endpoint| endpoint.pid);
    Some(with_mirror(root, |slot| {
        if slot.generation != generation {
            return false;
        }
        slot.generation += 1;
        slot.mirror = Some(Mirror {
            store,
            usage,
            live_until: Instant::now() + lifetime,
            pid,
        });
        true
    }))
}

/// Asks the running Orca for its accounts and updates the mirror.
fn refresh_mirror(root: &Path) -> Result<(), OrcaError> {
    let generation = mirror_generation(root);
    let snapshot = call(
        root,
        "accounts.list",
        json!({ "refreshUsage": false }),
        READ_TIMEOUT,
    )?;
    update_mirror(root, generation, &snapshot, MIRROR_LIFETIME)
        .map(|_| ())
        .ok_or_else(|| OrcaError::local("Orca sent an account list that Halv cannot read."))
}

/// Puts the answer of a change into the mirror. Returns false when the
/// mirror is down.
///
/// Why: Orca saves its store some seconds after a change, and a poll that
/// started before the change must not undo it. So the generation also
/// moves on when the mirror is down.
fn apply_answer(root: &Path, provider: WalletProvider, answer: &Value) -> bool {
    with_mirror(root, |slot| {
        slot.generation += 1;
        let Some(mirror) = slot
            .mirror
            .as_mut()
            .filter(|mirror| Instant::now() < mirror.live_until)
        else {
            return false;
        };
        let Some(state) =
            parse_provider_answer(provider, answer, mirror.store.provider(provider), root)
        else {
            return false;
        };
        *mirror.store.provider_mut(provider) = state;
        true
    })
}

/// Makes Halv show a change that Orca answered at once.
fn after_change(root: &Path, provider: WalletProvider, answer: &Value) {
    if apply_answer(root, provider, answer) {
        return;
    }
    if let Err(error) = refresh_mirror(root) {
        eprintln!(
            "wallet: could not read Orca's accounts after a change: {}",
            error.message
        );
    }
}

/// Makes `account_id` (or System default) the active account in Orca.
pub(super) fn select(
    root: &Path,
    provider: WalletProvider,
    account_id: Option<&str>,
) -> Result<(), String> {
    if let Some(id) = account_id {
        let needs_sign_in = read_store(root)
            .and_then(|store| {
                store
                    .provider(provider)
                    .find(id)
                    .map(|account| account.needs_sign_in)
            })
            .unwrap_or(false);
        if needs_sign_in {
            return Err(format!(
                "Sign in to this {} account again in Orca, then select it.",
                provider.display_name()
            ));
        }
    }
    let method = match provider {
        WalletProvider::Claude => "accounts.selectClaude",
        WalletProvider::Codex => "accounts.selectCodex",
    };
    let answer = call_starting(root, method, json!({ "accountId": account_id }))?;
    after_change(root, provider, &answer);
    Ok(())
}

/// Removes an account from Orca. Orca deletes its folder and keychain item.
pub(super) fn remove(
    root: &Path,
    provider: WalletProvider,
    account_id: &str,
) -> Result<(), String> {
    let method = match provider {
        WalletProvider::Claude => "accounts.removeClaude",
        WalletProvider::Codex => "accounts.removeCodex",
    };
    let answer = call_starting(root, method, json!({ "accountId": account_id }))?;
    after_change(root, provider, &answer);
    Ok(())
}

/// True when Orca gives each Claude account its own config folder. Such an
/// Orca does not take Claude logins from Halv.
pub(super) fn claude_profiles_in_use(root: &Path) -> bool {
    root.join(CLAUDE_PROFILES_DIR).is_dir()
}

/// Gives a signed-in CLI folder to Orca. Orca copies the login into its own
/// account folder, so the caller can delete the source folder after.
/// Returns the id of the new Orca account.
///
/// For Claude, Halv does not send the legacy credential digest. Without it,
/// Orca reads only the keychain item of the folder or the folder's
/// credential file, and never the shared default item.
///
/// An Orca that refuses Claude logins returns `CLAUDE_ADD_IN_ORCA`.
///
/// Returns `None` also when Halv did not know Orca's accounts before the
/// add. Why: any account in the answer could then look new, and the caller
/// would select the wrong account.
pub(super) fn add_from_folder(
    root: &Path,
    provider: WalletProvider,
    folder: &Path,
) -> Result<Option<String>, String> {
    let before = account_ids_before_add(root, provider);
    let folder = folder.to_string_lossy().to_string();
    let (method, params) = match provider {
        WalletProvider::Claude => (
            "accounts.addClaudeFromConfigDir",
            json!({ "configDir": folder }),
        ),
        WalletProvider::Codex => ("accounts.addCodexFromHome", json!({ "sourceHome": folder })),
    };
    let answer = match call_starting(root, method, params) {
        Ok(answer) => answer,
        Err(error) if provider == WalletProvider::Claude && claude_import_refused(&error) => {
            return Err(CLAUDE_ADD_IN_ORCA.to_string());
        }
        Err(error) => return Err(error.into()),
    };
    let added = before.and_then(|before| {
        parse_accounts(provider, answer.get("accounts"), None, root)
            .into_iter()
            .map(|account| account.id)
            .find(|id| !before.contains(id))
    });
    after_change(root, provider, &answer);
    Ok(added)
}

/// The ids of Orca's accounts before an add. When Halv cannot read Orca's
/// accounts now, it asks the running Orca. `None` means that Halv does not
/// know them.
fn account_ids_before_add(root: &Path, provider: WalletProvider) -> Option<Vec<String>> {
    let ids = |state: &OrcaProvider| {
        state
            .accounts
            .iter()
            .map(|account| account.id.clone())
            .collect::<Vec<_>>()
    };
    if let Some(store) = read_store(root) {
        return Some(ids(store.provider(provider)));
    }
    if !running(root) {
        return None;
    }
    let snapshot = call(
        root,
        "accounts.list",
        json!({ "refreshUsage": false }),
        READ_TIMEOUT,
    )
    .ok()?;
    let (store, _) = parse_snapshot(&snapshot, &OrcaStore::default(), root)?;
    Some(ids(store.provider(provider)))
}

/// Orca with per-account Claude folders keeps the method, but refuses it:
/// "Importing Claude logins is no longer supported."
fn claude_import_refused(error: &OrcaError) -> bool {
    error.unknown_method() || error.message.contains("no longer supported")
}

/// The usage that Orca knows for its accounts that are not active. Halv
/// reads only what Orca has; it never asks Orca to fetch, because each fetch
/// costs one provider request per account.
pub(super) fn inactive_usage(root: &Path) -> Result<Vec<OrcaAccountUsage>, String> {
    if let Some(usage) = with_mirror(root, |slot| {
        live_mirror(slot).map(|mirror| mirror.usage.clone())
    }) {
        return Ok(usage);
    }
    if !running(root) {
        return Ok(Vec::new());
    }
    let answer = call(
        root,
        "accounts.list",
        json!({ "refreshUsage": false }),
        READ_TIMEOUT,
    )?;
    Ok(parse_inactive_usage(&answer))
}

pub(super) fn parse_inactive_usage(snapshot: &Value) -> Vec<OrcaAccountUsage> {
    let limits = snapshot.get("rateLimits").unwrap_or(&Value::Null);
    let mut usage = Vec::new();
    for (provider, key) in [
        (WalletProvider::Claude, "inactiveClaudeAccounts"),
        (WalletProvider::Codex, "inactiveCodexAccounts"),
    ] {
        for entry in limits
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(account_id) = entry.get("accountId").and_then(Value::as_str) else {
                continue;
            };
            let Some(rate_limits) = entry.get("rateLimits").filter(|value| value.is_object())
            else {
                continue;
            };
            let windows = [
                ("session", "session"),
                ("weekly", "weekly"),
                ("fableWeekly", "fableWeekly"),
            ]
            .into_iter()
            .filter_map(|(kind, field)| {
                let window = rate_limits.get(field)?;
                Some(OrcaUsageWindow {
                    kind,
                    used_percent: window.get("usedPercent")?.as_f64()?,
                    resets_at: window.get("resetsAt").and_then(Value::as_i64),
                })
            })
            .collect::<Vec<_>>();
            if windows.is_empty() {
                continue;
            }
            usage.push(OrcaAccountUsage {
                provider,
                account_id: account_id.to_string(),
                windows,
                updated_at: entry
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            });
        }
    }
    usage
}

/// Reads the pointer file of Orca's Claude router.
///
/// No file means an Orca without per-account folders: it copies the
/// selected login into the default folder. An empty file means System
/// default. Else the file names the folder of the selected account. Halv
/// uses that folder only after Orca finished its setup, which Orca marks
/// with `profile.json` beside it.
pub(super) fn claude_route(root: &Path) -> ClaudeRoute {
    let pointer = root.join(CLAUDE_PROFILES_DIR).join(CLAUDE_POINTER_FILE);
    let bytes = match fs::read(&pointer) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return ClaudeRoute::System,
        Err(_) => return ClaudeRoute::Unreadable,
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return ClaudeRoute::Unreadable;
    };
    // Why: Orca's own `claude` function reads the file with `$(cat ...)`,
    // which drops trailing newlines only.
    let home = text.trim_end_matches('\n');
    if home.is_empty() {
        return ClaudeRoute::System;
    }
    let home = PathBuf::from(home);
    // Why: Orca's `claude` function runs nothing for a relative folder,
    // because the folder would depend on the working folder of the shell.
    if !home.is_absolute() {
        return ClaudeRoute::Unreadable;
    }
    let ready = home.is_dir()
        && home
            .parent()
            .is_some_and(|account| account.join(CLAUDE_PROFILE_MARKER).is_file());
    if ready {
        ClaudeRoute::Profile(home)
    } else {
        ClaudeRoute::NotSetUp(home)
    }
}

/// The inherited variables that Orca removes from a Claude launch in an
/// account folder, beside the ones that every managed Claude launch
/// removes.
pub(super) fn claude_auth_env_remove() -> Vec<&'static str> {
    claude_auth_env_remove_for(env::var_os(CLAUDE_CUSTOM_HEADERS_ENV).as_deref())
}

fn claude_auth_env_remove_for(custom_headers: Option<&std::ffi::OsStr>) -> Vec<&'static str> {
    let mut names = vec![CLAUDE_BEDROCK_TOKEN_ENV];
    // Why these words: Orca matches /authorization|x-api-key|api-key|bearer/i.
    let headers = custom_headers
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ["authorization", "api-key", "bearer"]
        .iter()
        .any(|word| headers.contains(word))
    {
        names.push(CLAUDE_CUSTOM_HEADERS_ENV);
    }
    names
}

/// Keeps the mirror of Orca's accounts and calls `changed` when the accounts
/// that Halv reads change. The function never returns.
pub(super) fn watch(root: &Path, mut changed: impl FnMut()) -> ! {
    let mut watcher = Watcher::new(root);
    loop {
        if watcher.step(root) {
            changed();
        }
    }
}

struct Watcher {
    link: Link,
    last: Option<OrcaStore>,
}

impl Watcher {
    fn new(root: &Path) -> Self {
        Self {
            link: Link::default(),
            last: read_store(root),
        }
    }

    /// One step of the watcher. Returns true when the accounts that Halv
    /// reads changed. While the mirror is down, the read checks the stamps
    /// of Orca's store files and reads SQLite only after Orca wrote.
    fn step(&mut self, root: &Path) -> bool {
        self.link.step(root);
        let store = read_store(root);
        if store == self.last {
            return false;
        }
        self.last = store;
        true
    }
}

/// The watcher's connection to the running Orca.
#[derive(Default)]
struct Link {
    /// The pid and socket of the Orca that the link talks to.
    process: Option<(Option<i64>, PathBuf)>,
    /// This Orca cannot stream accounts on its local socket, so the link
    /// polls.
    polling: bool,
    #[cfg(unix)]
    stream: Option<Stream>,
    failures: u32,
    /// Streams in a row that ended early.
    #[cfg(unix)]
    short_streams: u32,
    next_attempt: Option<Instant>,
}

impl Link {
    /// One step of the watcher. It waits for at most one tick.
    fn step(&mut self, root: &Path) {
        let process = endpoint(root)
            .filter(|endpoint| endpoint.pid.is_none_or(process_alive))
            .map(|endpoint| (endpoint.pid, endpoint.socket));
        if process.is_none() || process != self.process {
            // Why: a new Orca gets a new token, and it can stream where the
            // old one could not.
            if self.process.is_some() {
                mark_offline(root);
            }
            *self = Self {
                process: process.clone(),
                ..Self::default()
            };
            if process.is_none() {
                std::thread::sleep(WATCH_TICK);
                return;
            }
        }
        #[cfg(unix)]
        if self.stream.is_some() {
            self.read_stream(root);
            return;
        }
        let now = Instant::now();
        if let Some(next) = self.next_attempt.filter(|next| *next > now) {
            std::thread::sleep((next - now).min(WATCH_TICK));
            return;
        }
        if self.polling {
            self.poll(root);
        } else {
            self.subscribe(root);
        }
    }

    fn poll(&mut self, root: &Path) {
        match refresh_mirror(root) {
            Ok(()) => {
                self.failures = 0;
                self.next_attempt = Some(Instant::now() + MIRROR_POLL);
            }
            Err(error) => self.fail(root, &error),
        }
    }

    fn subscribe(&mut self, root: &Path) {
        match open_subscription(root) {
            #[cfg(unix)]
            Subscription::Stream(stream, snapshot) => {
                update_mirror(root, mirror_generation(root), &snapshot, STREAM_LIFETIME);
                self.failures = 0;
                self.stream = Some(stream);
            }
            Subscription::Unsupported => {
                // Why: Orca's local socket answers one request per
                // connection. A poll costs Orca no provider request.
                self.polling = true;
                self.poll(root);
            }
            Subscription::Failed(error) => self.fail(root, &error),
        }
    }

    #[cfg(unix)]
    fn read_stream(&mut self, root: &Path) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        match stream.next_event(WATCH_TICK) {
            StreamEvent::Snapshot(snapshot) => {
                update_mirror(root, mirror_generation(root), &snapshot, STREAM_LIFETIME);
            }
            StreamEvent::Quiet => extend_mirror(root, STREAM_LIFETIME),
            StreamEvent::Closed => {
                let short = stream.opened.elapsed() < STREAM_STABLE;
                self.stream = None;
                self.short_streams = if short { self.short_streams + 1 } else { 0 };
                if self.short_streams >= STREAM_UNSTABLE_LIMIT {
                    self.polling = true;
                }
                self.fail(root, &OrcaError::local("Orca closed the account stream."));
            }
        }
    }

    fn fail(&mut self, root: &Path, error: &OrcaError) {
        mark_offline(root);
        self.failures = self.failures.saturating_add(1);
        if self.failures == 1 {
            eprintln!(
                "wallet: Orca did not answer the account mirror: {}",
                error.message
            );
        }
        self.next_attempt = Some(Instant::now() + retry_delay(self.failures));
    }
}

/// 2, 4, 8, and 16 seconds, then 30 seconds.
fn retry_delay(failures: u32) -> Duration {
    let factor = 1_u32 << failures.saturating_sub(1).min(4);
    (RETRY_FIRST * factor).min(RETRY_LIMIT)
}

enum Subscription {
    /// Orca streams its accounts. The first snapshot comes with the stream.
    #[cfg(unix)]
    Stream(Stream, Value),
    /// Orca cannot stream accounts on its local socket.
    Unsupported,
    Failed(OrcaError),
}

/// One frame that Orca sent on a request socket.
#[derive(Debug, PartialEq)]
enum Frame {
    /// Orca sends this while a request runs. It carries no data.
    Keepalive,
    /// A frame of another request, or a frame that Halv does not know.
    Other,
    /// The first frame of an account stream, or a later change.
    Snapshot(Value),
    /// Orca closed the stream.
    End,
    /// A one-shot answer.
    Unary(Result<Value, OrcaError>),
}

/// Sorts a frame of the request `id`. Stream frames carry `streaming: true`
/// and a `result.type` of `ready`, `snapshot`, or `end`.
fn classify_frame(frame: &Value, id: &str) -> Frame {
    if frame.get("_keepalive").is_some() {
        return Frame::Keepalive;
    }
    if frame.get("id").and_then(Value::as_str) != Some(id) {
        return Frame::Other;
    }
    if frame.get("streaming").and_then(Value::as_bool) != Some(true) {
        return Frame::Unary(answer_result(frame));
    }
    let result = frame.get("result").unwrap_or(&Value::Null);
    match result.get("type").and_then(Value::as_str) {
        Some("ready" | "snapshot") => match result.get("snapshot") {
            Some(snapshot) if snapshot.is_object() => Frame::Snapshot(snapshot.clone()),
            _ => Frame::Other,
        },
        Some("end") => Frame::End,
        _ => Frame::Other,
    }
}

#[cfg(unix)]
struct Stream {
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
    id: String,
    /// The bytes of a frame that is not complete yet.
    pending: Vec<u8>,
    opened: Instant,
    last_write: Instant,
}

#[cfg(unix)]
enum StreamEvent {
    Snapshot(Value),
    /// No snapshot came, and the stream is still open.
    Quiet,
    Closed,
}

#[cfg(unix)]
impl Stream {
    /// Waits up to `wait` for the next account snapshot.
    fn next_event(&mut self, wait: Duration) -> StreamEvent {
        use std::io::{BufRead, ErrorKind, Write};
        if self.last_write.elapsed() >= STREAM_HEARTBEAT {
            // Why: Orca skips an empty line, and any traffic keeps the
            // socket open.
            if self.reader.get_mut().write_all(b"\n").is_err() {
                return StreamEvent::Closed;
            }
            self.last_write = Instant::now();
        }
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return StreamEvent::Quiet;
            }
            // Why a failure is not an end: macOS refuses the option on a
            // socket that Orca closed, and the frames that Orca sent before
            // are still readable. The earlier timeout stays in effect.
            let _ = self.reader.get_ref().set_read_timeout(Some(left));
            match self.reader.read_until(b'\n', &mut self.pending) {
                Ok(0) => return StreamEvent::Closed,
                Ok(_) => {}
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return StreamEvent::Quiet;
                }
                Err(_) => return StreamEvent::Closed,
            }
            if self.pending.last() != Some(&b'\n') {
                return StreamEvent::Closed;
            }
            let frame = serde_json::from_slice::<Value>(&self.pending);
            self.pending.clear();
            let Ok(frame) = frame else {
                continue;
            };
            match classify_frame(&frame, &self.id) {
                Frame::Snapshot(snapshot) => return StreamEvent::Snapshot(snapshot),
                Frame::End | Frame::Unary(_) => return StreamEvent::Closed,
                Frame::Keepalive | Frame::Other => {}
            }
        }
    }
}

#[cfg(unix)]
fn open_subscription(root: &Path) -> Subscription {
    use std::io::BufRead;
    let (mut reader, id) = match send(root, "accounts.subscribe", json!({}), READ_TIMEOUT) {
        Ok(sent) => sent,
        Err(error) => return Subscription::Failed(error),
    };
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut line = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Subscription::Failed(OrcaError::local(NO_ANSWER));
        }
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return Subscription::Failed(OrcaError::local(UNAVAILABLE)),
            Ok(_) => {}
            Err(_) => return Subscription::Failed(OrcaError::local(NO_ANSWER)),
        }
        let Ok(frame) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        match classify_frame(&frame, &id) {
            Frame::Keepalive | Frame::Other => {}
            Frame::Snapshot(snapshot) => {
                let now = Instant::now();
                let stream = Stream {
                    reader,
                    id,
                    pending: Vec::new(),
                    opened: now,
                    last_write: now,
                };
                return Subscription::Stream(stream, snapshot);
            }
            Frame::End => {
                return Subscription::Failed(OrcaError::local("Orca closed the account stream."));
            }
            Frame::Unary(Err(error)) if error.unknown_method() => return Subscription::Unsupported,
            Frame::Unary(Err(error)) => return Subscription::Failed(error),
            // A one-shot answer means that this socket cannot stream.
            Frame::Unary(Ok(_)) => return Subscription::Unsupported,
        }
    }
}

#[cfg(not(unix))]
fn open_subscription(_root: &Path) -> Subscription {
    Subscription::Unsupported
}

struct RuntimeEndpoint {
    socket: PathBuf,
    token: Option<String>,
    pid: Option<i64>,
}

fn endpoint(root: &Path) -> Option<RuntimeEndpoint> {
    let bytes = fs::read(root.join(RUNTIME_FILE)).ok()?;
    let metadata = serde_json::from_slice::<Value>(&bytes).ok()?;
    let from_list = metadata
        .get("transports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|transport| transport.get("kind").and_then(Value::as_str) == Some("unix"));
    // Orca before the transport list wrote one `transport` object.
    let transport = from_list.or_else(|| {
        metadata
            .get("transport")
            .filter(|transport| transport.get("kind").and_then(Value::as_str) == Some("unix"))
    })?;
    Some(RuntimeEndpoint {
        socket: PathBuf::from(transport.get("endpoint")?.as_str()?),
        token: metadata
            .get("authToken")
            .and_then(Value::as_str)
            .map(str::to_string),
        pid: metadata.get("pid").and_then(Value::as_i64),
    })
}

/// True when the Orca that wrote the runtime file is still running.
pub(super) fn running(root: &Path) -> bool {
    endpoint(root).is_some_and(|endpoint| endpoint.pid.is_none_or(process_alive))
}

#[cfg(unix)]
fn process_alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 only checks that the process exists.
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[cfg(not(unix))]
fn process_alive(_pid: i64) -> bool {
    true
}

/// Opens a connection to the running Orca and sends one request. Returns
/// the reader of the answer and the request id.
#[cfg(unix)]
fn send(
    root: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<(std::io::BufReader<std::os::unix::net::UnixStream>, String), OrcaError> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let endpoint = endpoint(root).ok_or_else(|| OrcaError::local(UNAVAILABLE))?;
    let mut stream =
        UnixStream::connect(&endpoint.socket).map_err(|_| OrcaError::local(UNAVAILABLE))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|_| OrcaError::local(UNAVAILABLE))?;
    let id = request_id();
    let mut request = json!({
        "id": id,
        "authToken": endpoint.token,
        "method": method,
        "params": params,
    })
    .to_string();
    request.push('\n');
    stream
        .write_all(request.as_bytes())
        .map_err(|_| OrcaError::local(UNAVAILABLE))?;
    Ok((std::io::BufReader::new(stream), id))
}

/// Sends one request to the running Orca and returns its result.
#[cfg(unix)]
pub(super) fn call(
    root: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, OrcaError> {
    use std::io::BufRead;

    let (mut reader, id) = send(root, method, params, timeout)?;
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(OrcaError::local(NO_ANSWER));
        }
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| OrcaError::local(NO_ANSWER))?;
        if read == 0 {
            return Err(OrcaError::local(UNAVAILABLE));
        }
        let Ok(frame) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        // Orca sends keepalive frames during long requests.
        if let Frame::Unary(result) = classify_frame(&frame, &id) {
            return result;
        }
    }
}

#[cfg(not(unix))]
pub(super) fn call(
    _root: &Path,
    _method: &str,
    _params: Value,
    _timeout: Duration,
) -> Result<Value, OrcaError> {
    Err(OrcaError::local(
        "Halv can change Orca accounts only on macOS and Linux. Use Orca to change accounts.",
    ))
}

/// Orca's answer, with its error code. Callers use the code to find out
/// which methods this Orca has.
fn answer_result(frame: &Value) -> Result<Value, OrcaError> {
    if frame.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
    }
    let error = frame.get("error");
    let text = |key: &str| {
        error
            .and_then(|error| error.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let message = text("message").unwrap_or("Orca could not complete the request.");
    Err(OrcaError {
        code: text("code").map(str::to_string),
        message: format!("Orca: {message}"),
    })
}

fn request_id() -> String {
    let value = rand::random::<u128>();
    let hex = format!("{value:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Sends a change to Orca, and starts Orca first when it is closed.
fn call_starting(root: &Path, method: &str, params: Value) -> Result<Value, OrcaError> {
    if running(root) {
        match call(root, method, params.clone(), CHANGE_TIMEOUT) {
            Err(error) if !running(root) => eprintln!("wallet: orca stopped: {}", error.message),
            result => return result,
        }
    }
    start_orca().map_err(|message| OrcaError::local(&message))?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        // The account list answers only after Orca's account services
        // are ready, so it is the readiness probe.
        if running(root)
            && call(
                root,
                "accounts.list",
                json!({ "refreshUsage": false }),
                READ_TIMEOUT,
            )
            .is_ok()
        {
            return call(root, method, params, CHANGE_TIMEOUT);
        }
        if Instant::now() >= deadline {
            return Err(OrcaError::local(
                "Orca did not start in time. Open Orca, then change the account again.",
            ));
        }
        std::thread::sleep(START_POLL);
    }
}

/// Opens Orca in the background, without focus.
#[cfg(all(target_os = "macos", not(test)))]
fn start_orca() -> Result<(), String> {
    let status = std::process::Command::new("open")
        .args(["-g", "-b", APP_BUNDLE_ID])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| "Halv could not start Orca. Open Orca and try again.".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Halv could not find the Orca app. Open Orca and try again.".to_string())
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn start_orca() -> Result<(), String> {
    Err("Orca is not running. Open Orca and try again.".to_string())
}

/// Puts a snapshot into the mirror, as the watcher does.
#[cfg(test)]
pub(super) fn mirror_snapshot(root: &Path, snapshot: &Value) {
    let applied = update_mirror(root, mirror_generation(root), snapshot, MIRROR_LIFETIME);
    assert_eq!(applied, Some(true), "the test snapshot must be readable");
}

/// Writes a profile with only SQLite state, as a new Orca profile has: the
/// profile index and a database with Orca's schema and one `settings`
/// document.
#[cfg(test)]
pub(super) fn write_sqlite_profile(root: &Path, settings: &Value) {
    let profile = root.join(PROFILES_DIR).join("work");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        root.join(PROFILE_INDEX_FILE),
        r#"{"activeProfileId":"work","profiles":[{"id":"work"}]}"#,
    )
    .unwrap();
    let connection = create_database(&profile.join(DATABASE_FILE), false);
    write_document(&connection, SETTINGS_DOMAIN, settings, 1, "h1");
}

/// Creates a database with the shape of Orca's profile state: schema 3 of
/// `profile-state-database-schema.ts`, without the automation tables.
#[cfg(test)]
fn create_database(path: &Path, wal: bool) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(path).unwrap();
    if wal {
        let mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS profile_state_meta (
               key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS profile_state_documents (
               domain TEXT PRIMARY KEY NOT NULL, payload TEXT NOT NULL,
               domain_version INTEGER NOT NULL, revision INTEGER NOT NULL,
               updated_at INTEGER NOT NULL, content_hash TEXT NOT NULL
             );",
        )
        .unwrap();
    connection
}

/// Writes a domain document the way Orca does: the row and the global
/// revision move together.
#[cfg(test)]
fn write_document(
    connection: &rusqlite::Connection,
    domain: &str,
    payload: &Value,
    revision: i64,
    hash: &str,
) {
    connection
        .execute(
            "INSERT INTO profile_state_documents
               (domain, payload, domain_version, revision, updated_at, content_hash)
             VALUES (?1, ?2, 1, ?3, 0, ?4)
             ON CONFLICT(domain) DO UPDATE SET
               payload = excluded.payload, revision = excluded.revision,
               content_hash = excluded.content_hash",
            rusqlite::params![domain, payload.to_string(), revision, hash],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO profile_state_meta (key, value) VALUES ('revision', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [revision.to_string()],
        )
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("halv-orca-{name}-{:016x}", rand::random::<u64>()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// A data folder with one profile, as Orca 1.4 lays it out.
        fn profile(&self) -> PathBuf {
            let profile = self.0.join(PROFILES_DIR).join("work");
            fs::create_dir_all(&profile).unwrap();
            fs::write(
                self.0.join(PROFILE_INDEX_FILE),
                r#"{"activeProfileId":"work","profiles":[{"id":"work"}]}"#,
            )
            .unwrap();
            profile
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> Value {
        json!({
            "settings": {
                "claudeManagedAccounts": [
                    {
                        "id": "a1", "email": "one@example.com",
                        "managedAuthPath": "/orca/claude-accounts/a1/auth",
                        "managedAuthRuntime": "host", "organizationName": "One Org",
                        "createdAt": 1700000000000_i64
                    },
                    {
                        "id": "w1", "email": "wsl@example.com",
                        "managedAuthPath": "C:/x", "managedAuthRuntime": "wsl", "createdAt": 1
                    },
                    { "id": "../bad", "email": "bad@example.com", "createdAt": 1 }
                ],
                "activeClaudeManagedAccountId": "w1",
                "activeClaudeManagedAccountIdsByRuntime": { "host": "a1", "wsl": {} },
                "codexManagedAccounts": [
                    {
                        "id": "c1", "email": "codex@example.com",
                        "workspaceLabel": "Team", "createdAt": 5
                    }
                ],
                "activeCodexManagedAccountId": "c1"
            }
        })
    }

    /// Bare settings, as Orca keeps them in its SQLite `settings` document.
    fn settings(active_claude: &str) -> Value {
        json!({
            "claudeManagedAccounts": [
                { "id": "a1", "email": "one@example.com", "managedAuthPath": "/orca/claude-accounts/a1/auth", "managedAuthRuntime": "host", "createdAt": 1 },
                { "id": "a2", "email": "two@example.com", "managedAuthPath": "/orca/claude-accounts/a2/auth", "managedAuthRuntime": "host", "createdAt": 2 }
            ],
            "activeClaudeManagedAccountId": active_claude,
            "activeClaudeManagedAccountIdsByRuntime": { "host": active_claude, "wsl": {} },
            "codexManagedAccounts": [],
            "activeCodexManagedAccountIdsByRuntime": { "host": null, "wsl": {} },
            "theme": "dark"
        })
    }

    /// A snapshot with the shape of `accounts.list` in Orca 1.4.
    fn snapshot(active_claude: Option<&str>) -> Value {
        json!({
            "claude": {
                "accounts": [
                    { "id": "a1", "email": "one@example.com", "managedAuthRuntime": "host", "authMethod": "subscription-oauth", "createdAt": 1, "updatedAt": 1, "lastAuthenticatedAt": 1 },
                    { "id": "a2", "email": "two@example.com", "managedAuthRuntime": "host", "authMethod": "unknown", "createdAt": 2, "updatedAt": 2, "lastAuthenticatedAt": 2, "needsSignIn": true },
                    { "id": "n3", "email": "new@example.com", "managedAuthRuntime": "host", "authMethod": "subscription-oauth", "createdAt": 3, "updatedAt": 3, "lastAuthenticatedAt": 3 }
                ],
                "activeAccountId": active_claude,
                "activeAccountIdsByRuntime": { "host": active_claude, "wsl": {} }
            },
            "codex": {
                "accounts": [],
                "activeAccountId": null,
                "activeAccountIdsByRuntime": { "host": null, "wsl": {} },
                "systemDefault": { "hasAuth": true, "authKind": "oauth", "email": null, "providerAccountId": null, "workspaceLabel": null }
            },
            "rateLimits": {
                "inactiveClaudeAccounts": [
                    { "accountId": "a2", "updatedAt": 10, "isFetching": false,
                      "rateLimits": { "session": { "usedPercent": 40, "resetsAt": 99 } } }
                ],
                "inactiveCodexAccounts": []
            }
        })
    }

    fn sql_files(profile: &Path) -> Vec<String> {
        let mut names = fs::read_dir(profile)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.starts_with(DATABASE_FILE))
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn host_accounts_and_selection_are_read() {
        let store = parse_store(&sample(), Path::new("/orca"));
        assert_eq!(store.claude.active_id.as_deref(), Some("a1"));
        assert_eq!(store.claude.accounts.len(), 1);
        let account = &store.claude.accounts[0];
        assert_eq!(account.email, "one@example.com");
        assert_eq!(account.organization_name.as_deref(), Some("One Org"));
        assert_eq!(account.path, PathBuf::from("/orca/claude-accounts/a1/auth"));
        assert_eq!(account.created_at, 1_700_000_000_000);
        assert!(!account.needs_sign_in);
    }

    #[test]
    fn a_missing_codex_path_uses_orca_layout() {
        let store = parse_store(&sample(), Path::new("/orca"));
        assert_eq!(store.codex.active_id.as_deref(), Some("c1"));
        assert_eq!(
            store.codex.accounts[0].path,
            PathBuf::from("/orca/codex-accounts/c1/home")
        );
        assert_eq!(
            store.codex.accounts[0].organization_name.as_deref(),
            Some("Team")
        );
    }

    #[test]
    fn a_null_host_slot_means_system_default() {
        let data = json!({ "settings": {
            "activeClaudeManagedAccountId": "a1",
            "activeClaudeManagedAccountIdsByRuntime": { "host": null, "wsl": {} }
        }});
        let store = parse_store(&data, Path::new("/orca"));
        assert_eq!(store.claude.active_id, None);
    }

    #[test]
    fn a_change_answer_keeps_known_paths() {
        let known = parse_store(&sample(), Path::new("/orca")).claude;
        let answer = json!({
            "accounts": [
                { "id": "a1", "email": "one@example.com" },
                { "id": "n2", "email": "new@example.com" }
            ],
            "activeAccountId": "n2",
            "activeAccountIdsByRuntime": { "host": "n2", "wsl": {} }
        });
        let state =
            parse_provider_answer(WalletProvider::Claude, &answer, &known, Path::new("/orca"))
                .unwrap();
        assert_eq!(state.active_id.as_deref(), Some("n2"));
        assert_eq!(
            state.accounts[0].path,
            PathBuf::from("/orca/claude-accounts/a1/auth")
        );
        assert_eq!(
            state.accounts[1].path,
            PathBuf::from("/orca/claude-accounts/n2/auth")
        );
    }

    #[test]
    fn a_new_claude_account_takes_its_own_orca_folder() {
        let scratch = Scratch::new("claude-folder");
        let home = scratch.0.join(CLAUDE_PROFILES_DIR).join("n3").join("home");
        fs::create_dir_all(&home).unwrap();
        let (store, _) =
            parse_snapshot(&snapshot(None), &OrcaStore::default(), &scratch.0).expect("snapshot");
        let account = store.claude.find("n3").unwrap();
        assert_eq!(account.path, home);
        assert_eq!(
            store.claude.find("a1").unwrap().path,
            scratch.0.join("claude-accounts/a1/auth")
        );
    }

    #[test]
    fn a_snapshot_marks_accounts_that_need_a_sign_in() {
        let (store, usage) = parse_snapshot(
            &snapshot(Some("a1")),
            &OrcaStore::default(),
            Path::new("/orca"),
        )
        .expect("snapshot");
        assert_eq!(store.claude.active_id.as_deref(), Some("a1"));
        assert!(!store.claude.find("a1").unwrap().needs_sign_in);
        assert!(store.claude.find("a2").unwrap().needs_sign_in);
        assert_eq!(store.codex, OrcaProvider::default());
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].account_id, "a2");
        assert_eq!(
            parse_snapshot(
                &json!({ "claude": {} }),
                &OrcaStore::default(),
                Path::new("/orca")
            ),
            None,
            "a snapshot without accounts is not readable"
        );
    }

    #[test]
    fn inactive_usage_keeps_known_windows() {
        let snapshot = json!({ "rateLimits": {
            "inactiveClaudeAccounts": [
                {
                    "accountId": "a2", "updatedAt": 10, "isFetching": false,
                    "rateLimits": {
                        "session": { "usedPercent": 12.5, "windowMinutes": 300, "resetsAt": 99 },
                        "weekly": { "usedPercent": 40, "windowMinutes": 10080, "resetsAt": null },
                        "fableWeekly": null
                    }
                },
                { "accountId": "a3", "updatedAt": 0, "isFetching": true, "rateLimits": null }
            ],
            "inactiveCodexAccounts": []
        }});
        let usage = parse_inactive_usage(&snapshot);
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].account_id, "a2");
        assert_eq!(usage[0].windows.len(), 2);
        assert_eq!(usage[0].windows[0].kind, "session");
        assert_eq!(usage[0].windows[0].resets_at, Some(99));
        assert_eq!(usage[0].windows[1].resets_at, None);
    }

    #[test]
    fn an_error_answer_keeps_orca_code() {
        let frame = json!({ "id": "x", "ok": false, "error": { "code": "e", "message": "No such account" } });
        let error = answer_result(&frame).unwrap_err();
        assert_eq!(error.message, "Orca: No such account");
        assert_eq!(error.code.as_deref(), Some("e"));
        assert!(!error.unknown_method());
        let missing = json!({ "id": "x", "ok": false, "error": { "code": "method_not_found", "message": "Unknown method: accounts.subscribe" } });
        assert!(answer_result(&missing).unwrap_err().unknown_method());
        let unary = json!({ "id": "x", "ok": false, "error": { "code": "method_not_supported", "message": "Method accounts.subscribe requires a streaming transport" } });
        assert!(answer_result(&unary).unwrap_err().unknown_method());
        assert_eq!(String::from(OrcaError::local("x")), "x");
    }

    #[test]
    fn a_refused_claude_import_is_recognized() {
        let refused = OrcaError {
            code: Some("runtime_error".to_string()),
            message: "Orca: Update the Orca CLI to add accounts. Importing Claude logins is no longer supported.".to_string(),
        };
        assert!(claude_import_refused(&refused));
        assert!(claude_import_refused(&OrcaError {
            code: Some("method_not_found".to_string()),
            message: "Orca: Unknown method: accounts.addClaudeFromConfigDir".to_string(),
        }));
        assert!(!claude_import_refused(&OrcaError::local(UNAVAILABLE)));
    }

    #[test]
    fn frames_are_sorted_by_kind() {
        let id = "req-1";
        assert_eq!(
            classify_frame(&json!({ "_keepalive": true }), id),
            Frame::Keepalive
        );
        assert_eq!(
            classify_frame(&json!({ "id": "other", "ok": true, "result": {} }), id),
            Frame::Other
        );
        let ready = json!({ "id": id, "ok": true, "streaming": true, "_meta": { "runtimeId": "r" },
            "result": { "type": "ready", "subscriptionId": "accounts-inproc-1", "snapshot": { "claude": {} } } });
        assert_eq!(
            classify_frame(&ready, id),
            Frame::Snapshot(json!({ "claude": {} }))
        );
        let change = json!({ "id": id, "ok": true, "streaming": true,
            "result": { "type": "snapshot", "snapshot": { "codex": {} } } });
        assert_eq!(
            classify_frame(&change, id),
            Frame::Snapshot(json!({ "codex": {} }))
        );
        let end = json!({ "id": id, "ok": true, "streaming": true, "result": { "type": "end" } });
        assert_eq!(classify_frame(&end, id), Frame::End);
        let unknown =
            json!({ "id": id, "ok": true, "streaming": true, "result": { "type": "later" } });
        assert_eq!(classify_frame(&unknown, id), Frame::Other);
        let unary = json!({ "id": id, "ok": true, "result": { "claude": {} } });
        assert_eq!(
            classify_frame(&unary, id),
            Frame::Unary(Ok(json!({ "claude": {} })))
        );
        let not_supported = json!({ "id": id, "ok": false,
            "error": { "code": "method_not_supported", "message": "Method accounts.subscribe requires a streaming transport" } });
        let Frame::Unary(Err(error)) = classify_frame(&not_supported, id) else {
            panic!("expected an error answer");
        };
        assert!(error.unknown_method());
    }

    #[test]
    fn retries_wait_longer_up_to_a_limit() {
        let delays = (1..=7).map(retry_delay).collect::<Vec<_>>();
        let seconds = delays.iter().map(Duration::as_secs).collect::<Vec<_>>();
        assert_eq!(seconds, vec![2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn the_profile_index_selects_the_data_file() {
        let scratch = Scratch::new("index");
        let profile = scratch.profile();
        fs::write(profile.join(DATA_FILE), sample().to_string()).unwrap();
        let store = read_store(&scratch.0).unwrap();
        assert_eq!(store.claude.active_id.as_deref(), Some("a1"));

        // Orca reads the backup index when the index is torn.
        fs::write(scratch.0.join(PROFILE_INDEX_FILE), "{ torn").unwrap();
        fs::write(
            scratch.0.join(PROFILE_INDEX_BACKUP_FILE),
            r#"{"activeProfileId":"work"}"#,
        )
        .unwrap();
        assert_eq!(
            store_file(&scratch.0),
            Some(StoreFile::Legacy(profile.join(DATA_FILE)))
        );
    }

    #[test]
    fn sqlite_settings_give_the_active_account() {
        let scratch = Scratch::new("sqlite");
        let profile = scratch.profile();
        // The frozen JSON file names another account. SQLite wins.
        fs::write(
            profile.join(DATA_FILE),
            json!({ "settings": settings("a1") }).to_string(),
        )
        .unwrap();
        let database = profile.join(DATABASE_FILE);
        {
            let writer = create_database(&database, false);
            write_document(&writer, "terminal", &json!({ "tabs": [] }), 4, "t4");
            write_document(&writer, SETTINGS_DOMAIN, &settings("a2"), 5, "s5");
        }
        assert_eq!(
            store_file(&scratch.0),
            Some(StoreFile::Database(database.clone()))
        );
        let before = sql_files(&profile);

        let store = read_saved_store(&scratch.0).expect("store");

        assert_eq!(store.claude.active_id.as_deref(), Some("a2"));
        assert_eq!(store.claude.accounts.len(), 2);
        assert_eq!(
            store.claude.find("a2").unwrap().path,
            PathBuf::from("/orca/claude-accounts/a2/auth")
        );
        assert_eq!(store.codex.active_id, None);
        // Halv created no `-wal` or `-shm` file in Orca's folder.
        assert_eq!(sql_files(&profile), before);
    }

    #[test]
    fn an_open_database_is_read_again_only_after_a_new_revision() {
        let scratch = Scratch::new("sqlite-live");
        let profile = scratch.profile();
        let database = profile.join(DATABASE_FILE);
        // Orca keeps its WAL database open while it runs.
        let writer = create_database(&database, true);
        write_document(&writer, SETTINGS_DOMAIN, &settings("a1"), 1, "s1");
        assert!(side_file(&database, "-wal").exists());
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a1")
        );

        // A switch in Orca writes a new revision.
        write_document(&writer, SETTINGS_DOMAIN, &settings("a2"), 2, "s2");
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a2")
        );

        // Other documents move the files but not the settings revision, so
        // Halv keeps its parsed settings.
        write_document(&writer, "terminal", &json!({ "tabs": [1] }), 3, "t3");
        writer
            .execute(
                "UPDATE profile_state_documents SET payload = ?1 WHERE domain = 'settings'",
                [settings("a1").to_string()],
            )
            .unwrap();
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a2")
        );
        drop(writer);
    }

    #[test]
    fn the_watcher_announces_a_switch_in_the_database_while_the_mirror_is_down() {
        let scratch = Scratch::new("watch-sqlite");
        let root = scratch.0.clone();
        let profile = scratch.profile();
        // Orca has its database open, but no runtime socket answers.
        let writer = create_database(&profile.join(DATABASE_FILE), true);
        write_document(&writer, SETTINGS_DOMAIN, &settings("a1"), 1, "s1");
        let mut watcher = Watcher::new(&root);
        assert_eq!(
            watcher
                .last
                .as_ref()
                .and_then(|store| store.claude.active_id.as_deref()),
            Some("a1")
        );

        // Orca saves other documents often. They announce nothing.
        write_document(&writer, "terminal", &json!({ "tabs": [] }), 2, "t2");
        assert!(!watcher.step(&root));

        // A switch in Orca reaches Halv at the next step.
        write_document(&writer, SETTINGS_DOMAIN, &settings("a2"), 3, "s3");
        assert!(watcher.step(&root));
        assert_eq!(
            read_store(&root).unwrap().claude.active_id.as_deref(),
            Some("a2")
        );
        drop(writer);
    }

    #[test]
    fn json_counts_only_without_a_database() {
        let scratch = Scratch::new("json-only");
        let profile = scratch.profile();
        fs::write(
            profile.join(DATA_FILE),
            json!({ "settings": settings("a1") }).to_string(),
        )
        .unwrap();
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a1")
        );

        // A database side file alone means that SQLite owns the profile, so
        // the JSON file no longer counts.
        let database = profile.join(DATABASE_FILE);
        fs::write(side_file(&database, "-wal"), b"").unwrap();
        assert_eq!(
            store_file(&scratch.0),
            Some(StoreFile::Database(database.clone()))
        );
        assert_eq!(read_saved_store(&scratch.0), None);
        // Halv did not open the log without its index, so SQLite created
        // no file in Orca's folder.
        assert_eq!(sql_files(&profile), vec![format!("{DATABASE_FILE}-wal")]);
    }

    #[test]
    fn a_log_without_its_index_keeps_the_last_good_store() {
        let scratch = Scratch::new("sqlite-no-shm");
        let profile = scratch.profile();
        let database = profile.join(DATABASE_FILE);
        {
            let writer = create_database(&database, false);
            write_document(&writer, SETTINGS_DOMAIN, &settings("a1"), 1, "s1");
        }
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a1")
        );

        fs::write(side_file(&database, "-wal"), b"").unwrap();
        let before = sql_files(&profile);
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a1")
        );
        assert_eq!(sql_files(&profile), before, "no `-shm` file was created");
    }

    #[test]
    fn settings_in_an_unknown_format_are_not_read_as_no_accounts() {
        let scratch = Scratch::new("sqlite-format");
        let profile = scratch.profile();
        let writer = create_database(&profile.join(DATABASE_FILE), true);
        let active = || read_saved_store(&scratch.0).map(|store| store.claude.active_id);
        // A later Orca can keep its accounts in another place. Empty lists
        // would send Codex launches to the System default account.
        let unknown = json!({ "theme": "dark" });
        write_document(&writer, SETTINGS_DOMAIN, &unknown, 1, "s1");
        assert_eq!(active(), None, "no store before a good read");

        write_document(&writer, SETTINGS_DOMAIN, &settings("a1"), 2, "s2");
        assert_eq!(active(), Some(Some("a1".to_string())));

        write_document(&writer, SETTINGS_DOMAIN, &unknown, 3, "s3");
        assert_eq!(
            active(),
            Some(Some("a1".to_string())),
            "the last good store stays"
        );
        drop(writer);
    }

    #[test]
    fn orca_login_variables_are_removed_as_orca_removes_them() {
        use std::ffi::OsStr;
        assert_eq!(
            claude_auth_env_remove_for(None),
            vec![CLAUDE_BEDROCK_TOKEN_ENV]
        );
        assert_eq!(
            claude_auth_env_remove_for(Some(OsStr::new("X-Trace: 1"))),
            vec![CLAUDE_BEDROCK_TOKEN_ENV]
        );
        for headers in [
            "Authorization: Bearer test",
            "x-api-key: test",
            "X-Custom: BEARER",
        ] {
            assert_eq!(
                claude_auth_env_remove_for(Some(OsStr::new(headers))),
                vec![CLAUDE_BEDROCK_TOKEN_ENV, CLAUDE_CUSTOM_HEADERS_ENV]
            );
        }
    }

    #[test]
    fn a_sqlite_only_profile_is_an_orca_store() {
        let scratch = Scratch::new("sqlite-only");
        let profile = scratch.profile();
        let database = profile.join(DATABASE_FILE);
        drop(create_database(&database, false));
        assert_eq!(
            store_file(&scratch.0),
            Some(StoreFile::Database(database.clone()))
        );
        // A fresh profile has no settings document yet: default settings.
        assert_eq!(read_saved_store(&scratch.0), Some(OrcaStore::default()));
        assert_eq!(store_file(&scratch.0.join("absent")), None);
    }

    #[test]
    fn the_database_uri_escapes_the_path() {
        assert_eq!(
            database_uri(
                Path::new("/Users/me/Library/Application Support/orca/profile-state.db"),
                false
            ),
            "file:/Users/me/Library/Application%20Support/orca/profile-state.db?mode=ro"
        );
        assert_eq!(
            database_uri(Path::new("/tmp/a?b#c.db"), true),
            "file:/tmp/a%3Fb%23c.db?mode=ro&immutable=1"
        );
    }

    #[test]
    fn the_claude_pointer_selects_the_route() {
        let scratch = Scratch::new("pointer");
        let root = &scratch.0;
        assert_eq!(claude_route(root), ClaudeRoute::System, "no pointer");
        let profiles = root.join(CLAUDE_PROFILES_DIR);
        fs::create_dir_all(&profiles).unwrap();
        let pointer = profiles.join(CLAUDE_POINTER_FILE);
        fs::write(&pointer, "").unwrap();
        assert_eq!(claude_route(root), ClaudeRoute::System, "System default");

        let home = profiles.join("a1").join("home");
        let text = home.to_string_lossy().to_string();
        fs::write(&pointer, &text).unwrap();
        assert_eq!(
            claude_route(root),
            ClaudeRoute::NotSetUp(home.clone()),
            "no folder"
        );
        fs::create_dir_all(&home).unwrap();
        assert_eq!(
            claude_route(root),
            ClaudeRoute::NotSetUp(home.clone()),
            "no setup marker"
        );
        fs::write(profiles.join("a1").join(CLAUDE_PROFILE_MARKER), "{}\n").unwrap();
        assert_eq!(claude_route(root), ClaudeRoute::Profile(home.clone()));
        fs::write(&pointer, format!("{text}\n")).unwrap();
        assert_eq!(claude_route(root), ClaudeRoute::Profile(home.clone()));

        fs::write(&pointer, "claude-profiles/a1/home").unwrap();
        assert_eq!(
            claude_route(root),
            ClaudeRoute::Unreadable,
            "a relative pointer"
        );
        fs::write(&pointer, [0xff, 0xfe]).unwrap();
        assert_eq!(claude_route(root), ClaudeRoute::Unreadable, "not UTF-8");
        assert!(claude_profiles_in_use(root));
    }

    #[test]
    fn an_older_answer_does_not_undo_a_change() {
        let scratch = Scratch::new("generation");
        let root = &scratch.0;
        mirror_snapshot(root, &snapshot(Some("a1")));
        let before_change = mirror_generation(root);

        let answer = snapshot(Some("n3"))["claude"].clone();
        assert!(apply_answer(root, WalletProvider::Claude, &answer));
        assert_eq!(
            read_store(root).unwrap().claude.active_id.as_deref(),
            Some("n3")
        );

        // A poll that started before the change answers after it.
        let stale = update_mirror(root, before_change, &snapshot(Some("a1")), MIRROR_LIFETIME);
        assert_eq!(stale, Some(false));
        assert_eq!(
            read_store(root).unwrap().claude.active_id.as_deref(),
            Some("n3")
        );

        mark_offline(root);
        assert_eq!(read_store(root), None, "no mirror and no saved store");
    }

    #[test]
    fn the_mirror_and_the_saved_store_give_the_same_accounts() {
        let root = Path::new("/orca");
        let account = |id: &str, created: i64, updated: i64| {
            json!({
                "id": id, "email": format!("{id}@example.com"),
                "managedAuthPath": format!("/orca/claude-accounts/{id}/auth"),
                "organizationUuid": format!("org-{id}"),
                "createdAt": created, "updatedAt": updated
            })
        };
        // Orca's settings keep the saved order. Orca's answers list the most
        // recently used account first.
        let saved = parse_settings(
            &json!({
                "claudeManagedAccounts": [account("a2", 2, 5), account("a1", 1, 3), account("a0", 1, 9)],
                "activeClaudeManagedAccountIdsByRuntime": { "host": "a1", "wsl": {} },
                "codexManagedAccounts": []
            }),
            root,
        );
        let answer = json!({
            "claude": {
                "accounts": [account("a0", 1, 9), account("a2", 2, 5), account("a1", 1, 3)],
                "activeAccountId": "a1",
                "activeAccountIdsByRuntime": { "host": "a1", "wsl": {} }
            },
            "codex": { "accounts": [], "activeAccountId": null }
        });

        let (mirror, _) = parse_snapshot(&answer, &saved, root).expect("snapshot");

        assert_eq!(mirror, saved);
        let ids = saved
            .claude
            .accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["a0", "a1", "a2"], "the oldest account comes first");
    }

    #[test]
    fn saved_claude_accounts_without_a_login_need_a_sign_in() {
        let scratch = Scratch::new("sign-in-saved");
        let root = scratch.0.clone();
        let account = |id: &str, email: &str| {
            json!({
                "id": id, "email": email, "managedAuthRuntime": "host", "createdAt": 1,
                "managedAuthPath": root.join(CLAUDE_ACCOUNTS_DIR).join(id).join("auth")
            })
        };
        write_sqlite_profile(
            &root,
            &json!({
                "claudeManagedAccounts": [
                    account("a1", "old@example.com"),
                    account("a2", "two@example.com"),
                    account("a3", "three@example.com")
                ],
                "activeClaudeManagedAccountIdsByRuntime": { "host": "a1", "wsl": {} },
                "codexManagedAccounts": []
            }),
        );
        // Orca gives each account its own folder. Only a1 signed in there.
        let home = claude_profile_home(&root, "a1");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join(".claude.json"),
            json!({ "oauthAccount": {
                "emailAddress": "one@example.com", "organizationUuid": "o1", "organizationName": "One Org"
            }})
            .to_string(),
        )
        .unwrap();
        let system = AccountIdentity {
            email: "TWO@example.com".to_string(),
            organization_id: None,
            organization_name: None,
            plan: None,
        };

        let store = mark_claude_sign_ins(&root, read_saved_store(&root).unwrap(), Some(&system));

        let a1 = store.claude.find("a1").unwrap();
        assert_eq!(
            a1.email, "one@example.com",
            "the login of the folder names it"
        );
        assert_eq!(a1.organization_name.as_deref(), Some("One Org"));
        assert!(!a1.needs_sign_in);
        assert!(
            !store.claude.find("a2").unwrap().needs_sign_in,
            "System default holds the same login"
        );
        assert!(store.claude.find("a3").unwrap().needs_sign_in);

        // Orca is closed. Halv refuses the account and does not start Orca.
        let error = select(&root, WalletProvider::Claude, Some("a3")).unwrap_err();
        assert_eq!(
            error,
            "Sign in to this Claude account again in Orca, then select it."
        );
    }

    #[test]
    fn a_wal_database_is_never_opened_in_a_way_that_writes_orca_files() {
        let scratch = Scratch::new("sqlite-journal");
        let profile = scratch.profile();
        let database = profile.join(DATABASE_FILE);
        {
            let writer = create_database(&database, true);
            write_document(&writer, SETTINGS_DOMAIN, &settings("a1"), 1, "s1");
        }
        // Orca closed the database, and an old `-journal` file stayed.
        assert!(!side_file(&database, "-wal").exists());
        fs::write(side_file(&database, "-journal"), b"").unwrap();
        let before = sql_files(&profile);
        assert_eq!(
            read_saved_store(&scratch.0)
                .unwrap()
                .claude
                .active_id
                .as_deref(),
            Some("a1")
        );
        assert_eq!(sql_files(&profile), before, "no `-wal` or `-shm` file");

        // SQLite deletes the `-wal` file of an empty database.
        let empty = Scratch::new("sqlite-empty");
        let profile = empty.profile();
        let database = profile.join(DATABASE_FILE);
        fs::write(&database, b"").unwrap();
        fs::write(side_file(&database, "-wal"), b"frames").unwrap();
        fs::write(side_file(&database, "-shm"), b"").unwrap();
        let before = sql_files(&profile);
        assert_eq!(read_saved_store(&empty.0), None);
        assert_eq!(sql_files(&profile), before, "the `-wal` file stays");
    }

    /// The pid of a process that ended.
    #[cfg(unix)]
    fn ended_process() -> i64 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        i64::from(pid)
    }

    #[cfg(unix)]
    #[test]
    fn a_quitting_orca_keeps_the_last_store_until_it_exits() {
        let scratch = Scratch::new("quit");
        let root = scratch.0.clone();
        let profile = scratch.profile();
        let writer = create_database(&profile.join(DATABASE_FILE), true);
        write_document(&writer, SETTINGS_DOMAIN, &settings("a1"), 1, "s1");
        fs::write(
            root.join(RUNTIME_FILE),
            json!({
                "pid": std::process::id(),
                "transports": [{ "kind": "unix", "endpoint": root.join("o.sock") }]
            })
            .to_string(),
        )
        .unwrap();
        // Orca switched to n3 and answered before it saved the switch.
        mirror_snapshot(&root, &snapshot(Some("n3")));
        let active = || read_store(&root).and_then(|store| store.claude.active_id);

        // Orca stops its socket while it quits. Its process still runs.
        fs::remove_file(root.join(RUNTIME_FILE)).unwrap();
        mark_offline(&root);
        assert_eq!(mirrored_store(&root), None);
        assert_eq!(active().as_deref(), Some("n3"), "no read while Orca quits");

        // The process ended, so Orca's files no longer change.
        let ended = ended_process();
        with_mirror(&root, |slot| {
            slot.mirror.as_mut().unwrap().pid = Some(ended)
        });
        assert_eq!(active().as_deref(), Some("a1"));

        // An Orca that does not answer for long is not quitting.
        let long_ago = Instant::now().checked_sub(MIRROR_LIFETIME + OFFLINE_HOLD);
        with_mirror(&root, |slot| {
            let mirror = slot.mirror.as_mut().unwrap();
            mirror.pid = Some(i64::from(std::process::id()));
            if let Some(long_ago) = long_ago {
                mirror.live_until = long_ago;
            }
        });
        if long_ago.is_some() {
            assert_eq!(active().as_deref(), Some("a1"));
        }
        drop(writer);
    }

    #[test]
    fn an_account_that_needs_a_sign_in_is_not_selected() {
        let scratch = Scratch::new("needs-sign-in");
        let root = &scratch.0;
        mirror_snapshot(root, &snapshot(Some("a1")));
        let error = select(root, WalletProvider::Claude, Some("a2")).unwrap_err();
        assert_eq!(
            error,
            "Sign in to this Claude account again in Orca, then select it."
        );
    }

    /// A fake Orca runtime socket. It answers each connection with the
    /// frames that `answer` returns for the request method.
    #[cfg(unix)]
    fn fake_orca(
        root: &Path,
        answer: impl Fn(&str, &str) -> Vec<Value> + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let socket = root.join("o.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::write(
            root.join(RUNTIME_FILE),
            json!({
                "pid": std::process::id(),
                "authToken": "test-token",
                "transports": [{ "kind": "unix", "endpoint": socket }]
            })
            .to_string(),
        )
        .unwrap();
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let mut line = String::new();
                if BufReader::new(&connection).read_line(&mut line).is_err() {
                    return;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["authToken"], "test-token");
                let id = request["id"].as_str().unwrap().to_string();
                let method = request["method"].as_str().unwrap().to_string();
                // No frames close the connection without an answer.
                for frame in answer(&method, &id) {
                    if connection
                        .write_all(format!("{frame}\n").as_bytes())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        })
    }

    #[cfg(unix)]
    #[test]
    fn the_mirror_polls_when_orca_cannot_stream() {
        let scratch = Scratch::new("poll");
        let root = scratch.0.clone();
        let calls = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = calls.clone();
        let _server = fake_orca(&root, move |method, id| {
            seen.lock().unwrap().push(method.to_string());
            match method {
                "accounts.subscribe" => vec![json!({ "id": id, "ok": false,
                    "error": { "code": "method_not_supported", "message": "Method accounts.subscribe requires a streaming transport" } })],
                "accounts.list" => vec![
                    json!({ "_keepalive": true }),
                    json!({ "id": id, "ok": true, "result": snapshot(Some("a1")) }),
                ],
                _ => Vec::new(),
            }
        });

        let mut link = Link::default();
        link.step(&root);

        assert!(link.polling);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                "accounts.subscribe".to_string(),
                "accounts.list".to_string()
            ]
        );
        let store = read_store(&root).expect("mirror");
        assert_eq!(store.claude.active_id.as_deref(), Some("a1"));
        assert_eq!(
            inactive_usage(&root).unwrap()[0].account_id,
            "a2",
            "usage comes from the mirror"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_stream_feeds_the_mirror_until_it_ends() {
        let scratch = Scratch::new("stream");
        let root = scratch.0.clone();
        let _server = fake_orca(&root, move |method, id| match method {
            "accounts.subscribe" => vec![
                json!({ "id": id, "ok": true, "streaming": true,
                    "result": { "type": "ready", "subscriptionId": "accounts-1", "snapshot": snapshot(Some("a1")) } }),
                json!({ "id": id, "ok": true, "streaming": true,
                    "result": { "type": "snapshot", "snapshot": snapshot(None) } }),
                json!({ "id": id, "ok": true, "streaming": true, "result": { "type": "end" } }),
            ],
            _ => Vec::new(),
        });

        let mut link = Link::default();
        link.step(&root);
        assert!(link.stream.is_some());
        assert_eq!(
            read_store(&root).unwrap().claude.active_id.as_deref(),
            Some("a1")
        );

        link.step(&root);
        let store = read_store(&root).unwrap();
        assert_eq!(store.claude.active_id, None, "the change in Orca arrived");
        assert_eq!(store.claude.accounts.len(), 3);

        link.step(&root);
        assert!(link.stream.is_none());
        assert_eq!(link.failures, 1);
        assert_eq!(mirrored_store(&root), None, "the mirror is down");
        assert_eq!(
            read_store(&root).map(|store| store.claude.accounts.len()),
            Some(3),
            "the last store stays while that Orca runs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn switches_in_halv_and_in_orca_reach_the_mirror() {
        let scratch = Scratch::new("switch");
        let root = scratch.0.clone();
        let active = std::sync::Arc::new(Mutex::new("a1".to_string()));
        let orca_active = active.clone();
        let _server = fake_orca(&root, move |method, id| {
            let mut active = orca_active.lock().unwrap();
            match method {
                "accounts.subscribe" => vec![json!({ "id": id, "ok": false,
                    "error": { "code": "method_not_found", "message": "Unknown method: accounts.subscribe" } })],
                "accounts.list" => {
                    vec![json!({ "id": id, "ok": true, "result": snapshot(Some(active.as_str())) })]
                }
                "accounts.selectClaude" => {
                    *active = "n3".to_string();
                    vec![json!({ "id": id, "ok": true, "result": snapshot(Some("n3"))["claude"] })]
                }
                _ => Vec::new(),
            }
        });
        let active_id = || read_store(&root).unwrap().claude.active_id;

        let mut watcher = Watcher::new(&root);
        assert!(watcher.step(&root), "the first poll fills the mirror");
        assert!(watcher.link.polling, "Orca without the method gets polls");
        assert_eq!(active_id().as_deref(), Some("a1"));

        // A switch in Halv shows at once and stays after the next poll.
        select(&root, WalletProvider::Claude, Some("n3")).unwrap();
        assert_eq!(active_id().as_deref(), Some("n3"));
        watcher.link.next_attempt = None;
        assert!(watcher.step(&root), "the watcher announces the switch once");
        watcher.link.next_attempt = None;
        assert!(!watcher.step(&root), "an equal poll announces nothing");
        assert_eq!(active_id().as_deref(), Some("n3"));

        // A switch in Orca arrives with the next poll.
        *active.lock().unwrap() = "a1".to_string();
        watcher.link.next_attempt = None;
        assert!(watcher.step(&root));
        assert_eq!(active_id().as_deref(), Some("a1"));
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_claude_import_asks_for_orca() {
        let scratch = Scratch::new("import");
        let root = scratch.0.clone();
        let _server = fake_orca(&root, move |method, id| match method {
            "accounts.addClaudeFromConfigDir" => vec![json!({ "id": id, "ok": false,
                "error": { "code": "runtime_error",
                    "message": "Update the Orca CLI to add accounts. Importing Claude logins is no longer supported." } })],
            _ => Vec::new(),
        });
        let error =
            add_from_folder(&root, WalletProvider::Claude, &root.join("sign-in")).unwrap_err();
        assert_eq!(error, CLAUDE_ADD_IN_ORCA);
    }

    /// The Codex part of an Orca answer with these accounts.
    #[cfg(unix)]
    fn codex_state(ids: &[&str]) -> Value {
        let accounts = ids
            .iter()
            .map(|id| json!({ "id": id, "email": format!("{id}@example.com"), "createdAt": 1 }))
            .collect::<Vec<_>>();
        json!({ "accounts": accounts, "activeAccountId": null })
    }

    #[cfg(unix)]
    #[test]
    fn an_add_selects_only_an_account_that_is_new() {
        for list_answers in [true, false] {
            let scratch = Scratch::new("add");
            let root = scratch.0.clone();
            // Halv cannot read Orca's store, so it asks the running Orca.
            let _server = fake_orca(&root, move |method, id| match method {
                "accounts.list" if list_answers => vec![json!({ "id": id, "ok": true,
                    "result": { "claude": { "accounts": [] }, "codex": codex_state(&["c-old"]) } })],
                "accounts.list" => vec![json!({ "id": id, "ok": false,
                    "error": { "code": "runtime_error", "message": "Busy" } })],
                "accounts.addCodexFromHome" => {
                    vec![
                        json!({ "id": id, "ok": true, "result": codex_state(&["c-old", "c-new"]) }),
                    ]
                }
                _ => Vec::new(),
            });

            let added = add_from_folder(&root, WalletProvider::Codex, &root.join("sign-in"));

            // Without the accounts from before, every account looks new.
            let expected = list_answers.then(|| "c-new".to_string());
            assert_eq!(added, Ok(expected), "list answers: {list_answers}");
        }
    }
}
