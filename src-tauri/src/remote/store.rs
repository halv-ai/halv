//! The device registry: the SQLite table `remote_devices` (docs/remote-protocol.md, section 9).
//!
//! The storage migration `0053_remote_devices` creates the table. The store opens its own
//! connection to the app database on first use, so the gateway never blocks other storage calls.
//! A revoke keeps the row and sets `revoked_at`. The host registry decides revocation.
//!
//! Each row has the host id of the identity that paired the device. [`HostDevices`] is the
//! registry of one host id, and each of its queries filters by that host id. Debug and release
//! builds share the database but use different identities, so they never share devices.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use halv_remote::host::{DeviceStore, NewDevice, StoreError, StoredDevice};
use halv_remote::rpc::Platform;
use halv_remote::{DeviceId, TokenHash};
use rusqlite::{params, Connection, OptionalExtension, Row};

const DEVICE_COLUMNS: &str =
    "id, name, platform, model, noise_public, token_hash, created_at, last_seen_at, revoked_at";

/// The device registry in the app database.
pub(crate) struct SqliteDeviceStore {
    path: PathBuf,
    connection: Mutex<Option<Connection>>,
}

impl SqliteDeviceStore {
    /// Makes a store for the database at `path`. The connection opens on first use.
    pub(crate) fn new(path: impl Into<PathBuf>) -> SqliteDeviceStore {
        SqliteDeviceStore {
            path: path.into(),
            connection: Mutex::new(None),
        }
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if connection.is_none() {
            let opened = Connection::open(&self.path).map_err(store_error)?;
            opened
                .busy_timeout(Duration::from_secs(5))
                .map_err(store_error)?;
            *connection = Some(opened);
        }
        operation(connection.as_ref().expect("the connection is open")).map_err(store_error)
    }

    /// Returns the registry of one host identity.
    pub(crate) fn for_host(self: &Arc<Self>, host_id: impl Into<String>) -> HostDevices {
        HostDevices {
            database: self.clone(),
            host_id: host_id.into(),
        }
    }
}

/// The devices of one host identity. Each query filters by the host id.
#[derive(Clone)]
pub(crate) struct HostDevices {
    database: Arc<SqliteDeviceStore>,
    host_id: String,
}

impl HostDevices {
    /// Returns the devices that are not revoked, newest first.
    pub(crate) fn active(&self) -> Result<Vec<StoredDevice>, StoreError> {
        let mut devices = self.list()?;
        devices.retain(|device| !device.is_revoked());
        devices.sort_by_key(|device| std::cmp::Reverse(device.created_at_ms));
        Ok(devices)
    }

    /// Revokes every device of this host. Returns the number of devices that it revoked.
    pub(crate) fn revoke_all(&self, at_ms: u64) -> Result<usize, StoreError> {
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE remote_devices SET revoked_at = ?2
                 WHERE host_id = ?1 AND revoked_at IS NULL",
                params![self.host_id, to_sql_time(at_ms)],
            )
        })
    }
}

fn store_error(error: rusqlite::Error) -> StoreError {
    StoreError(format!("the device registry failed: {error}"))
}

fn invalid_row(column: usize, message: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::<dyn std::error::Error + Send + Sync>::from(message),
    )
}

