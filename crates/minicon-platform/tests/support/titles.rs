//! Native title metadata, code-page selection and independent output fences.
use super::{
    buffers::{checked, create, wait_stage},
    mixed_output::bytes,
};
use std::{io, os::windows::io::AsRawHandle, path::Path};
use windows_sys::Win32::{
    Foundation::{GetLastError, HANDLE, SetLastError},
    System::Console::*,
};

fn wide(title: &str) -> Vec<u16> {
    title.encode_utf16().chain(Some(0)).collect()
}
fn set_w(title: &str) -> io::Result<()> {
    checked(unsafe { SetConsoleTitleW(wide(title).as_ptr()) })
}
fn hex<T: std::fmt::LowerHex>(units: &[T]) -> String {
    units
        .iter()
        .map(|u| format!("{u:x}"))
        .collect::<Vec<_>>()
        .join(",")
}
fn query(label: &str, report: &mut String) {
    for original in [false, true] {
        let mut units = [0x5555; 512];
        unsafe {
            SetLastError(0);
        }
        let count = unsafe {
            if original {
                GetConsoleOriginalTitleW(units.as_mut_ptr(), 512)
            } else {
                GetConsoleTitleW(units.as_mut_ptr(), 512)
            }
        };
        let error = unsafe { GetLastError() };
        report.push_str(&format!(
            "{label}:original={original};wide={count},{error};data={};next={:x}\n",
            hex(&units[..(count as usize).min(511)]),
            units[(count as usize).min(511)]
        ));
    }
    for capacity in [0, 1, 2, 3, 4, 5, 8, 64] {
        let mut data = [0x55; 64];
        unsafe {
            SetLastError(0);
        }
        let count = unsafe { GetConsoleTitleA(data.as_mut_ptr(), capacity) };
        let error = unsafe { GetLastError() };
        report.push_str(&format!(
            "{label}:ansi-cap={capacity};ret={count},{error};data={}\n",
            hex(&data[..8])
        ));
    }
    for original in [false, true] {
        for capacity in [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 64, 512] {
            let mut units = [0x5555; 512];
            unsafe {
                SetLastError(0);
            }
            let count = unsafe {
                if original {
                    GetConsoleOriginalTitleW(units.as_mut_ptr(), capacity)
                } else {
                    GetConsoleTitleW(units.as_mut_ptr(), capacity)
                }
            };
            report.push_str(&format!(
                "{label}:original={original};wide-cap={capacity};ret={count},{};data={}\n",
                unsafe { GetLastError() },
                hex(&units[..12])
            ));
            let mut data = [0x55; 512];
            unsafe {
                SetLastError(0);
            }
            let count = unsafe {
                if original {
                    GetConsoleOriginalTitleA(data.as_mut_ptr(), capacity)
                } else {
                    GetConsoleTitleA(data.as_mut_ptr(), capacity)
                }
            };
            report.push_str(&format!(
                "{label}:original={original};ansi-cap={capacity};ret={count},{};data={}\n",
                unsafe { GetLastError() },
                hex(&data[..12])
            ));
        }
    }
}
fn stage(output: HANDLE, path: &Path, n: usize, report: &mut String) -> io::Result<()> {
    query(&format!("stage{n}"), report);
    std::fs::write(path, report)?;
    let marker = wide(&format!("ttl-stage-{n}:"));
    let mut count = 0;
    checked(unsafe {
        WriteConsoleOutputCharacterW(
            output,
            marker.as_ptr(),
            (marker.len() - 1) as u32,
            COORD { X: 0, Y: 7 },
            &mut count,
        )
    })?;
    if count as usize != marker.len() - 1 {
        return Err(io::Error::other("short title fence"));
    }
    wait_stage(path, n)
}
pub(super) fn run(path: &Path) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(output, 7) })?;
    checked(unsafe { SetConsoleOutputCP(65001) })?;
    bytes(output, b"ROOT")?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    stage(output, path, 0, &mut report)?;
    set_w("native-中😀")?;
    stage(output, path, 1, &mut report)?;
    bytes(output, b"\x1b[3")?;
    set_w("during-fragment")?;
    stage(output, path, 2, &mut report)?;
    bytes(output, b"1mZ")?;
    stage(output, path, 3, &mut report)?;
    bytes(output, b"\x1b]2;vt-active\x07")?;
    stage(output, path, 4, &mut report)?;
    let other = create(0xc000_0000, 3)?;
    checked(unsafe { SetConsoleMode(other.as_raw_handle(), 7) })?;
    bytes(other.as_raw_handle(), b"\x1b]2;vt-inactive\x07HIDDEN")?;
    stage(output, path, 5, &mut report)?;
    checked(unsafe { SetConsoleCP(936) })?;
    let ansi = [b'A', 0xd6, 0xd0, 0];
    unsafe {
        SetLastError(0);
    }
    let ok = unsafe { SetConsoleTitleA(ansi.as_ptr()) };
    let error = unsafe { GetLastError() };
    report.push_str(&format!("set-936={ok},{error}\n"));
    stage(output, path, 6, &mut report)?;
    checked(unsafe { SetConsoleCP(65001) })?;
    let utf8 = b"B\xe4\xb8\xad\0";
    unsafe {
        SetLastError(0);
    }
    let ok = unsafe { SetConsoleTitleA(utf8.as_ptr()) };
    let error = unsafe { GetLastError() };
    report.push_str(&format!("set-65001={ok},{error}\n"));
    stage(output, path, 7, &mut report)?;
    set_w("")?;
    stage(output, path, 8, &mut report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(other.as_raw_handle()) })?;
    bytes(other.as_raw_handle(), b"!")?;
    stage(other.as_raw_handle(), path, 9, &mut report)?;
    set_w("A\x07\x1b]2;BAD")?;
    stage(other.as_raw_handle(), path, 10, &mut report)?;
    checked(unsafe { SetConsoleTitleA(c"".as_ptr().cast()) })?;
    stage(other.as_raw_handle(), path, 11, &mut report)?;
    let invalid = [0xc2, b'Q', 0];
    checked(unsafe { SetConsoleTitleA(invalid.as_ptr()) })?;
    stage(other.as_raw_handle(), path, 12, &mut report)?;
    Ok(())
}
