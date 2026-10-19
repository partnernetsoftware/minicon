//! Real Windows client bootstrap; this fixture never attaches to existing consoles.
use super::*;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
};

// Private native bootstrap layout from the pinned src/winconpty/winconpty.h.
// This deliberately does not call the system CreatePseudoConsole function.
#[repr(C)]
struct PseudoConsole {
    signal: HANDLE,
    reference: HANDLE,
    host_process: HANDLE,
}

struct Child {
    process: OwnedHandle,
    thread: OwnedHandle,
    pid: u32,
}

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: this unique handle names only our newly created fixture.
        // These fixtures use cmd builtin commands only; they launch no child tools.
        unsafe {
            TerminateProcess(self.process.as_raw_handle(), 0xfe);
            WaitForSingleObject(self.process.as_raw_handle(), 2000);
        }
    }
}

fn create_suspended_client(session: &DriverSession) -> io::Result<Child> {
    create_suspended_cmd(session, "exit 0")
}

fn create_suspended_cmd(session: &DriverSession, script: &str) -> io::Result<Child> {
    let mut bytes = 0;
    // SAFETY: documented sizing query; output lives through the call.
    unsafe {
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
    }
    let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
    let list = storage.as_mut_ptr().cast();
    // SAFETY: pointer-size-aligned writable storage holds the requested size.
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    struct Attributes(windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for Attributes {
        fn drop(&mut self) {
            // SAFETY: initialized list remains backed by storage until return.
            unsafe {
                DeleteProcThreadAttributeList(self.0);
            }
        }
    }
    let attributes = Attributes(list);
    let mut pseudo = PseudoConsole {
        // Not used for this rejected bootstrap: no resize/shutdown messages.
        signal: null_mut(),
        reference: session._reference.as_raw_handle(),
        // SAFETY: pseudo-handle denotes this host process without ownership.
        host_process: unsafe { GetCurrentProcess() },
    };
    // SAFETY: PseudoConsole is a repr(C) native layout; it and the referenced
    // driver session remain live until CreateProcessW consumes the attributes.
    let set = unsafe {
        UpdateProcThreadAttribute(
            attributes.0,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            (&mut pseudo as *mut PseudoConsole).cast(),
            size_of::<HANDLE>(),
            null_mut(),
            null(),
        )
    };
    if set == 0 {
        return Err(io::Error::last_os_error());
    }
    let system_root =
        std::env::var_os("SystemRoot").ok_or_else(|| io::Error::other("SystemRoot missing"))?;
    // Keep native separators for cmd's special command-line parser.
    let executable = std::path::PathBuf::from(system_root)
        .join("System32")
        .join("cmd.exe");
    let image: Vec<u16> = executable.as_os_str().encode_wide().chain([0]).collect();
    let command = format!("\"{}\" /d /c {script}", executable.display());
    let mut command: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOEXW {
        StartupInfo: windows_sys::Win32::System::Threading::STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: INVALID_HANDLE_VALUE,
            hStdOutput: INVALID_HANDLE_VALUE,
            hStdError: INVALID_HANDLE_VALUE,
            ..Default::default()
        },
        lpAttributeList: attributes.0,
    };
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: all strings are terminated; native attribute and output storage
    // are live. No ordinary handles are inherited and no system host is launched.
    if unsafe {
        CreateProcessW(
            image.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            &startup.StartupInfo,
            &mut information,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Child {
        // SAFETY: CreateProcessW transferred these two distinct live handles.
        process: unsafe { OwnedHandle::from_raw_handle(information.hProcess) },
        // SAFETY: the thread handle has independent unique ownership.
        thread: unsafe { OwnedHandle::from_raw_handle(information.hThread) },
        pid: information.dwProcessId,
    })
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
    assert_ne!(
        unsafe { ResumeThread(child.thread.as_raw_handle()) },
        u32::MAX
    );
    drain_owned_cmd(&session, &child, &mut host, &mut output)
}

fn drain_owned_cmd(
    session: &DriverSession,
    child: &Child,
    host: &mut crate::ConsoleHost,
    output: &mut Vec<u8>,
) -> CmdReceipt {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut dispatches = Vec::new();
    while std::time::Instant::now() < deadline {
        // SAFETY: nonblocking status query for our owned client.
        let exited =
            unsafe { WaitForSingleObject(child.process.as_raw_handle(), 0) } == WAIT_OBJECT_0;
        if exited && host.connected_clients() == 0 {
            break;
        }
        match session.receive(Duration::from_millis(25)) {
            Ok(request) => {
                let receipt = host
                    .dispatch(request, output)
                    .expect("dispatch real console request");
                eprintln!("dispatch={receipt:?}");
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
        unsafe { WaitForSingleObject(child.process.as_raw_handle(), 0) },
        WAIT_OBJECT_0,
        "cmd must finish within deadline"
    );
    let mut exit_code = 0;
    // SAFETY: owned process and writable exit-code storage.
    assert_ne!(
        unsafe { GetExitCodeProcess(child.process.as_raw_handle(), &mut exit_code) },
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
fn rust_console_driver_owned_host_rejects_a_foreign_session_without_harming_its_client() {
    let left = DriverSession::open().unwrap();
    let right = DriverSession::open().unwrap();
    let first = create_suspended_client(&left).unwrap();
    let foreign = create_suspended_client(&right).unwrap();
    let mut host = crate::ConsoleHost::new();
    let mut output = Vec::new();
    // SAFETY: only the two owned fixture threads are resumed.
    assert_ne!(
        unsafe { ResumeThread(first.thread.as_raw_handle()) },
        u32::MAX
    );
    let request = left.receive(Duration::from_secs(3)).unwrap();
    assert_eq!(request.descriptor.process, first.pid as usize);
    assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0);
    assert_eq!(host.connected_clients(), 1);
    assert_ne!(
        unsafe { ResumeThread(foreign.thread.as_raw_handle()) },
        u32::MAX
    );
    let request = right.receive(Duration::from_secs(3)).unwrap();
    assert_eq!(request.descriptor.process, foreign.pid as usize);
    let rejected = host.dispatch(request, &mut output).unwrap();
    assert_eq!(
        rejected.status, 0xc000_0008_u32 as i32,
        "a host cannot adopt a different driver session"
    );
    assert_eq!(host.connected_clients(), 1);
    // SAFETY: wait only for the child whose connection was rejected.
    assert_eq!(
        unsafe { WaitForSingleObject(foreign.process.as_raw_handle(), 3000) },
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
    assert_ne!(
        unsafe { ResumeThread(child.thread.as_raw_handle()) },
        u32::MAX
    );
    let request = session
        .receive(Duration::from_secs(3))
        .expect("receive kernel client connection");
    assert_eq!(
        request.descriptor.function, 1,
        "first operation must be CONNECT"
    );
    assert_eq!(
        request.descriptor.process, child.pid as usize,
        "driver must identify our actual child"
    );
    eprintln!(
        "Rust host PID={} received CONNECT for actual cmd PID={}",
        std::process::id(),
        child.pid
    );
    request
        .reject_unsupported()
        .expect("explicitly reject unimplemented attach");
    // SAFETY: wait for and query only our owned child.
    assert_eq!(
        unsafe { WaitForSingleObject(child.process.as_raw_handle(), 3000) },
        WAIT_OBJECT_0
    );
    let mut exit_code = 0;
    // SAFETY: child process handle and writable exit-code storage are live.
    assert_ne!(
        unsafe { GetExitCodeProcess(child.process.as_raw_handle(), &mut exit_code) },
        0
    );
    assert_ne!(
        exit_code, 0,
        "unsupported connection must not report a successful cmd run"
    );
}
