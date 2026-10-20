//! Independent result oracle and read-only observation of the held native child.
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE, WAIT_OBJECT_0},
    System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        WaitForSingleObject,
    },
};

pub fn kinds(system: bool) -> Vec<String> {
    let mut result = Vec::new();
    for api in ["wide", "file", "utf8"] {
        if system && api == "utf8" {
            continue;
        }
        for source in ["stdio", "file"] {
            for lifetime in ["exit", "detach"] {
                result.push(format!("{api}-{source}-{lifetime}"));
            }
        }
    }
    result
}
pub fn expected(kind: &str) -> String {
    let utf8 = kind.starts_with("utf8-");
    let stdio = kind.split('-').nth(1) == Some("stdio");
    let first = if utf8 { vec![228] } else { vec![97] };
    let rest = if stdio {
        vec![88]
    } else if utf8 {
        vec![184, 173, 195, 169, 13, 10]
    } else {
        vec![98, 99, 13, 10]
    };
    let after = if stdio { vec![] } else { vec![88] };
    format!("first={first:?};other=[90];rest={rest:?};after={after:?};mode=1;final_wait=258")
}
pub struct ChildObserver {
    pub handle: OwnedHandle,
}
impl ChildObserver {
    pub fn open(report: &Path) -> io::Result<Self> {
        let family = std::fs::read_to_string(report.with_extension("family"))?;
        let (pid, stamp) = family
            .trim()
            .split_once(',')
            .ok_or(io::ErrorKind::InvalidData)?;
        let pid = pid.parse::<u32>().map_err(|_| io::ErrorKind::InvalidData)?;
        let stamp = stamp
            .parse::<u64>()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        // This numeric PID is only the held private client's discovery key.
        // No signal/termination authority is requested; creation time binds
        // the retained process object before the test permits creator exit.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | 0x0010_0000, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let (mut born, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if unsafe {
            GetProcessTimes(
                handle.as_raw_handle() as HANDLE,
                &mut born,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let observed = u64::from(born.dwHighDateTime) << 32 | u64::from(born.dwLowDateTime);
        if observed != stamp {
            return Err(io::Error::other(
                "inherited child creation identity changed",
            ));
        }
        if unsafe { WaitForSingleObject(handle.as_raw_handle() as HANDLE, 0) } == WAIT_OBJECT_0 {
            return Err(io::Error::other("inherited child exited before release"));
        }
        Ok(Self { handle })
    }
    pub fn wait_exit(&self) -> io::Result<()> {
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle() as HANDLE, 8000) }
            != WAIT_OBJECT_0
        {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle() as HANDLE, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if code != 0 {
            return Err(io::Error::other(format!("inherited child exit {code}")));
        }
        Ok(())
    }
}
