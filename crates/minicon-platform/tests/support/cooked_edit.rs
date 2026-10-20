//! Independent SDK client for silent cooked editing and an actual cmd reader.
use super::mixed_output::{bytes, position};
use std::{io, path::Path};
use windows_sys::Win32::System::Console::*;

fn key(vk: u16, ch: u16, controls: u32, repeat: u16) -> INPUT_RECORD {
    let mut record = INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        ..INPUT_RECORD::default()
    };
    record.Event.KeyEvent.bKeyDown = 1;
    record.Event.KeyEvent.wRepeatCount = repeat;
    record.Event.KeyEvent.wVirtualKeyCode = vk;
    record.Event.KeyEvent.uChar.UnicodeChar = ch;
    record.Event.KeyEvent.dwControlKeyState = controls;
    record
}
fn text(value: &str) -> Vec<INPUT_RECORD> {
    value.encode_utf16().map(|ch| key(0, ch, 0, 1)).collect()
}
fn press(vk: u16) -> INPUT_RECORD {
    key(vk, if vk == 27 { 27 } else { 0 }, 0, 1)
}

fn put(input: windows_sys::Win32::Foundation::HANDLE, records: &[INPUT_RECORD]) -> io::Result<()> {
    let mut count = 0;
    // SAFETY: initialized bounded records and live written-count buffer.
    if unsafe { WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count) } == 0
        || count as usize != records.len()
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn run(kind: &str, path: &Path) -> io::Result<()> {
    let (echo, kind) = kind
        .strip_prefix("echo-")
        .map_or((false, kind), |kind| (true, kind));
    let mut mode = 0xa3;
    let mut initial = Vec::new();
    let mut wakeup = 0;
    let mut records = match kind {
        "insert" | "overwrite" | "cmd" | "powershell" | "implicit-insert" | "inherit-overwrite" => {
            if matches!(kind, "overwrite" | "inherit-overwrite") {
                mode = 0x83;
            } else if kind == "implicit-insert" {
                mode = 3;
            }
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 2), key(0, 88, 0, 1)]);
            r
        }
        "toggle" => {
            mode = 0x83;
            let mut r = text("abcd");
            r.extend([
                key(0x25, 0, 0, 2),
                press(0x2d),
                key(0, 88, 0, 1),
                press(0x2d),
                key(0, 89, 0, 1),
            ]);
            r
        }
        "delete" => {
            let mut r = text("abcd");
            r.extend([press(0x24), press(0x27), press(0x2e)]);
            r
        }
        "end" => {
            let mut r = text("abcd");
            r.extend([
                press(0x24),
                press(0x27),
                press(0x2e),
                press(0x23),
                key(0, 88, 0, 1),
            ]);
            r
        }
        "backspace" => {
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 2), key(8, 8, 0, 1), key(0, 88, 0, 1)]);
            r
        }
        "escape" | "escape-masked" => {
            if kind == "escape-masked" {
                wakeup = 1 << 27;
            }
            let mut r = text("abc");
            r.extend([press(27), key(0, 88, 0, 1)]);
            r
        }
        "initial" => {
            initial = "ab".encode_utf16().collect();
            vec![press(0x25), key(0, 88, 0, 1)]
        }
        "enter-middle" => {
            let mut r = text("abc");
            r.push(press(0x25));
            r
        }
        "unicode-motion" => {
            let mut r = text("A中😀e\u{301}Z");
            r.extend([
                press(0x24),
                key(0x27, 0, 0, 3),
                press(0x2e),
                key(8, 8, 0, 1),
                press(0x27),
                key(0, 88, 0, 1),
            ]);
            r
        }
        "unicode-overwrite" => {
            mode = 0x83;
            let mut r = text("A😀e\u{301}Z");
            r.extend([press(0x24), press(0x27), key(0, 88, 0, 1)]);
            r
        }
        "pair-overwrite" => {
            mode = 0x83;
            let mut r = text("abcd");
            r.extend([press(0x24), press(0x27)]);
            r.extend(text("😀"));
            r
        }
        "invalid" => vec![
            key(0, 65, 0, 1),
            key(0, 0xd800, 0, 1),
            key(0, 66, 0, 1),
            press(0x24),
            press(0x27),
            press(0x2e),
        ],
        "invalid-retained" => vec![
            key(0, 0xd800, 0, 1),
            key(0, 65, 0, 1),
            key(0, 0xdc00, 0, 1),
            press(0x24),
            press(0x27),
            key(0, 88, 0, 1),
        ],
        "f1" => {
            let mut r = text("abc");
            r.extend([press(0x24), press(0x70), key(0, 88, 0, 1)]);
            r
        }
        "edges" => {
            let mut r = vec![key(0x25, 0, 0, 2), press(0x2e), press(0x23)];
            r.extend(text("ab"));
            r.extend([
                key(0x27, 0, 0, 2),
                press(0x24),
                key(0x25, 0, 0, 2),
                press(0x2e),
                press(0x23),
                key(0x27, 0, 0, 2),
                press(0x2e),
                key(0, 88, 0, 1),
            ]);
            r
        }
        "word-wire-left" | "word-wire-right" => Vec::new(),
        "word-left"
        | "word-right"
        | "word-repeat"
        | "word-punctuation"
        | "word-unicode"
        | "word-tab"
        | "word-edges"
        | "word-punctuation-right"
        | "word-unicode-left"
        | "word-tab-left" => {
            let value = match kind {
                "word-punctuation" | "word-punctuation-right" => "ab.cd/ef  gh",
                "word-unicode" | "word-unicode-left" => "中😀  e\u{301} Z",
                "word-tab" | "word-tab-left" => "ab\tcd  ef",
                "word-edges" => "",
                _ => "one  two three  ",
            };
            let mut r = text(value);
            match kind {
                "word-right" => r.extend([press(0x24), key(0x27, 0, 4, 2)]),
                "word-repeat" => r.push(key(0x25, 0, 8, 2)),
                "word-punctuation" | "word-unicode-left" | "word-tab-left" => {
                    r.push(key(0x25, 0, 8, 2));
                }
                "word-punctuation-right" => r.extend([press(0x24), key(0x27, 0, 4, 1)]),
                "word-edges" => r.extend([key(0x25, 0, 8, 2), key(0x27, 0, 4, 2)]),
                _ => r.push(key(0x25, 0, 8, 1)),
            }
            r.push(key(0, 88, 0, 1));
            r
        }
        "ctrl-home" => {
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 2), key(0x24, 0, 8, 1), key(0, 88, 0, 1)]);
            r
        }
        "ctrl-end" => {
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 2), key(0x23, 0, 8, 1), key(0, 88, 0, 1)]);
            r
        }
        "repeat" => {
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 3), key(0, 88, 0, 1)]);
            r
        }
        "masked-middle" => {
            wakeup = 1 << 9;
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 2), key(9, 9, 16, 1)]);
            r
        }
        "literal-bs" => {
            mode = 0xa2;
            let mut r = text("ab");
            r.push(key(8, 8, 0, 1));
            r.extend(text("c"));
            r
        }
        "escape-alt" => {
            let mut r = text("ab");
            r.extend([key(27, 27, 1, 1), key(0, 88, 0, 1)]);
            r
        }
        "cross-row" => {
            let mut r = text("abcdefghijklmnopqrstu");
            r.extend([
                press(0x24),
                key(0x27, 0, 0, 16),
                press(0x2e),
                key(8, 8, 0, 1),
                key(0, 88, 0, 1),
                press(0x23),
            ]);
            r
        }
        "tab-erase" => {
            let mut r = text("ab\tCD");
            r.extend([
                press(0x24),
                key(0x27, 0, 0, 3),
                press(0x2e),
                key(8, 8, 0, 1),
                key(0, 88, 0, 1),
            ]);
            r
        }
        "control-erase" => {
            let mut r = text("a\u{1}b");
            r.extend([
                press(0x24),
                key(0x27, 0, 0, 2),
                key(8, 8, 0, 1),
                key(0, 88, 0, 1),
            ]);
            r
        }
        "wide-wrap" => {
            let mut r = text("abcdefghijklmn中Z");
            r.extend([press(0x25), key(8, 8, 0, 1), key(0, 88, 0, 1)]);
            r
        }
        "scroll-home" => {
            let mut r = text(&"a".repeat(150));
            r.extend([press(0x24), key(0, 88, 0, 1), press(0x23), key(8, 8, 0, 1)]);
            r
        }
        "initial-tab" => {
            initial = "ab\tc".encode_utf16().collect();
            vec![press(0x24), press(0x2e), key(0, 88, 0, 1), press(0x23)]
        }
        "initial-wide" => {
            initial = "中😀ab".encode_utf16().collect();
            vec![
                press(0x24),
                key(0x27, 0, 0, 2),
                key(8, 8, 0, 1),
                key(0, 88, 0, 1),
                press(0x23),
            ]
        }
        "pending-csi" => {
            let mut r = text("abcd");
            r.extend([key(0x25, 0, 0, 2), key(0, 88, 0, 1)]);
            r
        }
        "legacy-wrap" => text("abcdefghijklmno"),
        "tail-high" => {
            let mut r = text("ab");
            r.push(key(0, 0xd83d, 0, 1));
            r
        }
        "narrow" => {
            let mut r = text("abc");
            r.extend([
                press(0x24),
                press(0x27),
                press(0x2e),
                key(8, 8, 0, 1),
                key(0, 88, 0, 1),
            ]);
            r
        }
        "no-wrap" => {
            let mut r = text("abcdefghijklmnopqr");
            r.extend([key(0x25, 0, 0, 2), key(0, 88, 0, 1)]);
            r
        }
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    if echo {
        mode |= 4;
    }
    records.push(key(13, 13, 16, 1));
    // SAFETY: this isolated SDK client's console handles.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(input, mode) } == 0
        || unsafe { SetConsoleMode(output, 15) } == 0
        || unsafe { SetConsoleOutputCP(65001) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if kind == "inherit-overwrite"
        && unsafe { SetConsoleMode(input, if echo { 7 } else { 3 }) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"\x1b[2J\x1b[H>")?;
    if !initial.is_empty() {
        bytes(output, String::from_utf16_lossy(&initial).as_bytes())?;
    }
    let mut count = 0;
    if !matches!(kind, "powershell" | "word-wire-left" | "word-wire-right") {
        put(input, &records)?;
    }
    if echo || kind == "cmd" {
        bytes(output, b"\x1b]2;cooked-echo-begin:\x07")?;
    }
    if matches!(kind, "word-wire-left" | "word-wire-right") {
        // Keep readiness as the final title until the parent supplies input:
        // ConPTY may coalesce consecutive title changes into the latest one.
        bytes(output, b"\x1b]2;word-wire-ready:\x07")?;
    }
    if kind == "pending-csi" {
        bytes(output, b"\x1b[3")?;
    }
    if matches!(kind, "legacy-wrap" | "no-wrap")
        && unsafe { SetConsoleMode(output, if kind == "legacy-wrap" { 3 } else { 5 }) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    if kind == "powershell" {
        let script = path.with_extension("ps1");
        let ready = path.with_extension("client-ready");
        std::fs::write(
            &script,
            r#"Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SilentInput {
    [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr GetStdHandle(int h);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleMode(IntPtr h, uint mode);
}
'@
if (-not [SilentInput]::SetConsoleMode([SilentInput]::GetStdHandle(-10), 0xa3)) { exit 3 }
[IO.File]::WriteAllText($args[0], 'ready')
$line = [Console]::ReadLine()
[Console]::WriteLine('READ=' + $line)
exit 0
"#,
        )?;
        let image = std::env::var_os("MINICON_TEST_WINDOWS_POWERSHELL")
            .ok_or_else(|| io::Error::other("MINICON_TEST_WINDOWS_POWERSHELL is required"))?;
        let mut child = std::process::Command::new(image)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .arg(&ready)
            .spawn()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while !ready.exists() {
            if child.try_wait()?.is_some() {
                return Err(io::Error::other("PowerShell exited before read readiness"));
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        put(input, &records)?;
        report.push_str(&format!("powershell-exit:{:?}\n", child.wait()?.code()));
    } else if kind == "cmd" {
        use std::os::windows::process::CommandExt;
        let image = std::path::PathBuf::from(
            std::env::var_os("SystemRoot").ok_or(io::ErrorKind::NotFound)?,
        )
        .join("System32")
        .join("cmd.exe");
        let child = std::process::Command::new(image)
            .args(["/d", "/c"])
            .raw_arg("set /p MINICON_EDIT= & set MINICON_EDIT")
            .stdin(std::process::Stdio::inherit())
            .env_remove("MINICON_EDIT")
            .status()?;
        report.push_str(&format!("cmd-exit:{:?}\n", child.code()));
    } else {
        let mut buffer = [0_u16; 512];
        buffer[..initial.len()].copy_from_slice(&initial);
        let mut control = CONSOLE_READCONSOLE_CONTROL {
            nLength: std::mem::size_of::<CONSOLE_READCONSOLE_CONTROL>() as u32,
            nInitialChars: initial.len() as u32,
            dwCtrlWakeupMask: wakeup,
            dwControlKeyState: 0,
        };
        // SAFETY: bounded initialized UTF-16 output and SDK control/count buffers.
        if unsafe {
            ReadConsoleW(
                input,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut count,
                std::ptr::addr_of_mut!(control),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if count as usize > buffer.len() {
            return Err(io::Error::other("oversized read reply"));
        }
        report.push_str("read:");
        for unit in &buffer[..count as usize] {
            report.push_str(&format!("{unit:04x},"));
        }
        report.push_str(&format!("\ncontrols:{:08x}\n", control.dwControlKeyState));
    }
    let mut queued = 0;
    // SAFETY: initialized output count and private handle.
    if unsafe { GetNumberOfConsoleInputEvents(input, &mut queued) } == 0 {
        return Err(io::Error::last_os_error());
    }
    report.push_str(&format!("queued:{queued}\n"));
    if kind.starts_with("word-wire-") && queued != 0 {
        let mut pending = [INPUT_RECORD::default(); 8];
        let mut count = 0;
        // SAFETY: fixed initialized SDK record array and live count buffer.
        if unsafe { PeekConsoleInputW(input, pending.as_mut_ptr(), 8, &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        for record in &pending[..count as usize] {
            if record.EventType == KEY_EVENT as u16 {
                // SAFETY: KEY_EVENT tag selects the initialized key union.
                let key = unsafe { record.Event.KeyEvent };
                report.push_str(&format!(
                    "pending-key:{},{},{:04x},{:08x}\n",
                    key.bKeyDown,
                    key.wVirtualKeyCode,
                    unsafe { key.uChar.UnicodeChar },
                    key.dwControlKeyState
                ));
            } else {
                report.push_str(&format!("pending-event:{}\n", record.EventType));
            }
        }
    }
    if kind == "pending-csi" {
        bytes(output, b"1m!")?;
    }
    position(output, "cursor", &mut report)?;
    std::fs::write(path, report)?;
    if matches!(kind, "legacy-wrap" | "no-wrap") && unsafe { SetConsoleMode(output, 15) } == 0 {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"\x1b]2;cooked-edit-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
