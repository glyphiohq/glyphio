//! The thread that owns a capture from request to delivery.
//!
//! Anything that waits (the interactive region picker, the browser's accessibility tree, the
//! scroll loop, PNG encoding) must not hold the main thread: while it does, the tray menu cannot
//! open and Settings cannot repaint. Entry points therefore only *submit* a request here and
//! return; the worker runs it, and hops to the main thread only to create windows.
//!
//! Generic over the request so the seam is testable with a fake that blocks, standing in for
//! the picker.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;

pub struct Worker<R> {
    tx: mpsc::Sender<R>,
}

/// The worker thread has gone away, so nothing will ever run this request.
#[derive(Debug)]
pub struct WorkerGone;

impl std::fmt::Display for WorkerGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the capture worker is not running")
    }
}

impl std::error::Error for WorkerGone {}

impl<R: Send + 'static> Worker<R> {
    /// Start the thread. Requests run one at a time, in the order submitted. A request that
    /// panics is logged and dropped; it does not take the worker with it, or every later capture
    /// would fail until restart.
    pub fn spawn(mut run: impl FnMut(R) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<R>();
        std::thread::Builder::new()
            .name("capture-worker".into())
            .spawn(move || {
                for req in rx {
                    if catch_unwind(AssertUnwindSafe(|| run(req))).is_err() {
                        log::error!("a capture request panicked; the worker carries on");
                    }
                }
            })
            .expect("spawn capture worker");
        Self { tx }
    }

    /// Queue a request and return at once, whatever the worker is busy with.
    pub fn submit(&self, req: R) -> Result<(), WorkerGone> {
        self.tx.send(req).map_err(|_| WorkerGone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, sync_channel};
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn submit_returns_while_the_worker_is_blocked_in_the_picker() {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = sync_channel::<()>(0);
        let worker = Worker::spawn(move |_: u8| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap(); // the picker, open until the user chooses
        });

        let started = Instant::now();
        worker.submit(1).unwrap();
        entered_rx.recv_timeout(WAIT).unwrap(); // the worker is now inside the picker
        worker.submit(2).unwrap(); // a second submit still must not wait for it
        assert!(started.elapsed() < Duration::from_secs(1));

        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
    }

    #[test]
    fn requests_run_in_order_off_the_calling_thread() {
        let (done_tx, done_rx) = channel();
        let worker = Worker::spawn(move |n: u8| done_tx.send((n, std::thread::current().id())).unwrap());

        worker.submit(1).unwrap();
        worker.submit(2).unwrap();

        let (a, thread_a) = done_rx.recv_timeout(WAIT).unwrap();
        let (b, _) = done_rx.recv_timeout(WAIT).unwrap();
        assert_eq!((a, b), (1, 2));
        assert_ne!(thread_a, std::thread::current().id());
    }

    #[test]
    fn a_panicking_request_does_not_stop_later_ones() {
        let (done_tx, done_rx) = channel();
        let worker = Worker::spawn(move |n: u8| {
            if n == 0 {
                panic!("capture blew up");
            }
            done_tx.send(n).unwrap();
        });

        worker.submit(0).unwrap();
        worker.submit(7).unwrap();

        assert_eq!(done_rx.recv_timeout(WAIT).unwrap(), 7);
    }
}
