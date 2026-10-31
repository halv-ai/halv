//! The system keychain with a sealed-file path for large secrets.
//!
//! The Windows credential manager rejects a password above 2560 bytes of
//! UTF-16 (`CRED_MAX_CREDENTIAL_BLOB_SIZE`). A ChatGPT token set is about
//! 7 KB. On Windows, the store keeps a random key in the keychain entry and
//! writes the secret, sealed with that key, to a file in the app data
//! directory. macOS and Linux keychains accept large secrets, so they keep
//! the plain entry.
//!
//! The keychain entry holds one of two values:
//! - the plain secret, when it fits in the credential blob;
//! - a key envelope `halv-sealed-file:v1:<base64 key>`, when the secret lives
//!   in the sealed file.
//!
//! The sealed file holds a magic header, a random nonce and the ChaCha20-
//! Poly1305 ciphertext of the UTF-8 secret.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use zeroize::Zeroizing;

#[cfg(target_os = "windows")]
use crate::app_data;

/// The Windows credential blob limit in bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const WINDOWS_MAX_BLOB_BYTES: usize = 2560;
/// The sub-directory of the app data directory that holds the sealed files.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const SECRETS_DIR_NAME: &str = "secrets";

const KEY_ENTRY_PREFIX: &str = "halv-sealed-file:v1:";
const FILE_MAGIC: &[u8; 8] = b"HALVSF01";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
/// The largest sealed file the store reads. A larger file is corrupt.
const MAX_FILE_LEN: u64 = 1024 * 1024;

type SealKey = Zeroizing<[u8; KEY_LEN]>;

/// The raw keychain operations. Tests inject a fake.
pub(crate) trait Keychain {
    /// Reads one entry. A missing entry reads as `Ok(None)`.
    fn read(&mut self, service: &str, account: &str) -> Result<Option<String>, String>;
    /// Writes one entry. It replaces an existing entry.
    fn write(&mut self, service: &str, account: &str, secret: &str) -> Result<(), String>;
    /// Deletes one entry. A missing entry is not an error.
    fn delete(&mut self, service: &str, account: &str) -> Result<(), String>;
}

/// The system keychain through the `keyring` crate.
pub(crate) struct SystemKeychain;

impl SystemKeychain {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(service, account)
            .map_err(|error| format!("Failed to open keychain entry {service}/{account}: {error}"))
    }
}

impl Keychain for SystemKeychain {
    fn read(&mut self, service: &str, account: &str) -> Result<Option<String>, String> {
        match Self::entry(service, account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!(
                "Failed to read keychain entry {service}/{account}: {error}"
            )),
        }
    }

    fn write(&mut self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        Self::entry(service, account)?
            .set_password(secret)
            .map_err(|error| format!("Failed to write keychain entry {service}/{account}: {error}"))
    }

    fn delete(&mut self, service: &str, account: &str) -> Result<(), String> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!(
                "Failed to delete keychain entry {service}/{account}: {error}"
            )),
        }
    }
}

/// Reads a secret from the system keychain.
pub(crate) fn read_secret(service: &str, account: &str) -> Result<Option<String>, String> {
    match sealed_file_store() {
        Some(store) => store.read(&mut SystemKeychain, service, account),
        None => SystemKeychain.read(service, account),
    }
}

/// Writes a secret to the system keychain. A large secret goes to a sealed
/// file on Windows.
pub(crate) fn write_secret(service: &str, account: &str, secret: &str) -> Result<(), String> {
    match sealed_file_store() {
        Some(store) => store.write(&mut SystemKeychain, service, account, secret),
        None => SystemKeychain.write(service, account, secret),
    }
}

/// Deletes a secret from the system keychain and removes its sealed file.
pub(crate) fn delete_secret(service: &str, account: &str) -> Result<(), String> {
    match sealed_file_store() {
        Some(store) => store.delete(&mut SystemKeychain, service, account),
        None => SystemKeychain.delete(service, account),
    }
}

