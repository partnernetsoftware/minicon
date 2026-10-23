//! Load the actual PE entry with CRT stubs whose bool return has poisoned upper bits.
//! Delivery/ABI ownership: PRD 27. This tests process exits, not source spelling.
#![cfg(all(windows, target_arch = "x86_64"))]

use std::{fs, process::Command, time::SystemTime};

#[test]
fn pe_entry_observes_the_crt_boolean_return_abi() {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("startup-abi-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let startup = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/startup.rs");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    for failed_runtime in ["vcrt", "acrt", "none"] {
        let returns = [
            (
                "__vcrt_initialize",
                if failed_runtime == "vcrt" { 0x100 } else { 1 },
            ),
            (
                "__acrt_initialize",
                if failed_runtime == "acrt" { 0x100 } else { 1 },
            ),
            ("__acrt_uninitialize", 1),
            ("__isa_available_init", 0),
            ("main", 42),
        ];
        let mut asm = String::new();
        for (symbol, value) in returns {
            let instruction = format!("mov eax, {value}");
            asm.push_str(&format!(".globl {symbol}\n{symbol}:\n{instruction}\nret\n"));
        }
        let source = root.join(format!("fixture_{failed_runtime}.rs"));
        let executable = root.join(format!("fixture_{failed_runtime}.exe"));
        // Returning 0x100 is a valid false bool (low byte zero), but nonzero
        // when incorrectly consumed as i32. Success stubs return valid true.
        fs::write(
            &source,
            format!(
                "#![no_std]\n#![no_main]\n\
                 #[path = {startup:?}] mod startup;\n\
                 core::arch::global_asm!({asm:?});\n\
                 #[panic_handler]\n\
                 fn panic(_: &core::panic::PanicInfo<'_>) -> ! {{ loop {{}} }}\n"
            ),
        )
        .unwrap();
        let output = Command::new(&rustc)
            .args([
                "--edition=2024",
                "-Cpanic=abort",
                "-Copt-level=1",
                "-Clink-arg=/ENTRY:minicon_entry",
                "-Clink-arg=/NODEFAULTLIB",
            ])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("native rustc must build the PE entry fixture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status = Command::new(&executable).status().unwrap();
        let expected = if failed_runtime == "none" { 42 } else { 254 };
        assert_eq!(
            status.code(),
            Some(expected),
            "failed runtime: {failed_runtime}"
        );
    }
}
