//! The host identity in the keychain (docs/remote-protocol.md, section 4.1).
//!
//! The keychain item has the service `Halv` and the account `remote-identity`
//! (`remote-identity-dev` in debug builds). A read error is not a missing item:
//! after a read error, or when the stored text does not parse, the gateway stays off
//! and reports the error. It never makes a new identity in that case, because a new
//! identity cuts off every paired device.

use std::sync::Arc;

use halv_remote::keys::{KEYCHAIN_ACCOUNT, KEYCHAIN_ACCOUNT_DEV, KEYCHAIN_SERVICE};
use halv_remote::HostIdentity;
use zeroize::Zeroizing;

type ReadSecret = dyn Fn(&str, &str) -> Result<Option<String>, String> + Send + Sync;
type WriteSecret = dyn Fn(&str, &str, &str) -> Result<(), String> + Send + Sync;

/// The keychain functions of the gateway. Tests inject a fake keychain.
#[derive(Clone)]
pub(crate) struct Keychain {
    read: Arc<ReadSecret>,
    write: Arc<WriteSecret>,
}

impl Keychain {
    /// Makes a keychain from a read function and a write function.
    /// Each function gets the service and the account. A missing item reads as `Ok(None)`.
    pub(crate) fn new(
        read: impl Fn(&str, &str) -> Result<Option<String>, String> + Send + Sync + 'static,
        write: impl Fn(&str, &str, &str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Keychain {
        Keychain {
            read: Arc::new(read),
            write: Arc::new(write),
        }
    }

    /// Returns the system keychain.
    pub(crate) fn system() -> Keychain {
        Keychain::new(read_system_secret, write_system_secret)
    }
}

fn read_system_secret(service: &str, account: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(service, account)
        .map_err(|error| format!("Halv could not open the keychain: {error}"))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("Halv could not read the keychain: {error}")),
    }
}

fn write_system_secret(service: &str, account: &str, secret: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(service, account)
        .map_err(|error| format!("Halv could not open the keychain: {error}"))?;
    entry
        .set_password(secret)
        .map_err(|error| format!("Halv could not write to the keychain: {error}"))
}

/// Returns the keychain account of this build.
pub(crate) fn keychain_account() -> &'static str {
    if cfg!(debug_assertions) {
        KEYCHAIN_ACCOUNT_DEV
    } else {
        KEYCHAIN_ACCOUNT
    }
}

/// Reads the host identity. A missing item reads as `Ok(None)`.
///
/// A read error and an item that does not parse both fail.
pub(crate) fn load(keychain: &Keychain) -> Result<Option<HostIdentity>, String> {
    let stored = (keychain.read)(KEYCHAIN_SERVICE, keychain_account())?.map(Zeroizing::new);
    stored
        .map(|json| {
            HostIdentity::from_keychain_json(&json).map_err(|_| {
                "The remote identity in the keychain is not valid. Reset the identity in Settings to make a new one."
                    .to_string()
            })
        })
        .transpose()
}

/// Reads the host identity, or makes and stores a new one when the item is missing.
///
/// A read error and an item that does not parse both fail. They never make a new identity.
pub(crate) fn load_or_create(keychain: &Keychain) -> Result<HostIdentity, String> {
    if let Some(identity) = load(keychain)? {
        return Ok(identity);
    }
    let identity = HostIdentity::generate()
        .map_err(|_| "Halv could not make a remote identity.".to_string())?;
    store(keychain, &identity)?;
    Ok(identity)
}

