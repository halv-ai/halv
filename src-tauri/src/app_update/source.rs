//! The release feed behind the update manager. The plugin implementation
//! talks to GitHub; the tests use a fake.

use futures_util::future::BoxFuture;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};

/// A release that is newer than the running app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseInfo {
    pub version: String,
    pub notes: Option<String>,
}

/// Receives the size of each downloaded chunk and the total size.
pub(crate) type ChunkCallback = Box<dyn FnMut(usize, Option<u64>) + Send>;

/// A failed download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadError {
    pub message: String,
    /// The archive does not match the release signature.
    pub signature: bool,
}

impl DownloadError {
    pub(crate) fn other(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            signature: false,
        }
    }
}

pub(crate) trait UpdateSource: Send + Sync {
    /// Asks the feed for a release that is newer than the running app.
    fn check(&self, timeout: Duration) -> BoxFuture<'static, Result<Option<ReleaseInfo>, String>>;

    /// Downloads the archive of a checked release. The result has a valid
    /// release signature.
    fn download(
        &self,
        version: &str,
        on_chunk: ChunkCallback,
    ) -> BoxFuture<'static, Result<Vec<u8>, DownloadError>>;

    /// Installs the archive. It blocks, so never call it on the main thread:
    /// the macOS admin path waits for the main thread.
    fn install(&self, version: &str, bytes: Vec<u8>) -> Result<(), String>;
}

/// The Tauri updater plugin. The plugin `Update` has no public constructor,
/// so the source keeps each checked release by version. A check that finds a
/// newer release must not break the install of the staged one.
pub(crate) struct PluginUpdateSource {
    app: AppHandle,
    updates: Arc<Mutex<HashMap<String, Update>>>,
}

impl PluginUpdateSource {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self {
            app,
            updates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn update_for(&self, version: &str) -> Result<Update, String> {
        self.updates
            .lock()
            .expect("update source lock poisoned")
            .get(version)
            .cloned()
            .ok_or_else(|| format!("Halv {version} was not found in the release feed"))
    }
}

impl UpdateSource for PluginUpdateSource {
    fn check(&self, timeout: Duration) -> BoxFuture<'static, Result<Option<ReleaseInfo>, String>> {
        let exit_app = self.app.clone();
        // The Windows installer ends the process with `std::process::exit`
        // right after this hook, so Halv runs its exit cleanup here.
        let updater = self
            .app
            .updater_builder()
            .timeout(timeout)
            .on_before_exit(move || {
                crate::run_exit_cleanup(&exit_app);
                exit_app.cleanup_before_exit();
            })
            .build();
        let updates = self.updates.clone();
        Box::pin(async move {
            let update = updater
                .map_err(|error| error.to_string())?
                .check()
                .await
                .map_err(|error| error.to_string())?;
            let release = update.as_ref().map(|update| ReleaseInfo {
                version: update.version.clone(),
                notes: update.body.clone(),
            });
            if let Some(update) = update {
                updates
                    .lock()
                    .expect("update source lock poisoned")
                    .insert(update.version.clone(), update);
            }
            Ok(release)
        })
    }

    fn download(
        &self,
        version: &str,
        on_chunk: ChunkCallback,
    ) -> BoxFuture<'static, Result<Vec<u8>, DownloadError>> {
        let update = self.update_for(version);
        Box::pin(async move {
            update
                .map_err(DownloadError::other)?
                .download(on_chunk, || {})
                .await
                .map_err(|error| DownloadError {
                    // The plugin checks the signature after the download.
                    // These errors come only from that check.
                    signature: matches!(
                        error,
                        tauri_plugin_updater::Error::Minisign(_)
                            | tauri_plugin_updater::Error::Base64(_)
                            | tauri_plugin_updater::Error::SignatureUtf8(_)
                    ),
                    message: error.to_string(),
                })
        })
    }

    fn install(&self, version: &str, bytes: Vec<u8>) -> Result<(), String> {
        self.update_for(version)?
            .install(bytes)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicUsize, Ordering},
    };

    pub(crate) enum FakeDownload {
        /// Sends each chunk after the delay, then returns all bytes.
        Chunks {
            chunks: Vec<Vec<u8>>,
            delay: Duration,
        },
        Fail(String),
        /// Fails the signature check.
        BadSignature(String),
        /// Never sends a chunk.
        Hang,
    }

    #[derive(Default)]
    pub(crate) struct FakeSource {
        pub checks: Mutex<VecDeque<Result<Option<ReleaseInfo>, String>>>,
        pub downloads: Mutex<VecDeque<FakeDownload>>,
        pub installs: Mutex<Vec<(String, Vec<u8>)>>,
        pub install_error: Mutex<Option<String>>,
        pub check_calls: AtomicUsize,
        pub download_calls: AtomicUsize,
        /// Downloads that returned or that the manager dropped.
        pub ended_downloads: Arc<AtomicUsize>,
    }

    /// Counts a download future that ends, also when it is dropped.
    struct EndGuard(Arc<AtomicUsize>);

    impl Drop for EndGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl FakeSource {
        pub(crate) fn push_check(&self, result: Result<Option<ReleaseInfo>, String>) {
            self.checks.lock().unwrap().push_back(result);
        }

        pub(crate) fn push_download(&self, download: FakeDownload) {
            self.downloads.lock().unwrap().push_back(download);
        }
    }

    pub(crate) fn release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: version.to_string(),
            notes: Some(format!("## What's new\n- Halv {version}")),
        }
    }

    impl UpdateSource for FakeSource {
        fn check(
            &self,
            _timeout: Duration,
        ) -> BoxFuture<'static, Result<Option<ReleaseInfo>, String>> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            let result = self.checks.lock().unwrap().pop_front().unwrap_or(Ok(None));
            Box::pin(async move { result })
        }

        fn download(
            &self,
            _version: &str,
            mut on_chunk: ChunkCallback,
        ) -> BoxFuture<'static, Result<Vec<u8>, DownloadError>> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            let download = self
                .downloads
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(FakeDownload::Fail("no fake download".to_string()));
            let guard = EndGuard(self.ended_downloads.clone());
            Box::pin(async move {
                let _guard = guard;
                match download {
                    FakeDownload::Chunks { chunks, delay } => {
                        let total = chunks.iter().map(Vec::len).sum::<usize>() as u64;
                        let mut bytes = Vec::new();
                        for chunk in chunks {
                            tokio::time::sleep(delay).await;
                            on_chunk(chunk.len(), Some(total));
                            bytes.extend(chunk);
                        }
                        Ok(bytes)
                    }
                    FakeDownload::Fail(message) => Err(DownloadError::other(message)),
                    FakeDownload::BadSignature(message) => Err(DownloadError {
                        message,
                        signature: true,
                    }),
                    FakeDownload::Hang => std::future::pending().await,
                }
            })
        }

        fn install(&self, version: &str, bytes: Vec<u8>) -> Result<(), String> {
            if let Some(error) = self.install_error.lock().unwrap().clone() {
                return Err(error);
            }
            self.installs
                .lock()
                .unwrap()
                .push((version.to_string(), bytes));
            Ok(())
        }
    }
}
