//! Account wallet for the Claude Code and Codex CLIs.
//!
//! The user keeps several Claude and ChatGPT logins and picks one active
//! account per provider. Each managed account lives in its own CLI folder
//! (`CLAUDE_CONFIG_DIR` or `CODEX_HOME`) under the Halv app data folder. A
//! new CLI launch gets the folder of the active account. A running CLI keeps
//! the folder it started with. "System default" means no managed account:
//! the CLI uses the user's own folder, as before the wallet.
//!
//! The design adapts the managed accounts of Orca (stablyai/orca, MIT).
//! When Orca is installed, Halv uses Orca's account store instead of its
//! own registry. See `orca.rs`.

mod claude;
mod codex;
mod files;
mod launches;
mod orca;

pub(crate) use claude::read_keychain;

use crate::agent_sessions::{AgentKind, AgentSessionManager, SessionLaneKind, SessionMode};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};
use tauri::{AppHandle, Emitter, State};

const ROOT_DIR: &str = "accounts";
const REGISTRY_FILE: &str = "wallet.json";
const CORRUPT_REGISTRY_FILE: &str = "wallet.json.corrupt";
const MARKER_FILE: &str = "account.json";
const HOME_DIR: &str = "home";
const REGISTRY_VERSION: u32 = 1;
/// A sign-in folder without a login is kept this long, because a login
/// terminal can outlive an app restart.
const PENDING_GRACE: Duration = Duration::from_secs(6 * 60 * 60);
pub(crate) const WALLET_CHANGED_EVENT: &str = "wallet-changed";

static WALLET: Mutex<Option<Wallet>> = Mutex::new(None);
/// Sign-ins that Halv gave to Orca, or is giving now. The UI can finish a
/// sign-in twice: on the terminal exit and on "Save account".
static ORCA_ADDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Parsed Claude state files, keyed by path. See `cached_identity`.
static CLAUDE_IDENTITIES: Mutex<Option<HashMap<PathBuf, CachedIdentity>>> = Mutex::new(None);
/// The account folder of each agent pane. See `launches.rs`.
static LAUNCHES: Mutex<Option<launches::LaunchRegistry>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WalletProvider {
    Claude,
    Codex,
}

impl WalletProvider {
    const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    fn folder(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    fn home_variable(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE_CONFIG_DIR",
            Self::Codex => "CODEX_HOME",
        }
    }

    pub(crate) fn agent_kind(self) -> AgentKind {
        match self {
            Self::Claude => AgentKind::Claude,
            Self::Codex => AgentKind::Codex,
        }
    }

    fn subscription_lane(self) -> SessionLaneKind {
        match self {
            Self::Claude => SessionLaneKind::ClaudeSubscription,
            Self::Codex => SessionLaneKind::CodexChatGpt,
        }
    }
}

/// The login that a CLI folder holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AccountIdentity {
    pub email: String,
    /// Claude organization UUID, or ChatGPT account (workspace) ID.
    pub organization_id: Option<String>,
    pub organization_name: Option<String>,
    pub plan: Option<String>,
}

