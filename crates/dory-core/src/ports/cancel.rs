use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

type Hook = Box<dyn FnOnce() + Send>;

/// Cooperative cancellation shared by clones. Engines poll [`CancelToken::is_cancelled`] between
/// units of work. Work that blocks, such as a child process or an HTTP read, registers a hook with
/// [`CancelToken::on_cancel`] that interrupts it.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    hooks: Mutex<Vec<Hook>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Cancels every clone and runs the registered hooks once, on this thread. Calling it again
    /// does nothing.
    pub fn cancel(&self) {
        if self.inner.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let hooks = std::mem::take(
            &mut *self
                .inner
                .hooks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for hook in hooks {
            hook();
        }
    }

    /// Runs `hook` when the token is cancelled, or at once if it already was. Each hook runs at
    /// most once. The hook runs on the cancelling thread, so keep it short and non-blocking.
    pub fn on_cancel(&self, hook: impl FnOnce() + Send + 'static) {
        let mut hooks = self
            .inner
            .hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.is_cancelled() {
            drop(hooks);
            hook();
        } else {
            hooks.push(Box::new(hook));
        }
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn counter() -> (Arc<AtomicUsize>, impl FnOnce() + Send + 'static) {
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        (count, move || {
            c.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[test]
    fn clones_share_cancellation() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn hooks_run_once_however_often_cancel_is_called() {
        let token = CancelToken::new();
        let (count, hook) = counter();
        token.on_cancel(hook);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        token.cancel();
        token.clone().cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_hook_registered_after_cancel_runs_at_once() {
        let token = CancelToken::new();
        token.cancel();
        let (count, hook) = counter();
        token.on_cancel(hook);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_hook_may_register_another_without_deadlock() {
        let token = CancelToken::new();
        let inner = token.clone();
        let (count, second) = counter();
        token.on_cancel(move || inner.on_cancel(second));
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelling_from_another_thread_is_seen() {
        let token = CancelToken::new();
        let remote = token.clone();
        std::thread::spawn(move || remote.cancel()).join().unwrap();
        assert!(token.is_cancelled());
    }
}
