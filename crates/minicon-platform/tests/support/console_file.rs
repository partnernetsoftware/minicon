//! Owned console file opens shared by independent native fixtures.
use std::io;

pub(super) fn open(name: &str, access: u32) -> io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: terminated local console name; successful handle ownership moves
    // directly into File and is never reconstructed from a numeric PID.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            3,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}
