//! Native SDK caller: flush records without cancelling reads or cached text.
use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError};
use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

fn checked(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn key(character: u16) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        Event: INPUT_RECORD_0 {
            KeyEvent: KEY_EVENT_RECORD {
                bKeyDown: 1,
                wRepeatCount: 1,
                wVirtualKeyCode: 0,
                wVirtualScanCode: 0,
                uChar: KEY_EVENT_RECORD_0 {
                    UnicodeChar: character,
                },
                dwControlKeyState: 0,
            },
        },
    }
}

fn put(input: HANDLE, records: &[INPUT_RECORD]) -> io::Result<()> {
    let mut count = 0;
    // SAFETY: caller-owned live console and initialized record slice.
    checked(unsafe {
        WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count)
    })?;
    if count as usize != records.len() {
        return Err(io::Error::other("short input write"));
    }
    Ok(())
}

fn queued(input: HANDLE) -> io::Result<u32> {
    let mut count = 0;
    // SAFETY: live console handle and initialized count output.
    checked(unsafe { GetNumberOfConsoleInputEvents(input, &mut count) })?;
    Ok(count)
}

fn flush(input: HANDLE, raw: bool) -> io::Result<()> {
    // SAFETY: SDK operations on this fixture's retained console handles only.
    checked(unsafe {
        if raw {
            FlushFileBuffers(input)
        } else {
            FlushConsoleInputBuffer(input)
        }
    })
}

fn wide(input: HANDLE, capacity: usize) -> io::Result<Vec<u16>> {
    let mut units = vec![0; capacity];
    let mut count = 0;
    // SAFETY: initialized output with the exact requested unit capacity.
    checked(unsafe {
        ReadConsoleW(
            input,
            units.as_mut_ptr().cast(),
            capacity as u32,
            &mut count,
            std::ptr::null(),
        )
    })?;
    units.truncate(count as usize);
    Ok(units)
}

pub(super) fn run(mode: &str, path: &Path) -> io::Result<()> {
    let raw = mode.ends_with("-raw");
    // SAFETY: standard console handles live for this child's lifetime.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(input, 0) })?;
    // Drain startup records without depending on the API under test.
    while queued(input)? != 0 {
        let mut records = [INPUT_RECORD::default(); 32];
        let mut count = 0;
        checked(unsafe {
            ReadConsoleInputW(
                input,
                records.as_mut_ptr(),
                records.len() as u32,
                &mut count,
            )
        })?;
    }
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    if mode.contains("pressure") {
        put(input, &[key(65)])?;
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        checked(unsafe { SetConsoleMode(output, 15) })?;
        let units = [u16::from(b'P'); 8192];
        // Initial title plus eight chunks exceed the adapter's 64 KiB ring.
        // The eighth write leaves staging pending before the following flush.
        for _ in 0..8 {
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
                return Err(io::Error::other("short pressure write"));
            }
        }
        flush(input, raw)?;
        report.push_str(&format!("flushed={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        std::fs::write(path.with_extension("ready"), &report)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        while !path.with_extension("go").exists() {
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    } else if mode.contains("records") {
        let records = [
            key(65),
            INPUT_RECORD {
                EventType: FOCUS_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    FocusEvent: FOCUS_EVENT_RECORD { bSetFocus: 1 },
                },
            },
            INPUT_RECORD {
                EventType: WINDOW_BUFFER_SIZE_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    WindowBufferSizeEvent: WINDOW_BUFFER_SIZE_RECORD {
                        dwSize: COORD { X: 37, Y: 9 },
                    },
                },
            },
        ];
        put(input, &records)?;
        report.push_str(&format!("before={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        let read_only = super::console_file::open("CONIN$", 0x8000_0000)?;
        let write_only = super::console_file::open("CONIN$", 0x4000_0000)?;
        for (name, handle) in [
            ("read-only", read_only.as_raw_handle()),
            ("output", unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }),
            ("invalid", INVALID_HANDLE_VALUE),
        ] {
            unsafe { SetLastError(0) };
            let ok = unsafe {
                if raw {
                    FlushFileBuffers(handle)
                } else {
                    FlushConsoleInputBuffer(handle)
                }
            };
            let error = unsafe { GetLastError() };
            report.push_str(&format!("{name}={ok},{error},{}\n", queued(input)?));
        }
        flush(write_only.as_raw_handle(), raw)?;
        report.push_str(&format!("flushed={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        flush(input, raw)?;
        report.push_str(&format!("empty={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        put(input, &[key(90)])?;
        let mut records = [INPUT_RECORD::default(); 2];
        let mut count = 0;
        checked(unsafe { PeekConsoleInputW(input, records.as_mut_ptr(), 2, &mut count) })?;
        report.push_str(&format!("new={count},{},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        report.push_str(&format!("read={:?}\n", wide(input, 1)?));
    } else if mode.contains("pending") {
        checked(unsafe { SetConsoleMode(input, 3) })?;
        put(input, &[key(65), key(66)])?;
        let reader = super::console_file::open("CONIN$", 0xc000_0000)?;
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = wide(reader.as_raw_handle(), 16);
            let _ = send.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(8);
        while queued(input)? != 0 {
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        flush(input, raw)?;
        if receive.try_recv().is_ok() {
            return Err(io::Error::other("flush completed pending read"));
        }
        report.push_str(&format!("pending={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        put(input, &[key(67), key(13)])?;
        let text = receive
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| io::ErrorKind::TimedOut)??;
        report.push_str(&format!("read={text:?}\n"));
    } else {
        checked(unsafe { SetConsoleCP(65001) })?;
        let mut file = super::console_file::open("CONIN$", 0xc000_0000)?;
        let is_wide = mode.contains("wide");
        checked(unsafe { SetConsoleMode(input, if is_wide { 3 } else { 0 }) })?;
        put(
            input,
            &if is_wide {
                vec![key(65), key(66), key(13)]
            } else {
                vec![key(0x4e2d)]
            },
        )?;
        let first = if is_wide {
            wide(file.as_raw_handle(), 1)?[0]
        } else {
            let mut first = [0];
            file.read_exact(&mut first)?;
            u16::from(first[0])
        };
        report.push_str(&format!("first={first},{}\n", queued(input)?));
        flush(input, raw)?;
        report.push_str(&format!("flushed={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
        if is_wide {
            report.push_str(&format!("tail={:?}\n", wide(file.as_raw_handle(), 16)?));
        } else {
            put(input, &[key(90)])?;
            let mut tail = [0; 16];
            let count = file.read(&mut tail)?;
            report.push_str(&format!("tail={:?}\n", &tail[..count]));
        }
        report.push_str(&format!("after={},{}\n", queued(input)?, unsafe {
            WaitForSingleObject(input, 0)
        }));
    }
    std::fs::write(path, report)
}