/// Returns the sealed-file store of this platform, or `None` when the
/// platform keychain accepts large secrets.
#[cfg(target_os = "windows")]
fn sealed_file_store() -> Option<SealedFileStore> {
    let Some(app_data_dir) = app_data::cached_app_data_dir() else {
        eprintln!("secret store: the app data directory is not resolved yet; using the plain keychain entry");
        return None;
    };
    Some(SealedFileStore::new(
        app_data_dir.join(SECRETS_DIR_NAME),
        WINDOWS_MAX_BLOB_BYTES,
    ))
}

#[cfg(not(target_os = "windows"))]
fn sealed_file_store() -> Option<SealedFileStore> {
    None
}

/// Routes a secret to the keychain, or to a sealed file when the secret
/// exceeds the credential blob limit.
pub(crate) struct SealedFileStore {
    dir: PathBuf,
    max_blob_bytes: usize,
}

impl SealedFileStore {
    /// Makes a store that writes sealed files to `dir`. A secret whose
    /// UTF-16 encoding exceeds `max_blob_bytes` goes to a sealed file.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn new(dir: PathBuf, max_blob_bytes: usize) -> Self {
        Self {
            dir,
            max_blob_bytes,
        }
    }

    /// Reads a secret. A missing entry, or an envelope without its file,
    /// reads as `Ok(None)`.
    pub(crate) fn read(
        &self,
        keychain: &mut dyn Keychain,
        service: &str,
        account: &str,
    ) -> Result<Option<String>, String> {
        let Some(entry) = keychain.read(service, account)? else {
            return Ok(None);
        };
        let Some(key) = decode_key_entry(&entry) else {
            return Ok(Some(entry));
        };
        let path = self.path(service, account);
        let Some(sealed) = read_file(&path)? else {
            eprintln!(
                "secret store: keychain entry {service}/{account} points to a missing sealed file {}",
                path.display()
            );
            return Ok(None);
        };
        let plaintext = open(&key, &sealed).map_err(|error| {
            format!(
                "Failed to open the sealed secret file {}: {error}",
                path.display()
            )
        })?;
        String::from_utf8(plaintext).map(Some).map_err(|_| {
            format!(
                "The sealed secret file {} does not hold UTF-8 text",
                path.display()
            )
        })
    }

    /// Writes a secret. It replaces the keychain entry and the sealed file.
    pub(crate) fn write(
        &self,
        keychain: &mut dyn Keychain,
        service: &str,
        account: &str,
        secret: &str,
    ) -> Result<(), String> {
        let path = self.path(service, account);
        if fits_credential_blob(secret, self.max_blob_bytes) {
            keychain.write(service, account, secret)?;
            return remove_file(&path);
        }

        let existing_key = keychain
            .read(service, account)?
            .as_deref()
            .and_then(decode_key_entry);
        let (key, key_is_new) = match existing_key {
            Some(key) => (key, false),
            None => (generate_key(), true),
        };
        let sealed = seal(&key, secret.as_bytes())?;
        write_file(&self.dir, &path, &sealed)?;
        if key_is_new {
            keychain.write(service, account, &encode_key_entry(&key))?;
        }
        Ok(())
    }

    /// Deletes the keychain entry and the sealed file. Both may be absent.
    pub(crate) fn delete(
        &self,
        keychain: &mut dyn Keychain,
        service: &str,
        account: &str,
    ) -> Result<(), String> {
        let entry = keychain.delete(service, account);
        let file = remove_file(&self.path(service, account));
        entry.and(file)
    }

    /// Returns the sealed file of one keychain entry.
    pub(crate) fn path(&self, service: &str, account: &str) -> PathBuf {
        self.dir.join(file_name(service, account))
    }
}

/// Returns true when the Windows credential manager accepts the secret.
/// The manager stores the password as UTF-16.
pub(crate) fn fits_credential_blob(secret: &str, max_blob_bytes: usize) -> bool {
    secret.encode_utf16().count() * 2 <= max_blob_bytes
}

