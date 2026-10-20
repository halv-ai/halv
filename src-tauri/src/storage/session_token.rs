//! The Halv session token in the system keychain (issue #50, migration
//! 0059_auth_session_token_account).
//!
//! Rules:
//! - The keychain item has the service `Halv` and the account
//!   `halv-session-<uuid>`. Each sign-in makes a new account. The column
//!   `auth_session.token_account` names the account of the current session.
//!   Thus two app data folders (for example a debug build with fresh state)
//!   never share one item.
//! - The item holds the token and its server URL in one versioned value:
//!   `{"v":2,"server_url":"...","token":"..."}`. The token goes only to the
//!   URL in the item. A reader never takes the URL of a keychain token from
//!   the column `auth_session.server_url`. Any process of the same user can
//!   write the database, but on macOS it cannot read the item without a
//!   prompt. Without the binding, such a process could set the column to its
//!   own host (also in a copy of the database for `Halv --mcp-savings`), and
//!   Halv would read the item and send the token to that host.
//! - An item of an earlier build holds the bare token. The app binds that
//!   token to the normalized URL of the row and writes the item again, once.
//!   The savings MCP server does not use a bare token and never writes an
//!   item.
//! - A write validates the server URL with `auth::normalize_server_url`:
//!   https, or http to a loopback host for development.
//! - A process reads an item at most once and keeps the value in memory.
//!   macOS can ask for keychain access again after each ad-hoc signed QA
//!   build, so a read per request is not acceptable. The cache keeps only a
//!   successful read or a confirmed missing item. After a failed read (a
//!   locked keychain, a denied prompt, a short D-Bus failure), the vault
//!   reads the item again after a delay. The delay doubles after each failed
//!   read in a row, so a denied prompt does not come back each minute.
//! - The app does not delete an item that it could not read.
//! - Fallback: when the keychain is not available (Linux without a Secret
//!   Service, a locked keychain, a denied access), the token stays in the
//!   column `auth_session.token`. The database file has mode 0600 (#38). The
//!   app logs this once per process, without the token, and tries the move
//!   again at the next start. In this case readers take the URL from the
//!   column, as before. A process that can change the column can also read
//!   the token there, so a binding gives no protection.
//! - Every start moves a token from the column to the keychain. The step
//!   also deletes an item that the row names but that an earlier build
//!   replaced or signed out.
//! - A write that takes a token out of the column also rebuilds the table
//!   with secure_delete on, in the same transaction. Older builds rewrote the
//!   row with secure_delete off, so old copies of the row stay in the free
//!   space of the page. The rebuild writes zeros over the full page. The
//!   write then truncates the -wal file if no other connection is in the
//!   way. VACUUM is not an option: it can change the implicit rowids that
//!   `messages` and `messages_fts` use.
//! - The conditional writes compare the session that readers see (the URL
//!   and the token of the item). They then match the raw columns
//!   `server_url`, `token` and `token_account`. Each session has its own
//!   account, so these columns identify the session.
//! - Tests never touch the real keychain. A test build uses an in-memory store.

use super::{
    column_exists, new_uuid_v4, now_string, AuthSession, Storage, StorageError, StorageResult,
    AUTH_SESSION_SQL,
};
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OptionalExtension, TransactionBehavior,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

pub(super) const MIGRATION_AUTH_SESSION_TOKEN_ACCOUNT: &str = "0059_auth_session_token_account";

const ACCOUNT_PREFIX: &str = "halv-session-";
/// The version of the keychain value. Earlier builds wrote the bare token.
const ITEM_VERSION: u32 = 2;
/// The delay before the vault reads an item again after a failed read.
const READ_RETRY_DELAY: Duration = Duration::from_secs(60);
/// The retry delay doubles after each failed read in a row, at most this
/// number of times (16 minutes for the delay of 60 s).
const READ_RETRY_MAX_DOUBLINGS: u32 = 4;

/// A place for keychain values, keyed by account.
pub(crate) trait SessionTokenStore: Send + Sync {
    /// Reads the value of an account. A missing item reads as `Ok(None)`.
    fn read(&self, account: &str) -> Result<Option<String>, String>;
    /// Writes the value of an account.
    fn write(&self, account: &str, value: &str) -> Result<(), String>;
    /// Deletes the item of an account. A missing item is not an error.
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// The store of this build: the system keychain, or an in-memory store in
/// tests.
pub(crate) fn default_store() -> Arc<dyn SessionTokenStore> {
    #[cfg(not(test))]
    {
        Arc::new(crate::auth::KeychainSessionTokenStore)
    }
    #[cfg(test)]
    {
        Arc::new(MemorySessionTokenStore::default())
    }
}

/// A session token and the server URL that it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundToken {
    pub(crate) server_url: String,
    pub(crate) token: String,
}

/// The result of an item read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenRead {
    /// The item holds a token and its server URL.
    Bound(BoundToken),
    /// The item holds a bare token from an earlier build.
    Legacy(String),
    /// The item does not exist, or its value has an unknown form.
    Missing,
    /// The keychain did not give the item.
    Failed,
}

#[derive(Deserialize)]
struct ItemValue {
    v: u32,
    server_url: String,
    token: String,
}

/// The keychain value of a token and its server URL.
pub(crate) fn encode_item(bound: &BoundToken) -> String {
    serde_json::json!({
        "v": ITEM_VERSION,
        "server_url": bound.server_url,
        "token": bound.token,
    })
    .to_string()
}

/// Reads a keychain value. A bare token is a legacy value. A value with a
/// different version, a URL that is not valid and normalized, or no token has an
/// unknown form and reads as missing.
fn decode_item(value: &str) -> TokenRead {
    if !value.trim_start().starts_with('{') {
        return if value.trim().is_empty() {
            TokenRead::Missing
        } else {
            TokenRead::Legacy(value.to_string())
        };
    }
    let item = serde_json::from_str::<ItemValue>(value)
        .ok()
        .filter(|item| {
            item.v == ITEM_VERSION
                && !item.token.trim().is_empty()
                && crate::auth::normalize_server_url(&item.server_url)
                    .is_ok_and(|url| url == item.server_url)
        });
    match item {
        Some(item) => TokenRead::Bound(BoundToken {
            server_url: item.server_url,
            token: item.token,
        }),
        None => {
            eprintln!("Halv ignores a session token item with an unknown form");
            TokenRead::Missing
        }
    }
}

/// The delay before the next read after `failures` failed reads in a row.
fn retry_delay_after(first_delay: Duration, failures: u32) -> Duration {
    first_delay.saturating_mul(1 << failures.saturating_sub(1).min(READ_RETRY_MAX_DOUBLINGS))
}

/// True when two server URLs are the same after normalization.
fn same_server_url(left: &str, right: &str) -> bool {
    left == right
        || matches!(
            (
                crate::auth::normalize_server_url(left),
                crate::auth::normalize_server_url(right),
            ),
            (Ok(left), Ok(right)) if left == right
        )
}

enum CachedRead {
    /// A successful read. It never holds `TokenRead::Failed`.
    Read(TokenRead),
    /// A failed read: its time and the count of failed reads in a row.
    Failed { at: Instant, failures: u32 },
}

/// A session token store with a cache of the reads of this process.
pub(crate) struct SessionTokenVault {
    store: Arc<dyn SessionTokenStore>,
    cache: Mutex<HashMap<String, CachedRead>>,
    retry_delay: Duration,
    fallback_logged: AtomicBool,
}

