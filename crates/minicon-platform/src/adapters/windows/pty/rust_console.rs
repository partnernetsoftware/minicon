//! In-process Rust host behind the existing Windows PTY transport contract.
use super::*;
use minicon_console::{ClientOptions, ConsoleChild, ConsoleHost, ConsoleInput, DriverSession};
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{self, Receiver};
#[path = "rust_console/input.rs"]
mod input;
pub(super) use input::RustInput;

type Reply = mpsc::Sender<io::Result<()>>;
enum Control {
    Resize(TerminalSize, Reply),
    Key(NativeTerminalKey, u16, Reply),
    Interrupt(bool, mpsc::Sender<io::Result<bool>>),
}

#[derive(Debug)]
pub(super) struct RustConsole {
    child: Arc<ConsoleChild>,
    stop: Arc<AtomicBool>,
    mode: Arc<AtomicU32>,
    output: Arc<OutputPipe>,
    finished: Arc<(Mutex<bool>, Condvar)>,
    input: Arc<RustInput>,
}

impl RustConsole {
    fn request(&self, make: impl FnOnce(Reply) -> Control) -> io::Result<()> {
        self.input.request(make, PTY_WRITE_TIMEOUT)
    }

    pub(super) fn resize(&self, size: TerminalSize) -> io::Result<()> {
        self.request(|reply| Control::Resize(size, reply))
    }

    pub(super) fn send_key(&self, key: NativeTerminalKey, repeat: u16) -> io::Result<()> {
        self.request(|reply| Control::Key(key, repeat, reply))
    }

    pub(super) fn input_ownership(&self) -> NativeInputOwnership {
        let mode = self.mode.load(Ordering::Acquire);
        if mode & ENABLE_LINE_INPUT != 0 {
            NativeInputOwnership::Cooked
        } else if mode & ENABLE_VIRTUAL_TERMINAL_INPUT != 0 {
            NativeInputOwnership::RawVt
        } else {
            NativeInputOwnership::RawNative
        }
    }

    pub(super) fn terminate(&self) -> io::Result<PtyCleanupReceipt> {
        let observed = self.child.active_processes()?;
        self.close();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if self.child.active_processes()? == 0 {
                return Ok(PtyCleanupReceipt {
                    containment: "windows-job-object",
                    members_observed: observed,
                    members_terminated: observed,
                    verified_empty: true,
                });
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Rust console job did not become empty",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(super) fn close(&self) {
        self.stop.store(true, Ordering::Release);
        // Unblock a worker writing to a full output queue before waiting for it.
        self.output.begin_drain();
        let _ = self.child.terminate(1);
        let (lock, wake) = &*self.finished;
        let done = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = wake.wait_timeout_while(done, Duration::from_secs(2), |done| !*done);
    }
}

impl Drop for RustConsole {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn spawn(command: ChildCommand, size: TerminalSize) -> io::Result<SpawnedPty> {
    coord_from_size(size)?;
    if usize::from(size.rows) * usize::from(size.cols) > 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "console screen budget exceeded",
        ));
    }
    if breakaway_requested() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "owned Rust console requires job containment",
        ));
    }
    let image = resolve_application_path(&command)?;
    let line = command_line(&command)?;
    let line = OsString::from_wide(&line[..line.len() - 1]);
    let environment = environment_block(&command)?;
    let driver = DriverSession::open()?;
    let child = Arc::new(driver.spawn_suspended_with(
        &image,
        &line,
        ClientOptions {
            environment: environment.as_deref(),
            current_dir: command.current_dir.as_deref(),
        },
    )?);
    let process = child.process_handle().try_clone_to_owned()?;
    let pid = ProcessId::new(child.id()).map_err(io::Error::other)?;

    let output = Arc::new(OutputPipe::new());
    let (send, receive) = mpsc::sync_channel(32);
    let input = Arc::new(RustInput::new(send));
    let writer = Arc::new(InputWriter::Rust(Arc::clone(&input)));
    let state = Arc::new(RustConsole {
        child: Arc::clone(&child),
        stop: Arc::new(AtomicBool::new(false)),
        mode: Arc::new(AtomicU32::new(7)),
        output: Arc::clone(&output),
        finished: Arc::new((Mutex::new(false), Condvar::new())),
        input: Arc::clone(&input),
    });
    let stop = Arc::clone(&state.stop);
    let mode = Arc::clone(&state.mode);
    let finished = Arc::clone(&state.finished);
    let worker_child = Arc::clone(&child);
    let worker_output = Arc::clone(&output);
    // The worker owns driver/host borrows locally, with no self-referential or
    // leaked session allocation. It never owns the RustConsole shutdown owner.
    crate::threading::spawn_named_detached(
        "minicon-rust-console",
        Box::new(move || {
            let mut result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                serve(
                    driver,
                    &worker_child,
                    &input,
                    &worker_output,
                    &stop,
                    &mode,
                    receive,
                    size,
                )
            }))
            .unwrap_or_else(|_| Err(io::Error::other("Rust console worker panicked")));
            input.close();
            // ConDrv may cancel the final completion after every client has
            // exited. An empty private job is required before treating 995 as
            // normal EOF; no live client's I/O failure is hidden.
            if result
                .as_ref()
                .is_err_and(|error| error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32))
                && wait_for_empty_job(&worker_child, Duration::from_millis(500)).unwrap_or(false)
            {
                result = Ok(());
            }
            if result.is_err() {
                let _ = worker_child.terminate(1);
            }
            worker_output.finish(result.err());
            let (lock, wake) = &*finished;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            wake.notify_all();
        }),
    )?;
    let session = Arc::new(ConptySession {
        rust: Some(state),
        hpc: Mutex::new(None),
        agent: None,
        input: writer,
        output,
        passthrough: false,
    });
    child.resume()?;
    Ok(SpawnedPty {
        master: PtyMaster {
            session: Arc::clone(&session),
        },
        child: PtyChild {
            process,
            job: None,
            session,
            pid,
        },
    })
}

