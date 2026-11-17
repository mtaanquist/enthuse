//! Runs blocking work (folder listings, which can hang on an offline share)
//! on a worker thread and hands the result back on the UI thread.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Runs `work` on a new thread, then `done` with its result on the UI
/// thread. `done` may hold UI-thread-only values such as `Rc`.
pub fn spawn<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let slot = Arc::new(Mutex::new(Slot::<T> {
        value: None,
        waker: None,
    }));
    let writer = slot.clone();
    std::thread::spawn(move || {
        let value = work();
        let waker = {
            let Ok(mut s) = writer.lock() else { return };
            s.value = Some(value);
            s.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    });
    // Fails only if the event loop is gone, when nobody needs the result.
    let _ = slint::spawn_local(async move {
        let value = Pending(slot).await;
        done(value);
    });
}

struct Slot<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

struct Pending<T>(Arc<Mutex<Slot<T>>>);

impl<T> Future for Pending<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut s = match self.0.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        match s.value.take() {
            Some(v) => Poll::Ready(v),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