impl SessionTokenVault {
    pub(crate) fn new(store: Arc<dyn SessionTokenStore>) -> Self {
        Self::with_retry_delay(store, READ_RETRY_DELAY)
    }

    pub(crate) fn with_retry_delay(
        store: Arc<dyn SessionTokenStore>,
        retry_delay: Duration,
    ) -> Self {
        Self {
            store,
            cache: Mutex::new(HashMap::new()),
            retry_delay,
            fallback_logged: AtomicBool::new(false),
        }
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<String, CachedRead>> {
        self.cache.lock().expect("session token lock poisoned")
    }

    /// Reads the item of an account. A cached read needs no keychain access.
    /// After a failed read, the step reads the store again only after the
    /// retry delay.
    pub(crate) fn read(&self, account: &str) -> TokenRead {
        let mut cache = self.cache();
        match cache.get(account) {
            Some(CachedRead::Read(read)) => return read.clone(),
            Some(CachedRead::Failed { at, failures })
                if at.elapsed() < retry_delay_after(self.retry_delay, *failures) =>
            {
                return TokenRead::Failed;
            }
            _ => {}
        }
        self.read_store(&mut cache, account)
    }

    /// Reads the item of an account from the store now, also within the
    /// retry delay. A cached token needs no new read. Only a user action (a
    /// sign-in or a sign-out) calls this step, so it causes no prompt storm.
    fn read_again(&self, account: &str) -> TokenRead {
        let mut cache = self.cache();
        if let Some(CachedRead::Read(read @ (TokenRead::Bound(_) | TokenRead::Legacy(_)))) =
            cache.get(account)
        {
            return read.clone();
        }
        self.read_store(&mut cache, account)
    }

    fn read_store(&self, cache: &mut HashMap<String, CachedRead>, account: &str) -> TokenRead {
        match self.store.read(account) {
            Ok(value) => {
                let read = value.as_deref().map_or(TokenRead::Missing, decode_item);
                cache.insert(account.to_string(), CachedRead::Read(read.clone()));
                read
            }
            Err(error) => {
                eprintln!("Halv could not read the session token from the keychain: {error}");
                let failures = match cache.get(account) {
                    Some(CachedRead::Failed { failures, .. }) => failures.saturating_add(1),
                    _ => 1,
                };
                cache.insert(
                    account.to_string(),
                    CachedRead::Failed {
                        at: Instant::now(),
                        failures,
                    },
                );
                TokenRead::Failed
            }
        }
    }

    /// The token and the URL of an item, for a reader that must not write
    /// the keychain (the savings MCP server). A bare token of an earlier
    /// build gives no token here, because it names no URL. The app binds it
    /// at its next read, so this vault reads the item again after the retry
    /// delay.
    pub(crate) fn bound_token(&self, account: &str) -> Option<BoundToken> {
        match self.read(account) {
            TokenRead::Bound(bound) => Some(bound),
            TokenRead::Legacy(_) => {
                self.cache().insert(
                    account.to_string(),
                    CachedRead::Failed {
                        at: Instant::now(),
                        failures: 1,
                    },
                );
                None
            }
            TokenRead::Missing | TokenRead::Failed => None,
        }
    }

    /// Writes a token and its URL under a new account and returns the
    /// account.
    fn write_new(&self, bound: &BoundToken) -> Result<String, String> {
        let account = format!("{ACCOUNT_PREFIX}{}", new_uuid_v4());
        self.store.write(&account, &encode_item(bound))?;
        self.cache().insert(
            account.clone(),
            CachedRead::Read(TokenRead::Bound(bound.clone())),
        );
        Ok(account)
    }

    /// Binds the bare token of an account to a URL and writes the item
    /// again. It returns false when the cache no longer holds that bare
    /// token, for example after a sign-out deleted the item. Thus the step
    /// never writes a deleted item again. When the write fails, this process
    /// still uses the binding, and the next start tries the write again.
    fn bind_legacy(&self, account: &str, bound: &BoundToken) -> bool {
        let mut cache = self.cache();
        let legacy = matches!(
            cache.get(account),
            Some(CachedRead::Read(TokenRead::Legacy(token))) if *token == bound.token
        );
        if !legacy {
            return false;
        }
        if let Err(error) = self.store.write(account, &encode_item(bound)) {
            eprintln!(
                "Halv could not bind the session token to its server URL in the keychain: {error}"
            );
        }
        cache.insert(
            account.to_string(),
            CachedRead::Read(TokenRead::Bound(bound.clone())),
        );
        true
    }

    /// Deletes the item of an account. The step logs a failure and continues.
    fn delete(&self, account: &str) {
        self.cache().remove(account);
        if let Err(error) = self.store.delete(account) {
            eprintln!("Halv could not delete the session token from the keychain: {error}");
        }
    }

    /// Deletes the item of an account, but not an item that this process
    /// could not read. After a failed read, the step reads the item once
    /// more. When that read also fails, the item stays: it can hold a session
    /// that the server did not end, and the app could not see that session.
    fn delete_if_readable(&self, account: &str) {
        let last_read_failed = matches!(self.cache().get(account), Some(CachedRead::Failed { .. }));
        if last_read_failed && self.read_again(account) == TokenRead::Failed {
            eprintln!(
                "Halv keeps a session token item in the keychain, because it could not read it"
            );
            return;
        }
        self.delete(account);
    }

    /// Logs once per process that the token stays in the database.
    fn log_fallback(&self, error: &str) {
        if !self.fallback_logged.swap(true, Ordering::Relaxed) {
            eprintln!(
                "Halv keeps the session token in its database, because the keychain is not available: {error}"
            );
        }
    }
}

/// Adds the column `auth_session.token_account`. The step is idempotent. It
/// also makes a missing table, so it does not depend on the migration order.
pub(super) fn apply_auth_session_token_account_migration(
    connection: &Connection,
) -> StorageResult<()> {
    connection.execute_batch(AUTH_SESSION_SQL)?;
    if !column_exists(connection, "auth_session", "token_account")? {
        connection.execute_batch("ALTER TABLE auth_session ADD COLUMN token_account TEXT;")?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AUTH_SESSION_TOKEN_ACCOUNT, now_string()],
    )?;
    Ok(())
}

/// Runs an auth session write in one immediate transaction with
/// secure_delete on. `write` returns its value and true when it took a token
/// out of the column `auth_session.token`. In that case the step rebuilds the
/// table before the commit and truncates the -wal file after the commit.
fn write_auth_session<T>(
    connection: &mut Connection,
    write: impl FnOnce(&Connection) -> StorageResult<(T, bool)>,
) -> StorageResult<T> {
    let previous_mode =
        match connection.pragma_query_value(None, "secure_delete", |row| row.get::<_, i64>(0))? {
            0 => "OFF",
            2 => "FAST",
            _ => "ON",
        };
    connection.pragma_update(None, "secure_delete", "ON")?;
    let written = (|| -> StorageResult<(T, bool)> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (value, removed_token) = write(&transaction)?;
        if removed_token {
            rebuild_auth_session_table(&transaction)?;
        }
        transaction.commit()?;
        Ok((value, removed_token))
    })();
    let _ = connection.pragma_update(None, "secure_delete", previous_mode);
    let (value, removed_token) = written?;
    if removed_token {
        // The -wal file can still hold pages from before the rebuild.
        truncate_wal_without_waiting(connection);
    }
    Ok(value)
}