fn wait_for_empty_job(child: &ConsoleChild, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.active_processes()? == 0 {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn settle_native_result<T>(
    result: io::Result<T>,
    child: &ConsoleChild,
    closed: &mut bool,
) -> io::Result<()> {
    match result {
        Ok(_) => Ok(()),
        Err(error)
            if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32)
                && wait_for_empty_job(child, Duration::from_millis(500)).unwrap_or(false) =>
        {
            *closed = true;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

const OUTPUT_STAGING_CAPACITY: usize = 4 * 1024 * 1024;
struct QueueWriter<'a> {
    output: &'a OutputPipe,
    pending: VecDeque<u8>,
}
impl QueueWriter<'_> {
    fn drain(&mut self) {
        while !self.pending.is_empty() {
            let (first, _) = self.pending.as_slices();
            let count = self.output.try_push(first);
            if count == 0 {
                break;
            }
            self.pending.drain(..count);
        }
    }
    fn ready(&self) -> bool {
        self.pending.is_empty()
    }
}
impl io::Write for QueueWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > OUTPUT_STAGING_CAPACITY - self.pending.len() {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "console output staging budget exceeded",
            ));
        }
        self.pending.extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn serve(
    driver: DriverSession,
    child: &ConsoleChild,
    input: &RustInput,
    output: &OutputPipe,
    stop: &AtomicBool,
    mode: &AtomicU32,
    controls: Receiver<Control>,
    size: TerminalSize,
) -> io::Result<()> {
    let mut host = ConsoleHost::for_child(child)?;
    let mut sink = QueueWriter {
        output,
        pending: VecDeque::new(),
    };
    // System ConPTY starts output in processed, wrapping VT mode (0x07).
    host.set_output_mode(7, &mut sink)?;
    host.resize(size.cols, size.rows, &mut sink)?;
    let mut pending = Vec::new();
    let mut previous_cr = false;
    let mut escape_wait = None;
    let mut input_blocked = false;
    let mut driver_closed = false;
    while !stop.load(Ordering::Acquire) {
        sink.drain();
        if driver_closed {
            input.close();
            // Requests racing the final native disconnect must not wait for
            // output consumption before their reply channels disconnect.
            for control in controls.try_iter().take(32) {
                drop(control);
            }
            if sink.ready() {
                host.finish_output(&mut sink)?;
                if sink.ready() {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        // Bound each lap so input cannot starve native request dispatch.
        for control in controls.try_iter().take(32) {
            let (result, reply) = match control {
                Control::Resize(size, reply) => (
                    if sink.ready() && !host.has_pending_output() {
                        host.resize(size.cols, size.rows, &mut sink)
                    } else {
                        Err(io::ErrorKind::WouldBlock.into())
                    },
                    reply,
                ),
                Control::Interrupt(pressure_only, reply) => {
                    let result = if host.input_mode() & 1 != 0 && (!pressure_only || input_blocked)
                    {
                        host.send_ctrl_c().map(|_| true)
                    } else {
                        Ok(false)
                    };
                    let _ = reply.send(result);
                    continue;
                }
                Control::Key(key, repeat, reply) => {
                    if key != NativeTerminalKey::Break
                        && (!sink.ready() || host.has_pending_output())
                    {
                        let _ = reply.send(Err(io::ErrorKind::WouldBlock.into()));
                        continue;
                    }
                    if key == NativeTerminalKey::Break {
                        input.flush()?;
                        pending.clear();
                        previous_cr = false;
                        escape_wait = None;
                    }
                    let key = native_console_key_event(key, repeat);
                    let event = ConsoleInput::Key {
                        down: true,
                        repeat: key.repeat_count,
                        virtual_key: key.virtual_key_code,
                        scan_code: key.virtual_scan_code,
                        character: key.unicode_char,
                        controls: key.control_key_state,
                    };
                    (host.enqueue_input(&[event], &mut sink).map(|_| ()), reply)
                }
            };
            let _ = reply.send(result);
        }
        if sink.ready() {
            settle_native_result(host.resume_output(&mut sink), child, &mut driver_closed)?;
            mode.store(host.input_mode(), Ordering::Release);
        }
        input.fill_pending(&mut pending)?;
        let mut input_progress = false;
        let mut valid = match std::str::from_utf8(&pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        };
        {
            // VT transport becomes native key identity for record readers.
            // The host converts keys back to sequences only for VT text reads.
            if let Some(escape) = pending[..valid].iter().position(|byte| *byte == 27)
                && sink.ready()
                && !host.has_pending_output()
            {
                if escape != 0 {
                    valid = escape;
                } else {
                    let decoded = crate::selected::console_input::decode_key(&pending[..valid]);
                    if let Some((key, used)) = decoded {
                        let scan_code = match key.virtual_key {
                            0x25 => 0x4b,
                            0x26 => 0x48,
                            0x27 => 0x4d,
                            0x28 => 0x50,
                            _ => 0,
                        };
                        let event = ConsoleInput::Key {
                            down: true,
                            repeat: 1,
                            virtual_key: key.virtual_key,
                            scan_code,
                            character: key.unicode,
                            controls: if scan_code != 0 {
                                ENHANCED_KEY
                            } else if key.control {
                                8
                            } else {
                                0
                            },
                        };
                        match host.enqueue_input(&[event], &mut sink) {
                            Ok(_) => {
                                pending.drain(..used);
                                input_progress = true;
                                previous_cr = false;
                                escape_wait = None;
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                            Err(error) => settle_native_result(
                                Err::<(), _>(error),
                                child,
                                &mut driver_closed,
                            )?,
                        }
                        valid = 0;
                    } else if pending.len() == 1 {
                        let start = escape_wait.get_or_insert_with(Instant::now);
                        // A lone Escape needs a finite distinction from a split CSI.
                        if start.elapsed() < Duration::from_millis(30) {
                            valid = 0;
                        } else {
                            escape_wait = None;
                        }
                    } else {
                        if pending.len() > 128 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "console escape sequence budget exceeded",
                            ));
                        }
                        escape_wait = None;
                        valid = 0;
                    }
                }
            } else {
                escape_wait = None;
            }
        }
        if valid != 0 && sink.ready() && !host.has_pending_output() {
            let mut cr = previous_cr;
            let text: String = std::str::from_utf8(&pending[..valid])
                .unwrap()
                .chars()
                .filter_map(|ch| {
                    let skip = cr && ch == '\n';
                    cr = ch == '\r';
                    (!skip).then_some(if ch == '\u{7f}' { '\u{8}' } else { ch })
                })
                .collect();
            match host.enqueue_text(&text, &mut sink) {
                Ok(_) => {
                    pending.drain(..valid);
                    input_progress = true;
                    previous_cr = cr;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => settle_native_result(Err::<(), _>(error), child, &mut driver_closed)?,
            }
        }
        input_blocked = !input_progress && !pending.is_empty();
        if input_blocked && host.input_mode() & 1 != 0 {
            let interrupted = if let Some(at) = pending.iter().position(|byte| *byte == 3) {
                pending.remove(at);
                true
            } else {
                input.take_queued_interrupt()?
            };
            if interrupted {
                settle_native_result(host.send_ctrl_c(), child, &mut driver_closed)?;
            }
        }
        if driver_closed {
            continue;
        }
        if child.wait(Duration::ZERO)?.is_some()
            && host.connected_clients() == 0
            && child.active_processes()? == 0
        {
            driver_closed = true;
            continue;
        }
        match driver.receive(Duration::from_millis(5)) {
            Ok(request) => {
                settle_native_result(
                    host.dispatch_with_output_ready(request, sink.ready(), &mut sink),
                    child,
                    &mut driver_closed,
                )?;
                mode.store(host.input_mode(), Ordering::Release);
                // API-level rejection was completed to the requesting client;
                // it is not a transport failure or authority to kill siblings.
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => settle_native_result(Err::<(), _>(error), child, &mut driver_closed)?,
        }
    }
    host.cancel_pending_reads()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn rust_console_pty_output_staging_bounds_writes_atomically_and_discards_only_on_shutdown() {
        let output = OutputPipe::new();
        let mut sink = QueueWriter {
            output: &output,
            pending: VecDeque::new(),
        };
        sink.write_all(&vec![b'x'; OUTPUT_STAGING_CAPACITY])
            .unwrap();
        sink.drain();
        assert_eq!(
            sink.pending.len(),
            OUTPUT_STAGING_CAPACITY - OUTPUT_QUEUE_CAPACITY
        );
        assert_eq!(
            output.state.lock().unwrap().queue.len(),
            OUTPUT_QUEUE_CAPACITY
        );
        let error = sink
            .write_all(&vec![b'y'; OUTPUT_QUEUE_CAPACITY + 1])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);
        assert_eq!(
            sink.pending.len(),
            OUTPUT_STAGING_CAPACITY - OUTPUT_QUEUE_CAPACITY
        );
        let mut buffered = vec![0; OUTPUT_QUEUE_CAPACITY];
        assert_eq!(output.read(&mut buffered).unwrap(), OUTPUT_QUEUE_CAPACITY);
        assert!(buffered.iter().all(|byte| *byte == b'x'));
        sink.drain();
        assert!(
            !sink.ready(),
            "normal pressure must retain unconsumed bytes"
        );
        output.begin_drain();
        sink.drain();
        assert!(sink.ready(), "only explicit shutdown discards staging");
    }

    fn suspended_client() -> (DriverSession, Arc<ConsoleChild>) {
        let session = DriverSession::open().unwrap();
        let image = PathBuf::from(env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe");
        let command = OsString::from(format!("\"{}\" /d /c exit 0", image.display()));
        let child = session.spawn_suspended(&image, &command).unwrap();
        (session, Arc::new(child))
    }

    #[test]
    fn rust_console_pty_final_cancellation_requires_an_empty_job_and_preserves_other_errors() {
        let (_session, child) = suspended_client();
        let cancelled = || io::Error::from_raw_os_error(ERROR_OPERATION_ABORTED as i32);
        let mut closed = false;
        let error =
            settle_native_result(Err::<(), _>(cancelled()), &child, &mut closed).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERROR_OPERATION_ABORTED as i32));
        assert!(!closed);
        child.terminate(1).unwrap();
        assert!(wait_for_empty_job(&child, Duration::from_secs(1)).unwrap());
        settle_native_result(Err::<(), _>(cancelled()), &child, &mut closed).unwrap();
        assert!(closed);
        closed = false;
        let error = settle_native_result(
            Err::<(), _>(io::Error::from_raw_os_error(5)),
            &child,
            &mut closed,
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        assert!(!closed);
        settle_native_result(Ok(17), &child, &mut closed).unwrap();
        assert!(!closed);
    }

    #[test]
    fn rust_console_pty_cancelled_job_wait_observes_a_delayed_empty_transition() {
        let (_session, child) = suspended_client();
        assert_eq!(child.active_processes().unwrap(), 1);
        let later = Arc::clone(&child);
        let terminate = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            later.terminate(1).unwrap();
        });
        let empty = wait_for_empty_job(&child, Duration::from_secs(1)).unwrap();
        terminate.join().unwrap();
        assert!(
            empty,
            "one nonempty observation must not decide final cancellation EOF"
        );
        assert_eq!(child.wait(Duration::from_secs(1)).unwrap(), Some(1));
    }

    #[test]
    fn rust_console_pty_cancelled_job_wait_keeps_a_live_client_nonempty() {
        let (_session, child) = suspended_client();
        assert!(!wait_for_empty_job(&child, Duration::from_millis(10)).unwrap());
        assert_eq!(child.active_processes().unwrap(), 1);
        assert_eq!(child.wait(Duration::ZERO).unwrap(), None);
    }

    #[test]
    fn rust_console_pty_shutdown_releases_a_worker_at_actual_output_capacity() {
        let image = PathBuf::from(env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        let (master, child) = ChildCommand::new(image)
            .arg("/d")
            .arg("/c")
            .arg(
                "for /l %i in (1,1,10000) do @echo rust-owned-output-pressure-01234567890123456789",
            )
            .rust_console()
            .spawn()
            .unwrap()
            .into_parts();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let full =
                master.session.output.state.lock().unwrap().queue.len() == OUTPUT_QUEUE_CAPACITY;
            if full {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "actual output queue never reached capacity"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let rust = master.session.rust.as_ref().unwrap();
        assert!(!*rust.finished.0.lock().unwrap());
        let started = Instant::now();
        let receipt = child.terminate_forcefully().unwrap();
        assert!(receipt.verified_empty);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            *rust.finished.0.lock().unwrap(),
            "close must release the blocked host worker, not only kill its client"
        );
    }
}
