//! Real client fixtures use the same job-contained launcher as consumers.
use super::*;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, TerminateProcess};
type Child = crate::ConsoleChild;

fn create_suspended_client(session: &DriverSession) -> io::Result<Child> {
    create_suspended_cmd(session, "exit 0")
}

fn create_suspended_cmd(session: &DriverSession, script: &str) -> io::Result<Child> {
    let root =
        std::env::var_os("SystemRoot").ok_or_else(|| io::Error::other("SystemRoot missing"))?;
    let image = std::path::PathBuf::from(root)
        .join("System32")
        .join("cmd.exe");
    let command = format!("\"{}\" /d /c {script}", image.display());
    session.spawn_suspended(&image, std::ffi::OsStr::new(&command))
}

struct CmdReceipt {
    output: Vec<u8>,
    exit_code: u32,
    dispatches: Vec<crate::DispatchReceipt>,
}

fn run_owned_cmd(script: &str) -> CmdReceipt {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_cmd(&session, script).unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    // SAFETY: resume only our owned suspended fixture.
    child.resume().unwrap();
    drain_owned_cmd(&session, &child, &mut host, &mut output)
}

#[test]
fn rust_console_driver_disconnect_reclaims_its_connection_default_handles() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_client(&session).unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    child.resume().unwrap();
    let receipt = drain_owned_cmd(&session, &child, &mut host, &mut output);
    assert_eq!(receipt.exit_code, 0);
    assert_eq!(
        host.open_handles(),
        0,
        "connection defaults have no separate kernel file close notification"
    );
}

#[test]
fn rust_console_driver_control_scope_rejects_connect_from_another_private_job() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_client(&session).unwrap();
    let other_session = DriverSession::open().unwrap();
    let other_child = create_suspended_client(&other_session).unwrap();
    let mut host = crate::ConsoleHost::for_child(&other_child).unwrap();
    child.resume().unwrap();
    let request = session.receive(Duration::from_secs(3)).unwrap();
    assert_eq!(request.descriptor.function, 1);
    let receipt = host.dispatch(request, &mut Vec::new()).unwrap();
    assert_eq!(receipt.status, 0xc0000022u32 as i32);
    assert_eq!(host.connected_clients(), 0);
    assert_eq!(host.open_handles(), 0);
}

#[test]
fn rust_console_driver_control_targets_are_released_on_disconnect() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_client(&session).unwrap();
    let mut host = crate::ConsoleHost::for_child(&child).unwrap();
    let mut output = Vec::new();
    let mut admitted = false;
    child.resume().unwrap();
    let receipt = drain_owned_cmd_with(
        &session,
        &child,
        &mut host,
        &mut output,
        |receipt, host, _, _| {
            if receipt.function == 1 {
                assert_eq!(host.control_targets(), 1);
                admitted = true;
            }
        },
    );
    assert_eq!(receipt.exit_code, 0);
    assert!(admitted);
    assert_eq!(host.control_targets(), 0);
}