/// Copies the -wal file into the database file and truncates it, without a
/// wait. The caller holds the storage lock, and a wait would stop all
/// storage calls for up to the busy timeout (5 s). Thus the step turns off
/// the busy handler for the checkpoint. When another connection reads an
/// older snapshot (for example the savings MCP server), the checkpoint copies
/// what it can and stops. The old -wal frames then go at a later checkpoint:
/// the automatic checkpoint, or the checkpoint when the last connection
/// closes.
fn truncate_wal_without_waiting(connection: &Connection) {
    let busy_timeout_ms = connection
        .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
        .map(|milliseconds| milliseconds.max(0) as u64)
        .unwrap_or(5_000);
    if let Err(error) = connection.busy_timeout(Duration::ZERO) {
        eprintln!("Halv could not prepare the database log truncation: {error}");
        return;
    }
    let checkpoint = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        row.get::<_, i64>(0)
    });
    if let Err(error) = connection.busy_timeout(Duration::from_millis(busy_timeout_ms)) {
        eprintln!("Halv could not set the database busy timeout again: {error}");
    }
    match checkpoint {
        Ok(0) => {}
        Ok(_) => eprintln!(
            "Halv truncates the database log later, because another connection reads the database"
        ),
        Err(error) => eprintln!(
            "Halv could not truncate the database log after it removed the session token: {error}"
        ),
    }
}

/// Drops and creates the table `auth_session` again with the same schema
/// and rows. With secure_delete on, the drop writes zeros over the old
/// pages, so old copies of the row leave the database file. Callers run
/// this step in a transaction.
fn rebuild_auth_session_table(connection: &Connection) -> StorageResult<()> {
    let table_sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'auth_session'",
        [],
        |row| row.get(0),
    )?;
    let other_sql = connection
        .prepare(
            "SELECT sql FROM sqlite_master
             WHERE tbl_name = 'auth_session' AND type <> 'table' AND sql IS NOT NULL",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let rows = {
        let mut statement = connection.prepare("SELECT * FROM auth_session")?;
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, Value>(index))
                    .collect::<Result<Vec<_>, _>>()
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    connection.execute_batch("DROP TABLE auth_session;")?;
    connection.execute_batch(&table_sql)?;
    for row in rows {
        let placeholders = vec!["?"; row.len()].join(", ");
        connection.execute(
            &format!("INSERT INTO auth_session VALUES ({placeholders})"),
            params_from_iter(row),
        )?;
    }
    for sql in other_sql {
        connection.execute_batch(&sql)?;
    }
    Ok(())
}

/// The auth session row as stored. `session.token` holds the column value.
/// The column is empty unless the fallback keeps the token in the database.
struct StoredAuthSession {
    session: AuthSession,
    token_account: Option<String>,
}

impl Storage {
    /// The auth session as readers must use it. For a token in the keychain,
    /// `server_url` is the URL of the keychain item, not the column value.
    pub fn get_auth_session(&self) -> StorageResult<Option<AuthSession>> {
        Ok(self
            .stored_auth_session()?
            .map(|stored| self.session_of(&stored)))
    }

    /// Returns the auth session for a sign-out. After a failed keychain
    /// read, the vault waits for the retry delay. A sign-out reads the item
    /// again at once, so it has the token and can end the session on the
    /// server.
    pub fn get_auth_session_for_sign_out(&self) -> StorageResult<Option<AuthSession>> {
        if let Some(stored) = self.stored_auth_session()? {
            if let (None, Some(account)) = (
                stored.session.token.as_ref(),
                stored.token_account.as_deref(),
            ) {
                self.session_token.read_again(account);
            }
        }
        self.get_auth_session()
    }

    pub fn save_auth_session(&self, session: &AuthSession) -> StorageResult<()> {
        // The token goes only to this URL, so the step validates it first.
        let server_url = crate::auth::normalize_server_url(&session.server_url)
            .map_err(StorageError::Validation)?;
        let (token, token_account) = match session.token.as_deref() {
            None => (None, None),
            Some(token) => match self.session_token.write_new(&BoundToken {
                server_url: server_url.clone(),
                token: token.to_string(),
            }) {
                Ok(account) => (None, Some(account)),
                Err(error) => {
                    self.session_token.log_fallback(&error);
                    (Some(token.to_string()), None)
                }
            },
        };
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let saved = write_auth_session(&mut connection, |connection| {
            let (previous_token, previous_account) = connection
                .query_row(
                    "SELECT token, token_account FROM auth_session WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .optional()?
                .unwrap_or_default();
            connection.execute(
                "INSERT INTO auth_session
                    (id, server_url, token, token_account, email, plan, entitlements_json, synced_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    server_url = excluded.server_url,
                    token = excluded.token,
                    token_account = excluded.token_account,
                    email = excluded.email,
                    plan = excluded.plan,
                    entitlements_json = excluded.entitlements_json,
                    synced_at = excluded.synced_at",
                params![
                    &server_url,
                    &token,
                    &token_account,
                    &session.email,
                    &session.plan,
                    &session.entitlements_json,
                    &session.synced_at
                ],
            )?;
            let removed_token = previous_token.is_some() && previous_token != token;
            Ok((previous_account, removed_token))
        });
        drop(connection);
        match saved {
            Ok(previous_account) => {
                if let Some(previous) =
                    previous_account.filter(|previous| Some(previous) != token_account.as_ref())
                {
                    self.session_token.delete_if_readable(&previous);
                }
                Ok(())
            }
            Err(error) => {
                if let Some(account) = &token_account {
                    self.session_token.delete(account);
                }
                Err(error)
            }
        }
    }

    pub fn clear_auth_session_if_current(
        &self,
        expected_server_url: &str,
        expected_token: Option<&str>,
    ) -> StorageResult<bool> {
        let Some(stored) =
            self.stored_auth_session_if_current(expected_server_url, expected_token)?
        else {
            return Ok(false);
        };
        let updated = write_auth_session(
            &mut self.connection.lock().expect("storage lock poisoned"),
            |connection| {
                let updated = connection.execute(
                    "UPDATE auth_session
                     SET token = NULL,
                         token_account = NULL,
                         email = NULL,
                         plan = NULL,
                         entitlements_json = NULL,
                         synced_at = NULL
                     WHERE id = 1
                       AND server_url IS ?1
                       AND token IS ?2
                       AND token_account IS ?3",
                    params![
                        &stored.session.server_url,
                        &stored.session.token,
                        &stored.token_account
                    ],
                )?;
                // In the fallback the column held the token.
                Ok((updated, updated == 1 && stored.session.token.is_some()))
            },
        )?;
        if updated == 1 {
            if let Some(account) = &stored.token_account {
                self.session_token.delete_if_readable(account);
            }
        }
        Ok(updated == 1)
    }

    pub fn update_auth_session_if_current(
        &self,
        expected_server_url: &str,
        expected_token: &str,
        email: &str,
        plan: &str,
        entitlements_json: &str,
        synced_at: &str,
    ) -> StorageResult<bool> {
        let Some(stored) =
            self.stored_auth_session_if_current(expected_server_url, Some(expected_token))?
        else {
            return Ok(false);
        };
        let updated = self
            .connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE auth_session
                 SET email = ?1,
                     plan = ?2,
                     entitlements_json = ?3,
                     synced_at = ?4
                 WHERE id = 1
                   AND server_url IS ?5
                   AND token IS ?6
                   AND token_account IS ?7",
                params![
                    email,
                    plan,
                    entitlements_json,
                    synced_at,
                    &stored.session.server_url,
                    &stored.session.token,
                    &stored.token_account
                ],
            )?;
        Ok(updated == 1)
    }

    pub(crate) fn update_auth_entitlements_if_current(
        &self,
        expected_server_url: &str,
        expected_token: &str,
        entitlements_json: &str,
    ) -> StorageResult<bool> {
        let Some(stored) =
            self.stored_auth_session_if_current(expected_server_url, Some(expected_token))?
        else {
            return Ok(false);
        };
        let updated = self
            .connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE auth_session
                 SET entitlements_json = ?1
                 WHERE id = 1
                   AND server_url IS ?2
                   AND token IS ?3
                   AND token_account IS ?4",
                params![
                    entitlements_json,
                    &stored.session.server_url,
                    &stored.session.token,
                    &stored.token_account
                ],
            )?;
        Ok(updated == 1)
    }

