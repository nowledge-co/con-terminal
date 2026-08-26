#![cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use parking_lot::{Condvar, Mutex};

// A maximally expanded 16 MiB paste can occupy 32 MiB of ordinary output,
// while a simultaneous Kitty clipboard reply can base64-expand to about
// 22 MiB. Keep enough bounded headroom for both without reordering the FIFO.
const DEFAULT_MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_CONTROL_RESERVE_BYTES: usize = 24 * 1024 * 1024;

struct QueueState {
    payloads: VecDeque<Box<[u8]>>,
    queued_bytes: usize,
    senders: usize,
    closed: bool,
}

struct Shared {
    state: Mutex<QueueState>,
    ready: Condvar,
    max_queued_bytes: usize,
    max_regular_bytes: usize,
}

/// Cloneable, non-blocking producer for one ordered PTY input stream.
///
/// libghostty invokes WRITE_PTY while its parser lock is held, so callbacks
/// must never perform pipe or socket I/O directly. This queue bounds retained
/// memory and hands all writes to one worker, preserving byte ordering between
/// terminal replies, keyboard input, paste, and resize control frames. Regular
/// input cannot consume the reserved tail of the budget, so a terminal reply
/// or key release can still enter the same FIFO behind an accepted large paste.
pub(crate) struct PtyWriteQueue {
    shared: Arc<Shared>,
}

pub(crate) struct PtyWriteWorker {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl PtyWriteQueue {
    pub(crate) fn spawn<F>(thread_name: &str, write: F) -> io::Result<(Self, PtyWriteWorker)>
    where
        F: FnMut(&[u8]) -> io::Result<()> + Send + 'static,
    {
        Self::spawn_with_limits(
            thread_name,
            DEFAULT_MAX_QUEUED_BYTES,
            DEFAULT_CONTROL_RESERVE_BYTES,
            write,
        )
    }

    fn spawn_with_limits<F>(
        thread_name: &str,
        max_queued_bytes: usize,
        control_reserve_bytes: usize,
        mut write: F,
    ) -> io::Result<(Self, PtyWriteWorker)>
    where
        F: FnMut(&[u8]) -> io::Result<()> + Send + 'static,
    {
        let max_regular_bytes = max_queued_bytes.saturating_sub(control_reserve_bytes);
        let shared = Arc::new(Shared {
            state: Mutex::new(QueueState {
                payloads: VecDeque::new(),
                queued_bytes: 0,
                senders: 1,
                closed: false,
            }),
            ready: Condvar::new(),
            max_queued_bytes,
            max_regular_bytes,
        });
        let worker_shared = shared.clone();
        let handle = thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                loop {
                    let payload = {
                        let mut state = worker_shared.state.lock();
                        while state.payloads.is_empty() && !state.closed {
                            worker_shared.ready.wait(&mut state);
                        }
                        if state.closed {
                            state.payloads.clear();
                            state.queued_bytes = 0;
                            return;
                        }
                        state.payloads.pop_front().expect("queue checked non-empty")
                    };

                    let payload_len = payload.len();
                    if let Err(err) = write(&payload) {
                        log::debug!("PTY writer thread stopped after host I/O failure: {err}");
                        let mut state = worker_shared.state.lock();
                        state.closed = true;
                        state.payloads.clear();
                        state.queued_bytes = 0;
                        worker_shared.ready.notify_all();
                        return;
                    }

                    let mut state = worker_shared.state.lock();
                    state.queued_bytes = state.queued_bytes.saturating_sub(payload_len);
                }
            })?;

        Ok((
            Self {
                shared: shared.clone(),
            },
            PtyWriteWorker {
                shared,
                handle: Some(handle),
            },
        ))
    }

    pub(crate) fn enqueue(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.enqueue_owned_with_limit(bytes.into(), self.shared.max_regular_bytes)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn enqueue_owned(&self, payload: Box<[u8]>) -> io::Result<()> {
        self.enqueue_owned_with_limit(payload, self.shared.max_regular_bytes)
    }

    pub(crate) fn enqueue_with_reserved_capacity(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.enqueue_owned_with_limit(bytes.into(), self.shared.max_queued_bytes)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn enqueue_owned_with_reserved_capacity(
        &self,
        payload: Box<[u8]>,
    ) -> io::Result<()> {
        self.enqueue_owned_with_limit(payload, self.shared.max_queued_bytes)
    }

    fn enqueue_owned_with_limit(&self, payload: Box<[u8]>, limit: usize) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        let mut state = self.shared.state.lock();
        if state.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "PTY input queue is closed",
            ));
        }
        let Some(total) = state.queued_bytes.checked_add(payload.len()) else {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "PTY input queue byte count overflowed",
            ));
        };
        if total > limit {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "PTY input queue is full",
            ));
        }

        state.payloads.push_back(payload);
        state.queued_bytes = total;
        self.shared.ready.notify_one();
        Ok(())
    }
}