#[test]
fn rust_console_driver_dead_reader_does_not_block_a_live_caller_on_the_shared_file() {
    use std::os::windows::io::AsHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    let session = DriverSession::open().unwrap();
    let script = "cmd /d /c \"cmd /d /c set /p MINICON_SHARED_CHILD=child-wait:&set /p MINICON_SHARED_PARENT=parent-wait:&set MINICON_SHARED_PARENT\" <CONIN$";
    let root = create_suspended_cmd(&session, script).unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    let mut connections = Vec::new();
    let mut killed = false;
    let mut supplied = false;
    let mut dead_caller = None;
    let mut disconnected = false;
    let mut parent_waiting = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    root.resume().unwrap();
    while std::time::Instant::now() < deadline {
        if root.wait(Duration::ZERO).unwrap().is_some() && host.connected_clients() == 0 {
            break;
        }
        let request = match session.receive(Duration::from_millis(20)) {
            Ok(request) => request,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(error) => panic!("receive shared-file client: {error}"),
        };
        let function = request.descriptor.function;
        let caller = request.descriptor.process;
        if function == 1 {
            connections.push(u32::try_from(request.descriptor.process).unwrap());
        }
        let receipt = host.dispatch(request, &mut output).unwrap();
        // cmd probes SetConsoleMode on the read-only CONIN$ redirection.
        assert!(
            receipt.status == 0
                || (receipt.api == Some(0x01000002) && receipt.status == 0xc0000022u32 as i32),
            "{receipt:?}"
        );
        if function == 2 && Some(caller) == dead_caller {
            disconnected = true;
        }
        if receipt.pending && !killed {
            dead_caller = Some(caller);
            assert!(String::from_utf8_lossy(&output).contains("child-wait:"));
            assert_eq!(
                connections.len(),
                3,
                "outer owner, waiting parent and reading child must all connect"
            );
            assert_eq!(host.pending_reads(), 1);
            let pid = *connections.last().unwrap();
            // SAFETY: PID comes from the live kernel CONNECT, not enumeration.
            // Membership is verified before any termination authority is used.
            let raw = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            assert!(!raw.is_null());
            let process = unsafe { OwnedHandle::from_raw_handle(raw) };
            assert!(root.contains_process(process.as_handle()).unwrap());
            assert_eq!(
                unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
                WAIT_TIMEOUT
            );
            assert_ne!(
                unsafe { TerminateProcess(process.as_raw_handle(), 0xee) },
                0
            );
            assert_eq!(
                unsafe { WaitForSingleObject(process.as_raw_handle(), 3000) },
                WAIT_OBJECT_0
            );
            killed = true;
        } else if killed && receipt.pending {
            parent_waiting = String::from_utf8_lossy(&output).contains("parent-wait:");
        }
        if disconnected && parent_waiting && !supplied {
            assert_eq!(
                host.pending_reads(),
                1,
                "the dead caller must be removed while its creator still holds the file"
            );
            host.enqueue_text("kept\r", &mut output).unwrap();
            supplied = true;
        }
    }
    assert!(
        killed && supplied,
        "shared file handoff did not finish: {}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(root.wait(Duration::ZERO).unwrap(), Some(0));
    assert_eq!(host.connected_clients(), 0);
    assert_eq!(host.pending_reads(), 0);
    assert!(String::from_utf8_lossy(&output).contains("MINICON_SHARED_PARENT=kept"));
}

fn drain_owned_cmd<'session>(
    session: &'session DriverSession,
    child: &Child,
    host: &mut crate::ConsoleHost<'session>,
    output: &mut Vec<u8>,
) -> CmdReceipt {
    drain_owned_cmd_with(session, child, host, output, |_, _, _, _| {})
}

fn drain_owned_cmd_with<'session>(
    session: &'session DriverSession,
    child: &Child,
    host: &mut crate::ConsoleHost<'session>,
    output: &mut Vec<u8>,
    mut on_dispatch: impl FnMut(
        &crate::DispatchReceipt,
        &mut crate::ConsoleHost<'session>,
        &mut Vec<u8>,
        &Child,
    ),
) -> CmdReceipt {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut dispatches = Vec::new();
    while std::time::Instant::now() < deadline {
        // SAFETY: nonblocking status query for our owned client.
        let exited = unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 0) }
            == WAIT_OBJECT_0;
        if exited && host.connected_clients() == 0 {
            break;
        }
        match session.receive(Duration::from_millis(25)) {
            Ok(request) => {
                let receipt = host
                    .dispatch(request, output)
                    .expect("dispatch real console request");
                eprintln!("dispatch={receipt:?}");
                on_dispatch(&receipt, host, output, child);
                dispatches.push(receipt);
                assert!(
                    dispatches.len() < 256,
                    "unexpected request storm in small cmd fixture"
                );
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => panic!("receive failed: {error}"),
        }
    }
    assert_eq!(
        unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 0) },
        WAIT_OBJECT_0,
        "cmd must finish within deadline"
    );
    let mut exit_code = 0;
    // SAFETY: owned process and writable exit-code storage.
    assert_ne!(
        unsafe { GetExitCodeProcess(child.process_handle().as_raw_handle(), &mut exit_code) },
        0
    );
    assert_eq!(
        host.connected_clients(),
        0,
        "DISCONNECT must reclaim the client's state"
    );
    assert!(
        dispatches.iter().all(|call| call.status == 0),
        "baseline cmd must not rely on unsupported API fallbacks: {dispatches:?}"
    );
    CmdReceipt {
        output: std::mem::take(output),
        exit_code,
        dispatches,
    }
}

