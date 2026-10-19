//! Whole-string Windows code-page conversion; callers own API-specific policy.
use std::{io, ptr};
use windows_sys::Win32::Globalization::{MultiByteToWideChar, WideCharToMultiByte};

pub(super) fn decode(code_page: u32, bytes: &[u8]) -> io::Result<Vec<u16>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let length =
        i32::try_from(bytes.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut units = vec![0; bytes.len()];
    // SAFETY: live, disjoint buffers; output capacity is at least the number
    // of UTF-16 units producible by these bytes, and both lengths fit i32.
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            length,
            units.as_mut_ptr(),
            length,
        )
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    units.truncate(written as usize);
    Ok(units)
}

/// None means the encoded string exceeds the supplied byte capacity. Query
/// before allocation so small native read buffers do not allocate a full copy.
pub(super) fn encode(
    code_page: u32,
    units: &[u16],
    capacity: usize,
) -> io::Result<Option<Vec<u8>>> {
    if units.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let length =
        i32::try_from(units.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: live input with checked count; a null output requests its size.
    let size = unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            units.as_ptr(),
            length,
            ptr::null_mut(),
            0,
            ptr::null(),
            ptr::null_mut(),
        )
    };
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    if size as usize > capacity {
        return Ok(None);
    }
    let mut bytes = vec![0; size as usize];
    // SAFETY: same live input and exact queried, disjoint output capacity.
    let written = unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            units.as_ptr(),
            length,
            bytes.as_mut_ptr(),
            size,
            ptr::null(),
            ptr::null_mut(),
        )
    };
    if written != size {
        return Err(io::Error::last_os_error());
    }
    Ok(Some(bytes))
}
