//! Private-job console control delivery. No caller-console group broadcast.
use crate::ConsoleChild;
use std::collections::BTreeMap;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE, WAIT_TIMEOUT};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Threading::{
    GetProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    WaitForSingleObject,
};

type NativeControl = unsafe extern "system" fn(u32, *mut std::ffi::c_void, u32) -> i32;

// Public Microsoft Terminal conwinuserrefs.h / Win32 ConsoleControl.cpp.
// ProcessId is a PID encoded as a HANDLE, not the retained process handle.
#[repr(C)]
struct EndTask {
    process_id: usize,
    window: usize,
    event: u32,
    flags: u32,
}

struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: matching owned LoadLibraryExW reference.
        unsafe { FreeLibrary(self.0) };
    }
}

pub(crate) struct ControlScope<'child> {
    child: &'child ConsoleChild,
    targets: BTreeMap<usize, OwnedHandle>,
    native: NativeControl,
    _library: Library,
}

impl<'child> ControlScope<'child> {
    pub(crate) fn new(child: &'child ConsoleChild) -> io::Result<Self> {
        let name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        // SAFETY: fixed system DLL name, restricted system directory search.
        let module =
            unsafe { LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        let library = Library(module);
        // SAFETY: owned live module, fixed terminated export name.
        let address = unsafe { GetProcAddress(module, c"ConsoleControl".as_ptr().cast()) }
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::Unsupported, "ConsoleControl unavailable")
            })?;
        // SAFETY: signature/layout follow the pinned public native declaration.
        let native = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NativeControl>(address)
        };
        Ok(Self {
            child,
            targets: BTreeMap::new(),
            native,
            _library: library,
        })
    }

    pub(crate) fn admit(&mut self, token: usize, pid: u32, console_app: bool) -> io::Result<()> {
        // Capture before CONNECT completion: the client has not yet been
        // released by our server. Never recover signal targets by PID later.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let contained = self.child.contains_process(process.as_handle())?;
        if !contained || unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_TIMEOUT
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "console client is not live in the owned Job",
            ));
        }
        self.targets.insert(token, process);
        // Register the admitted console application with Win32/CSRSS before
        // releasing CONNECT, as the native host's NotifyConsoleApplication does.
        if console_app {
            let mut information = [pid, 1_u32];
            unsafe {
                (self.native)(1, information.as_mut_ptr().cast(), 8);
            }
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, token: usize) {
        self.targets.remove(&token);
    }

    pub(crate) fn targets(&self) -> usize {
        self.targets.len()
    }

    pub(crate) fn ctrl_c(&self) -> usize {
        let mut attempted = 0;
        for process in self.targets.values() {
            // The retained handle pins object/PID identity even across exit.
            // SAFETY: live owned process handle; zero-time status query.
            if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_TIMEOUT {
                continue;
            }
            let pid = unsafe { GetProcessId(process.as_raw_handle()) };
            if pid == 0 {
                continue;
            }
            let mut task = EndTask {
                process_id: pid as usize,
                window: 0,
                event: 0,
                flags: 1,
            };
            // SAFETY: only admitted private Job targets; exact initialized ABI.
            // EndTask's status does not prove delivery. Client receipts do.
            unsafe {
                (self.native)(
                    7,
                    (&mut task as *mut EndTask).cast(),
                    size_of::<EndTask>() as u32,
                )
            };
            attempted += 1;
        }
        attempted
    }
}
