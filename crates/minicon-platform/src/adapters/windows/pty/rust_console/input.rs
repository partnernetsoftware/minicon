//! Bounded Rust-host input. Ordinary writes serialize; controls have their
//! existing independent bounded channel and are interpreted by the host.
use super::{Control, WRITE_CHUNK_SIZE};
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Condvar, Mutex, TryLockError};
use std::time::{Duration, Instant};

const INPUT_FRAMES: usize = 16;

#[derive(Default)]
struct Queue {
    frames: VecDeque<Vec<u8>>,
    generation: u64,
}

pub(in super::super) struct RustInput {
    queue: Mutex<Queue>,
    space: Condvar,
    operation: Mutex<()>,
    closed: AtomicBool,
    controls: SyncSender<Control>,
}

impl std::fmt::Debug for RustInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never expose queued terminal text in session diagnostics.
        f.debug_struct("RustInput")
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl RustInput {
    pub(super) fn new(controls: SyncSender<Control>) -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            space: Condvar::new(),
            operation: Mutex::new(()),
            closed: AtomicBool::new(false),
            controls,
        }
    }

    pub(super) fn request<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<io::Result<T>>) -> Control,
        timeout: Duration,
    ) -> io::Result<T> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let (send, receive) = mpsc::channel();
        self.controls
            .try_send(make(send))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::from(io::ErrorKind::WouldBlock),
                mpsc::TrySendError::Disconnected(_) => io::Error::from(io::ErrorKind::BrokenPipe),
            })?;
        receive.recv_timeout(timeout).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => io::Error::from(io::ErrorKind::TimedOut),
            mpsc::RecvTimeoutError::Disconnected => io::Error::from(io::ErrorKind::BrokenPipe),
        })?
    }

    fn try_interrupt(&self, pressure_only: bool, timeout: Duration) -> io::Result<bool> {
        self.request(|reply| Control::Interrupt(pressure_only, reply), timeout)
    }

    pub(in super::super) fn write_all(&self, bytes: &[u8], timeout: Duration) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        // A physical Ctrl+C can interrupt a different thread's blocked write.
        // Raw-mode refusal sends the same byte through ordinary serialization.
        if bytes == [3] && self.try_interrupt(false, timeout)? {
            return Ok(());
        }
        let acquiring = Instant::now();
        let _operation = loop {
            if self.closed.load(Ordering::Acquire) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            match self.operation.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(_)) => {
                    return Err(io::Error::other("Rust input writer poisoned"));
                }
                Err(TryLockError::WouldBlock) if acquiring.elapsed() >= timeout => {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        let generation = self
            .queue
            .lock()
            .map_err(|_| io::Error::other("Rust input queue poisoned"))?
            .generation;
        let mut last_progress = Instant::now();
        let mut cursor = 0;
        let mut skipped = None;
        while cursor < bytes.len() {
            let end = bytes.len().min(cursor + WRITE_CHUNK_SIZE);
            let mut frame = bytes[cursor..end].to_vec();
            if let Some(index) = skipped
                && (cursor..end).contains(&index)
            {
                frame.remove(index - cursor);
            }
            loop {
                if self.closed.load(Ordering::Acquire) {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                let remaining = timeout.saturating_sub(last_progress.elapsed());
                if remaining.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                let mut queue = self
                    .queue
                    .lock()
                    .map_err(|_| io::Error::other("Rust input queue poisoned"))?;
                if queue.generation != generation {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "input write interrupted by Break",
                    ));
                }
                if frame.is_empty() || queue.frames.len() < INPUT_FRAMES {
                    if !frame.is_empty() {
                        queue.frames.push_back(frame);
                    }
                    break;
                }
                drop(queue);
                // Only one unsubmitted control offset is retained. It cannot
                // duplicate a byte already visible to the worker's queue scan.
                if skipped.is_none()
                    && let Some(index) = bytes[cursor..].iter().position(|byte| *byte == 3)
                {
                    match self.try_interrupt(true, remaining) {
                        Ok(true) => {
                            let index = cursor + index;
                            skipped = Some(index);
                            if index < end {
                                frame.remove(index - cursor);
                            }
                        }
                        Ok(false) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(error) => return Err(error),
                    }
                }
                let queue = self
                    .queue
                    .lock()
                    .map_err(|_| io::Error::other("Rust input queue poisoned"))?;
                let _ = self
                    .space
                    .wait_timeout(queue, Duration::from_millis(2))
                    .map_err(|_| io::Error::other("Rust input queue poisoned"))?;
            }
            cursor = end;
            if skipped.is_some_and(|index| index < cursor) {
                skipped = None;
            }
            last_progress = Instant::now();
        }
        Ok(())
    }

    pub(super) fn fill_pending(&self, pending: &mut Vec<u8>) -> io::Result<()> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| io::Error::other("Rust input queue poisoned"))?;
        while pending.len() < WRITE_CHUNK_SIZE {
            let Some(front) = queue.frames.front_mut() else {
                break;
            };
            let count = front.len().min(WRITE_CHUNK_SIZE - pending.len());
            pending.extend_from_slice(&front[..count]);
            front.drain(..count);
            if front.is_empty() {
                queue.frames.pop_front();
                self.space.notify_all();
            }
        }
        Ok(())
    }

    pub(super) fn take_queued_interrupt(&self) -> io::Result<bool> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| io::Error::other("Rust input queue poisoned"))?;
        for (index, frame) in queue.frames.iter_mut().enumerate() {
            if let Some(at) = frame.iter().position(|byte| *byte == 3) {
                frame.remove(at);
                if frame.is_empty() {
                    queue.frames.remove(index);
                    self.space.notify_all();
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn flush(&self) -> io::Result<()> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| io::Error::other("Rust input queue poisoned"))?;
        queue.generation = queue
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Rust input generation exhausted"))?;
        queue.frames.clear();
        self.space.notify_all();
        Ok(())
    }

    pub(in super::super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.space.notify_all();
    }
}
