// Cooperative cancellation without tokio-util or tokio's macros: a flag for
// checks between steps, and a Notify to wake whatever is being awaited. Dropping
// the raced future (an HTTP request, a tool run, a sleep) aborts it.

use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;

use tokio::sync::Notify;

#[derive(Clone, Default)]
pub struct Cancel(Arc<Inner>);

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Runs `fut` until it finishes (`Some`) or the token is cancelled (`None`,
    /// and `fut` is dropped).
    pub async fn run<F: Future>(&self, fut: F) -> Option<F::Output> {
        let mut fut = pin!(fut);
        let mut notified = pin!(self.0.notify.notified());
        poll_fn(|cx| {
            // Poll the notification first: that registers the waiter, so a
            // cancel() landing after the flag check below still wakes us.
            if notified.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            if self.is_cancelled() {
                return Poll::Ready(None);
            }
            fut.as_mut().poll(cx).map(Some)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    #[test]
    fn run_finishes_when_not_cancelled() {
        let cancel = Cancel::new();
        assert_eq!(block_on(cancel.run(async { 7 })), Some(7));
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn run_is_none_when_cancelled_before() {
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(cancel.is_cancelled());
        assert_eq!(block_on(cancel.run(async { 7 })), None);
    }

    #[test]
    fn run_is_none_when_cancelled_while_pending() {
        let cancel = Cancel::new();
        let result = block_on(async {
            let other = cancel.clone();
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                other.cancel();
            });
            cancel.run(std::future::pending::<()>()).await
        });
        assert_eq!(result, None);
    }
}