    /// The start step for the session token. It moves a token from the
    /// column `auth_session.token` to the keychain, bound to the normalized
    /// URL of the row. It returns true when it moved a token. When the
    /// keychain is not available, the token stays in the column for the next
    /// start. For a row without a token, the step forgets an item that an
    /// earlier build signed out.
    pub(crate) fn move_session_token_to_keychain(&self) -> StorageResult<bool> {
        let Some(stored) = self.stored_auth_session()? else {
            return Ok(false);
        };
        let Some(token) = stored.session.token.as_deref() else {
            self.forget_signed_out_token_account(&stored)?;
            return Ok(false);
        };
        let server_url = match crate::auth::normalize_server_url(&stored.session.server_url) {
            Ok(server_url) => server_url,
            Err(error) => {
                // The app rejects this URL for requests. The token stays
                // where it is, as in the fallback.
                eprintln!(
                    "Halv keeps the session token in its database, because the stored server URL is not valid: {error}"
                );
                return Ok(false);
            }
        };
        let account = match self.session_token.write_new(&BoundToken {
            server_url: server_url.clone(),
            token: token.to_string(),
        }) {
            Ok(account) => account,
            Err(error) => {
                self.session_token.log_fallback(&error);
                return Ok(false);
            }
        };
        let moved = write_auth_session(
            &mut self.connection.lock().expect("storage lock poisoned"),
            |connection| {
                let moved = connection.execute(
                    "UPDATE auth_session
                     SET server_url = ?1,
                         token = NULL,
                         token_account = ?2
                     WHERE id = 1
                       AND server_url IS ?3
                       AND token IS ?4
                       AND token_account IS ?5",
                    params![
                        &server_url,
                        &account,
                        &stored.session.server_url,
                        token,
                        &stored.token_account
                    ],
                )? == 1;
                Ok((moved, moved))
            },
        );
        match moved {
            Ok(true) => {
                // A column token next to an account means that an earlier
                // build, which does not know the column `token_account`,
                // signed in again. Its new session replaced the session of
                // that item.
                if let Some(previous) = &stored.token_account {
                    self.session_token.delete(previous);
                }
            }
            _ => self.session_token.delete(&account),
        }
        moved
    }

