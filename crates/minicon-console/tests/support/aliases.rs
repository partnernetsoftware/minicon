//! Independent native SDK client: no terminal-library dependency or private API.
use super::queued_reads::{check, publish, raw_key, raw_write, wait};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, HANDLE, SetLastError},
    System::Console::*,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
fn key(unit: u16, repeat: u16) -> INPUT_RECORD {
    let mut record = raw_key(unit, repeat);
    record.Event.KeyEvent.wVirtualKeyCode = if (97..=122).contains(&unit) {
        unit - 32
    } else {
        unit
    };
    record
}
fn read(input: HANDLE, api: &str, capacity: u32) -> io::Result<Vec<u16>> {
    let mut units = [0_u16; 128];
    let mut bytes = [0_u8; 128];
    let mut count = 0;
    require(capacity <= 128, "fixture read capacity")?;
    let ok = unsafe {
        if api == "wide" {
            ReadConsoleW(
                input,
                units.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            )
        } else {
            ReadConsoleA(
                input,
                bytes.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            )
        }
    };
    check(ok)?;
    require(count <= capacity, ("read length", count, capacity))?;
    // GetLastError is unspecified after a successful ReadConsole. Data and
    // count are the oracle; failure paths separately verify the native error.
    Ok(if api == "wide" {
        units[..count as usize].to_vec()
    } else {
        bytes[..count as usize]
            .iter()
            .copied()
            .map(u16::from)
            .collect()
    })
}
fn require(ok: bool, detail: impl std::fmt::Debug) -> io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(format!("alias SDK: {detail:?}")))
    }
}
fn add(source: &str, target: &str, exe: &str) -> io::Result<()> {
    check(unsafe {
        AddConsoleAliasW(
            wide(source).as_ptr(),
            wide(target).as_ptr(),
            wide(exe).as_ptr(),
        )
    })
}
fn get(source: &str, exe: &str, expected: &str) -> io::Result<()> {
    let mut data = [0xaaaa; 128];
    let count = unsafe {
        GetConsoleAliasW(
            wide(source).as_ptr(),
            data.as_mut_ptr(),
            256,
            wide(exe).as_ptr(),
        )
    };
    let expected = wide(expected);
    require(
        count as usize == expected.len() * 2 && data[..expected.len()] == expected,
        (
            source,
            exe,
            count,
            unsafe { GetLastError() },
            &data[..expected.len()],
        ),
    )
}
fn table() -> io::Result<()> {
    add("MiXeD", "Echo $1", "One.EXE")?;
    add("mixed", "Other", "two.exe")?;
    get("MIXED", "ONE.exe", "Echo $1")?;
    get("mixed", "two.exe", "Other")?;
    add("mixed", "Replaced", "ONE.EXE")?;
    get("mixed", "one.exe", "Replaced")?;
    unsafe {
        let mut data = [0xaaaa; 128];
        SetLastError(0);
        let n = GetConsoleAliasW(
            wide("mixed").as_ptr(),
            data.as_mut_ptr(),
            4,
            wide("one.exe").as_ptr(),
        );
        require(
            n == 4 && GetLastError() == 122,
            ("short W", n, GetLastError(), data[0]),
        )?;
        let mut bytes = [0xaa; 128];
        SetLastError(0);
        let n = GetConsoleAliasA(
            c"mixed".as_ptr().cast(),
            bytes.as_mut_ptr(),
            2,
            c"one.exe".as_ptr().cast(),
        );
        require(
            n == 4 && GetLastError() == 122,
            ("short A", n, GetLastError(), bytes[0]),
        )?;
        let expected = wide("mixed=Replaced");
        let n = GetConsoleAliasesLengthW(wide("ONE.exe").as_ptr());
        require(n as usize == expected.len() * 2, ("length W", n))?;
        let n = GetConsoleAliasesW(data.as_mut_ptr(), n, wide("one.exe").as_ptr());
        require(
            n as usize == expected.len() * 2 && data[..expected.len()] == expected,
            ("list W", n),
        )?;
        SetLastError(0);
        let n = GetConsoleAliasesW(data.as_mut_ptr(), 2, wide("one.exe").as_ptr());
        require(
            n == 0 && GetLastError() == 111,
            ("short list", n, GetLastError()),
        )?;
        SetLastError(0);
        let ok = AddConsoleAliasW(
            wide("").as_ptr(),
            wide("x").as_ptr(),
            wide("one.exe").as_ptr(),
        );
        require(
            ok == 0 && GetLastError() == 87,
            ("empty source", ok, GetLastError()),
        )?;
    }
    add("mixed", "", "one.exe")?;
    unsafe {
        let mut data = [0xaaaa; 128];
        SetLastError(0);
        let n = GetConsoleAliasW(
            wide("mixed").as_ptr(),
            data.as_mut_ptr(),
            256,
            wide("one.exe").as_ptr(),
        );
        require(
            n == 0 && GetLastError() == 31,
            ("deleted", n, GetLastError(), data[0]),
        )?;
        require(
            GetConsoleAliasesLengthW(wide("one.exe").as_ptr()) == 0,
            "empty table",
        )?;
        let n = GetConsoleAliasExesLengthW();
        let expected = wide("one.exe")
            .into_iter()
            .chain(wide("two.exe"))
            .collect::<Vec<_>>();
        require(n as usize == expected.len() * 2, ("exe length", n))?;
        let n = GetConsoleAliasExesW(data.as_mut_ptr(), n);
        let mut names = data[..n as usize / 2]
            .split(|u| *u == 0)
            .filter(|v| !v.is_empty())
            .map(<[u16]>::to_vec)
            .collect::<Vec<_>>();
        names.sort();
        require(
            names == vec![wide("one.exe")[..7].to_vec(), wide("two.exe")[..7].to_vec()],
            ("exe list", n, names),
        )?;
        SetLastError(0);
        let n = GetConsoleAliasExesW(data.as_mut_ptr(), 2);
        require(
            n == 0 && GetLastError() == 111,
            ("short exes", n, GetLastError()),
        )?;
    }
    // Exercise all six A APIs using the input code page, including multibyte lengths.
    for (cp, source, target) in [
        (1252, vec![233], vec![224, 88]),
        (936, vec![0xd6, 0xd0], vec![0xce, 0xc4, 88]),
        (65001, vec![0xe4, 0xb8, 0xad], vec![0xc3, 0xa9, 88]),
    ] {
        check(unsafe { SetConsoleCP(cp) })?;
        let mut s = source.clone();
        s.push(0);
        let mut t = target.clone();
        t.push(0);
        unsafe {
            check(AddConsoleAliasA(
                s.as_ptr(),
                t.as_ptr(),
                c"ansi.exe".as_ptr().cast(),
            ))?;
            let mut bytes = [0xaa; 128];
            let n = GetConsoleAliasA(
                s.as_ptr(),
                bytes.as_mut_ptr(),
                128,
                c"ANSI.EXE".as_ptr().cast(),
            );
            require(
                n as usize == t.len() && bytes[..t.len()] == t,
                ("get A", cp, n, &bytes[..t.len()]),
            )?;
            let expected = source
                .into_iter()
                .chain([61])
                .chain(target)
                .chain([0])
                .collect::<Vec<_>>();
            let n = GetConsoleAliasesLengthA(c"ansi.exe".as_ptr().cast());
            require(n as usize == expected.len(), ("length A", cp, n))?;
            let n = GetConsoleAliasesA(bytes.as_mut_ptr(), n, c"ansi.exe".as_ptr().cast());
            require(
                n as usize == expected.len() && bytes[..expected.len()] == expected,
                ("list A", cp, n),
            )?;
            let n = GetConsoleAliasExesLengthA();
            require(n == 25, ("exe length A", n))?;
            let n = GetConsoleAliasExesA(bytes.as_mut_ptr(), n);
            let mut names = bytes[..n as usize]
                .split(|u| *u == 0)
                .filter(|v| !v.is_empty())
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>();
            names.sort();
            require(
                names
                    == vec![
                        b"ansi.exe".to_vec(),
                        b"one.exe".to_vec(),
                        b"two.exe".to_vec(),
                    ],
                ("exes A", n, names),
            )?;
            check(AddConsoleAliasA(
                s.as_ptr(),
                c"".as_ptr().cast(),
                c"ansi.exe".as_ptr().cast(),
            ))?;
        }
    }
    Ok(())
}
fn cooked(kind: &str) -> io::Result<()> {
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let image = std::env::current_exe()?;
    let exe = image.file_name().unwrap().to_string_lossy();
    let ansi = kind.ends_with("ansi");
    check(unsafe { SetConsoleCP(65001) })?;
    check(unsafe {
        SetConsoleMode(
            input,
            if kind == "silent" {
                3
            } else if kind == "raw" {
                1
            } else {
                7
            },
        )
    })?;
    let multiline = kind.starts_with("multiline") || kind.starts_with("split");
    let target = if kind == "literal" {
        "first\nsecond"
    } else if multiline {
        "first $1$Tsecond $*"
    } else {
        "$9:$1:$2:$*: $L$G$B $$ $Q $"
    };
    add("shortcut", target, &exe)?;
    add("shortcut", "WRONG EXE", "other.exe")?;
    let command = if kind == "arguments" {
        "shortcut 1 2 3 4 5 6 7 8 9 ten\r"
    } else if kind == "leading" {
        " shortcut a b  \r"
    } else {
        "shortcut a b  \r"
    };
    raw_write(
        input,
        &command
            .encode_utf16()
            .map(|u| key(u, 1))
            .collect::<Vec<_>>(),
    )?;
    let api = if ansi { "ansi" } else { "wide" };
    let expected = if kind == "literal" {
        "first\nsecond\r\n".to_owned()
    } else if kind == "arguments" {
        "9:1:2:1 2 3 4 5 6 7 8 9 ten: <>| $$ $Q $\r\n".to_owned()
    } else if matches!(kind, "silent" | "raw" | "leading") {
        command.replace('\r', if kind == "raw" { "\r" } else { "\r\n" })
    } else if multiline {
        "first a\r\nsecond a b  \r\n".to_owned()
    } else {
        ":a:b:a b  : <>| $$ $Q $\r\n".to_owned()
    };
    let mut actual = Vec::new();
    if kind.starts_with("split") {
        let first = read(input, api, 3)?;
        require(first == vec![102, 105, 114], ("split first", &first))?;
        actual.extend(first);
    }
    let first = read(input, api, 128)?;
    if multiline {
        let want = if kind.starts_with("split") {
            "st a\r\n"
        } else {
            "first a\r\n"
        };
        require(
            first == want.encode_utf16().collect::<Vec<_>>(),
            ("line boundary", &first),
        )?;
    }
    actual.extend(first);
    if multiline {
        // Completed alias continuation must precede fresh raw input on this file.
        check(unsafe { SetConsoleMode(input, 1) })?;
        raw_write(input, &[raw_key(90, 1)])?;
        actual.extend(read(input, api, 128)?);
        let fresh = read(input, api, 128)?;
        require(fresh == vec![90], ("fresh input", fresh))?;
    }
    require(
        actual == expected.encode_utf16().collect::<Vec<_>>(),
        (kind, actual, expected),
    )
}
fn bounds() -> io::Result<()> {
    for n in 0..64 {
        add("s", "x", &format!("exe{n}"))?;
    }
    unsafe {
        SetLastError(0);
        let ok = AddConsoleAliasW(
            wide("s").as_ptr(),
            wide("x").as_ptr(),
            wide("overflow").as_ptr(),
        );
        require(
            ok == 0 && GetLastError() == 8,
            ("exe budget", ok, GetLastError()),
        )?;
    }
    add("s", "still-live", "exe0")?;
    get("s", "exe0", "still-live")
}
fn budget() -> io::Result<()> {
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    check(unsafe { SetConsoleMode(input, 7) })?;
    let image = std::env::current_exe()?;
    add(
        "huge",
        &"$1".repeat(10000),
        &image.file_name().unwrap().to_string_lossy(),
    )?;
    raw_write(
        input,
        &"huge four\r"
            .encode_utf16()
            .map(|u| key(u, 1))
            .collect::<Vec<_>>(),
    )?;
    unsafe {
        let mut data = [0u16; 32];
        let mut count = 0;
        SetLastError(0);
        let ok = ReadConsoleW(
            input,
            data.as_mut_ptr().cast(),
            32,
            &mut count,
            std::ptr::null_mut(),
        );
        require(
            ok == 0 && GetLastError() == 8 && count == 0,
            ("expansion budget", ok, GetLastError(), count),
        )?;
    }
    check(unsafe { SetConsoleMode(input, 1) })?;
    raw_write(input, &[raw_key(90, 1)])?;
    require(
        read(input, "wide", 32)? == vec![90],
        "read after expansion rejection",
    )
}
fn table_budget(units: bool) -> io::Result<()> {
    let target = if units {
        "X".repeat(32000)
    } else {
        "X".to_owned()
    };
    let limit = if units { 8 } else { 1024 };
    for n in 0..limit {
        add(&format!("s{n}"), &target, "bounded.exe")?;
    }
    unsafe {
        SetLastError(0);
        let ok = AddConsoleAliasW(
            wide("overflow").as_ptr(),
            wide(&target).as_ptr(),
            wide("bounded.exe").as_ptr(),
        );
        require(
            ok == 0 && GetLastError() == 8,
            ("table budget", units, ok, GetLastError()),
        )?;
    }
    add("s0", "replaced", "bounded.exe")?;
    get("s0", "bounded.exe", "replaced")?;
    if !units {
        add("s1", "", "bounded.exe")?;
    }
    add("overflow", &target, "bounded.exe")
}
fn cmd(report: &Path) -> io::Result<()> {
    add("shortcut", "echo alias-cmd-$*$Texit /b 23", "cmd.exe")?;
    let image = Path::new(&std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join("cmd.exe");
    let mut child = std::process::Command::new(image)
        .args(["/d", "/q"])
        .env("PROMPT", "ALIAS_CMD_READY$G")
        .current_dir(report.parent().unwrap())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    wait(deadline, || Ok(report.with_extension("go").exists()))?;
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    raw_write(
        input,
        &"shortcut value\r"
            .encode_utf16()
            .map(|u| key(u, 1))
            .collect::<Vec<_>>(),
    )?;
    while child.try_wait()?.is_none() {
        require(Instant::now() < deadline, "cmd alias exit timed out")?;
        std::thread::sleep(Duration::from_millis(2));
    }
    require(child.wait()?.code() == Some(23), "actual cmd alias exit")
}
pub(crate) fn run(kind: &str, report: &Path) -> io::Result<()> {
    let result = match kind {
        "table" => table(),
        "bounds" => bounds(),
        "counts" => table_budget(false),
        "units" => table_budget(true),
        "budget" => budget(),
        "cmd" => cmd(report),
        _ => cooked(kind),
    };
    if let Err(error) = &result {
        publish(report, &format!("{error:?}"))?;
    } else {
        publish(report, &format!("alias-{kind}-ok"))?;
    }
    result?;
    wait(Instant::now() + Duration::from_secs(10), || {
        Ok(report.with_extension("opened").exists())
    })
}