fn to_sql_time(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn from_sql_time(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn device_from_row(row: &Row<'_>) -> rusqlite::Result<StoredDevice> {
    let id: String = row.get(0)?;
    let noise_public: Vec<u8> = row.get(4)?;
    let token_hash: String = row.get(5)?;
    Ok(StoredDevice {
        id: DeviceId::parse(&id).map_err(|_| invalid_row(0, "invalid device id"))?,
        name: row.get(1)?,
        platform: Platform::from(row.get::<_, String>(2)?),
        model: row.get(3)?,
        noise_public: noise_public
            .try_into()
            .map_err(|_| invalid_row(4, "invalid device key"))?,
        token_hash: TokenHash::parse(&token_hash)
            .map_err(|_| invalid_row(5, "invalid token hash"))?,
        created_at_ms: from_sql_time(row.get(6)?),
        last_seen_at_ms: row.get::<_, Option<i64>>(7)?.map(from_sql_time),
        revoked_at_ms: row.get::<_, Option<i64>>(8)?.map(from_sql_time),
    })
}

impl DeviceStore for HostDevices {
    fn list(&self) -> Result<Vec<StoredDevice>, StoreError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {DEVICE_COLUMNS} FROM remote_devices
                 WHERE host_id = ?1 ORDER BY created_at, id"
            ))?;
            let rows = statement.query_map(params![self.host_id], device_from_row)?;
            rows.collect()
        })
    }

    fn find_by_noise_key(&self, key: &[u8; 32]) -> Result<Option<StoredDevice>, StoreError> {
        self.database.with_connection(|connection| {
            connection
                .query_row(
                    &format!(
                        "SELECT {DEVICE_COLUMNS} FROM remote_devices
                         WHERE host_id = ?1 AND noise_public = ?2"
                    ),
                    params![self.host_id, key.as_slice()],
                    device_from_row,
                )
                .optional()
        })
    }

    fn insert(&self, device: NewDevice) -> Result<StoredDevice, StoreError> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO remote_devices
                    (id, host_id, name, platform, model, noise_public, token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    device.id.as_str(),
                    self.host_id,
                    device.name,
                    device.platform.as_str(),
                    device.model,
                    device.noise_public.as_slice(),
                    device.token_hash.as_str(),
                    to_sql_time(device.created_at_ms),
                ],
            )
        })?;
        Ok(StoredDevice {
            id: device.id,
            name: device.name,
            platform: device.platform,
            model: device.model,
            noise_public: device.noise_public,
            token_hash: device.token_hash,
            created_at_ms: device.created_at_ms,
            last_seen_at_ms: None,
            revoked_at_ms: None,
        })
    }

    fn revoke(&self, id: &DeviceId, at_ms: u64) -> Result<bool, StoreError> {
        self.database.with_connection(|connection| {
            // COALESCE keeps the time of the first revoke.
            connection
                .execute(
                    "UPDATE remote_devices SET revoked_at = COALESCE(revoked_at, ?3)
                     WHERE host_id = ?1 AND id = ?2",
                    params![self.host_id, id.as_str(), to_sql_time(at_ms)],
                )
                .map(|changed| changed > 0)
        })
    }

    fn touch_last_seen(&self, id: &DeviceId, at_ms: u64) -> Result<(), StoreError> {
        self.database.with_connection(|connection| {
            connection
                .execute(
                    "UPDATE remote_devices SET last_seen_at = ?3 WHERE host_id = ?1 AND id = ?2",
                    params![self.host_id, id.as_str(), to_sql_time(at_ms)],
                )
                .map(|_| ())
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::storage::Storage;

    /// A temporary app database with every storage migration.
    pub(crate) struct TestDatabase {
        directory: PathBuf,
        pub(crate) path: PathBuf,
        _storage: Storage,
    }

    impl TestDatabase {
        pub(crate) fn new(label: &str) -> TestDatabase {
            let directory =
                std::env::temp_dir().join(format!("halv-remote-{label}-{}", rand::random::<u64>()));
            std::fs::create_dir_all(&directory).expect("create the test directory");
            let path = directory.join("halv.sqlite3");
            let storage = Storage::open(&path).expect("open the test storage");
            TestDatabase {
                directory,
                path,
                _storage: storage,
            }
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    pub(crate) fn new_device(key: u8, created_at_ms: u64) -> NewDevice {
        NewDevice {
            id: DeviceId::generate().unwrap(),
            name: format!("Phone {key}"),
            platform: Platform::Ios,
            model: Some("iPhone".to_string()),
            noise_public: [key; 32],
            token_hash: TokenHash::of(&[key; 32]),
            created_at_ms,
        }
    }

    /// The host ids of the tests. Real host ids have 22 characters (section 4.2).
    pub(crate) const HOST_A: &str = "hostAAAAAAAAAAAAAAAAAA";
    const HOST_B: &str = "hostBBBBBBBBBBBBBBBBBB";

    fn table_columns(connection: &Connection) -> Vec<String> {
        connection
            .prepare("SELECT name FROM pragma_table_info('remote_devices') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn the_migration_creates_the_device_table() {
        let database = TestDatabase::new("migration");
        let connection = Connection::open(&database.path).unwrap();
        assert_eq!(
            table_columns(&connection),
            [
                "id",
                "host_id",
                "name",
                "platform",
                "model",
                "noise_public",
                "token_hash",
                "created_at",
                "last_seen_at",
                "revoked_at"
            ]
        );
        let applied: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = '0053_remote_devices'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1);
        // A second open runs the migrations again without an error.
        drop(Storage::open(&database.path).expect("open the storage again"));
    }

    #[test]
    fn the_migration_adds_the_host_id_to_a_development_table() {
        let database = TestDatabase::new("migration-dev");
        let connection = Connection::open(&database.path).unwrap();
        // The table of development builds before the host scope.
        connection
            .execute_batch(
                "DROP TABLE remote_devices;
                 CREATE TABLE remote_devices (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    platform TEXT NOT NULL,
                    model TEXT NULL,
                    noise_public BLOB NOT NULL UNIQUE,
                    token_hash TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    last_seen_at INTEGER NULL,
                    revoked_at INTEGER NULL
                 );",
            )
            .unwrap();
        let old = new_device(9, 1_000);
        connection
            .execute(
                "INSERT INTO remote_devices
                    (id, name, platform, noise_public, token_hash, created_at)
                 VALUES (?1, 'Old phone', 'ios', ?2, ?3, 1000)",
                params![
                    old.id.as_str(),
                    old.noise_public.as_slice(),
                    old.token_hash.as_str()
                ],
            )
            .unwrap();
        drop(connection);

        drop(Storage::open(&database.path).expect("open the storage again"));
        let connection = Connection::open(&database.path).unwrap();
        assert!(table_columns(&connection)
            .iter()
            .any(|name| name == "host_id"));
        // A row without a host id belongs to no host.
        let store = Arc::new(SqliteDeviceStore::new(&database.path)).for_host(HOST_A);
        assert!(store.list().unwrap().is_empty());
        store.insert(new_device(1, 2_000)).unwrap();
        assert_eq!(store.active().unwrap().len(), 1);
    }

    #[test]
    fn the_store_inserts_finds_touches_and_revokes_devices() {
        let database = TestDatabase::new("store");
        let store = Arc::new(SqliteDeviceStore::new(&database.path)).for_host(HOST_A);
        assert!(store.list().unwrap().is_empty());

        let first = store.insert(new_device(1, 1_000)).unwrap();
        let second = store.insert(new_device(2, 2_000)).unwrap();
        assert_eq!(store.list().unwrap(), vec![first.clone(), second.clone()]);
        assert_eq!(
            store.find_by_noise_key(&[2; 32]).unwrap(),
            Some(second.clone())
        );
        assert_eq!(store.find_by_noise_key(&[3; 32]).unwrap(), None);

        store.touch_last_seen(&first.id, 1_500).unwrap();
        assert_eq!(
            store
                .find_by_noise_key(&[1; 32])
                .unwrap()
                .unwrap()
                .last_seen_at_ms,
            Some(1_500)
        );

        assert!(store.revoke(&first.id, 3_000).unwrap());
        assert!(store.revoke(&first.id, 4_000).unwrap());
        let revoked = store.find_by_noise_key(&[1; 32]).unwrap().unwrap();
        assert_eq!(revoked.revoked_at_ms, Some(3_000), "the first time stays");
        assert!(!store.revoke(&DeviceId::generate().unwrap(), 5_000).unwrap());

        assert_eq!(store.list().unwrap().len(), 2, "list keeps revoked devices");
        let active = store.active().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, second.id);
    }

    #[test]
    fn each_query_sees_only_the_devices_of_its_host() {
        let database = TestDatabase::new("hosts");
        let database_store = Arc::new(SqliteDeviceStore::new(&database.path));
        let host_a = database_store.for_host(HOST_A);
        let host_b = database_store.for_host(HOST_B);
        let phone_a = host_a.insert(new_device(1, 1_000)).unwrap();
        let phone_b = host_b.insert(new_device(2, 2_000)).unwrap();

        assert_eq!(host_a.list().unwrap(), vec![phone_a.clone()]);
        assert_eq!(host_b.list().unwrap(), vec![phone_b.clone()]);
        assert_eq!(host_a.find_by_noise_key(&[2; 32]).unwrap(), None);
        assert_eq!(host_b.find_by_noise_key(&[1; 32]).unwrap(), None);

        // A revoke or a touch of another host changes nothing.
        assert!(!host_a.revoke(&phone_b.id, 3_000).unwrap());
        host_a.touch_last_seen(&phone_b.id, 3_000).unwrap();
        assert_eq!(host_b.active().unwrap(), vec![phone_b.clone()]);

        // A reset of host A revokes only the devices of host A.
        let second_a = host_a.insert(new_device(3, 4_000)).unwrap();
        assert_eq!(host_a.revoke_all(5_000).unwrap(), 2);
        assert!(host_a.active().unwrap().is_empty());
        assert_eq!(
            host_a
                .find_by_noise_key(&second_a.noise_public)
                .unwrap()
                .unwrap()
                .revoked_at_ms,
            Some(5_000)
        );
        assert_eq!(host_b.active().unwrap(), vec![phone_b]);
        assert_eq!(host_a.revoke_all(6_000).unwrap(), 0, "a revoke time stays");
    }

    #[test]
    fn a_device_key_is_unique_for_each_host() {
        let database = TestDatabase::new("unique");
        let database_store = Arc::new(SqliteDeviceStore::new(&database.path));
        let host_a = database_store.for_host(HOST_A);
        host_a.insert(new_device(7, 1_000)).unwrap();
        assert!(host_a.insert(new_device(7, 2_000)).is_err());
        // Each pairing makes a new device key (section 4.3), but the hosts stay apart.
        database_store
            .for_host(HOST_B)
            .insert(new_device(7, 3_000))
            .unwrap();

        let connection = Connection::open(&database.path).unwrap();
        let duplicate = connection.execute(
            "INSERT INTO remote_devices
                (id, host_id, name, platform, noise_public, token_hash, created_at)
             VALUES ('x', ?1, 'x', 'ios', ?2, 'h', 0)",
            params![HOST_A, [7_u8; 32].as_slice()],
        );
        assert_eq!(
            duplicate.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
    }
}