    /// An earlier build that does not know the column `token_account` signs
    /// out with `token`, `email` and `plan` set to NULL and keeps
    /// `token_account`. A sign-in of this build always stores an email and a
    /// plan next to the account, so the app can detect such a row. The step
    /// treats the row as signed out, as the earlier build did. It clears the
    /// column and deletes the item. A row that an earlier build signed in to
    /// again holds a column token; the move step deletes its old item.
    fn forget_signed_out_token_account(&self, stored: &StoredAuthSession) -> StorageResult<()> {
        let Some(account) = stored.token_account.as_deref() else {
            return Ok(());
        };
        if stored.session.email.is_some() || stored.session.plan.is_some() {
            return Ok(());
        }
        let updated = self
            .connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE auth_session
                 SET token_account = NULL
                 WHERE id = 1
                   AND token IS NULL
                   AND token_account IS ?1
                   AND email IS NULL
                   AND plan IS NULL",
                params![account],
            )?;
        if updated == 1 {
            self.session_token.delete(account);
        }
        Ok(())
    }

    fn stored_auth_session(&self) -> StorageResult<Option<StoredAuthSession>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT server_url, token, email, plan, entitlements_json, synced_at, token_account
                 FROM auth_session
                 WHERE id = 1",
                [],
                |row| {
                    Ok(StoredAuthSession {
                        session: AuthSession {
                            server_url: row.get(0)?,
                            token: row.get(1)?,
                            email: row.get(2)?,
                            plan: row.get(3)?,
                            entitlements_json: row.get(4)?,
                            synced_at: row.get(5)?,
                        },
                        token_account: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Returns the stored row when the session that readers see has the
    /// expected server URL and token. The caller then writes with the raw
    /// columns as the condition.
    fn stored_auth_session_if_current(
        &self,
        expected_server_url: &str,
        expected_token: Option<&str>,
    ) -> StorageResult<Option<StoredAuthSession>> {
        let Some(stored) = self.stored_auth_session()? else {
            return Ok(None);
        };
        let session = self.session_of(&stored);
        let current = same_server_url(&session.server_url, expected_server_url)
            && session.token.as_deref() == expected_token;
        Ok(current.then_some(stored))
    }

    /// The session of a row as readers must use it.
    fn session_of(&self, stored: &StoredAuthSession) -> AuthSession {
        let mut session = stored.session.clone();
        if session.token.is_some() {
            // The fallback: the column holds the token, so the column URL
            // stays the destination. A process that can change that URL can
            // also read the token in the column.
            return session;
        }
        let bound = stored
            .token_account
            .as_deref()
            .and_then(|account| self.bound_token_of(account, &stored.session.server_url));
        if let Some(bound) = bound {
            session.server_url = bound.server_url;
            session.token = Some(bound.token);
        }
        session
    }

    /// The token of an item and its URL. A bare token of an earlier build
    /// gets the normalized URL of the row, once.
    fn bound_token_of(&self, account: &str, row_server_url: &str) -> Option<BoundToken> {
        match self.session_token.read(account) {
            TokenRead::Bound(bound) => Some(bound),
            TokenRead::Legacy(token) => {
                let server_url = match crate::auth::normalize_server_url(row_server_url) {
                    Ok(server_url) => server_url,
                    Err(error) => {
                        eprintln!(
                            "Halv does not use the session token, because the stored server URL is not valid: {error}"
                        );
                        return None;
                    }
                };
                let bound = BoundToken { server_url, token };
                self.session_token
                    .bind_legacy(account, &bound)
                    .then_some(bound)
            }
            TokenRead::Missing | TokenRead::Failed => None,
        }
    }
}

/// An in-memory session token store for tests. It counts the reads and can
/// act as an unavailable keychain, or as a keychain whose reads fail.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemorySessionTokenStore {
    items: Mutex<std::collections::BTreeMap<String, String>>,
    reads: std::sync::atomic::AtomicUsize,
    unavailable: AtomicBool,
    reads_fail: AtomicBool,
}

#[cfg(test)]
impl MemorySessionTokenStore {
    pub(crate) fn set_available(&self, available: bool) {
        self.unavailable.store(!available, Ordering::SeqCst);
    }

    /// Makes reads fail while writes and deletes still work, as after a
    /// denied read prompt.
    pub(crate) fn set_reads_fail(&self, fail: bool) {
        self.reads_fail.store(fail, Ordering::SeqCst);
    }

    /// The raw values, keyed by account.
    pub(crate) fn items(&self) -> std::collections::BTreeMap<String, String> {
        self.items.lock().unwrap().clone()
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn check_available(&self) -> Result<(), String> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err("test keychain is not available".to_string())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
impl SessionTokenStore for MemorySessionTokenStore {
    fn read(&self, account: &str) -> Result<Option<String>, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.check_available()?;
        if self.reads_fail.load(Ordering::SeqCst) {
            return Err("test keychain read failed".to_string());
        }
        Ok(self.items.lock().unwrap().get(account).cloned())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), String> {
        self.check_available()?;
        self.items
            .lock()
            .unwrap()
            .insert(account.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.check_available()?;
        self.items.lock().unwrap().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    const SERVER: &str = "https://api.halv.test";
    const ATTACKER: &str = "https://attacker.test";
    /// A fake token as long as a real session JWT. The moved row is thus
    /// much shorter than the old row.
    const LEGACY_TOKEN: &str = concat!(
        "legacy-token-5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c",
        "5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c",
        "5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c5f2c",
    );

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-{name}-{}.sqlite3", new_uuid_v4()))
    }

    fn remove_db(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.as_os_str().to_owned();
            file.push(suffix);
            let _ = std::fs::remove_file(file);
        }
    }

    fn open(path: &Path, store: &Arc<MemorySessionTokenStore>) -> Storage {
        Storage::open_with_session_token_store(path, store.clone()).expect("storage should open")
    }

    /// Opens storage with a vault that reads again at once after a failure.
    fn open_without_retry_delay(path: &Path, store: &Arc<MemorySessionTokenStore>) -> Storage {
        Storage::open_with_session_token_vault(
            path,
            SessionTokenVault::with_retry_delay(store.clone(), Duration::ZERO),
        )
        .expect("storage should open")
    }

    fn session(token: Option<&str>) -> AuthSession {
        AuthSession {
            server_url: SERVER.to_string(),
            token: token.map(str::to_string),
            email: Some("user@example.test".to_string()),
            plan: Some("pro".to_string()),
            entitlements_json: None,
            synced_at: None,
        }
    }

    fn bound(server_url: &str, token: &str) -> String {
        encode_item(&BoundToken {
            server_url: server_url.to_string(),
            token: token.to_string(),
        })
    }

    /// The tokens in the store, keyed by account.
    fn item_tokens(store: &MemorySessionTokenStore) -> BTreeMap<String, String> {
        store
            .items()
            .into_iter()
            .map(|(account, value)| match decode_item(&value) {
                TokenRead::Bound(bound) => (account, bound.token),
                TokenRead::Legacy(token) => (account, token),
                other => panic!("unexpected test item: {other:?}"),
            })
            .collect()
    }

    /// The raw columns `token` and `token_account`, read past the vault.
    fn raw_token_columns(path: &Path) -> (Option<String>, Option<String>) {
        Connection::open(path)
            .expect("test database should open")
            .query_row(
                "SELECT token, token_account FROM auth_session WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("auth session row should exist")
    }

    /// Changes the stored server URL past the app, as a same-user process can.
    fn set_raw_server_url(path: &Path, server_url: &str) {
        Connection::open(path)
            .expect("test database should open")
            .execute(
                "UPDATE auth_session SET server_url = ?1 WHERE id = 1",
                params![server_url],
            )
            .expect("server URL should update");
    }

    fn stored_token(storage: &Storage) -> Option<String> {
        storage
            .get_auth_session()
            .expect("auth session should query")
            .expect("auth session should exist")
            .token
    }

    /// True when a database file holds any 12 bytes of the token in a row.
    fn file_holds_token_bytes(path: &Path, suffix: &str, token: &str) -> bool {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        let Ok(bytes) = std::fs::read(file) else {
            return false;
        };
        token.as_bytes().windows(12).any(|fragment| {
            bytes
                .windows(fragment.len())
                .any(|window| window == fragment)
        })
    }

    #[test]
    fn sign_in_stores_the_token_and_its_url_in_the_keychain_only() {
        let path = test_db_path("token-sign-in");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);

        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");

        let (column, account) = raw_token_columns(&path);
        assert_eq!(column, None);
        let account = account.expect("the row should name a keychain account");
        assert!(account.starts_with(ACCOUNT_PREFIX));
        assert_eq!(
            store.items(),
            BTreeMap::from([(account, bound(SERVER, "token-a"))])
        );
        let value: serde_json::Value =
            serde_json::from_str(&bound(SERVER, "token-a")).expect("the value should be JSON");
        assert_eq!(
            value,
            serde_json::json!({ "v": 2, "server_url": SERVER, "token": "token-a" })
        );
        assert_eq!(stored_token(&storage).as_deref(), Some("token-a"));
        // The write fills the cache, so the read needs no keychain access.
        assert_eq!(store.reads(), 0);

        drop(storage);
        remove_db(&path);
    }

    #[test]
    fn sign_in_validates_and_normalizes_the_server_url() {
        let path = test_db_path("token-url");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);

        for server_url in ["http://attacker.test", "ftp://api.halv.test", " "] {
            let error = storage
                .save_auth_session(&AuthSession {
                    server_url: server_url.to_string(),
                    ..session(Some("token-a"))
                })
                .expect_err("an unsafe server URL should be rejected");
            assert!(matches!(error, StorageError::Validation(_)));
        }
        assert!(store.items().is_empty());
        assert_eq!(storage.get_auth_session().expect("query"), None);

        storage
            .save_auth_session(&AuthSession {
                server_url: format!("{SERVER}/"),
                ..session(Some("token-a"))
            })
            .expect("a trailing slash should normalize");
        let saved = storage
            .get_auth_session()
            .expect("query")
            .expect("session should exist");
        assert_eq!(saved.server_url, SERVER);
        assert_eq!(
            store.items().into_values().collect::<Vec<_>>(),
            vec![bound(SERVER, "token-a")]
        );
        // The conditional writes accept both forms of the URL.
        assert!(storage
            .update_auth_entitlements_if_current(&format!("{SERVER}/"), "token-a", "{}")
            .expect("update should be checked"));

        // A development server on a loopback host can use http.
        storage
            .save_auth_session(&AuthSession {
                server_url: "http://127.0.0.1:8787".to_string(),
                ..session(Some("token-b"))
            })
            .expect("a loopback http URL should save");

        drop(storage);
        remove_db(&path);
    }

    #[test]
    fn a_changed_database_url_does_not_change_where_the_token_goes() {
        let path = test_db_path("token-oracle");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");

        // A same-user process points the row at its own host.
        set_raw_server_url(&path, ATTACKER);
        let current = storage
            .get_auth_session()
            .expect("query")
            .expect("session should exist");
        assert_eq!(current.server_url, SERVER);
        assert_eq!(current.token.as_deref(), Some("token-a"));
        drop(storage);

        // Also a new process (for example a copy of the database) reads the
        // URL of the item.
        let restarted = open(&path, &store);
        let current = restarted
            .get_auth_session()
            .expect("query")
            .expect("session should exist");
        assert_eq!(current.server_url, SERVER);
        assert_eq!(current.token.as_deref(), Some("token-a"));
        assert!(!restarted
            .update_auth_entitlements_if_current(ATTACKER, "token-a", "{}")
            .expect("update should be checked"));
        assert!(restarted
            .update_auth_session_if_current(
                SERVER,
                "token-a",
                "user@example.test",
                "pro",
                "{}",
                "2026-10-16T12:00:00+00:00",
            )
            .expect("update should be checked"));
        assert!(!restarted
            .clear_auth_session_if_current(ATTACKER, Some("token-a"))
            .expect("sign-out should be checked"));
        assert!(restarted
            .clear_auth_session_if_current(SERVER, Some("token-a"))
            .expect("sign-out should clear the session"));
        assert!(store.items().is_empty());

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn a_bare_token_item_is_bound_to_the_row_url_once() {
        let path = test_db_path("token-legacy-item");
        drop(open(&path, &Arc::new(MemorySessionTokenStore::default())));
        let account = format!("{ACCOUNT_PREFIX}legacy");
        // An earlier build of this change wrote the bare token.
        let store = Arc::new(MemorySessionTokenStore::default());
        store
            .write(&account, "bare-token")
            .expect("legacy item should write");
        Connection::open(&path)
            .expect("test database should open")
            .execute(
                "INSERT INTO auth_session (id, server_url, token_account, email, plan)
                 VALUES (1, ?1, ?2, 'user@example.test', 'pro')",
                params![format!("{SERVER}/"), &account],
            )
            .expect("legacy row should insert");

        let storage = open(&path, &store);
        let current = storage
            .get_auth_session()
            .expect("query")
            .expect("session should exist");
        assert_eq!(current.server_url, SERVER);
        assert_eq!(current.token.as_deref(), Some("bare-token"));
        assert_eq!(
            store.items(),
            BTreeMap::from([(account.clone(), bound(SERVER, "bare-token"))])
        );
        assert_eq!(store.reads(), 1);
        drop(storage);

        // After the binding, a changed row URL has no effect.
        set_raw_server_url(&path, ATTACKER);
        let restarted = open(&path, &store);
        assert_eq!(
            restarted
                .get_auth_session()
                .expect("query")
                .expect("session should exist")
                .server_url,
            SERVER
        );
        assert_eq!(
            store.items(),
            BTreeMap::from([(account, bound(SERVER, "bare-token"))])
        );

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn an_item_with_an_unknown_form_gives_no_token() {
        for value in [
            r#"{"v":3,"server_url":"https://api.halv.test","token":"token-a"}"#,
            r#"{"v":2,"server_url":"http://attacker.test","token":"token-a"}"#,
            r#"{"v":2,"server_url":"https://api.halv.test/","token":"token-a"}"#,
            r#"{"v":2,"server_url":"https://api.halv.test","token":" "}"#,
            r#"{"v":2,"token":"token-a"}"#,
            "{not json",
            "  ",
        ] {
            assert_eq!(decode_item(value), TokenRead::Missing, "{value}");
        }
        assert_eq!(
            decode_item(&bound(SERVER, "token-a")),
            TokenRead::Bound(BoundToken {
                server_url: SERVER.to_string(),
                token: "token-a".to_string(),
            })
        );
        assert_eq!(
            decode_item("token-a"),
            TokenRead::Legacy("token-a".to_string())
        );
    }

    #[test]
    fn sign_out_deletes_the_keychain_item() {
        let path = test_db_path("token-sign-out");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");

        assert!(storage
            .clear_auth_session_if_current(SERVER, Some("token-a"))
            .expect("sign-out should clear the session"));

        assert!(store.items().is_empty());
        assert_eq!(raw_token_columns(&path), (None, None));
        let cleared = storage
            .get_auth_session()
            .expect("cleared session should query")
            .expect("the server URL should remain");
        assert_eq!(cleared.token, None);
        assert_eq!(cleared.email, None);
        assert_eq!(cleared.server_url, SERVER);

        drop(storage);
        remove_db(&path);
    }

    #[test]
    fn account_switch_replaces_the_keychain_item() {
        let path = test_db_path("token-switch");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("first session should save");
        storage
            .save_auth_session(&session(Some("token-b")))
            .expect("second session should save");

        assert_eq!(
            item_tokens(&store).into_values().collect::<Vec<_>>(),
            vec!["token-b".to_string()]
        );
        assert_eq!(stored_token(&storage).as_deref(), Some("token-b"));
        assert!(!storage
            .clear_auth_session_if_current(SERVER, Some("token-a"))
            .expect("stale sign-out should be checked"));
        assert!(!storage
            .update_auth_session_if_current(
                SERVER,
                "token-a",
                "user@example.test",
                "pro",
                "{}",
                "2026-10-15T12:00:00+00:00",
            )
            .expect("stale update should be checked"));
        assert!(storage
            .update_auth_entitlements_if_current(SERVER, "token-b", r#"{"canUseApp":true}"#)
            .expect("current entitlements should update"));
        assert!(storage
            .update_auth_session_if_current(
                SERVER,
                "token-b",
                "user@example.test",
                "pro",
                "{}",
                "2026-10-15T12:00:00+00:00",
            )
            .expect("current session should update"));

        drop(storage);
        remove_db(&path);
    }

    #[test]
    fn start_moves_a_database_token_to_the_keychain_once() {
        let path = test_db_path("token-move");
        // An older build kept the token in the column.
        drop(open(&path, &Arc::new(MemorySessionTokenStore::default())));
        Connection::open(&path)
            .expect("test database should open")
            .execute(
                "INSERT INTO auth_session (id, server_url, token, email) VALUES (1, ?1, ?2, ?3)",
                params![format!("{SERVER}/"), LEGACY_TOKEN, "user@example.test"],
            )
            .expect("legacy session should insert");
        assert!(file_holds_token_bytes(&path, "", LEGACY_TOKEN));

        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);

        assert!(!file_holds_token_bytes(&path, "", LEGACY_TOKEN));
        assert!(!file_holds_token_bytes(&path, "-wal", LEGACY_TOKEN));
        let (column, account) = raw_token_columns(&path);
        assert_eq!(column, None);
        let account = account.expect("the row should name a keychain account");
        // The item binds the token to the normalized URL of the row.
        assert_eq!(
            store.items(),
            BTreeMap::from([(account.clone(), bound(SERVER, LEGACY_TOKEN))])
        );
        assert_eq!(stored_token(&storage).as_deref(), Some(LEGACY_TOKEN));
        drop(storage);

        let restarted = open(&path, &store);
        assert_eq!(raw_token_columns(&path), (None, Some(account)));
        assert_eq!(store.items().len(), 1);
        assert!(!restarted
            .move_session_token_to_keychain()
            .expect("a second move should do nothing"));
        assert_eq!(stored_token(&restarted).as_deref(), Some(LEGACY_TOKEN));

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn start_deletes_the_item_that_an_earlier_build_replaced() {
        let path = test_db_path("token-move-stale");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        let (_, old_account) = raw_token_columns(&path);
        let old_account = old_account.expect("the row should name a keychain account");
        drop(storage);

        // A downgrade: the earlier build signs in again. It writes the token
        // column and keeps the column `token_account`.
        Connection::open(&path)
            .expect("test database should open")
            .execute(
                "UPDATE auth_session SET token = 'token-b', email = 'other@example.test' WHERE id = 1",
                [],
            )
            .expect("earlier build sign-in should update");

        let restarted = open(&path, &store);
        let (column, account) = raw_token_columns(&path);
        assert_eq!(column, None);
        let account = account.expect("the row should name a keychain account");
        assert_ne!(account, old_account);
        assert_eq!(
            store.items(),
            BTreeMap::from([(account, bound(SERVER, "token-b"))])
        );
        assert_eq!(stored_token(&restarted).as_deref(), Some("token-b"));

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn start_forgets_an_item_after_an_earlier_build_signed_out() {
        let path = test_db_path("token-stale-sign-out");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        drop(storage);

        // A downgrade: the earlier build signs in and signs out. Its
        // sign-out keeps the column `token_account`.
        Connection::open(&path)
            .expect("test database should open")
            .execute(
                "UPDATE auth_session
                 SET token = NULL, email = NULL, plan = NULL,
                     entitlements_json = NULL, synced_at = NULL
                 WHERE id = 1",
                [],
            )
            .expect("earlier build sign-out should update");

        let restarted = open(&path, &store);
        assert_eq!(raw_token_columns(&path), (None, None));
        assert!(store.items().is_empty());
        assert_eq!(stored_token(&restarted), None);

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn start_keeps_a_signed_in_row_of_this_build() {
        let path = test_db_path("token-start-keeps");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        let columns = raw_token_columns(&path);
        drop(storage);

        let restarted = open(&path, &store);
        assert_eq!(raw_token_columns(&path), columns);
        assert_eq!(store.items().len(), 1);
        assert_eq!(stored_token(&restarted).as_deref(), Some("token-a"));

        drop(restarted);
        remove_db(&path);
    }

    /// Entitlements before and after a paid checkout. The second text is
    /// shorter, so the row shrinks and the old cell stays in the free space
    /// of the page.
    const TRIAL_ENTITLEMENTS: &str = r#"{"trialConfirmed":false,"paymentConfirmed":false,"billingRequired":true,"canUseApp":false,"plan":"basic","message":"Start your trial to keep using Halv.","checkoutUrl":"https://checkout.example.test/c/pay/cs_test_0000000000000000000000","billingPortalUrl":null,"savings":{}}"#;
    const PAID_ENTITLEMENTS: &str = r#"{"trialConfirmed":true,"paymentConfirmed":true,"billingRequired":false,"canUseApp":true,"plan":"pro","message":null,"checkoutUrl":null,"billingPortalUrl":null,"savings":{}}"#;

    #[test]
    fn start_removes_old_copies_of_a_rewritten_legacy_row() {
        let path = test_db_path("token-move-history");
        drop(open(&path, &Arc::new(MemorySessionTokenStore::default())));
        // As long as a token from random_urlsafe_token.
        let token = "Tk_fake0B2c3D4e5F6g7H8i9J0kLmNoPqRsTuVwXyZ1";
        {
            // An older build signed in, then synced trial and paid
            // entitlements, with secure_delete off.
            let connection = Connection::open(&path).expect("test database should open");
            connection
                .execute(
                    "INSERT INTO auth_session (id, server_url, token, email, plan)
                     VALUES (1, ?1, ?2, 'user@example.test', 'basic')",
                    params![SERVER, token],
                )
                .expect("legacy session should insert");
            for (plan, entitlements) in [("basic", TRIAL_ENTITLEMENTS), ("pro", PAID_ENTITLEMENTS)]
            {
                connection
                    .execute(
                        "UPDATE auth_session SET plan = ?1, entitlements_json = ?2 WHERE id = 1",
                        params![plan, entitlements],
                    )
                    .expect("legacy entitlements should update");
            }
            connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .expect("legacy checkpoint should run");
        }

        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);

        assert!(!file_holds_token_bytes(&path, "", token));
        assert!(!file_holds_token_bytes(&path, "-wal", token));
        let session = storage
            .get_auth_session()
            .expect("auth session should query")
            .expect("auth session should exist");
        assert_eq!(session.token.as_deref(), Some(token));
        assert_eq!(session.plan.as_deref(), Some("pro"));
        assert_eq!(
            session.entitlements_json.as_deref(),
            Some(PAID_ENTITLEMENTS)
        );
        assert_eq!(session.email.as_deref(), Some("user@example.test"));
        // The rebuilt table keeps its schema and its single-row check.
        assert!(Connection::open(&path)
            .expect("test database should open")
            .execute(
                "INSERT INTO auth_session (id, server_url) VALUES (2, ?1)",
                params![SERVER],
            )
            .is_err());

        drop(storage);
        remove_db(&path);
    }

    #[test]
    fn fallback_sign_in_and_sign_out_leave_no_old_token_bytes() {
        let path = test_db_path("token-fallback-history");
        let store = Arc::new(MemorySessionTokenStore::default());
        store.set_available(false);
        let storage = open(&path, &store);
        let first = "Tk_first_q7Lm2Zx9Rv4Bn8Kc3Wd6Hs1Jf5Gt0Yp_A1";
        let second = "Tk_second_Ua3Ei7Oy1Pw5Qz9Xc2Vb6Nm4Lk8Jh_B22";
        let sync = |token: &str| {
            for (plan, entitlements) in [("basic", TRIAL_ENTITLEMENTS), ("pro", PAID_ENTITLEMENTS)]
            {
                assert!(storage
                    .update_auth_session_if_current(
                        SERVER,
                        token,
                        "user@example.test",
                        plan,
                        entitlements,
                        "2026-10-15T12:00:00+00:00",
                    )
                    .expect("entitlements should update"));
            }
        };

        storage
            .save_auth_session(&session(Some(first)))
            .expect("first sign-in should work without a keychain");
        sync(first);
        // An account switch replaces the token in the column.
        storage
            .save_auth_session(&session(Some(second)))
            .expect("second sign-in should work without a keychain");
        assert!(!file_holds_token_bytes(&path, "", first));
        assert!(!file_holds_token_bytes(&path, "-wal", first));
        assert_eq!(stored_token(&storage).as_deref(), Some(second));

        sync(second);
        assert!(storage
            .clear_auth_session_if_current(SERVER, Some(second))
            .expect("sign-out should work without a keychain"));
        assert!(!file_holds_token_bytes(&path, "", second));
        assert!(!file_holds_token_bytes(&path, "-wal", second));
        assert_eq!(raw_token_columns(&path), (None, None));

        drop(storage);
        remove_db(&path);
    }

    #[test]
    fn a_token_removal_does_not_wait_for_a_reader() {
        let path = test_db_path("token-checkpoint");
        let store = Arc::new(MemorySessionTokenStore::default());
        store.set_available(false);
        let storage = open(&path, &store);
        let token = "Tk_reader_Ua3Ei7Oy1Pw5Qz9Xc2Vb6Nm4Lk8Jh_C33";
        storage
            .save_auth_session(&session(Some(token)))
            .expect("sign-in should work without a keychain");

        // Another connection (for example the savings MCP server) holds a
        // read transaction on the current snapshot.
        let reader = Connection::open(&path).expect("reader should open");
        reader.execute_batch("BEGIN").expect("read should begin");
        reader
            .query_row("SELECT count(*) FROM auth_session", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("reader should read");

        let started = Instant::now();
        assert!(storage
            .clear_auth_session_if_current(SERVER, Some(token))
            .expect("sign-out should work without a keychain"));
        // The busy timeout is 5 s. A blocking checkpoint waits that long.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the sign-out waited {:?} for the reader",
            started.elapsed()
        );
        let busy_timeout: i64 = storage
            .connection
            .lock()
            .unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .expect("busy timeout should query");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(raw_token_columns(&path), (None, None));

        // The old -wal frames go at a later checkpoint, here at the close.
        reader.execute_batch("COMMIT").expect("read should end");
        drop(reader);
        drop(storage);
        assert!(!file_holds_token_bytes(&path, "", token));
        assert!(!file_holds_token_bytes(&path, "-wal", token));

        remove_db(&path);
    }

    #[test]
    fn a_start_reads_the_keychain_once() {
        let path = test_db_path("token-read-once");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        drop(storage);

        let restarted = open(&path, &store);
        for _ in 0..3 {
            assert_eq!(stored_token(&restarted).as_deref(), Some("token-a"));
        }
        assert_eq!(store.reads(), 1);

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn unavailable_keychain_keeps_the_token_in_the_database() {
        let path = test_db_path("token-fallback");
        let store = Arc::new(MemorySessionTokenStore::default());
        store.set_available(false);
        let storage = open(&path, &store);

        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("sign-in should work without a keychain");
        assert_eq!(
            raw_token_columns(&path),
            (Some("token-a".to_string()), None)
        );
        assert_eq!(stored_token(&storage).as_deref(), Some("token-a"));
        assert!(storage
            .clear_auth_session_if_current(SERVER, Some("token-a"))
            .expect("sign-out should work without a keychain"));
        assert_eq!(raw_token_columns(&path), (None, None));

        storage
            .save_auth_session(&session(Some("token-b")))
            .expect("second sign-in should work without a keychain");
        assert!(storage
            .update_auth_session_if_current(
                SERVER,
                "token-b",
                "user@example.test",
                "pro",
                "{}",
                "2026-10-15T12:00:00+00:00",
            )
            .expect("update should work without a keychain"));
        drop(storage);
        assert!(store.items().is_empty());

        // A later start with a keychain moves the token.
        store.set_available(true);
        let restarted = open(&path, &store);
        let (column, account) = raw_token_columns(&path);
        assert_eq!(column, None);
        assert!(account.is_some());
        assert_eq!(stored_token(&restarted).as_deref(), Some("token-b"));
        assert!(restarted
            .clear_auth_session_if_current(SERVER, Some("token-b"))
            .expect("sign-out should clear the session"));
        assert!(store.items().is_empty());

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn a_failed_keychain_read_waits_for_the_retry_delay() {
        let path = test_db_path("token-read-failure");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        drop(storage);

        store.set_available(false);
        let restarted = open(&path, &store);
        assert_eq!(stored_token(&restarted), None);
        store.set_available(true);
        // Within the retry delay, the vault does not ask again.
        assert_eq!(stored_token(&restarted), None);
        assert_eq!(store.reads(), 1);

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn a_failed_keychain_read_is_retried() {
        let path = test_db_path("token-read-retry");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        drop(storage);

        // A short keychain failure at start reads as signed out.
        store.set_reads_fail(true);
        let restarted = open_without_retry_delay(&path, &store);
        assert_eq!(stored_token(&restarted), None);
        assert_eq!(store.reads(), 1);

        // After the retry delay, the next read gets the token. The process
        // needs no restart.
        store.set_reads_fail(false);
        assert_eq!(stored_token(&restarted).as_deref(), Some("token-a"));
        assert_eq!(store.reads(), 2);
        // A successful read stays in the cache.
        assert_eq!(stored_token(&restarted).as_deref(), Some("token-a"));
        assert_eq!(store.reads(), 2);

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn the_retry_delay_grows_after_failed_reads_in_a_row() {
        let first = Duration::from_secs(60);
        assert_eq!(retry_delay_after(first, 1), first);
        assert_eq!(retry_delay_after(first, 2), first * 2);
        assert_eq!(retry_delay_after(first, 3), first * 4);
        assert_eq!(retry_delay_after(first, 5), first * 16);
        assert_eq!(retry_delay_after(first, 40), first * 16);
        assert_eq!(retry_delay_after(Duration::ZERO, 9), Duration::ZERO);
    }

    #[test]
    fn sign_in_keeps_an_item_that_could_not_be_read() {
        let path = test_db_path("token-keep-unread");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        let (_, old_account) = raw_token_columns(&path);
        let old_account = old_account.expect("the row should name a keychain account");
        drop(storage);

        // The read fails, so the app shows the sign-in screen. The user signs
        // in again while the keychain still does not give the old item.
        store.set_reads_fail(true);
        let restarted = open(&path, &store);
        assert_eq!(stored_token(&restarted), None);
        restarted
            .save_auth_session(&session(Some("token-b")))
            .expect("new session should save");
        let tokens = item_tokens(&store);
        assert_eq!(
            tokens.get(&old_account).map(String::as_str),
            Some("token-a")
        );
        assert_eq!(tokens.len(), 2);
        assert_eq!(stored_token(&restarted).as_deref(), Some("token-b"));
        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn sign_in_deletes_an_old_item_that_reads_again() {
        let path = test_db_path("token-delete-reread");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        drop(storage);

        store.set_reads_fail(true);
        let restarted = open(&path, &store);
        assert_eq!(stored_token(&restarted), None);
        // The keychain works again before the user signs in. The sign-in
        // reads the old item once more and then deletes it.
        store.set_reads_fail(false);
        restarted
            .save_auth_session(&session(Some("token-b")))
            .expect("new session should save");
        assert_eq!(
            item_tokens(&store).into_values().collect::<Vec<_>>(),
            vec!["token-b".to_string()]
        );
        assert_eq!(store.reads(), 2);

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn sign_out_reads_the_keychain_again_after_a_failed_read() {
        let path = test_db_path("token-sign-out-retry");
        let store = Arc::new(MemorySessionTokenStore::default());
        let storage = open(&path, &store);
        storage
            .save_auth_session(&session(Some("token-a")))
            .expect("session should save");
        drop(storage);

        // A short keychain failure at start reads as signed out.
        store.set_available(false);
        let restarted = open(&path, &store);
        assert_eq!(stored_token(&restarted), None);
        store.set_available(true);
        assert_eq!(stored_token(&restarted), None);
        assert_eq!(store.reads(), 1);

        // The sign-out gets the token, so it can end the server session.
        let signing_out = restarted
            .get_auth_session_for_sign_out()
            .expect("auth session should query")
            .expect("auth session should exist");
        assert_eq!(signing_out.token.as_deref(), Some("token-a"));
        assert_eq!(signing_out.server_url, SERVER);
        assert_eq!(store.reads(), 2);
        assert!(restarted
            .clear_auth_session_if_current(SERVER, Some("token-a"))
            .expect("sign-out should clear the session"));
        assert!(store.items().is_empty());

        // A token in the cache needs no new read.
        restarted
            .save_auth_session(&session(Some("token-b")))
            .expect("session should save");
        assert_eq!(
            restarted
                .get_auth_session_for_sign_out()
                .expect("auth session should query")
                .and_then(|session| session.token)
                .as_deref(),
            Some("token-b")
        );
        assert_eq!(store.reads(), 2);

        drop(restarted);
        remove_db(&path);
    }

    #[test]
    fn a_reader_without_keychain_writes_ignores_a_bare_token() {
        let store = Arc::new(MemorySessionTokenStore::default());
        store
            .write("halv-session-legacy", "bare-token")
            .expect("legacy item should write");
        store
            .write("halv-session-bound", &bound(SERVER, "token-a"))
            .expect("bound item should write");
        let vault = SessionTokenVault::with_retry_delay(store.clone(), Duration::ZERO);

        assert_eq!(vault.bound_token("halv-session-legacy"), None);
        assert_eq!(
            vault.bound_token("halv-session-bound"),
            Some(BoundToken {
                server_url: SERVER.to_string(),
                token: "token-a".to_string(),
            })
        );
        // The reader never writes the item.
        assert_eq!(
            store.items().get("halv-session-legacy").map(String::as_str),
            Some("bare-token")
        );
        // After the app binds the item, the reader reads it again.
        store
            .write("halv-session-legacy", &bound(SERVER, "bare-token"))
            .expect("bound item should write");
        assert_eq!(
            vault
                .bound_token("halv-session-legacy")
                .map(|bound| bound.token),
            Some("bare-token".to_string())
        );
    }
}