#[test]
fn rust_console_driver_owned_host_delivers_text_after_a_real_blocking_cmd_read() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_cmd(&session,
        "set MINICON_OWNED_INPUT_CHECK=&set /p MINICON_OWNED_INPUT_CHECK=rust-input:&set MINICON_OWNED_INPUT_CHECK").unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    let mut observed_wait = false;
    // SAFETY: our newly created suspended fixture is the sole resumed thread.
    child.resume().unwrap();
    let receipt = drain_owned_cmd_with(
        &session,
        &child,
        &mut host,
        &mut output,
        |call, host, output, child| {
            if call.pending {
                assert!(
                    !observed_wait,
                    "the supplied complete line must satisfy the pending input"
                );
                observed_wait = true;
                assert_eq!(host.pending_reads(), 1);
                // SAFETY: the actual client must still be alive before input arrives.
                assert_eq!(
                    unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 0) },
                    WAIT_TIMEOUT
                );
                assert!(
                    String::from_utf8_lossy(output).contains("rust-input:"),
                    "the real prompt must precede our input"
                );
                assert_eq!(
                    host.enqueue_text("typed-by-rust", output).unwrap(),
                    0,
                    "line input must wait for Enter"
                );
                assert_eq!(host.pending_reads(), 1);
                // SAFETY: the owned client must remain blocked on the partial line.
                assert_eq!(
                    unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 0) },
                    WAIT_TIMEOUT
                );
                assert_eq!(host.enqueue_text("\r", output).unwrap(), 1);
            }
        },
    );
    assert!(
        observed_wait,
        "input must be deferred rather than completed as empty success"
    );
    assert_eq!(receipt.exit_code, 0);
    let text = String::from_utf8(receipt.output).unwrap();
    assert!(
        text.ends_with("MINICON_OWNED_INPUT_CHECK=typed-by-rust\r\n"),
        "client received a different input value: {text:?}"
    );
    assert_eq!(host.pending_reads(), 0);
}

#[test]
fn rust_console_driver_owned_host_edits_unicode_before_completing_line_input() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_cmd(&session,
        "set MINICON_OWNED_EDIT_CHECK=&set /p MINICON_OWNED_EDIT_CHECK=rust-edit:&set MINICON_OWNED_EDIT_CHECK").unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    let mut observed_wait = false;
    // SAFETY: our owned suspended fixture thread only.
    child.resume().unwrap();
    let receipt = drain_owned_cmd_with(
        &session,
        &child,
        &mut host,
        &mut output,
        |call, host, output, _| {
            if call.pending {
                assert!(!observed_wait);
                observed_wait = true;
                assert_eq!(host.enqueue_text("A😀", output).unwrap(), 0);
                assert_eq!(host.enqueue_text("\x08中\r", output).unwrap(), 1);
            }
        },
    );
    assert!(observed_wait);
    assert_eq!(receipt.exit_code, 0);
    let text = String::from_utf8(receipt.output).unwrap();
    assert!(
        text.ends_with("MINICON_OWNED_EDIT_CHECK=A中\r\n"),
        "backspace must remove one Unicode scalar: {text:?}"
    );
}

#[test]
fn rust_console_driver_owned_host_cancellation_wakes_a_blocked_read_as_failure() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_cmd(&session,
        "set /p MINICON_OWNED_CANCEL=rust-cancel: && echo unexpected-read-success || echo rust-read-cancelled").unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    let mut cancelled = false;
    // SAFETY: our owned suspended fixture thread only.
    child.resume().unwrap();
    let receipt = drain_owned_cmd_with(
        &session,
        &child,
        &mut host,
        &mut output,
        |call, host, _, child| {
            if call.pending {
                assert!(!cancelled);
                cancelled = true;
                // SAFETY: confirm this actual owned client is still waiting.
                assert_eq!(
                    unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 0) },
                    WAIT_TIMEOUT
                );
                assert_eq!(host.cancel_pending_reads().unwrap(), 1);
                assert_eq!(host.pending_reads(), 0);
            }
        },
    );
    assert!(cancelled, "must cancel a real deferred read");
    eprintln!("cancelled cmd exit={:#x}", receipt.exit_code);
    let text = String::from_utf8(receipt.output).unwrap();
    assert!(
        text.contains("rust-read-cancelled"),
        "client did not observe read failure: {text:?}"
    );
    assert!(!text.contains("unexpected-read-success"));
}

#[test]
fn rust_console_driver_owned_host_reclaims_reads_after_the_client_is_killed() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_cmd(&session, "set /p MINICON_OWNED_KILLED=rust-wait:").unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    let mut killed = false;
    // SAFETY: our owned suspended fixture thread only.
    child.resume().unwrap();
    let receipt = drain_owned_cmd_with(
        &session,
        &child,
        &mut host,
        &mut output,
        |call, host, _, child| {
            if call.pending {
                assert!(!killed);
                killed = true;
                assert_eq!(host.pending_reads(), 1);
                // SAFETY: terminate only the fixture identified by its owned handle.
                assert_ne!(
                    unsafe { TerminateProcess(child.process_handle().as_raw_handle(), 0x42) },
                    0
                );
            }
        },
    );
    assert!(killed);
    assert_eq!(receipt.exit_code, 0x42);
    assert_eq!(
        host.pending_reads(),
        0,
        "DISCONNECT must retire the dead client's deferred request"
    );
}

