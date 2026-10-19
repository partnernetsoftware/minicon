//! Fenced long-output observations with independently sized buffer and peer.
use super::{
    buffers::{checked, position, stage},
    mixed_output::bytes,
};
use std::{io, path::Path};
use windows_sys::Win32::System::Console::*;

pub(super) fn run(path: &Path, mode: u32, unicode: bool) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(output, mode) })?;
    checked(unsafe { SetConsoleScreenBufferSize(output, COORD { X: 24, Y: 16 }) })?;
    let mut report = format!(
        "build={};mode={mode};unicode={unicode}\n",
        super::output_modes::windows_build()?
    );
    for n in 0..6 {
        let text = match n {
            0 | 5 => "01234567890123456789".to_owned(),
            1 => "\r\nL01\r\nL02\r\nL03\r\nL04\r\nL05\r\n".to_owned(),
            2 => (6..20).map(|line| format!("L{line:02}\r\n")).collect(),
            3 => {
                position(output, 0, 0)?;
                "R".to_owned()
            }
            4 => "\r\nBACK\r\n".to_owned(),
            _ => unreachable!(),
        };
        if unicode {
            let units: Vec<_> = text.encode_utf16().collect();
            let mut count = 0;
            checked(unsafe {
                WriteConsoleW(
                    output,
                    units.as_ptr().cast(),
                    units.len() as u32,
                    &mut count,
                    std::ptr::null(),
                )
            })?;
            if count as usize != units.len() {
                return Err(io::Error::other("short viewport W write"));
            }
        } else {
            bytes(output, text.as_bytes())?;
        }
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        checked(unsafe { GetConsoleScreenBufferInfo(output, &mut info) })?;
        report.push_str(&format!(
            "stage{n}:view={},{},{},{};cursor={},{}\n",
            info.srWindow.Left,
            info.srWindow.Top,
            info.srWindow.Right,
            info.srWindow.Bottom,
            info.dwCursorPosition.X,
            info.dwCursorPosition.Y
        ));
        for row in [0, 1, 2, 15] {
            let mut units = [0; 24];
            let mut count = 0;
            checked(unsafe {
                ReadConsoleOutputCharacterW(
                    output,
                    units.as_mut_ptr(),
                    units.len() as u32,
                    COORD { X: 0, Y: row },
                    &mut count,
                )
            })?;
            report.push_str(&format!(
                "stage{n}:row{row}={:?}\n",
                String::from_utf16_lossy(&units[..count as usize]).trim_end_matches(' ')
            ));
        }
        checked(unsafe { SetConsoleMode(output, 7) })?;
        stage(output, path, n, &report)?;
        checked(unsafe { SetConsoleMode(output, mode) })?;
    }
    Ok(())
}
