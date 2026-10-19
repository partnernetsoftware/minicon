//! Real Windows client bootstrap; this fixture never attaches to existing consoles.
use super::*;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread, STARTUPINFOEXW,
    TerminateProcess, UpdateProcThreadAttribute,
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
        // No child descendants can run: the connection is explicitly rejected.
        unsafe {
            TerminateProcess(self.process.as_raw_handle(), 0xfe);
            WaitForSingleObject(self.process.as_raw_handle(), 2000);
        }
    }
}

fn create_suspended_client(session: &DriverSession) -> io::Result<Child> {
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
    let executable = std::path::PathBuf::from(system_root).join("System32/cmd.exe");
    let image: Vec<u16> = executable.as_os_str().encode_wide().chain([0]).collect();
    let command = format!("\"{}\" /d /c exit 0", executable.display());
    let mut command: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOEXW {
        StartupInfo: windows_sys::Win32::System::Threading::STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
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
