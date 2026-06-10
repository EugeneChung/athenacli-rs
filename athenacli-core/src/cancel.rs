//! Process-wide cancellation flag, the Rust stand-in for Python's
//! `KeyboardInterrupt`. The REPL arms a SIGINT listener that sets the flag;
//! long-running work (the Athena poll loop, `watch` sleeps) checks it and
//! stops. One-shot `-e` mode never arms the listener, so Ctrl-C keeps its
//! default terminate behavior there.

use std::sync::atomic::{AtomicBool, Ordering};

static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn request() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn reset() {
    CANCEL.store(false, Ordering::SeqCst);
}

pub fn requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

/// Marker error for a user-cancelled operation. The REPL prints nothing for
/// it, mirroring Python's silent `except KeyboardInterrupt: pass`.
#[derive(Debug, thiserror::Error)]
#[error("cancelled")]
pub struct Cancelled;
