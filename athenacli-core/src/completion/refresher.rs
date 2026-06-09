//! Background metadata refresh. Replaces Python `CompletionRefresher`'s thread +
//! restart Event with a tokio task that builds a fresh `Metadata` and swaps it
//! into an `ArcSwap` the completer reads lock-free every keystroke (master plan
//! decision #2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::runtime::Handle;

use super::metadata::{Metadata, Querier};
use crate::parse::scanner;

pub struct Refresher {
    current: Arc<ArcSwap<Metadata>>,
    running: Arc<AtomicBool>,
    restart: Arc<AtomicBool>,
    handle: Handle,
    querier: Querier,
}

impl Refresher {
    pub fn new(handle: Handle, querier: Querier) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(Metadata::default())),
            running: Arc::new(AtomicBool::new(false)),
            restart: Arc::new(AtomicBool::new(false)),
            handle,
            querier,
        }
    }

    /// The shared cache the completer loads from on every keystroke.
    pub fn metadata(&self) -> Arc<ArcSwap<Metadata>> {
        self.current.clone()
    }

    /// Kick off a refresh. If one is already running, coalesce: signal it to
    /// rebuild once more when it finishes (mirrors `_restart_refresh.set()`).
    pub fn refresh(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            self.restart.store(true, Ordering::SeqCst);
            return;
        }
        let current = self.current.clone();
        let running = self.running.clone();
        let restart = self.restart.clone();
        let querier = self.querier.clone();
        self.handle.spawn(async move {
            loop {
                restart.store(false, Ordering::SeqCst);
                let meta = querier.build_metadata().await;
                current.store(Arc::new(meta));
                if !restart.load(Ordering::SeqCst) {
                    break;
                }
            }
            running.store(false, Ordering::SeqCst);
        });
    }
}

/// Whether running `sql` likely changed the schema, so completion should
/// refresh. Port of Python `need_completion_refresh`.
pub fn need_refresh(sql: &str) -> bool {
    scanner::query_starts_with(sql, &["use", "create", "drop", "alter"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_triggers() {
        assert!(need_refresh("USE analytics"));
        assert!(need_refresh("CREATE TABLE t (x int)"));
        assert!(need_refresh("drop table t"));
        assert!(!need_refresh("SELECT 1"));
        assert!(!need_refresh("SHOW TABLES"));
    }
}
