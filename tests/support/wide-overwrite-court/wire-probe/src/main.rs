// Dump the raw ConPTY wire bytes for the misalignment fixture: the boundary
// between "ConPTY synthesized it wrong" and "minicon parsed it wrong".
use minicon_platform::pty::{ChildCommand, TerminalSize};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Contract for any binary that opens platform PTYs: hand agent re-exec
    // invocations to the adapter before doing anything else. Skipping this is
    // how the first run of this probe fork-bombed itself: the shell inherits
    // AGENTERM_FORCE_CONSOLE_AGENT=1 from the hosting minicon GUI, selection
    // picked the agent, and the agent re-executed this binary.
    if let Some(code) = minicon_platform::pty::run_if_console_agent(&args) {
        std::process::exit(code);
    }
    // This probe exists to capture the ConPTY wire specifically; drop the
    // inherited forcing so automatic selection picks the pseudoconsole.
    // SAFETY: single-threaded here, before any spawn.
    unsafe { std::env::remove_var("AGENTERM_FORCE_CONSOLE_AGENT") };
    let fixture = args.get(1).expect("usage: wire-probe <fixture.ps1> <out.bin>");
    let out_path = args.get(2).expect("usage: wire-probe <fixture.ps1> <out.bin>");

    let command = ChildCommand::new("powershell")
        .size(TerminalSize { rows: 24, cols: 88 })
        .env("TERM", "xterm-256color")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(fixture.as_str());

    let (master, child) = command.spawn().expect("spawn pty").into_parts();
    println!("backend: {}", master.backend_kind());
    let mut file = std::fs::File::create(out_path).expect("create out");
    let mut buffer = [0u8; 4096];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let mut all: Vec<u8> = Vec::new();
    while std::time::Instant::now() < deadline {
        match master.io().read(&mut buffer) {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Ok(n) => {
                all.extend_from_slice(&buffer[..n]);
                if all
                    .windows(b"FIXTURE_DONE".len())
                    .any(|w| w == b"FIXTURE_DONE")
                {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    file.write_all(&all).expect("write");
    println!("wire bytes: {}", all.len());
    let _ = child.terminate_forcefully();
    child.close_pseudoconsole();
}