/// Returns the sealed file name of one keychain entry. Characters outside
/// `[A-Za-z0-9._-]` become `_`.
fn file_name(service: &str, account: &str) -> String {
    let sanitize = |text: &str| -> String {
        text.chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("{}-{}.bin", sanitize(service), sanitize(account))
}

fn generate_key() -> SealKey {
    Zeroizing::new(rand::random::<[u8; KEY_LEN]>())
}

fn encode_key_entry(key: &SealKey) -> String {
    format!("{KEY_ENTRY_PREFIX}{}", STANDARD.encode(key.as_slice()))
}

/// Decodes a key envelope. Any other entry value decodes as `None`.
fn decode_key_entry(entry: &str) -> Option<SealKey> {
    let encoded = entry.strip_prefix(KEY_ENTRY_PREFIX)?;
    let decoded = Zeroizing::new(STANDARD.decode(encoded).ok()?);
    let key: [u8; KEY_LEN] = decoded.as_slice().try_into().ok()?;
    Some(Zeroizing::new(key))
}

/// Seals a secret: `FILE_MAGIC || nonce || ciphertext`.
pub(crate) fn seal(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce_bytes = rand::random::<[u8; NONCE_LEN]>();
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), plaintext)
        .map_err(|_| "Failed to seal the secret".to_string())?;
    let mut sealed = Vec::with_capacity(FILE_MAGIC.len() + NONCE_LEN + ciphertext.len());
    sealed.extend_from_slice(FILE_MAGIC);
    sealed.extend_from_slice(&nonce_bytes);
    sealed.extend_from_slice(&ciphertext);
    Ok(sealed)
}

/// Opens a sealed secret. A wrong key, a changed byte or a bad header fails.
pub(crate) fn open(key: &[u8; KEY_LEN], sealed: &[u8]) -> Result<Vec<u8>, String> {
    let header_len = FILE_MAGIC.len() + NONCE_LEN;
    if sealed.len() < header_len || &sealed[..FILE_MAGIC.len()] != FILE_MAGIC {
        return Err("The sealed secret has an unknown format".to_string());
    }
    let nonce = Nonce::try_from(&sealed[FILE_MAGIC.len()..header_len])
        .map_err(|_| "The sealed secret has an unknown format".to_string())?;
    let cipher = ChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(&nonce, &sealed[header_len..])
        .map_err(|_| "The sealed secret does not match its key".to_string())
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let describe = |error: io::Error| format!("Failed to read {}: {error}", path.display());
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(describe(error)),
    };
    if metadata.len() > MAX_FILE_LEN {
        return Err(format!(
            "The sealed secret file {} is larger than {MAX_FILE_LEN} bytes",
            path.display()
        ));
    }
    fs::read(path).map(Some).map_err(describe)
}

/// Writes the sealed bytes through a temporary file, then renames it over
/// the final path.
fn write_file(dir: &Path, path: &Path, sealed: &[u8]) -> Result<(), String> {
    crate::app_data::create_private_dir(dir).map_err(|error| {
        format!(
            "Failed to create the secrets directory {}: {error}",
            dir.display()
        )
    })?;
    let temporary = path.with_extension("tmp");
    let write = || -> io::Result<()> {
        let mut file = crate::app_data::private_file_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        io::Write::write_all(&mut file, sealed)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    };
    write().map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "Failed to write the sealed secret file {}: {error}",
            path.display()
        )
    })
}

fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to remove the sealed secret file {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeKeychain {
        entries: HashMap<(String, String), String>,
        max_blob_bytes: Option<usize>,
    }

    impl FakeKeychain {
        fn windows() -> Self {
            Self {
                entries: HashMap::new(),
                max_blob_bytes: Some(WINDOWS_MAX_BLOB_BYTES),
            }
        }

        fn entry(&self, service: &str, account: &str) -> Option<&String> {
            self.entries
                .get(&(service.to_string(), account.to_string()))
        }
    }

    impl Keychain for FakeKeychain {
        fn read(&mut self, service: &str, account: &str) -> Result<Option<String>, String> {
            Ok(self.entry(service, account).cloned())
        }

        fn write(&mut self, service: &str, account: &str, secret: &str) -> Result<(), String> {
            if let Some(limit) = self.max_blob_bytes {
                if !fits_credential_blob(secret, limit) {
                    return Err(format!(
                        "password encoded as UTF-16 is longer than platform limit of {limit} bytes"
                    ));
                }
            }
            self.entries.insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn delete(&mut self, service: &str, account: &str) -> Result<(), String> {
            self.entries
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-secret-store-{label}-{}",
                rand::random::<u64>()
            ));
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn large_secret() -> String {
        "x".repeat(4 * 1024)
    }

    #[test]
    fn seal_and_open_round_trip_a_4kb_secret() {
        let key = rand::random::<[u8; KEY_LEN]>();
        let secret = large_secret();

        let sealed = seal(&key, secret.as_bytes()).expect("seal should succeed");
        assert_ne!(&sealed[FILE_MAGIC.len() + NONCE_LEN..], secret.as_bytes());

        let opened = open(&key, &sealed).expect("open should succeed");
        assert_eq!(opened, secret.as_bytes());
    }

    // The blob was sealed by `seal` with chacha20poly1305 0.10.1, the version
    // before the bump, with key 00..1f and nonce a0..ab. Sealed files that
    // Windows users already have on disk must keep opening.
    #[test]
    fn open_reads_a_file_sealed_by_chacha20poly1305_0_10() {
        const SEALED_HEX: &str = "48414c5653463031a0a1a2a3a4a5a6a7a8a9aaab64ca14296d95a7cc\
                                  cc6a97348f9f9e89f82af3c972ef3e6363765a749b6cf2b6e6a5b68ff0";
        let sealed: Vec<u8> = (0..SEALED_HEX.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&SEALED_HEX[index..index + 2], 16).unwrap())
            .collect();
        let key: [u8; KEY_LEN] = std::array::from_fn(|index| index as u8);

        let opened = open(&key, &sealed).expect("open should read the old format");
        assert_eq!(opened, b"halv sealed secret v1");

        let mut wrong_key = key;
        wrong_key[31] ^= 0x01;
        assert!(open(&wrong_key, &sealed).is_err());
    }

    #[test]
    fn open_rejects_a_wrong_key_and_a_changed_byte() {
        let key = rand::random::<[u8; KEY_LEN]>();
        let sealed = seal(&key, b"token-json").expect("seal should succeed");

        let mut wrong_key = key;
        wrong_key[0] ^= 0x01;
        assert!(open(&wrong_key, &sealed).is_err());

        let mut changed = sealed.clone();
        let last = changed.len() - 1;
        changed[last] ^= 0x01;
        assert!(open(&key, &changed).is_err());

        assert!(open(&key, b"HALVSF01").is_err());
        assert!(open(&key, b"not a sealed file at all").is_err());
    }

    #[test]
    fn a_large_secret_goes_to_a_sealed_file_and_reads_back() {
        let temp = TempDir::new("large");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();
        let secret = large_secret();

        store
            .write(&mut keychain, "Halv", "chatgpt-app-tokens", &secret)
            .expect("write should succeed");

        let entry = keychain
            .entry("Halv", "chatgpt-app-tokens")
            .expect("keychain entry should exist");
        assert!(entry.starts_with(KEY_ENTRY_PREFIX));
        assert!(fits_credential_blob(entry, WINDOWS_MAX_BLOB_BYTES));
        let path = store.path("Halv", "chatgpt-app-tokens");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Halv-chatgpt-app-tokens.bin")
        );
        let on_disk = fs::read(&path).expect("sealed file should exist");
        assert!(!on_disk
            .windows(16)
            .any(|window| window == &secret.as_bytes()[..16]));

        let read = store
            .read(&mut keychain, "Halv", "chatgpt-app-tokens")
            .expect("read should succeed");
        assert_eq!(read.as_deref(), Some(secret.as_str()));
    }

    #[test]
    fn a_rewrite_keeps_the_key_and_replaces_the_file() {
        let temp = TempDir::new("rewrite");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();

        store
            .write(&mut keychain, "Halv", "tokens", &large_secret())
            .expect("first write should succeed");
        let first_entry = keychain.entry("Halv", "tokens").cloned();
        let refreshed = format!("{}-refreshed", large_secret());
        store
            .write(&mut keychain, "Halv", "tokens", &refreshed)
            .expect("second write should succeed");

        assert_eq!(keychain.entry("Halv", "tokens").cloned(), first_entry);
        let read = store
            .read(&mut keychain, "Halv", "tokens")
            .expect("read should succeed");
        assert_eq!(read.as_deref(), Some(refreshed.as_str()));
    }

    #[test]
    fn a_small_secret_stays_in_the_keychain_and_drops_a_stale_file() {
        let temp = TempDir::new("small");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();

        store
            .write(&mut keychain, "Halv", "openai-api-key", &large_secret())
            .expect("large write should succeed");
        assert!(store.path("Halv", "openai-api-key").exists());

        store
            .write(&mut keychain, "Halv", "openai-api-key", "sk-short")
            .expect("small write should succeed");

        assert_eq!(
            keychain.entry("Halv", "openai-api-key").map(String::as_str),
            Some("sk-short")
        );
        assert!(!store.path("Halv", "openai-api-key").exists());
        let read = store
            .read(&mut keychain, "Halv", "openai-api-key")
            .expect("read should succeed");
        assert_eq!(read.as_deref(), Some("sk-short"));
    }

    #[test]
    fn a_plain_legacy_entry_reads_as_the_secret() {
        let temp = TempDir::new("legacy");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();
        keychain
            .write("Halv", "chatgpt-app-tokens", "{\"legacy\":true}")
            .expect("plain write should succeed");

        let read = store
            .read(&mut keychain, "Halv", "chatgpt-app-tokens")
            .expect("read should succeed");
        assert_eq!(read.as_deref(), Some("{\"legacy\":true}"));
        assert_eq!(
            store
                .read(&mut keychain, "Halv", "missing")
                .expect("missing read should succeed"),
            None
        );
    }

    #[test]
    fn an_envelope_without_its_file_reads_as_absent() {
        let temp = TempDir::new("orphan");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();
        keychain
            .write("Halv", "tokens", &encode_key_entry(&generate_key()))
            .expect("envelope write should succeed");

        let read = store
            .read(&mut keychain, "Halv", "tokens")
            .expect("read should succeed");
        assert_eq!(read, None);
    }

    #[test]
    fn a_file_sealed_with_another_key_fails_to_read() {
        let temp = TempDir::new("wrong-key");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();
        store
            .write(&mut keychain, "Halv", "tokens", &large_secret())
            .expect("write should succeed");
        keychain
            .write("Halv", "tokens", &encode_key_entry(&generate_key()))
            .expect("envelope write should succeed");

        let error = store
            .read(&mut keychain, "Halv", "tokens")
            .expect_err("a wrong key should fail");
        assert!(error.contains("does not match its key"), "{error}");
    }

    #[test]
    fn delete_removes_the_entry_and_the_file() {
        let temp = TempDir::new("delete");
        let store = SealedFileStore::new(temp.0.join(SECRETS_DIR_NAME), WINDOWS_MAX_BLOB_BYTES);
        let mut keychain = FakeKeychain::windows();
        store
            .write(&mut keychain, "Halv", "tokens", &large_secret())
            .expect("write should succeed");

        store
            .delete(&mut keychain, "Halv", "tokens")
            .expect("delete should succeed");

        assert!(keychain.entry("Halv", "tokens").is_none());
        assert!(!store.path("Halv", "tokens").exists());
        store
            .delete(&mut keychain, "Halv", "tokens")
            .expect("a second delete should succeed");
    }

    #[test]
    fn the_blob_check_counts_utf16_bytes() {
        assert!(fits_credential_blob(
            &"a".repeat(1280),
            WINDOWS_MAX_BLOB_BYTES
        ));
        assert!(!fits_credential_blob(
            &"a".repeat(1281),
            WINDOWS_MAX_BLOB_BYTES
        ));
        assert!(!fits_credential_blob(
            &"é".repeat(1281),
            WINDOWS_MAX_BLOB_BYTES
        ));
        assert!(!fits_credential_blob(
            &"😀".repeat(641),
            WINDOWS_MAX_BLOB_BYTES
        ));
    }

    #[test]
    fn file_names_keep_only_safe_characters() {
        assert_eq!(
            file_name("Halv", "mcp-oauth-my server/1"),
            "Halv-mcp-oauth-my_server_1.bin"
        );
        assert_eq!(
            file_name("TokenSaver", "chatgpt-app-tokens"),
            "TokenSaver-chatgpt-app-tokens.bin"
        );
    }
}
