//! In-process Rust host behind the existing Windows PTY transport contract.
use super::*;
use minicon_console::{ClientOptions, ConsoleChild, ConsoleHost, ConsoleInput, DriverSession};
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{self, Receiver, SyncSender};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

type Reply = mpsc::Sender<io::Result<()>>;
enum Control {
    Resize(TerminalSize, Reply),
    Key(NativeTerminalKey, u16, Reply),
}

#[derive(Debug)]
pub(super) struct RustConsole {
    child: Arc<ConsoleChild>,
    stop: Arc<AtomicBool>,
    mode: Arc<AtomicU32>,
    output: Arc<OutputPipe>,
    finished: Arc<(Mutex<bool>, Condvar)>,
    control: SyncSender<Control>,
}

impl RustConsole {
    fn request(&self, make: impl FnOnce(Reply) -> Control) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        let (send, receive) = mpsc::channel();
        self.control
            .try_send(make(send))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::from(io::ErrorKind::WouldBlock),
                mpsc::TrySendError::Disconnected(_) => io::Error::from(io::ErrorKind::BrokenPipe),
            })?;
        receive
            .recv_timeout(PTY_WRITE_TIMEOUT)
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
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
    let input = create_input_pipe(PIPE_BUFFER_SIZE)?;
    let writer = Arc::new(InputWriter::new(input.write)?);
    let output = Arc::new(OutputPipe::new());
    let (send, receive) = mpsc::sync_channel(32);
    let state = Arc::new(RustConsole {
        child: Arc::clone(&child),
        stop: Arc::new(AtomicBool::new(false)),
        mode: Arc::new(AtomicU32::new(7)),
        output: Arc::clone(&output),
        finished: Arc::new((Mutex::new(false), Condvar::new())),
        control: send,
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
                    input.read,
                    &worker_output,
                    &stop,
                    &mode,
                    receive,
                    size,
                )
            }))
            .unwrap_or_else(|_| Err(io::Error::other("Rust console worker panicked")));
            // ConDrv may cancel the final completion after every client has
            // exited. An empty private job is required before treating 995 as
            // normal EOF; no live client's I/O failure is hidden.
            if result
                .as_ref()
                .is_err_and(|error| error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32))
                && worker_child.active_processes().ok() == Some(0)
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

struct QueueWriter<'a>(&'a OutputPipe);
impl io::Write for QueueWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.push(bytes);
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
    input: OwnedHandle,
    output: &OutputPipe,
    stop: &AtomicBool,
    mode: &AtomicU32,
    controls: Receiver<Control>,
    size: TerminalSize,
) -> io::Result<()> {
    let mut host = ConsoleHost::for_child(child)?;
    let mut sink = QueueWriter(output);
    host.resize(size.cols, size.rows, &mut sink)?;
    let mut pending = Vec::new();
    let mut previous_cr = false;
    let mut escape_wait = None;
    while !stop.load(Ordering::Acquire) {
        // Bound each lap so input cannot starve native request dispatch.
        for control in controls.try_iter().take(32) {
            let (result, reply) = match control {
                Control::Resize(size, reply) => {
                    (host.resize(size.cols, size.rows, &mut sink), reply)
                }
                Control::Key(key, repeat, reply) => {
                    let (virtual_key, scan_code) = match key {
                        NativeTerminalKey::Up => (0x26, 0x48),
                        NativeTerminalKey::Down => (0x28, 0x50),
                    };
                    let event = ConsoleInput::Key {
                        down: true,
                        repeat,
                        virtual_key,
                        scan_code,
                        character: 0,
                        controls: ENHANCED_KEY,
                    };
                    (host.enqueue_input(&[event], &mut sink).map(|_| ()), reply)
                }
            };
            let _ = reply.send(result);
        }
        if pending.len() < WRITE_CHUNK_SIZE {
            let mut available = 0;
            // SAFETY: sole reader of this owned pipe; query does not block.
            let ok = unsafe {
                PeekNamedPipe(
                    input.as_raw_handle(),
                    null_mut(),
                    0,
                    null_mut(),
                    &mut available,
                    null_mut(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if !stop.load(Ordering::Acquire) {
                    return Err(error);
                }
                break;
            }
            if available != 0 {
                let mut bytes = [0; WRITE_CHUNK_SIZE];
                let mut count = 0;
                let amount = (available as usize).min(bytes.len() - pending.len());
                // SAFETY: only the already available byte count is read, with a
                // matching writable buffer on the non-overlapped read endpoint.
                if unsafe {
                    ReadFile(
                        input.as_raw_handle(),
                        bytes.as_mut_ptr(),
                        amount as u32,
                        &mut count,
                        null_mut(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                pending.extend_from_slice(&bytes[..count as usize]);
            }
        }
        let mut valid = match std::str::from_utf8(&pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        };
        {
            // VT transport becomes native key identity for record readers.
            // The host converts keys back to sequences only for VT text reads.
            if let Some(escape) = pending[..valid].iter().position(|byte| *byte == 27) {
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
                                previous_cr = false;
                                escape_wait = None;
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                            Err(error) => return Err(error),
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
        if valid != 0 {
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
                    previous_cr = cr;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        match driver.receive(Duration::from_millis(5)) {
            Ok(request) => {
                host.dispatch(request, &mut sink)?;
                mode.store(host.input_mode(), Ordering::Release);
                // API-level rejection was completed to the requesting client;
                // it is not a transport failure or authority to kill siblings.
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }
        if child.wait(Duration::ZERO)?.is_some() && host.connected_clients() == 0 {
            break;
        }
    }
    host.cancel_pending_reads()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
