//! Native client bootstrap with ownership established before the client runs.
use crate::DriverSession;
use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

// Native ABI documented in the pinned src/winconpty/winconpty.h.
#[repr(C)]
struct PseudoConsole {
    signal: HANDLE,
    reference: HANDLE,
    host_process: HANDLE,
}

/// One client and its descendant job. Dropping this owner kills the entire job.
pub struct ConsoleChild {
    process: OwnedHandle,
    thread: OwnedHandle,
    job: Option<OwnedHandle>,
    assigned: bool,
    resumed: AtomicBool,
    pid: u32,
}

impl DriverSession {
    /// Launch with an explicit Windows command line, suspended until resume().
    /// `application` is absolute; `command_line` includes the quoted image name.
    /// No shell quoting or environment override is inferred at this native layer.
    pub fn spawn_suspended(
        &self,
        application: &Path,
        command_line: &OsStr,
    ) -> io::Result<ConsoleChild> {
        if !application.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute client image required",
            ));
        }
        let image = wide(application.as_os_str())?;
        let mut command = wide(command_line)?;
        let job = make_job()?;
        let mut bytes = 0;
        // SAFETY: documented attribute-list size query.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        // SAFETY: aligned, writable storage of the native requested size.
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for Attributes {
            fn drop(&mut self) {
                unsafe {
                    DeleteProcThreadAttributeList(self.0);
                }
            }
        }
        let attributes = Attributes(list);
        let mut pseudo = PseudoConsole {
            // Reserved: Rust host resize/control integration remains separate
            // from the system conhost signal-pipe protocol.
            signal: null_mut(),
            reference: self.client_reference(),
            // SAFETY: borrowed pseudo-handle for the actual Rust host process.
            host_process: unsafe { GetCurrentProcess() },
        };
        // SAFETY: native layout and driver reference remain live through creation.
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.0,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                (&mut pseudo as *mut PseudoConsole).cast(),
                size_of::<HANDLE>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
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
        // SAFETY: all native buffers are live and terminated. The child cannot
        // execute before assignment to our kill-on-close job. No stdio leaks in.
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
        let mut child = ConsoleChild {
            // SAFETY: successful creation uniquely transfers both native handles.
            process: unsafe { OwnedHandle::from_raw_handle(information.hProcess) },
            // SAFETY: the thread handle has independent transferred ownership.
            thread: unsafe { OwnedHandle::from_raw_handle(information.hThread) },
            job: Some(job),
            assigned: false,
            resumed: AtomicBool::new(false),
            pid: information.dwProcessId,
        };
        // SAFETY: both handles are live; assignment changes no Rust ownership.
        if unsafe {
            AssignProcessToJobObject(
                child.job.as_ref().unwrap().as_raw_handle(),
                child.process.as_raw_handle(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        child.assigned = true;
        Ok(child)
    }
}

impl ConsoleChild {
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn process_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }

    pub fn resume(&self) -> io::Result<()> {
        if self.resumed.swap(true, Ordering::AcqRel) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "client already resumed",
            ));
        }
        // SAFETY: the initial thread was created suspended and is uniquely owned.
        let previous = unsafe { ResumeThread(self.thread.as_raw_handle()) };
        if previous == u32::MAX {
            self.resumed.store(false, Ordering::Release);
            return Err(io::Error::last_os_error());
        }
        if previous != 1 {
            self.resumed.store(false, Ordering::Release);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "client was not suspended exactly once",
            ));
        }
        Ok(())
    }

    pub fn wait(&self, timeout: Duration) -> io::Result<Option<u32>> {
        let ms = u32::try_from(timeout.as_millis())
            .ok()
            .filter(|ms| *ms < u32::MAX)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "finite client wait required")
            })?;
        // SAFETY: owned live process handle and bounded wait.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), ms) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: completed process handle and writable result.
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(code))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
        // SAFETY: terminate only the job this owner created and assigned.
        if unsafe { TerminateJobObject(self.job.as_ref().unwrap().as_raw_handle(), exit_code) } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn contains_process(&self, process: BorrowedHandle<'_>) -> io::Result<bool> {
        let mut contained = 0;
        // SAFETY: read-only membership query using live handles, with no global PID scan.
        if unsafe {
            IsProcessInJob(
                process.as_raw_handle(),
                self.job.as_ref().unwrap().as_raw_handle(),
                &mut contained,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(contained != 0)
    }

    pub fn active_processes(&self) -> io::Result<u32> {
        let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: matching native structure, valid job and result buffer.
        if unsafe {
            QueryInformationJobObject(
                self.job.as_ref().unwrap().as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut information as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(information.ActiveProcesses)
    }
}

impl Drop for ConsoleChild {
    fn drop(&mut self) {
        if !self.assigned {
            // SAFETY: creation failed before job assignment; reclaim our still
            // suspended client, which has never executed an application thread.
            unsafe {
                TerminateProcess(self.process.as_raw_handle(), 0xfe);
            }
        }
        drop(self.job.take()); // kill-on-close also reclaims all descendants.
        // SAFETY: bounded wait on our owned root process; no PID lookup/reuse.
        unsafe {
            WaitForSingleObject(self.process.as_raw_handle(), 2000);
        }
    }
}

fn make_job() -> io::Result<OwnedHandle> {
    // SAFETY: an unnamed, non-inheritable native job.
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful creation uniquely transfers the handle.
    let job = unsafe { OwnedHandle::from_raw_handle(job) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: live native job and matching, initialized limit structure.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = value.encode_wide().collect();
    if units.len() >= 32767 || units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid native command string",
        ));
    }
    units.push(0);
    Ok(units)
}
