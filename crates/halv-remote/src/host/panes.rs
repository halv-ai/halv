//! The pane list and its revision (sections 8.4 and 8.5).
//!
//! The engine keeps one list for all sessions. The revision goes up by 1 for each change.
//! It starts at the start time of the engine in epoch milliseconds, so the revisions of a
//! restarted engine are normally above the revisions of the engine before it.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::rpc::{Pane, PaneList};

/// The shared pane list.
#[derive(Debug)]
pub(crate) struct PaneCache {
    current: Mutex<Arc<PaneList>>,
    sender: watch::Sender<Arc<PaneList>>,
    /// Makes each fetch and update one step, so an older fetch never replaces a newer one.
    refresh: tokio::sync::Mutex<()>,
}

impl PaneCache {
    pub(crate) fn new(start_rev: u64) -> PaneCache {
        let list = Arc::new(PaneList {
            panes: Vec::new(),
            rev: start_rev,
        });
        let (sender, _) = watch::channel(list.clone());
        PaneCache {
            current: Mutex::new(list),
            sender,
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    /// Returns a receiver that sees each new revision.
    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<PaneList>> {
        self.sender.subscribe()
    }

    /// Stores a new list. The revision changes only when the panes changed.
    fn update(&self, panes: Vec<Pane>) -> Arc<PaneList> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if current.panes != panes {
            *current = Arc::new(PaneList {
                panes,
                rev: current.rev.saturating_add(1),
            });
            self.sender.send_replace(current.clone());
        }
        current.clone()
    }

    /// Reads the panes with `fetch` in `spawn_blocking` and stores them.
    pub(crate) async fn refresh<F>(&self, fetch: F) -> Arc<PaneList>
    where
        F: FnOnce() -> Vec<Pane> + Send + 'static,
    {
        let _serial = self.refresh.lock().await;
        match tokio::task::spawn_blocking(fetch).await {
            Ok(panes) => self.update(panes),
            // A panic in the backend keeps the last list.
            Err(_) => self
                .current
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{PaneKind, PaneState};

    fn pane(title: &str) -> Pane {
        Pane {
            session: "term-1".to_owned(),
            title: title.to_owned(),
            kind: PaneKind::Shell,
            project: None,
            cwd: None,
            cols: 80,
            rows: 24,
            state: PaneState::Running,
            exit_code: None,
            last_output_at: None,
            status: None,
        }
    }

    #[tokio::test]
    async fn revision_changes_only_with_the_panes() {
        let cache = PaneCache::new(100);
        let mut receiver = cache.subscribe();
        let first = cache.refresh(|| vec![pane("a")]).await;
        assert_eq!(first.rev, 101);
        assert!(receiver.has_changed().unwrap());
        receiver.borrow_and_update();
        let same = cache.refresh(|| vec![pane("a")]).await;
        assert_eq!(same.rev, 101);
        assert!(!receiver.has_changed().unwrap());
        let second = cache.refresh(|| vec![pane("b")]).await;
        assert_eq!(second.rev, 102);
        assert_eq!(receiver.borrow_and_update().panes[0].title, "b");
    }
}