impl AccountIdentity {
    /// One email can belong to several organizations or workspaces. Each pair
    /// is a separate account.
    fn same_account(&self, record: &AccountRecord) -> bool {
        self.email.eq_ignore_ascii_case(&record.email)
            && self.organization_id == record.organization_id
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registry {
    #[serde(default = "registry_version")]
    version: u32,
    #[serde(default)]
    claude: ProviderRegistry,
    #[serde(default)]
    codex: ProviderRegistry,
}

fn registry_version() -> u32 {
    REGISTRY_VERSION
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderRegistry {
    #[serde(default)]
    active_id: Option<String>,
    #[serde(default)]
    accounts: Vec<AccountRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountRecord {
    id: String,
    email: String,
    #[serde(default)]
    organization_id: Option<String>,
    #[serde(default)]
    organization_name: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountMarker {
    version: u32,
    provider: WalletProvider,
    id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletState {
    pub claude: WalletProviderState,
    pub codex: WalletProviderState,
    /// True when Halv could not read the registry at startup and rebuilt it
    /// from the account folders.
    pub recovered: bool,
    /// Where the accounts live.
    pub store: WalletStoreKind,
}

/// The owner of the account list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WalletStoreKind {
    /// Halv's own registry below the app data folder.
    Halv,
    /// Orca's account store. Orca also owns the active account.
    Orca,
}

/// The usage of one window of an account that is not active.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletUsageWindow {
    pub kind: &'static str,
    pub used_percent: f64,
    /// Unix milliseconds.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletAccountUsage {
    pub provider: WalletProvider,
    pub account_id: String,
    pub windows: Vec<WalletUsageWindow>,
    /// Unix milliseconds.
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletProviderState {
    pub active_id: Option<String>,
    pub system_default: SystemDefaultAccount,
    pub accounts: Vec<WalletAccount>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDefaultAccount {
    pub email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletAccount {
    pub id: String,
    pub email: String,
    pub organization_name: Option<String>,
    pub plan: Option<String>,
    /// The account folder is gone from disk. The account cannot launch.
    pub missing: bool,
    /// Orca refuses to select the account until the user signs in to it
    /// again in Orca. Halv's own accounts never set it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_sign_in: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletSelectResult {
    pub state: WalletState,
    /// Agent panes that still run another account than the active account.
    pub stale_sessions: Vec<WalletStaleSession>,
}

/// A live agent pane that still runs an account that is no longer active.
/// A restart with the CLI session resumed moves it to the active account.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletStaleSession {
    pub session_id: String,
    pub provider: WalletProvider,
    /// The active account when the pane started. `None` is System default.
    pub previous_account_id: Option<String>,
    /// The active account now. `None` is System default.
    pub next_account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletAddFinishResult {
    pub state: WalletState,
    /// False while the sign-in folder holds no login yet.
    pub added: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletPendingAdd {
    pub pending_id: String,
    pub terminal_id: String,
}

/// The folder a provider's CLI uses for the active account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActiveHome {
    System,
    Managed(PathBuf),
    /// The active account folder is gone. Halv refuses to launch with it,
    /// because a fallback would run the wrong account.
    Missing(PathBuf),
    /// A folder that Orca owns. Halv points the CLI at it, but never
    /// changes it.
    Linked(PathBuf),
    /// An Orca account that Halv cannot use yet: Orca has not set up its
    /// folder, or Halv cannot read Orca's choice now. Halv refuses to
    /// launch, because a fallback would run another account.
    NotReady(Option<PathBuf>),
}

/// Environment changes that point a CLI at an account folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AccountEnvironment {
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
}

impl AccountEnvironment {
    pub(crate) fn apply(&self, command: &mut std::process::Command) {
        for name in &self.remove {
            command.env_remove(name);
        }
        for (name, value) in &self.set {
            command.env(name, value);
        }
    }

    pub(crate) fn apply_tokio(&self, command: &mut tokio::process::Command) {
        for name in &self.remove {
            command.env_remove(name);
        }
        for (name, value) in &self.set {
            command.env(name, value);
        }
    }

    fn extend(&mut self, other: AccountEnvironment) {
        self.remove.extend(other.remove);
        self.set.extend(other.set);
    }
}

struct PendingAccount {
    id: String,
    home: PathBuf,
}

struct CachedIdentity {
    modified: Option<SystemTime>,
    length: u64,
    identity: Option<AccountIdentity>,
}

/// The user's own CLI folders. Managed accounts share resources and history
/// with them.
#[derive(Debug, Clone, Default)]
struct SystemFolders {
    claude: Option<PathBuf>,
    claude_state: Option<PathBuf>,
    codex: Option<PathBuf>,
}

impl SystemFolders {
    fn from_environment() -> Self {
        Self {
            claude: claude::system_config_dir(),
            claude_state: claude::system_state_file(),
            codex: codex::system_home(),
        }
    }
}

pub(crate) struct Wallet {
    root: PathBuf,
    registry: Registry,
    recovered: bool,
    /// Tests replace the user's folders. `None` reads the environment.
    system: Option<SystemFolders>,
    /// Orca's data folder. Orca owns the accounts when the Orca app is on
    /// this computer and its active profile holds Orca's store. `None`
    /// turns the Orca store off.
    orca: Option<PathBuf>,
    /// Finds the Orca app. Tests replace it.
    orca_installed: fn(&Path) -> bool,
}

impl Wallet {
    /// Opens the wallet below `root`. An unreadable registry is moved aside,
    /// and Halv rebuilds the account list from the account folders on disk.
    pub(crate) fn open(root: PathBuf) -> Self {
        let mut wallet = Self::open_with(root, None);
        wallet.orca = orca::data_root();
        // Why now: the first check for the Orca app can start a slow lookup
        // on its own thread. Halv starts it before a launch needs the answer.
        if let Some(root) = wallet
            .orca
            .as_deref()
            .filter(|root| orca::store_file(root).is_some())
        {
            orca::installed(root);
        }
        wallet
    }

    fn open_with(root: PathBuf, system: Option<SystemFolders>) -> Self {
        let path = root.join(REGISTRY_FILE);
        let mut recovered = false;
        let registry = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Registry>(&bytes) {
                Ok(registry) => sanitize(registry),
                Err(_) => {
                    let backup = root.join(CORRUPT_REGISTRY_FILE);
                    if let Err(error) = fs::rename(&path, &backup) {
                        eprintln!("wallet: could not move the unreadable registry aside: {error}");
                    }
                    recovered = true;
                    Registry::default()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Registry::default(),
            Err(error) => {
                eprintln!("wallet: could not read the account list: {error}");
                recovered = true;
                Registry::default()
            }
        };
        let mut wallet = Self {
            root,
            registry,
            recovered,
            system,
            orca: None,
            orca_installed: orca::installed,
        };
        if wallet.sweep_orphans() {
            if let Err(error) = wallet.save() {
                eprintln!("wallet: could not save the account list: {error}");
            }
        }
        wallet
    }

    fn system_folders(&self) -> SystemFolders {
        self.system
            .clone()
            .unwrap_or_else(SystemFolders::from_environment)
    }

    /// Orca's data folder when Orca owns the accounts. A new Orca profile
    /// keeps its store only in SQLite, so either store file counts.
    fn orca_root(&self) -> Option<&Path> {
        self.orca
            .as_deref()
            .filter(|root| orca::store_file(root).is_some() && (self.orca_installed)(root))
    }

    fn orca_store(&self) -> Option<orca::OrcaStore> {
        orca::read_store(self.orca_root()?)
    }

    /// The id of the active account, without the rest of the state.
    fn active_id(&self, provider: WalletProvider) -> Option<String> {
        match self.orca_root() {
            Some(root) => orca::read_store(root).and_then(|store| {
                let state = store.provider(provider);
                state
                    .active_id
                    .clone()
                    .filter(|id| state.find(id).is_some())
            }),
            None => self.provider(provider).active_id.clone(),
        }
    }

    /// The folder of a sign-in that `begin_add` prepared.
    fn pending_home(&self, provider: WalletProvider, id: &str) -> Result<PathBuf, String> {
        if !valid_account_id(id) || !self.owns_folder(provider, id) {
            return Err("Halv could not find this sign-in. Start the sign-in again.".to_string());
        }
        Ok(self.home(provider, id))
    }

    fn provider(&self, provider: WalletProvider) -> &ProviderRegistry {
        match provider {
            WalletProvider::Claude => &self.registry.claude,
            WalletProvider::Codex => &self.registry.codex,
        }
    }

    fn provider_mut(&mut self, provider: WalletProvider) -> &mut ProviderRegistry {
        match provider {
            WalletProvider::Claude => &mut self.registry.claude,
            WalletProvider::Codex => &mut self.registry.codex,
        }
    }

    fn provider_dir(&self, provider: WalletProvider) -> PathBuf {
        self.root.join(provider.folder())
    }

    fn account_dir(&self, provider: WalletProvider, id: &str) -> PathBuf {
        self.provider_dir(provider).join(id)
    }

    fn home(&self, provider: WalletProvider, id: &str) -> PathBuf {
        self.account_dir(provider, id).join(HOME_DIR)
    }

    fn find(&self, provider: WalletProvider, id: &str) -> Option<&AccountRecord> {
        self.provider(provider)
            .accounts
            .iter()
            .find(|account| account.id == id)
    }

    fn save(&self) -> Result<(), String> {
        files::create_private_dir(&self.root)
            .map_err(|error| format!("Halv could not create the account folder: {error}"))?;
        let mut text = serde_json::to_string_pretty(&self.registry)
            .map_err(|error| format!("Halv could not encode the account list: {error}"))?;
        text.push('\n');
        files::write_private_file(&self.root.join(REGISTRY_FILE), text.as_bytes())
            .map_err(|error| format!("Halv could not save the account list: {error}"))
    }

    pub(crate) fn active_home(&self, provider: WalletProvider) -> ActiveHome {
        if let Some(root) = self.orca_root() {
            return orca_active_home(root, provider);
        }
        match self.provider(provider).active_id.as_deref() {
            None => ActiveHome::System,
            Some(id) => {
                let home = self.home(provider, id);
                if home.is_dir() {
                    ActiveHome::Managed(home)
                } else {
                    ActiveHome::Missing(home)
                }
            }
        }
    }

    pub(crate) fn state(&self) -> WalletState {
        if let Some(root) = self.orca_root() {
            // Why an empty list when Halv cannot read Orca's store: Halv's own
            // registry would show accounts that no launch uses.
            let store = orca::read_store(root).unwrap_or_default();
            return WalletState {
                claude: orca_provider_state(
                    WalletProvider::Claude,
                    store.provider(WalletProvider::Claude),
                ),
                codex: orca_provider_state(
                    WalletProvider::Codex,
                    store.provider(WalletProvider::Codex),
                ),
                recovered: false,
                store: WalletStoreKind::Orca,
            };
        }
        WalletState {
            claude: self.provider_state(WalletProvider::Claude),
            codex: self.provider_state(WalletProvider::Codex),
            recovered: self.recovered,
            store: WalletStoreKind::Halv,
        }
    }

    fn provider_state(&self, provider: WalletProvider) -> WalletProviderState {
        let registry = self.provider(provider);
        WalletProviderState {
            active_id: registry.active_id.clone(),
            system_default: system_default_account(provider),
            accounts: registry
                .accounts
                .iter()
                .map(|account| {
                    let home = self.home(provider, &account.id);
                    let missing = !home.is_dir();
                    // The folder is the truth after a re-login. The registry
                    // keeps the plan, which only the add step reads for Claude.
                    let live = (!missing).then(|| live_identity(provider, &home)).flatten();
                    WalletAccount {
                        id: account.id.clone(),
                        email: live
                            .as_ref()
                            .map(|identity| identity.email.clone())
                            .unwrap_or_else(|| account.email.clone()),
                        organization_name: live
                            .as_ref()
                            .map(|identity| identity.organization_name.clone())
                            .unwrap_or_else(|| account.organization_name.clone()),
                        plan: live
                            .and_then(|identity| identity.plan)
                            .or_else(|| account.plan.clone()),
                        missing,
                        needs_sign_in: false,
                        created_at: account.created_at.clone(),
                    }
                })
                .collect(),
        }
    }

    pub(crate) fn select(
        &mut self,
        provider: WalletProvider,
        account_id: Option<&str>,
    ) -> Result<(), String> {
        if let Some(id) = account_id {
            if self.find(provider, id).is_none() {
                return Err(format!(
                    "That {} account is no longer in the wallet.",
                    provider.display_name()
                ));
            }
            if !self.home(provider, id).is_dir() {
                return Err(format!(
                    "The folder of this {} account is missing. Remove the account and add it again.",
                    provider.display_name()
                ));
            }
        }
        let previous = self.provider(provider).active_id.clone();
        self.provider_mut(provider).active_id = account_id.map(str::to_string);
        if let Err(error) = self.save() {
            self.provider_mut(provider).active_id = previous;
            return Err(error);
        }
        Ok(())
    }

    fn begin_add(&self, provider: WalletProvider) -> Result<PendingAccount, String> {
        let id = new_account_id();
        let account_dir = self.account_dir(provider, &id);
        files::create_private_dir(&self.root)
            .and_then(|()| files::create_private_dir(&self.provider_dir(provider)))
            .and_then(|()| files::create_private_dir(&account_dir))
            .map_err(|error| format!("Halv could not create the account folder: {error}"))?;
        let marker = AccountMarker {
            version: REGISTRY_VERSION,
            provider,
            id: id.clone(),
        };
        let marker_text = serde_json::to_string(&marker).map_err(|error| error.to_string())?;
        let result =
            files::write_private_file(&account_dir.join(MARKER_FILE), marker_text.as_bytes())
                .map_err(|error| format!("Halv could not mark the account folder: {error}"))
                .and_then(|()| provision(provider, &account_dir, &self.system_folders()));
        if let Err(error) = result {
            let _ = files::remove_tree(&account_dir);
            return Err(error);
        }
        Ok(PendingAccount {
            home: account_dir.join(HOME_DIR),
            id,
        })
    }

    /// Saves a sign-in folder as an account and makes it active.
    ///
    /// Returns `Ok(false)` while the folder holds no login yet. The folder
    /// stays, because the login terminal can still be running. A cancel or
    /// the startup sweep deletes it.
    fn finish_add(&mut self, provider: WalletProvider, id: &str) -> Result<bool, String> {
        if !valid_account_id(id) || !self.owns_folder(provider, id) {
            return Err("Halv could not find this sign-in. Start the sign-in again.".to_string());
        }
        if self.find(provider, id).is_some() {
            return Ok(true);
        }
        let home = self.home(provider, id);
        let Some(mut identity) = read_identity(provider, &home) else {
            return Ok(false);
        };
        if let Some(existing) = self.duplicate_of(provider, &identity) {
            let email = existing.email.clone();
            self.discard_folder(provider, id);
            return Err(format!(
                "{email} is already in the {} wallet.",
                provider.display_name()
            ));
        }
        if provider == WalletProvider::Claude {
            let source_state = self.system_folders().claude_state;
            if let Err(error) = claude::finish_sign_in(&home, source_state.as_deref()) {
                eprintln!("wallet: {error}");
            }
            identity.plan = claude::read_plan(&home);
        }
        let now = chrono::Utc::now().to_rfc3339();
        let previous = self.provider(provider).clone();
        let registry = self.provider_mut(provider);
        registry.accounts.push(AccountRecord {
            id: id.to_string(),
            email: identity.email,
            organization_id: identity.organization_id,
            organization_name: identity.organization_name,
            plan: identity.plan,
            created_at: now.clone(),
            updated_at: now,
        });
        registry.active_id = Some(id.to_string());
        if let Err(error) = self.save() {
            *self.provider_mut(provider) = previous;
            return Err(error);
        }
        Ok(true)
    }

    fn duplicate_of(
        &self,
        provider: WalletProvider,
        identity: &AccountIdentity,
    ) -> Option<&AccountRecord> {
        self.provider(provider)
            .accounts
            .iter()
            .find(|account| identity.same_account(account))
    }

    /// Deletes a sign-in folder that never became an account.
    fn discard_pending(&self, provider: WalletProvider, id: &str) -> Result<(), String> {
        if !valid_account_id(id) || self.find(provider, id).is_some() {
            return Ok(());
        }
        if self.owns_folder(provider, id) {
            self.discard_folder(provider, id);
        }
        Ok(())
    }

    fn discard_folder(&self, provider: WalletProvider, id: &str) {
        if let Err(error) = self.delete_folder(provider, id) {
            eprintln!("wallet: could not delete an unfinished sign-in folder: {error}");
        }
    }

    fn delete_folder(&self, provider: WalletProvider, id: &str) -> io::Result<()> {
        if provider == WalletProvider::Claude {
            claude::delete_keychain_items(&self.home(provider, id));
        }
        files::remove_tree(&self.account_dir(provider, id))
    }

    /// Removes an account, its folder, and the keychain items it owns. An
    /// active account falls back to System default.
    pub(crate) fn remove(&mut self, provider: WalletProvider, id: &str) -> Result<(), String> {
        if self.find(provider, id).is_none() {
            return Err(format!(
                "That {} account is no longer in the wallet.",
                provider.display_name()
            ));
        }
        // Why first: no new launch may pick the folder while Halv deletes it.
        if self.provider(provider).active_id.as_deref() == Some(id) {
            self.select(provider, None)?;
        }
        if valid_account_id(id) && self.owns_folder(provider, id) {
            self.delete_folder(provider, id).map_err(|error| {
                eprintln!("wallet: could not delete an account folder: {error}");
                format!(
                    "Halv could not delete the folder of this {} account. Close the sessions that use it, then try again.",
                    provider.display_name()
                )
            })?;
        }
        let previous = self.provider(provider).clone();
        self.provider_mut(provider)
            .accounts
            .retain(|account| account.id != id);
        if let Err(error) = self.save() {
            *self.provider_mut(provider) = previous;
            return Err(error);
        }
        Ok(())
    }

    /// True when `<root>/<provider>/<id>` carries Halv's marker for that id.
    fn owns_folder(&self, provider: WalletProvider, id: &str) -> bool {
        let marker = self.account_dir(provider, id).join(MARKER_FILE);
        fs::read(marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AccountMarker>(&bytes).ok())
            .is_some_and(|marker| marker.provider == provider && marker.id == id)
    }

    /// Adopts finished sign-in folders that are not in the registry, and
    /// deletes old unfinished ones. Returns true when the registry changed.
    fn sweep_orphans(&mut self) -> bool {
        let mut changed = false;
        for provider in WalletProvider::ALL {
            let Ok(entries) = fs::read_dir(self.provider_dir(provider)) else {
                continue;
            };
            let mut ids = entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|id| valid_account_id(id))
                .collect::<Vec<_>>();
            ids.sort();
            for id in ids {
                if self.find(provider, &id).is_some() || !self.owns_folder(provider, &id) {
                    continue;
                }
                let home = self.home(provider, &id);
                match read_identity(provider, &home) {
                    Some(identity) if self.duplicate_of(provider, &identity).is_none() => {
                        let created = folder_time(&self.account_dir(provider, &id))
                            .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339())
                            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                        self.provider_mut(provider).accounts.push(AccountRecord {
                            id: id.clone(),
                            email: identity.email,
                            organization_id: identity.organization_id,
                            organization_name: identity.organization_name,
                            plan: identity.plan,
                            created_at: created.clone(),
                            updated_at: created,
                        });
                        changed = true;
                    }
                    Some(_) => self.discard_folder(provider, &id),
                    None => {
                        let old = folder_time(&self.account_dir(provider, &id))
                            .and_then(|time| time.elapsed().ok())
                            .is_some_and(|age| age > PENDING_GRACE);
                        if old {
                            self.discard_folder(provider, &id);
                        }
                    }
                }
            }
        }
        changed
    }
}

/// The folder that Orca gives to a new launch.
///
/// Claude follows Orca's pointer file. Without it, Orca copies the selected
/// login into the default Claude Code slots, so a Claude launch keeps the
/// user's own folder. Orca switches Codex per launch through `CODEX_HOME`.
fn orca_active_home(root: &Path, provider: WalletProvider) -> ActiveHome {
    match provider {
        WalletProvider::Claude => match orca::claude_route(root) {
            orca::ClaudeRoute::System => ActiveHome::System,
            orca::ClaudeRoute::Profile(home) => ActiveHome::Linked(home),
            orca::ClaudeRoute::NotSetUp(home) => ActiveHome::NotReady(Some(home)),
            orca::ClaudeRoute::Unreadable => ActiveHome::NotReady(None),
        },
        WalletProvider::Codex => {
            let Some(store) = orca::read_store(root) else {
                return ActiveHome::NotReady(None);
            };
            let codex = store.provider(provider);
            match codex.active_id.as_deref().and_then(|id| codex.find(id)) {
                Some(account) if account.path.is_dir() => ActiveHome::Linked(account.path.clone()),
                Some(account) => ActiveHome::Missing(account.path.clone()),
                None => ActiveHome::System,
            }
        }
    }
}

fn orca_provider_state(
    provider: WalletProvider,
    store: &orca::OrcaProvider,
) -> WalletProviderState {
    WalletProviderState {
        active_id: store
            .active_id
            .clone()
            .filter(|id| store.find(id).is_some()),
        system_default: system_default_account(provider),
        accounts: store
            .accounts
            .iter()
            .map(|account| WalletAccount {
                id: account.id.clone(),
                email: account.email.clone(),
                organization_name: account.organization_name.clone(),
                plan: None,
                missing: !account.path.is_dir(),
                needs_sign_in: account.needs_sign_in,
                created_at: chrono::DateTime::from_timestamp_millis(account.created_at)
                    .unwrap_or_default()
                    .to_rfc3339(),
            })
            .collect(),
    }
}

fn folder_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path.join(MARKER_FILE))
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn sanitize(mut registry: Registry) -> Registry {
    for provider in [&mut registry.claude, &mut registry.codex] {
        let mut seen = std::collections::HashSet::new();
        provider
            .accounts
            .retain(|account| valid_account_id(&account.id) && seen.insert(account.id.clone()));
        if provider
            .active_id
            .as_ref()
            .is_some_and(|id| !provider.accounts.iter().any(|account| &account.id == id))
        {
            provider.active_id = None;
        }
    }
    registry.version = REGISTRY_VERSION;
    registry
}

/// Account ids become folder names, so only a safe alphabet is valid.
fn valid_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A short id keeps the folder paths short. Codex places a socket in its
/// home, and the OS limits socket paths to about 100 bytes.
fn new_account_id() -> String {
    format!("{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff)
}

fn provision(
    provider: WalletProvider,
    account_dir: &Path,
    system: &SystemFolders,
) -> Result<(), String> {
    let home = account_dir.join(HOME_DIR);
    match provider {
        WalletProvider::Claude => claude::provision(&home, account_dir, system.claude.as_deref()),
        WalletProvider::Codex => codex::provision(&home, system.codex.as_deref()),
    }
}

fn read_identity(provider: WalletProvider, home: &Path) -> Option<AccountIdentity> {
    match provider {
        WalletProvider::Claude => claude::read_identity(&claude::state_file(Some(home))?),
        WalletProvider::Codex => codex::read_identity(home),
    }
}

fn system_default_account(provider: WalletProvider) -> SystemDefaultAccount {
    let identity = match provider {
        WalletProvider::Claude => {
            claude::system_state_file().and_then(|path| cached_identity(&path))
        }
        WalletProvider::Codex => codex::system_home().and_then(|home| codex::read_identity(&home)),
    };
    SystemDefaultAccount {
        email: identity.as_ref().map(|identity| identity.email.clone()),
        plan: identity.and_then(|identity| identity.plan),
    }
}

/// A Claude state file can be several megabytes, and every running Claude
/// Code rewrites it. Halv parses it again only when it changed.
fn cached_identity(path: &Path) -> Option<AccountIdentity> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok();
    let length = metadata.len();
    let mut guard = CLAUDE_IDENTITIES
        .lock()
        .expect("wallet identity cache lock poisoned");
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(cached) = cache.get(path) {
        if cached.modified == modified && cached.length == length {
            return cached.identity.clone();
        }
    }
    let identity = claude::read_identity(path);
    cache.insert(
        path.to_path_buf(),
        CachedIdentity {
            modified,
            length,
            identity: identity.clone(),
        },
    );
    identity
}

/// The login an account folder holds now. A re-login in the folder can
/// change it after the account was added.
fn live_identity(provider: WalletProvider, home: &Path) -> Option<AccountIdentity> {
    match provider {
        WalletProvider::Claude => {
            claude::state_file(Some(home)).and_then(|path| cached_identity(&path))
        }
        WalletProvider::Codex => codex::read_identity(home),
    }
}

/// Loads the wallet from the Halv app data folder. Call once at startup.
pub(crate) fn init(app_data_dir: &Path) {
    let root = app_data_dir.join(ROOT_DIR);
    *LAUNCHES.lock().expect("wallet launch lock poisoned") =
        Some(launches::LaunchRegistry::open(&root, now_ms()));
    let wallet = Wallet::open(root);
    *WALLET.lock().expect("wallet lock poisoned") = Some(wallet);
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

/// The wallet provider of a subscription pane. Other lanes do not use the
/// wallet.
fn subscription_provider(
    agent_kind: AgentKind,
    lane_kind: SessionLaneKind,
) -> Option<WalletProvider> {
    WalletProvider::ALL.into_iter().find(|provider| {
        provider.agent_kind() == agent_kind && provider.subscription_lane() == lane_kind
    })
}

/// Records the account folder of a new agent pane from its launch
/// environment.
pub(crate) fn record_launch(
    session_id: &str,
    agent_kind: AgentKind,
    lane_kind: SessionLaneKind,
    env: &[(OsString, OsString)],
) {
    let Some(provider) = subscription_provider(agent_kind, lane_kind) else {
        return;
    };
    let home = launches::launch_home(provider, env);
    let account_id = with_wallet(|wallet| Ok(wallet.active_id(provider)))
        .ok()
        .flatten();
    if let Some(registry) = LAUNCHES
        .lock()
        .expect("wallet launch lock poisoned")
        .as_mut()
    {
        registry.record(session_id, provider, home, account_id, now_ms());
    }
}

/// The folder that a new launch of `provider` gets now. `None` when Halv
/// cannot launch with the active account, because then a restart would
/// fail.
fn current_launch_home(provider: WalletProvider) -> Option<launches::LaunchHome> {
    match active_home(provider) {
        ActiveHome::System => Some(launches::LaunchHome::System),
        ActiveHome::Managed(home) | ActiveHome::Linked(home) => {
            Some(launches::LaunchHome::Folder(home))
        }
        ActiveHome::Missing(_) | ActiveHome::NotReady(_) => None,
    }
}

/// `current_launch_home` of each provider, read once before the launch
/// registry lock.
fn current_launch_homes() -> impl Fn(WalletProvider) -> Option<launches::LaunchHome> {
    let homes = WalletProvider::ALL.map(|provider| (provider, current_launch_home(provider)));
    move |provider| {
        homes
            .iter()
            .find(|(candidate, _)| *candidate == provider)
            .and_then(|(_, home)| home.clone())
    }
}

/// The live agent panes in Halv's terminals that run another account than
/// the active account. Chat sessions and external terminals cannot restart
/// in place, so they never count.
fn stale_sessions(manager: &AgentSessionManager) -> Vec<WalletStaleSession> {
    let live = manager
        .list()
        .into_iter()
        .filter(|session| {
            session.mode == SessionMode::TerminalEmbedded
                && matches!(
                    session.status.as_str(),
                    "starting" | "running" | "waiting_input"
                )
        })
        .filter_map(|session| {
            subscription_provider(session.agent_kind, session.lane_kind)
                .map(|provider| (session.id, provider))
        })
        .collect::<Vec<_>>();
    if live.is_empty() {
        return Vec::new();
    }
    let homes = current_launch_homes();
    let next_ids = WalletProvider::ALL.map(|provider| {
        let id = with_wallet(|wallet| Ok(wallet.active_id(provider)))
            .ok()
            .flatten();
        (provider, id)
    });
    let stale = LAUNCHES
        .lock()
        .expect("wallet launch lock poisoned")
        .as_ref()
        .map(|registry| registry.stale(&live, &homes))
        .unwrap_or_default();
    stale
        .into_iter()
        .map(|launch| WalletStaleSession {
            next_account_id: next_ids
                .iter()
                .find(|(provider, _)| *provider == launch.provider)
                .and_then(|(_, id)| id.clone()),
            session_id: launch.session_id,
            provider: launch.provider,
            previous_account_id: launch.previous_account_id,
        })
        .collect()
}

fn with_wallet<T>(operation: impl FnOnce(&mut Wallet) -> Result<T, String>) -> Result<T, String> {
    let mut guard = WALLET.lock().expect("wallet lock poisoned");
    let wallet = guard
        .as_mut()
        .ok_or_else(|| "The account wallet is not ready yet.".to_string())?;
    operation(wallet)
}

/// The active folder of a provider. A process without a wallet, such as the
/// savings MCP server, reports System default and reads the inherited
/// environment instead.
pub(crate) fn active_home(provider: WalletProvider) -> ActiveHome {
    WALLET
        .lock()
        .expect("wallet lock poisoned")
        .as_ref()
        .map(|wallet| wallet.active_home(provider))
        .unwrap_or(ActiveHome::System)
}

/// The Claude config folder for usage reads: the active account folder, else
/// `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(crate) fn claude_config_dir() -> Option<PathBuf> {
    match active_home(WalletProvider::Claude) {
        ActiveHome::Managed(home) | ActiveHome::Missing(home) | ActiveHome::Linked(home) => {
            Some(home)
        }
        ActiveHome::NotReady(home) => home,
        ActiveHome::System => claude::system_config_dir(),
    }
}

/// The Claude state file (`.claude.json`) of the active account.
pub(crate) fn claude_state_file() -> Option<PathBuf> {
    match active_home(WalletProvider::Claude) {
        ActiveHome::Managed(home) | ActiveHome::Missing(home) | ActiveHome::Linked(home) => {
            claude::state_file(Some(&home))
        }
        ActiveHome::NotReady(home) => home.and_then(|home| claude::state_file(Some(&home))),
        ActiveHome::System => claude::system_state_file(),
    }
}

/// The keychain services that can hold the active Claude credential, in
/// read order.
///
/// A managed account reads only its own item, so Halv never shows the usage
/// of another account. An Orca account folder reads the item that a launch
/// in it uses. System default also tries the plain item name, which Claude
/// Code before 2.1 used for a custom `CLAUDE_CONFIG_DIR`.
pub(crate) fn claude_active_keychain_services() -> Vec<String> {
    claude_keychain_services_for(
        &active_home(WalletProvider::Claude),
        claude::system_keychain_service,
    )
}

fn claude_keychain_services_for(
    active: &ActiveHome,
    system_service: impl FnOnce() -> String,
) -> Vec<String> {
    match active {
        ActiveHome::Managed(home)
        | ActiveHome::Missing(home)
        | ActiveHome::Linked(home)
        | ActiveHome::NotReady(Some(home)) => {
            vec![claude::keychain_service(Some(home))]
        }
        ActiveHome::NotReady(None) => Vec::new(),
        ActiveHome::System => {
            let service = system_service();
            let plain = claude::KEYCHAIN_SERVICE.to_string();
            if service == plain {
                vec![service]
            } else {
                vec![service, plain]
            }
        }
    }
}

/// The Codex home for usage reads: the active account home, else
/// `CODEX_HOME`, else `~/.codex`.
pub(crate) fn codex_home() -> Option<PathBuf> {
    match active_home(WalletProvider::Codex) {
        ActiveHome::Managed(home) | ActiveHome::Missing(home) | ActiveHome::Linked(home) => {
            Some(home)
        }
        ActiveHome::NotReady(home) => home,
        ActiveHome::System => codex::system_home(),
    }
}

/// A cache key that changes when the active account changes.
pub(crate) fn usage_cache_key(provider: WalletProvider) -> String {
    match provider {
        // An Orca switch changes the login in the same folder and keychain
        // item, so the Orca account id is part of the key.
        WalletProvider::Claude => format!(
            "{:?}|{:?}|{:?}",
            claude_config_dir(),
            claude_active_keychain_services(),
            orca_active_id(WalletProvider::Claude)
        ),
        WalletProvider::Codex => format!("{:?}", codex_home()),
    }
}

fn environment_for(provider: WalletProvider, home: &Path) -> AccountEnvironment {
    let remove = match provider {
        WalletProvider::Claude => claude::LAUNCH_ENV_REMOVE
            .iter()
            .map(OsString::from)
            .collect(),
        WalletProvider::Codex => Vec::new(),
    };
    AccountEnvironment {
        set: vec![(
            OsString::from(provider.home_variable()),
            home.as_os_str().to_os_string(),
        )],
        remove,
    }
}

/// Environment changes for a folder that Orca owns. For Claude, Halv sets
/// the same variables as Orca's own launches. The folder text stays exactly
/// as Orca wrote it, because Claude Code names the keychain item of the
/// folder from that text.
fn linked_environment(provider: WalletProvider, home: &Path) -> AccountEnvironment {
    let mut environment = environment_for(provider, home);
    if provider == WalletProvider::Claude {
        environment.set.push((
            OsString::from(orca::CLAUDE_INJECTED_CONFIG_DIR_ENV),
            home.as_os_str().to_os_string(),
        ));
        environment.remove.extend(
            orca::claude_auth_env_remove()
                .into_iter()
                .map(OsString::from),
        );
    }
    environment
}

fn environment_for_active(
    provider: WalletProvider,
    active: &ActiveHome,
) -> Result<AccountEnvironment, String> {
    match active {
        ActiveHome::System => Ok(AccountEnvironment::default()),
        ActiveHome::Missing(_) => Err(format!(
            "The folder of the active {} account is missing. Choose another account in Settings > Connections.",
            provider.display_name()
        )),
        ActiveHome::NotReady(home) => Err(match (provider, home) {
            (WalletProvider::Claude, Some(_)) => "Orca has not finished setting up the selected Claude account. Wait a moment, then try again. If this continues, select the account again in Orca.",
            (WalletProvider::Claude, None) => "Halv could not read the selected Claude account from Orca. Select the account again in Orca, then try again.",
            (WalletProvider::Codex, _) => "Halv could not read the active Codex account from Orca. Open Orca, then try again.",
        }
        .to_string()),
        ActiveHome::Managed(home) => Ok(environment_for(provider, home)),
        ActiveHome::Linked(home) => Ok(linked_environment(provider, home)),
    }
}

/// Environment changes that a shell or a session of the other provider
/// gives to a CLI that the user starts in it later. A folder that Orca is
/// still setting up counts, because Orca usually finishes before the CLI
/// starts. The changes keep the user's login variables.
fn inherited_environment(provider: WalletProvider, active: &ActiveHome) -> AccountEnvironment {
    let mut environment = match active {
        ActiveHome::Managed(home) => environment_for(provider, home),
        ActiveHome::Linked(home) | ActiveHome::NotReady(Some(home)) => {
            linked_environment(provider, home)
        }
        ActiveHome::System | ActiveHome::Missing(_) | ActiveHome::NotReady(None) => {
            AccountEnvironment::default()
        }
    };
    environment.remove.clear();
    environment
}

/// The active Orca account of a provider, when Orca owns the accounts.
fn orca_active_id(provider: WalletProvider) -> Option<String> {
    WALLET
        .lock()
        .expect("wallet lock poisoned")
        .as_ref()
        .and_then(Wallet::orca_store)
        .and_then(|store| store.provider(provider).active_id.clone())
}

/// Environment changes for a new CLI launch of `provider` with the active
/// account. An empty result keeps the user's own CLI folder.
pub(crate) fn launch_environment(provider: WalletProvider) -> Result<AccountEnvironment, String> {
    let active = active_home(provider);
    if let ActiveHome::Managed(home) = &active {
        // Links and settings can change between launches, so each launch
        // refreshes them. A failure keeps the launch going.
        if let Some(account_dir) = home.parent() {
            if let Err(error) = provision(provider, account_dir, &SystemFolders::from_environment())
            {
                eprintln!("wallet: {error}");
            }
        }
    }
    environment_for_active(provider, &active)
}

/// Environment changes for a short probe of the active account, such as
/// `claude auth status`. The probe does not refresh the account folder.
pub(crate) fn probe_environment(provider: WalletProvider) -> Result<AccountEnvironment, String> {
    environment_for_active(provider, &active_home(provider))
}

/// Environment changes for a plain shell terminal.
///
/// A `claude` or `codex` that the user types in a new shell uses the
/// active accounts. The function never fails, so a missing account folder
/// can never stop a shell from opening. It keeps the user's login
/// variables, because the shell can run other tools that need them.
pub(crate) fn shell_environment() -> AccountEnvironment {
    let mut environment = AccountEnvironment::default();
    for provider in WalletProvider::ALL {
        environment.extend(inherited_environment(provider, &active_home(provider)));
    }
    environment
}

/// Environment changes for a subscription session of `primary`.
///
/// The session also gets the active account folder of the other provider,
/// so a CLI that the session starts, and Halv's quota probe inside the
/// session, use the active accounts too. Only the primary account removes
/// inherited login variables.
pub(crate) fn session_environment(
    primary: WalletProvider,
    include_other: bool,
) -> Result<AccountEnvironment, String> {
    let mut environment = launch_environment(primary)?;
    if include_other {
        let other = match primary {
            WalletProvider::Claude => WalletProvider::Codex,
            WalletProvider::Codex => WalletProvider::Claude,
        };
        environment.extend(inherited_environment(other, &active_home(other)));
    }
    Ok(environment)
}

fn current_state() -> Result<WalletState, String> {
    with_wallet(|wallet| Ok(wallet.state()))
}

fn emit_changed(app: &AppHandle, state: &WalletState) {
    if let Err(error) = app.emit(WALLET_CHANGED_EVENT, state) {
        eprintln!("wallet: could not announce the account change: {error}");
    }
}

#[tauri::command]
pub fn wallet_list() -> Result<WalletState, String> {
    current_state()
}

/// The agent panes that still run an account that is no longer active.
#[tauri::command]
pub fn wallet_stale_sessions(
    manager: State<'_, AgentSessionManager>,
) -> Result<Vec<WalletStaleSession>, String> {
    Ok(stale_sessions(manager.inner()))
}

/// Keeps panes on their account. They ask again after the next switch to
/// another account.
#[tauri::command]
pub fn wallet_keep_sessions(session_ids: Vec<String>) -> Result<(), String> {
    let homes = current_launch_homes();
    if let Some(registry) = LAUNCHES
        .lock()
        .expect("wallet launch lock poisoned")
        .as_mut()
    {
        registry.keep(&session_ids, homes);
    }
    Ok(())
}

/// Orca's data folder when Orca owns the accounts. Orca calls can take
/// seconds, so the commands run them without the wallet lock.
fn orca_route() -> Result<Option<PathBuf>, String> {
    with_wallet(|wallet| Ok(wallet.orca_root().map(Path::to_path_buf)))
}

async fn run_blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("Halv could not finish the account change: {error}"))?
}

#[tauri::command]
pub async fn wallet_select(
    app: AppHandle,
    manager: State<'_, AgentSessionManager>,
    provider: WalletProvider,
    account_id: Option<String>,
) -> Result<WalletSelectResult, String> {
    let state = match orca_route()? {
        Some(root) => {
            run_blocking(move || orca::select(&root, provider, account_id.as_deref())).await?;
            current_state()?
        }
        None => with_wallet(|wallet| {
            wallet.select(provider, account_id.as_deref())?;
            Ok(wallet.state())
        })?,
    };
    emit_changed(&app, &state);
    Ok(WalletSelectResult {
        stale_sessions: stale_sessions(manager.inner()),
        state,
    })
}

/// Prepares a new account folder and opens the CLI login in a terminal.
#[tauri::command]
pub fn wallet_add_launch(
    app: AppHandle,
    terminals: State<'_, crate::terminal::TerminalManager>,
    provider: WalletProvider,
) -> Result<WalletPendingAdd, String> {
    // Why before the login: an Orca with per-account Claude folders refuses
    // a Claude login from Halv, so the sign-in would be lost work.
    if provider == WalletProvider::Claude
        && orca_route()?.is_some_and(|root| orca::claude_profiles_in_use(&root))
    {
        return Err(orca::CLAUDE_ADD_IN_ORCA.to_string());
    }
    let pending = with_wallet(|wallet| wallet.begin_add(provider))?;
    let environment = environment_for(provider, &pending.home);
    match crate::agent_sessions::spawn_login_terminal(
        app,
        terminals.inner(),
        provider.agent_kind(),
        &environment,
    ) {
        Ok(terminal_id) => Ok(WalletPendingAdd {
            pending_id: pending.id,
            terminal_id,
        }),
        Err(error) => {
            let _ = with_wallet(|wallet| wallet.discard_pending(provider, &pending.id));
            Err(error)
        }
    }
}

/// Saves the login of a finished sign-in terminal as a wallet account.
///
/// With Orca, Halv gives the signed-in folder to Orca, makes the new Orca
/// account active, and deletes the folder. Orca keeps its own copy.
#[tauri::command]
pub async fn wallet_add_finish(
    app: AppHandle,
    provider: WalletProvider,
    pending_id: String,
) -> Result<WalletAddFinishResult, String> {
    if let Some(root) = orca_route()? {
        if ORCA_ADDS
            .lock()
            .expect("orca add lock poisoned")
            .contains(&pending_id)
        {
            return Ok(WalletAddFinishResult {
                state: current_state()?,
                added: true,
            });
        }
        let home = with_wallet(|wallet| wallet.pending_home(provider, &pending_id))?;
        if read_identity(provider, &home).is_none() {
            return Ok(WalletAddFinishResult {
                state: current_state()?,
                added: false,
            });
        }
        {
            let mut adds = ORCA_ADDS.lock().expect("orca add lock poisoned");
            if adds.contains(&pending_id) {
                drop(adds);
                return Ok(WalletAddFinishResult {
                    state: current_state()?,
                    added: true,
                });
            }
            adds.push(pending_id.clone());
        }
        let folder = home.clone();
        let result = run_blocking(move || {
            if let Some(id) = orca::add_from_folder(&root, provider, &folder)? {
                orca::select(&root, provider, Some(&id))?;
            }
            Ok(())
        })
        .await;
        if let Err(error) = result {
            // Why: a failed hand-over keeps the folder, so "Save account" can retry.
            ORCA_ADDS
                .lock()
                .expect("orca add lock poisoned")
                .retain(|id| id != &pending_id);
            if error == orca::CLAUDE_ADD_IN_ORCA {
                // Why: Orca never takes this login. The startup sweep would
                // adopt the folder into Halv's own registry.
                with_wallet(|wallet| {
                    wallet.discard_folder(provider, &pending_id);
                    Ok(())
                })?;
            }
            return Err(error);
        }
        with_wallet(|wallet| {
            wallet.discard_folder(provider, &pending_id);
            Ok(())
        })?;
        let state = current_state()?;
        emit_changed(&app, &state);
        return Ok(WalletAddFinishResult { state, added: true });
    }
    let (added, state) = with_wallet(|wallet| {
        let added = wallet.finish_add(provider, &pending_id)?;
        Ok((added, wallet.state()))
    })?;
    if added {
        emit_changed(&app, &state);
    }
    Ok(WalletAddFinishResult { state, added })
}

#[tauri::command]
pub fn wallet_add_cancel(provider: WalletProvider, pending_id: String) -> Result<(), String> {
    with_wallet(|wallet| wallet.discard_pending(provider, &pending_id))
}

#[tauri::command]
pub async fn wallet_remove(
    app: AppHandle,
    provider: WalletProvider,
    account_id: String,
) -> Result<WalletState, String> {
    let state = match orca_route()? {
        Some(root) => {
            run_blocking(move || orca::remove(&root, provider, &account_id)).await?;
            current_state()?
        }
        None => with_wallet(|wallet| {
            wallet.remove(provider, &account_id)?;
            Ok(wallet.state())
        })?,
    };
    emit_changed(&app, &state);
    Ok(state)
}

/// The usage of the accounts that are not active. Only Orca knows it; with
/// Halv's own registry the list is empty.
#[tauri::command]
pub async fn wallet_usage() -> Result<Vec<WalletAccountUsage>, String> {
    let Some(root) = orca_route()? else {
        return Ok(Vec::new());
    };
    let usage = run_blocking(move || orca::inactive_usage(&root)).await?;
    Ok(usage
        .into_iter()
        .map(|account| WalletAccountUsage {
            provider: account.provider,
            account_id: account.account_id,
            windows: account
                .windows
                .into_iter()
                .map(|window| WalletUsageWindow {
                    kind: window.kind,
                    used_percent: window.used_percent,
                    resets_at: window.resets_at,
                })
                .collect(),
            updated_at: account.updated_at,
        })
        .collect())
}

/// Announces account changes that the user makes in Orca, so Halv's views
/// follow Orca within seconds. One thread keeps the live mirror of Orca's
/// accounts. Orca's store changes for many other reasons, so Halv announces
/// only a changed account state.
pub(crate) fn spawn_orca_watcher(app: AppHandle) {
    let Some(root) = orca::data_root() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("halv-orca-accounts".to_string())
        .spawn(move || {
            let mut last_state = current_state().ok();
            orca::watch(&root, || {
                let Ok(state) = current_state() else {
                    return;
                };
                if last_state.as_ref() != Some(&state) {
                    emit_changed(&app, &state);
                    last_state = Some(state);
                }
            })
        });
    if let Err(error) = spawned {
        eprintln!("wallet: could not watch Orca's accounts: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("halv-wallet-{name}-{:016x}", rand::random::<u64>()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Scratch {
        fn open(&self) -> Wallet {
            let system = self.0.join("system");
            let claude = system.join(".claude");
            let codex = system.join(".codex");
            fs::create_dir_all(&claude).unwrap();
            fs::create_dir_all(&codex).unwrap();
            Wallet::open_with(
                self.0.join(ROOT_DIR),
                Some(SystemFolders {
                    claude_state: Some(system.join(".claude.json")),
                    claude: Some(claude),
                    codex: Some(codex),
                }),
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sign_in_claude(home: &Path, email: &str, organization: &str) {
        fs::write(
            home.join(".claude.json"),
            json!({
                "oauthAccount": {
                    "emailAddress": email,
                    "organizationUuid": organization,
                    "organizationName": "Org"
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    fn sign_in_codex(home: &Path, email: &str, account: &str) {
        use base64::Engine;
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account,
                "chatgpt_plan_type": "plus"
            }
        });
        let token = format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
        );
        fs::write(
            home.join("auth.json"),
            json!({ "tokens": { "id_token": token, "account_id": account } }).to_string(),
        )
        .unwrap();
    }

    fn add_codex(wallet: &mut Wallet, email: &str, account: &str) -> Result<String, String> {
        let pending = wallet.begin_add(WalletProvider::Codex)?;
        sign_in_codex(&pending.home, email, account);
        assert!(wallet.finish_add(WalletProvider::Codex, &pending.id)?);
        Ok(pending.id)
    }

    #[test]
    fn only_subscription_panes_get_a_launch_record() {
        assert_eq!(
            subscription_provider(AgentKind::Claude, SessionLaneKind::ClaudeSubscription),
            Some(WalletProvider::Claude)
        );
        assert_eq!(
            subscription_provider(AgentKind::Codex, SessionLaneKind::CodexChatGpt),
            Some(WalletProvider::Codex)
        );
        assert_eq!(
            subscription_provider(AgentKind::Claude, SessionLaneKind::ClaudeApiKey),
            None
        );
        assert_eq!(
            subscription_provider(AgentKind::Codex, SessionLaneKind::Custom),
            None
        );
    }

    #[test]
    fn an_empty_wallet_uses_the_system_default() {
        let scratch = Scratch::new("empty");
        let wallet = scratch.open();
        assert_eq!(
            wallet.active_home(WalletProvider::Claude),
            ActiveHome::System
        );
        assert_eq!(
            wallet.active_home(WalletProvider::Codex),
            ActiveHome::System
        );
        let state = wallet.state();
        assert!(state.claude.accounts.is_empty());
        assert!(state.codex.accounts.is_empty());
        assert!(!state.recovered);
        assert_eq!(
            environment_for_active(WalletProvider::Claude, &ActiveHome::System).unwrap(),
            AccountEnvironment::default()
        );
    }

    #[test]
    fn a_finished_sign_in_becomes_the_active_account() {
        let scratch = Scratch::new("add");
        let mut wallet = scratch.open();
        let id = add_codex(&mut wallet, "one@example.com", "acct-1").unwrap();

        let state = wallet.state();
        assert_eq!(state.codex.active_id.as_deref(), Some(id.as_str()));
        assert_eq!(state.codex.accounts.len(), 1);
        assert_eq!(state.codex.accounts[0].email, "one@example.com");
        assert_eq!(state.codex.accounts[0].plan.as_deref(), Some("plus"));
        let home = wallet.home(WalletProvider::Codex, &id);
        assert_eq!(
            wallet.active_home(WalletProvider::Codex),
            ActiveHome::Managed(home.clone())
        );
        let environment =
            environment_for_active(WalletProvider::Codex, &ActiveHome::Managed(home.clone()))
                .unwrap();
        assert_eq!(
            environment.set,
            vec![(OsString::from("CODEX_HOME"), home.into_os_string())]
        );

        let reopened = scratch.open();
        assert_eq!(
            reopened.state().codex.active_id.as_deref(),
            Some(id.as_str())
        );
    }

    #[test]
    fn an_unfinished_sign_in_waits_until_a_cancel() {
        let scratch = Scratch::new("unfinished");
        let mut wallet = scratch.open();
        let pending = wallet.begin_add(WalletProvider::Claude).unwrap();
        let folder = wallet.account_dir(WalletProvider::Claude, &pending.id);
        assert!(folder.is_dir());

        let added = wallet
            .finish_add(WalletProvider::Claude, &pending.id)
            .unwrap();

        assert!(!added);
        assert!(folder.is_dir(), "a running login keeps its folder");
        assert!(wallet.state().claude.accounts.is_empty());
        wallet
            .discard_pending(WalletProvider::Claude, &pending.id)
            .unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn a_duplicate_identity_is_refused() {
        let scratch = Scratch::new("duplicate");
        let mut wallet = scratch.open();
        add_codex(&mut wallet, "one@example.com", "acct-1").unwrap();
        let pending = wallet.begin_add(WalletProvider::Codex).unwrap();
        sign_in_codex(&pending.home, "ONE@example.com", "acct-1");

        let error = wallet
            .finish_add(WalletProvider::Codex, &pending.id)
            .unwrap_err();

        assert!(error.contains("already in the Codex wallet"));
        assert!(!wallet
            .account_dir(WalletProvider::Codex, &pending.id)
            .exists());
        assert_eq!(wallet.state().codex.accounts.len(), 1);

        // The same email in another workspace is a separate account.
        add_codex(&mut wallet, "one@example.com", "acct-2").unwrap();
        assert_eq!(wallet.state().codex.accounts.len(), 2);
    }

    #[test]
    fn removing_the_active_account_falls_back_to_the_system_default() {
        let scratch = Scratch::new("remove");
        let mut wallet = scratch.open();
        let first = add_codex(&mut wallet, "one@example.com", "acct-1").unwrap();
        let second = add_codex(&mut wallet, "two@example.com", "acct-2").unwrap();
        assert_eq!(
            wallet.state().codex.active_id.as_deref(),
            Some(second.as_str())
        );

        wallet.remove(WalletProvider::Codex, &second).unwrap();

        let state = wallet.state();
        assert_eq!(state.codex.active_id, None);
        assert_eq!(state.codex.accounts.len(), 1);
        assert_eq!(state.codex.accounts[0].id, first);
        assert!(!wallet.account_dir(WalletProvider::Codex, &second).exists());
        assert_eq!(
            wallet.active_home(WalletProvider::Codex),
            ActiveHome::System
        );
    }

    #[test]
    fn select_switches_between_accounts_and_the_system_default() {
        let scratch = Scratch::new("select");
        let mut wallet = scratch.open();
        let first = add_codex(&mut wallet, "one@example.com", "acct-1").unwrap();
        add_codex(&mut wallet, "two@example.com", "acct-2").unwrap();

        wallet.select(WalletProvider::Codex, Some(&first)).unwrap();
        assert_eq!(
            wallet.state().codex.active_id.as_deref(),
            Some(first.as_str())
        );
        wallet.select(WalletProvider::Codex, None).unwrap();
        assert_eq!(wallet.state().codex.active_id, None);
        assert!(wallet.select(WalletProvider::Codex, Some("abc")).is_err());
        assert!(wallet.select(WalletProvider::Claude, Some(&first)).is_err());
    }

    #[test]
    fn a_missing_folder_blocks_the_launch_and_the_selection() {
        let scratch = Scratch::new("missing");
        let mut wallet = scratch.open();
        let first = add_codex(&mut wallet, "one@example.com", "acct-1").unwrap();
        let second = add_codex(&mut wallet, "two@example.com", "acct-2").unwrap();
        files::remove_tree(&wallet.home(WalletProvider::Codex, &second)).unwrap();

        let active = wallet.active_home(WalletProvider::Codex);
        assert!(matches!(active, ActiveHome::Missing(_)));
        assert!(environment_for_active(WalletProvider::Codex, &active).is_err());
        let state = wallet.state();
        assert!(state.codex.accounts.iter().any(|account| account.missing));

        files::remove_tree(&wallet.home(WalletProvider::Codex, &first)).unwrap();
        wallet.select(WalletProvider::Codex, None).unwrap();
        assert!(wallet.select(WalletProvider::Codex, Some(&first)).is_err());
        wallet.remove(WalletProvider::Codex, &first).unwrap();
        assert_eq!(wallet.state().codex.accounts.len(), 1);
    }

    #[test]
    fn a_corrupt_registry_is_rebuilt_from_the_account_folders() {
        let scratch = Scratch::new("corrupt");
        let root = scratch.0.join(ROOT_DIR);
        let mut wallet = scratch.open();
        let id = add_codex(&mut wallet, "one@example.com", "acct-1").unwrap();
        fs::write(root.join(REGISTRY_FILE), "{ not json").unwrap();

        let recovered = scratch.open();

        let state = recovered.state();
        assert!(state.recovered);
        assert_eq!(state.codex.active_id, None);
        assert_eq!(state.codex.accounts.len(), 1);
        assert_eq!(state.codex.accounts[0].id, id);
        assert!(root.join(CORRUPT_REGISTRY_FILE).is_file());
        let saved = fs::read_to_string(root.join(REGISTRY_FILE)).unwrap();
        assert!(serde_json::from_str::<Registry>(&saved).is_ok());
    }

    #[test]
    fn a_registry_with_bad_entries_is_cleaned() {
        let registry = sanitize(Registry {
            version: 0,
            claude: ProviderRegistry {
                active_id: Some("../escape".to_string()),
                accounts: vec![AccountRecord {
                    id: "../escape".to_string(),
                    email: "x@example.com".to_string(),
                    organization_id: None,
                    organization_name: None,
                    plan: None,
                    created_at: String::new(),
                    updated_at: String::new(),
                }],
            },
            codex: ProviderRegistry::default(),
        });
        assert!(registry.claude.accounts.is_empty());
        assert_eq!(registry.claude.active_id, None);
        assert_eq!(registry.version, REGISTRY_VERSION);
    }

    #[test]
    fn claude_accounts_get_their_own_config_folder() {
        let scratch = Scratch::new("claude");
        let mut wallet = scratch.open();
        let pending = wallet.begin_add(WalletProvider::Claude).unwrap();
        sign_in_claude(&pending.home, "dev@example.com", "org-1");

        wallet
            .finish_add(WalletProvider::Claude, &pending.id)
            .unwrap();

        let state = wallet.state();
        assert_eq!(state.claude.accounts[0].email, "dev@example.com");
        let saved_state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(pending.home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(saved_state["hasCompletedOnboarding"], json!(true));
        let environment = environment_for(WalletProvider::Claude, &pending.home);
        assert_eq!(
            environment.set,
            vec![(
                OsString::from("CLAUDE_CONFIG_DIR"),
                pending.home.as_os_str().to_os_string()
            )]
        );
        assert!(environment
            .remove
            .contains(&OsString::from("ANTHROPIC_API_KEY")));
        assert!(environment
            .remove
            .contains(&OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR")));
    }

    #[test]
    fn an_orphan_sign_in_with_a_login_is_adopted() {
        let scratch = Scratch::new("orphan");
        let wallet = scratch.open();
        let pending = wallet.begin_add(WalletProvider::Codex).unwrap();
        sign_in_codex(&pending.home, "late@example.com", "acct-9");
        let unfinished = wallet.begin_add(WalletProvider::Codex).unwrap();

        let reopened = scratch.open();

        let state = reopened.state();
        assert_eq!(state.codex.accounts.len(), 1);
        assert_eq!(state.codex.accounts[0].email, "late@example.com");
        assert_eq!(state.codex.active_id, None);
        // A recent unfinished sign-in can still be in progress, so it stays.
        assert!(reopened
            .account_dir(WalletProvider::Codex, &unfinished.id)
            .is_dir());
    }

    #[test]
    fn a_managed_claude_account_reads_only_its_own_keychain_item() {
        let home = PathBuf::from("/tmp/halv-accounts/claude/0123456789ab/home");
        let managed = claude_keychain_services_for(&ActiveHome::Managed(home.clone()), || {
            unreachable!("a managed account never reads the system item")
        });
        assert_eq!(managed, vec![claude::keychain_service(Some(&home))]);
        assert_ne!(managed[0], claude::KEYCHAIN_SERVICE);

        let plain = claude_keychain_services_for(&ActiveHome::System, || {
            claude::KEYCHAIN_SERVICE.to_string()
        });
        assert_eq!(plain, vec![claude::KEYCHAIN_SERVICE.to_string()]);
        let custom = claude_keychain_services_for(&ActiveHome::System, || {
            "Claude Code-credentials-0badf00d".to_string()
        });
        assert_eq!(
            custom,
            vec![
                "Claude Code-credentials-0badf00d".to_string(),
                claude::KEYCHAIN_SERVICE.to_string()
            ]
        );
    }

    #[test]
    fn account_ids_are_safe_folder_names() {
        for _ in 0..32 {
            let id = new_account_id();
            assert_eq!(id.len(), 12);
            assert!(valid_account_id(&id));
        }
        assert!(!valid_account_id(""));
        assert!(!valid_account_id("../x"));
        assert!(!valid_account_id("A"));
        assert!(!valid_account_id("a/b"));
    }

    fn orca_wallet(scratch: &Scratch, codex_home_exists: bool) -> Wallet {
        let root = scratch.0.join("orca");
        let codex_home = root.join("codex-accounts").join("c1").join("home");
        if codex_home_exists {
            fs::create_dir_all(&codex_home).unwrap();
        }
        fs::create_dir_all(root.join("claude-accounts").join("a1").join("auth")).unwrap();
        fs::write(
            root.join("orca-data.json"),
            json!({ "settings": {
                "claudeManagedAccounts": [{
                    "id": "a1", "email": "one@example.com",
                    "managedAuthPath": root.join("claude-accounts/a1/auth"),
                    "managedAuthRuntime": "host", "createdAt": 1700000000000_i64
                }],
                "activeClaudeManagedAccountIdsByRuntime": { "host": "a1", "wsl": {} },
                "codexManagedAccounts": [{
                    "id": "c1", "email": "codex@example.com",
                    "managedHomePath": codex_home, "createdAt": 1
                }],
                "activeCodexManagedAccountIdsByRuntime": { "host": "c1", "wsl": {} }
            }})
            .to_string(),
        )
        .unwrap();
        let mut wallet = scratch.open();
        wallet.orca = Some(root);
        wallet.orca_installed = |_| true;
        wallet
    }

    #[test]
    fn orca_accounts_replace_the_halv_registry() {
        let scratch = Scratch::new("orca-state");
        let mut wallet = scratch.open();
        add_codex(&mut wallet, "halv@example.com", "acct-h").unwrap();
        let wallet = orca_wallet(&scratch, true);

        let state = wallet.state();
        assert_eq!(state.store, WalletStoreKind::Orca);
        assert_eq!(state.claude.active_id.as_deref(), Some("a1"));
        assert_eq!(state.claude.accounts.len(), 1);
        assert_eq!(state.claude.accounts[0].email, "one@example.com");
        assert!(!state.claude.accounts[0].missing);
        assert_eq!(state.codex.accounts.len(), 1);
        assert_eq!(state.codex.accounts[0].email, "codex@example.com");
    }

    #[test]
    fn an_orca_claude_account_uses_the_default_claude_folder() {
        let scratch = Scratch::new("orca-claude");
        let wallet = orca_wallet(&scratch, true);
        assert_eq!(
            wallet.active_home(WalletProvider::Claude),
            ActiveHome::System
        );
    }

    #[test]
    fn an_orca_codex_account_points_the_cli_at_orca_folder() {
        let scratch = Scratch::new("orca-codex");
        let wallet = orca_wallet(&scratch, true);
        let home = scratch.0.join("orca/codex-accounts/c1/home");
        let active = wallet.active_home(WalletProvider::Codex);
        assert_eq!(active, ActiveHome::Linked(home.clone()));
        let environment = environment_for_active(WalletProvider::Codex, &active).unwrap();
        assert_eq!(
            environment.set,
            vec![(OsString::from("CODEX_HOME"), home.clone().into_os_string())]
        );
        // Halv never writes into a folder that Orca owns.
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
    }

    #[test]
    fn a_missing_orca_codex_folder_blocks_the_launch() {
        let scratch = Scratch::new("orca-missing");
        let wallet = orca_wallet(&scratch, false);
        let active = wallet.active_home(WalletProvider::Codex);
        assert!(matches!(active, ActiveHome::Missing(_)));
        assert!(environment_for_active(WalletProvider::Codex, &active).is_err());
        assert!(wallet.state().codex.accounts[0].missing);
    }

    #[test]
    fn without_orca_data_the_halv_registry_stays_in_use() {
        let scratch = Scratch::new("orca-absent");
        let mut wallet = scratch.open();
        wallet.orca = Some(scratch.0.join("no-orca"));
        assert_eq!(wallet.state().store, WalletStoreKind::Halv);
    }

    #[test]
    fn a_sqlite_only_orca_profile_needs_the_orca_app() {
        let scratch = Scratch::new("orca-sqlite");
        let root = scratch.0.join("orca");
        let codex_home = root.join("codex-accounts/c1/home");
        fs::create_dir_all(&codex_home).unwrap();
        orca::write_sqlite_profile(
            &root,
            &json!({
                "claudeManagedAccounts": [
                    { "id": "a1", "email": "one@example.com", "managedAuthRuntime": "host", "createdAt": 1 },
                    { "id": "a2", "email": "two@example.com", "managedAuthRuntime": "host", "createdAt": 2 }
                ],
                "activeClaudeManagedAccountId": "a1",
                "activeClaudeManagedAccountIdsByRuntime": { "host": "a2", "wsl": {} },
                "codexManagedAccounts": [
                    { "id": "c1", "email": "codex@example.com", "managedHomePath": codex_home, "createdAt": 3 }
                ],
                "activeCodexManagedAccountIdsByRuntime": { "host": "c1", "wsl": {} }
            }),
        );
        let mut wallet = scratch.open();
        wallet.orca = Some(root);
        wallet.orca_installed = |_| true;

        let state = wallet.state();
        assert_eq!(state.store, WalletStoreKind::Orca);
        assert_eq!(state.claude.active_id.as_deref(), Some("a2"));
        assert_eq!(state.claude.accounts.len(), 2);
        assert_eq!(state.codex.active_id.as_deref(), Some("c1"));
        assert_eq!(
            wallet.active_home(WalletProvider::Codex),
            ActiveHome::Linked(codex_home)
        );

        // Orca's data without the Orca app does not own the accounts.
        wallet.orca_installed = |_| false;
        assert_eq!(wallet.state().store, WalletStoreKind::Halv);
        assert_eq!(
            wallet.active_home(WalletProvider::Codex),
            ActiveHome::System
        );
    }

    /// Writes Orca's Claude pointer to the folder of `id`. `marker` adds the
    /// file that Orca writes when it finished the setup of the folder.
    fn point_claude_at(root: &Path, id: &str, marker: bool) -> PathBuf {
        let account = root.join("claude-profiles").join(id);
        let home = account.join("home");
        fs::create_dir_all(&home).unwrap();
        if marker {
            fs::write(account.join("profile.json"), "{}").unwrap();
        }
        fs::write(
            root.join("claude-profiles/selected-host"),
            home.to_string_lossy().as_bytes(),
        )
        .unwrap();
        home
    }

    #[test]
    fn a_routed_orca_claude_account_points_claude_at_its_folder() {
        let scratch = Scratch::new("orca-routed");
        let wallet = orca_wallet(&scratch, true);
        let home = point_claude_at(&scratch.0.join("orca"), "a1", true);

        let active = wallet.active_home(WalletProvider::Claude);
        assert_eq!(active, ActiveHome::Linked(home.clone()));
        let environment = environment_for_active(WalletProvider::Claude, &active).unwrap();
        let text = home.clone().into_os_string();
        assert_eq!(
            environment.set,
            vec![
                (OsString::from("CLAUDE_CONFIG_DIR"), text.clone()),
                (OsString::from("ORCA_CLAUDE_INJECTED_CONFIG_DIR"), text),
            ]
        );
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            "AWS_BEARER_TOKEN_BEDROCK",
        ] {
            assert!(environment.remove.contains(&OsString::from(name)), "{name}");
        }
        // The plan pill reads the keychain item of the folder that the
        // launch uses.
        assert_eq!(
            claude_keychain_services_for(&active, || {
                unreachable!("a routed account never reads the system item")
            }),
            vec![claude::keychain_service(Some(&home))]
        );
        // A shell keeps the user's login variables.
        let shell = inherited_environment(WalletProvider::Claude, &active);
        assert_eq!(shell.set.len(), 2);
        assert!(shell.remove.is_empty());
    }

    #[test]
    fn an_orca_claude_folder_without_setup_blocks_the_launch() {
        let scratch = Scratch::new("orca-unready");
        let wallet = orca_wallet(&scratch, true);
        let root = scratch.0.join("orca");
        let home = point_claude_at(&root, "a1", false);

        let active = wallet.active_home(WalletProvider::Claude);
        assert_eq!(active, ActiveHome::NotReady(Some(home.clone())));
        let error = environment_for_active(WalletProvider::Claude, &active).unwrap_err();
        assert!(
            error.contains("select the account again in Orca"),
            "{error}"
        );
        // A `claude` that the user types in a shell later finds the folder
        // that Orca sets up, not another account.
        let shell = inherited_environment(WalletProvider::Claude, &active);
        assert_eq!(
            shell.set[0],
            (OsString::from("CLAUDE_CONFIG_DIR"), home.into_os_string())
        );

        // A missing folder blocks the launch too.
        fs::remove_dir_all(root.join("claude-profiles/a1")).unwrap();
        assert!(matches!(
            wallet.active_home(WalletProvider::Claude),
            ActiveHome::NotReady(Some(_))
        ));

        // An empty pointer means System default.
        fs::write(root.join("claude-profiles/selected-host"), "").unwrap();
        assert_eq!(
            wallet.active_home(WalletProvider::Claude),
            ActiveHome::System
        );
    }

    #[test]
    fn an_orca_pointer_without_an_absolute_folder_blocks_claude() {
        let scratch = Scratch::new("orca-pointer");
        let wallet = orca_wallet(&scratch, true);
        let profiles = scratch.0.join("orca/claude-profiles");
        fs::create_dir_all(&profiles).unwrap();
        // Orca never writes a relative folder. A shell would find it below
        // its own working folder.
        fs::write(profiles.join("selected-host"), "claude-profiles/a1/home").unwrap();

        let active = wallet.active_home(WalletProvider::Claude);
        assert_eq!(active, ActiveHome::NotReady(None));
        let error = environment_for_active(WalletProvider::Claude, &active).unwrap_err();
        assert!(
            error.contains("could not read the selected Claude account"),
            "{error}"
        );
        assert_eq!(
            inherited_environment(WalletProvider::Claude, &active),
            AccountEnvironment::default()
        );
        assert!(claude_keychain_services_for(&active, || {
            unreachable!("Halv reads no keychain item for an unknown account")
        })
        .is_empty());
    }

    #[test]
    fn orca_accounts_that_need_a_sign_in_are_marked() {
        let scratch = Scratch::new("orca-sign-in");
        let wallet = orca_wallet(&scratch, true);
        orca::mirror_snapshot(
            &scratch.0.join("orca"),
            &json!({
                "claude": {
                    "accounts": [
                        { "id": "a1", "email": "one@example.com", "createdAt": 1 },
                        { "id": "a2", "email": "two@example.com", "createdAt": 2, "needsSignIn": true }
                    ],
                    "activeAccountId": "a1",
                    "activeAccountIdsByRuntime": { "host": "a1", "wsl": {} }
                },
                "codex": {
                    "accounts": [],
                    "activeAccountId": null,
                    "activeAccountIdsByRuntime": { "host": null, "wsl": {} }
                }
            }),
        );

        let state = wallet.state();
        assert_eq!(state.claude.active_id.as_deref(), Some("a1"));
        assert!(!state.claude.accounts[0].needs_sign_in);
        assert!(state.claude.accounts[1].needs_sign_in);
        // The UI gets the field only when it is set.
        let accounts = serde_json::to_value(&state.claude.accounts).unwrap();
        assert!(accounts[0].get("needsSignIn").is_none());
        assert_eq!(accounts[1]["needsSignIn"], json!(true));
    }

    #[test]
    fn an_unreadable_orca_store_blocks_codex_launches() {
        let scratch = Scratch::new("orca-unreadable");
        let root = scratch.0.join("orca");
        let profile = root.join("profiles/work");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            root.join("orca-profile-index.json"),
            r#"{"activeProfileId":"work"}"#,
        )
        .unwrap();
        // Halv does not open a write-ahead log without its index.
        fs::write(profile.join("profile-state.db-wal"), b"").unwrap();
        let mut wallet = scratch.open();
        wallet.orca = Some(root);
        wallet.orca_installed = |_| true;

        let active = wallet.active_home(WalletProvider::Codex);
        assert_eq!(active, ActiveHome::NotReady(None));
        assert!(environment_for_active(WalletProvider::Codex, &active).is_err());
        let state = wallet.state();
        assert_eq!(state.store, WalletStoreKind::Orca);
        assert!(state.codex.accounts.is_empty());
        assert!(!profile.join("profile-state.db").exists());
        assert!(!profile.join("profile-state.db-shm").exists());
    }
}