#[test]
fn rust_console_driver_dropping_an_unhandled_connection_cancels_the_actual_client() {
    let session = DriverSession::open().unwrap();
    let child = create_suspended_client(&session).unwrap();
    // SAFETY: our owned suspended fixture thread only.
    child.resume().unwrap();
    let request = session.receive(Duration::from_secs(3)).unwrap();
    assert_eq!(request.descriptor.function, 1);
    assert_eq!(request.descriptor.process, child.id() as usize);
    drop(request);
    // SAFETY: wait/query only our owned fixture, not a globally looked-up PID.
    assert_eq!(
        unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 3000) },
        WAIT_OBJECT_0
    );
    let mut exit = 0;
    assert_ne!(
        unsafe { GetExitCodeProcess(child.process_handle().as_raw_handle(), &mut exit) },
        0
    );
    assert_ne!(exit, 0);
}

#[test]
fn rust_console_driver_owned_host_rejects_a_foreign_session_without_harming_its_client() {
    let left = DriverSession::open().unwrap();
    let right = DriverSession::open().unwrap();
    let first = create_suspended_client(&left).unwrap();
    let foreign = create_suspended_client(&right).unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    // SAFETY: only the two owned fixture threads are resumed.
    first.resume().unwrap();
    let request = left.receive(Duration::from_secs(3)).unwrap();
    assert_eq!(request.descriptor.process, first.id() as usize);
    assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0);
    assert_eq!(host.connected_clients(), 1);
    foreign.resume().unwrap();
    let request = right.receive(Duration::from_secs(3)).unwrap();
    assert_eq!(request.descriptor.process, foreign.id() as usize);
    let rejected = host.dispatch(request, &mut output).unwrap();
    assert_eq!(
        rejected.status, 0xc000_0008_u32 as i32,
        "a host cannot adopt a different driver session"
    );
    assert_eq!(host.connected_clients(), 1);
    // SAFETY: wait only for the child whose connection was rejected.
    assert_eq!(
        unsafe { WaitForSingleObject(foreign.process_handle().as_raw_handle(), 3000) },
        WAIT_OBJECT_0
    );
    let receipt = drain_owned_cmd(&left, &first, &mut host, &mut output);
    assert_eq!(
        receipt.exit_code, 0,
        "the original session must continue normally"
    );
    assert!(receipt.output.is_empty());
}

#[test]
fn rust_console_driver_owned_host_executes_real_cmd_output_and_exit() {
    let receipt = run_owned_cmd("echo rust-console-owned-output");
    assert_eq!(receipt.exit_code, 0, "real cmd command must succeed");
    assert_eq!(
        receipt.output, b"rust-console-owned-output\r\n",
        "actual driver output must reach the Rust sink"
    );
    assert!(
        receipt
            .dispatches
            .iter()
            .any(|call| (call.function == 5 || call.api == Some(0x0100_0006)) && call.status == 0)
    );
}

#[test]
fn rust_console_driver_owned_host_preserves_real_cmd_unicode_and_nonzero_exit() {
    let receipt = run_owned_cmd("echo 中文😀 & exit /b 7");
    assert_eq!(receipt.exit_code, 7, "preserve the actual client exit code");
    assert_eq!(
        receipt.output,
        "中文😀 \r\n".as_bytes(),
        "preserve the UTF-16 console write, including cmd's space before &"
    );
    assert!(
        receipt
            .dispatches
            .iter()
            .any(|call| call.api == Some(0x0100_0006) && call.status == 0)
    );
}

#[test]
fn rust_console_driver_receives_and_rejects_a_real_cmd_connection() {
    let session = DriverSession::open().expect("private Rust console server");
    let child = create_suspended_client(&session).expect("select our reference for cmd");
    // SAFETY: start only our owned suspended fixture thread.
    child.resume().unwrap();
    let request = session
        .receive(Duration::from_secs(3))
        .expect("receive kernel client connection");
    assert_eq!(
        request.descriptor.function, 1,
        "first operation must be CONNECT"
    );
    assert_eq!(
        request.descriptor.process,
        child.id() as usize,
        "driver must identify our actual child"
    );
    eprintln!(
        "Rust host PID={} received CONNECT for actual cmd PID={}",
        std::process::id(),
        child.id()
    );
    request
        .reject_unsupported()
        .expect("explicitly reject unimplemented attach");
    // SAFETY: wait for and query only our owned child.
    assert_eq!(
        unsafe { WaitForSingleObject(child.process_handle().as_raw_handle(), 3000) },
        WAIT_OBJECT_0
    );
    let mut exit_code = 0;
    // SAFETY: child process handle and writable exit-code storage are live.
    assert_ne!(
        unsafe { GetExitCodeProcess(child.process_handle().as_raw_handle(), &mut exit_code) },
        0
    );
    assert_ne!(
        exit_code, 0,
        "unsupported connection must not report a successful cmd run"
    );
}