impl Clone for PtyWriteQueue {
    fn clone(&self) -> Self {
        self.shared.state.lock().senders += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for PtyWriteQueue {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock();
        state.senders = state.senders.saturating_sub(1);
        if state.senders == 0 {
            state.closed = true;
            self.shared.ready.notify_all();
        }
    }
}

impl PtyWriteWorker {
    fn close_queue(&self) {
        let mut state = self.shared.state.lock();
        state.closed = true;
        state.payloads.clear();
        state.queued_bytes = 0;
        self.shared.ready.notify_all();
    }

    /// Stop accepting input and join the writer. The platform owner must close
    /// or terminate the PTY peer first so an already-running OS write unblocks.
    pub(crate) fn shutdown(&mut self) {
        self.close_queue();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PtyWriteWorker {
    fn drop(&mut self) {
        // Error paths may drop a prepared session before a child exists. Wake
        // the worker but do not implicitly join: platform session Drop owns the
        // required peer-close-before-join ordering for live PTYs.
        self.close_queue();
    }
}

#[cfg(test)]
mod tests {
    use super::PtyWriteQueue;
    use parking_lot::{Condvar, Mutex};
    use std::io::ErrorKind;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn queue_bounds_in_flight_bytes_without_blocking_the_producer() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = gate.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (queue, mut worker) =
            PtyWriteQueue::spawn_with_limits("pty-write-test", 4, 0, move |_| {
                entered_tx.send(()).expect("signal writer entry");
                let (open, ready) = &*worker_gate;
                let mut open = open.lock();
                while !*open {
                    ready.wait(&mut open);
                }
                Ok(())
            })
            .expect("spawn writer");

        queue.enqueue(b"full").expect("fill queue budget");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("writer must start");
        let err = queue.enqueue(b"x").expect_err("queue must reject overflow");
        assert_eq!(err.kind(), ErrorKind::WouldBlock);

        let (open, ready) = &*gate;
        *open.lock() = true;
        ready.notify_all();
        worker.shutdown();
    }

    #[test]
    fn control_writes_use_reserved_capacity_without_reordering_the_fifo() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = gate.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (written_tx, written_rx) = mpsc::channel();
        let (queue, mut worker) =
            PtyWriteQueue::spawn_with_limits("pty-write-reserve-test", 6, 2, move |bytes| {
                entered_tx.send(()).expect("signal writer entry");
                let (open, ready) = &*worker_gate;
                let mut open = open.lock();
                while !*open {
                    ready.wait(&mut open);
                }
                written_tx
                    .send(bytes.to_vec())
                    .expect("record completed write");
                Ok(())
            })
            .expect("spawn writer");

        queue.enqueue(b"user").expect("fill regular budget");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("writer must start");
        assert_eq!(
            queue
                .enqueue(b"x")
                .expect_err("regular input must preserve the reserve")
                .kind(),
            ErrorKind::WouldBlock
        );
        queue
            .enqueue_with_reserved_capacity(b"ok")
            .expect("control write must use reserved capacity");
        assert_eq!(
            queue
                .enqueue_with_reserved_capacity(b"x")
                .expect_err("control write must remain bounded")
                .kind(),
            ErrorKind::WouldBlock
        );

        let (open, ready) = &*gate;
        *open.lock() = true;
        ready.notify_all();
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("reserved control write must reach the worker");
        assert_eq!(
            written_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("regular write must complete first"),
            b"user"
        );
        assert_eq!(
            written_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("control write must complete second"),
            b"ok"
        );
        worker.shutdown();
    }
}
