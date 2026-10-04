//! Cooperative cancellation.
//!
//! Cancellation is an engine mechanism, not a UI flag: workers check the
//! shared token at every queue pop and on every child entry, so large scans
//! stop promptly without panicking, corrupting state, or leaking threads.
//! The final [`crate::summary::ScanStatus::Cancelled`] state is typed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared cancellation flag. Clone into workers; `cancel()` from anywhere.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    flag: Arc<AtomicBool>,
}

impl CancelHandle {
    pub fn new() -> Self {
        CancelHandle {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Cheap check performed on every entry and directory pop.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

impl Default for CancelHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_idempotent_and_visible_across_clones() {
        let handle = CancelHandle::new();
        let clone = handle.clone();
        assert!(!handle.is_cancelled());
        handle.cancel();
        handle.cancel();
        assert!(clone.is_cancelled());
    }
}