/// Writes the identity to the keychain. It replaces the stored item.
pub(crate) fn store(keychain: &Keychain, identity: &HostIdentity) -> Result<(), String> {
    let json = identity.to_keychain_json();
    (keychain.write)(KEYCHAIN_SERVICE, keychain_account(), &json)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// An in-memory keychain. It never touches the system keychain.
    #[derive(Default)]
    pub(crate) struct FakeKeychain {
        pub(crate) items: Mutex<HashMap<(String, String), String>>,
        pub(crate) read_error: Mutex<Option<String>>,
        pub(crate) write_error: Mutex<Option<String>>,
        pub(crate) writes: Mutex<usize>,
    }

    impl FakeKeychain {
        pub(crate) fn keychain(self: &Arc<Self>) -> Keychain {
            let reader = self.clone();
            let writer = self.clone();
            Keychain::new(
                move |service, account| {
                    if let Some(error) = reader.read_error.lock().unwrap().clone() {
                        return Err(error);
                    }
                    Ok(reader
                        .items
                        .lock()
                        .unwrap()
                        .get(&(service.to_string(), account.to_string()))
                        .cloned())
                },
                move |service, account, secret| {
                    if let Some(error) = writer.write_error.lock().unwrap().clone() {
                        return Err(error);
                    }
                    *writer.writes.lock().unwrap() += 1;
                    writer.items.lock().unwrap().insert(
                        (service.to_string(), account.to_string()),
                        secret.to_string(),
                    );
                    Ok(())
                },
            )
        }

        pub(crate) fn stored(&self) -> Option<String> {
            self.items
                .lock()
                .unwrap()
                .get(&(KEYCHAIN_SERVICE.to_string(), keychain_account().to_string()))
                .cloned()
        }
    }

    #[test]
    fn the_account_follows_the_build_profile() {
        let expected = if cfg!(debug_assertions) {
            "remote-identity-dev"
        } else {
            "remote-identity"
        };
        assert_eq!(keychain_account(), expected);
        assert_eq!(KEYCHAIN_SERVICE, "Halv");
    }

    #[test]
    fn a_missing_item_makes_and_stores_a_new_identity() {
        let fake = Arc::new(FakeKeychain::default());
        let keychain = fake.keychain();
        let created = load_or_create(&keychain).expect("a new identity");
        assert_eq!(*fake.writes.lock().unwrap(), 1);
        let stored = fake.stored().expect("the keychain holds the identity");
        let reloaded = HostIdentity::from_keychain_json(&stored).unwrap();
        assert_eq!(reloaded.host_id(), created.host_id());

        let loaded = load_or_create(&keychain).expect("the stored identity");
        assert_eq!(loaded.host_id(), created.host_id());
        assert_eq!(loaded.noise_public(), created.noise_public());
        assert_eq!(*fake.writes.lock().unwrap(), 1, "a load does not write");
    }

    #[test]
    fn a_read_error_never_makes_a_new_identity() {
        let fake = Arc::new(FakeKeychain::default());
        *fake.read_error.lock().unwrap() = Some("the keychain is locked".to_string());
        let error = load_or_create(&fake.keychain()).unwrap_err();
        assert!(error.contains("locked"));
        assert_eq!(*fake.writes.lock().unwrap(), 0);
        assert!(fake.stored().is_none());
    }

    #[test]
    fn a_stored_item_that_does_not_parse_is_kept() {
        let fake = Arc::new(FakeKeychain::default());
        fake.items.lock().unwrap().insert(
            (KEYCHAIN_SERVICE.to_string(), keychain_account().to_string()),
            "{\"v\":2}".to_string(),
        );
        let error = load_or_create(&fake.keychain()).unwrap_err();
        assert!(error.contains("not valid"));
        assert_eq!(*fake.writes.lock().unwrap(), 0);
        assert_eq!(fake.stored().as_deref(), Some("{\"v\":2}"));
    }

    #[test]
    fn load_never_makes_an_identity() {
        let fake = Arc::new(FakeKeychain::default());
        let keychain = fake.keychain();
        assert!(load(&keychain).unwrap().is_none());
        assert_eq!(*fake.writes.lock().unwrap(), 0);
        let created = load_or_create(&keychain).unwrap();
        assert_eq!(
            load(&keychain).unwrap().map(|identity| identity.host_id()),
            Some(created.host_id())
        );
        *fake.read_error.lock().unwrap() = Some("the keychain is locked".to_string());
        assert!(load(&keychain).is_err());
    }

    #[test]
    fn a_write_error_fails_the_new_identity() {
        let fake = Arc::new(FakeKeychain::default());
        *fake.write_error.lock().unwrap() = Some("write denied".to_string());
        assert_eq!(
            load_or_create(&fake.keychain()).unwrap_err(),
            "write denied"
        );
    }
}
