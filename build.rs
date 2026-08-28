fn main() {
    const ICON: &str = "assets/minicon.ico";
    const ICON_BUDGET: u64 = 16 * 1024;
    println!("cargo:rerun-if-changed={ICON}");
    let icon_bytes = std::fs::metadata(ICON)
        .expect("minicon icon is missing")
        .len();
    assert!(
        icon_bytes <= ICON_BUDGET,
        "minicon icon is {icon_bytes} bytes; compact resource budget is {ICON_BUDGET}"
    );
    // Build scripts compile for the host, so `#[cfg(windows)]` would silently
    // remove this target policy from macOS/Linux cross-builds. Cargo's target
    // environment is the only authority here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        override_msvcrt_defaultlib();
        // The MSVC runtime is three separable pieces: the C startup files, the
        // VC runtime, and the Universal CRT. They normally have to share a
        // linkage model, with one documented exception -- startup and VC
        // runtime static, UCRT dynamic -- which is exactly the combination
        // this product wants. The UCRT is a Windows component and is present
        // on every supported system; VCRUNTIME140.dll is a redistributable
        // and is not, so a dynamic reference to it makes a clean Windows
        // Server 2016 refuse to start the program.
        //
        // Mixing the models is what fails loudly: leaving msvcrt.lib (the
        // dynamic startup) alongside libvcruntime.lib links, then dies at
        // STATUS_STACK_BUFFER_OVERRUN. The exclusions below are not
        // decoration, they are the half that makes this work. Same directive
        // set as the static_vcruntime crate, inlined rather than taken as a
        // dependency because it is six linker flags and this build script is
        // already the owner of this binary's link line.
        // The release set in every profile, with no debug-CRT variant: Rust
        // links the release CRT on windows-msvc regardless of profile, so
        // naming `libvcruntimed.lib` here leaves `__CxxFrameHandler3`
        // unresolved and the dev build stops linking.
        for library in [
            "libvcruntimed.lib",
            "vcruntime.lib",
            "vcruntimed.lib",
            "libcmtd.lib",
            "msvcrt.lib",
            "msvcrtd.lib",
            "libucrt.lib",
            "libucrtd.lib",
        ] {
            println!("cargo:rustc-link-arg=/NODEFAULTLIB:{library}");
        }
        for library in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
            println!("cargo:rustc-link-arg=/DEFAULTLIB:{library}");
        }
        println!("cargo:rustc-link-arg-bin=minicon=/ENTRY:minicon_entry");
        // `startup.rs` explicitly walks XI/XC/XP/XT while the PE loader owns
        // XL through the TLS Directory. The default CRT entry is intentionally
        // absent, so link.exe cannot infer that every `.CRT` family is handled.
        println!("cargo:rustc-link-arg-bin=minicon=/IGNORE:4210");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(ICON)
            .set("ProductName", "MiniCon")
            .set("FileDescription", "MiniCon standalone terminal")
            .set("OriginalFilename", "minicon.exe")
            .set("InternalName", "minicon");
        resource
            .compile()
            .expect("failed to embed minicon resources");
    }
}

/// Shadows rustc's hard-coded `msvcrt.lib` directive with an empty COFF
/// object. `/NODEFAULTLIB` alone is insufficient on newer MSVC toolsets: the
/// unresolved default can still make `libcmt` contribute startup objects that
/// require private static-UCRT symbols, defeating the deliberate dynamic-UCRT
/// boundary above.
///
/// The compact COFF archive technique is adapted from Chris Denton's
/// `static_vcruntime` implementation, also distributed under MIT/Apache-2.0.
fn override_msvcrt_defaultlib() {
    let machine: &[u8] = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => &[0x64, 0x86],
        Ok("x86") => &[0x4c, 0x01],
        _ => return,
    };
    let object: &[u8] = &[
        1, 0, 94, 3, 96, 98, 60, 0, 0, 0, 1, 0, 0, 0, 0, 0, 132, 1, 46, 100, 114, 101, 99, 116,
        118, 101, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 10, 16, 0, 46, 100, 114, 101, 99, 116, 118, 101, 0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 4, 0,
        0, 0,
    ];
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let library = output.join("msvcrt.lib");
    let mut bytes = Vec::with_capacity(machine.len() + object.len());
    bytes.extend_from_slice(machine);
    bytes.extend_from_slice(object);
    std::fs::write(&library, bytes).expect("failed to write msvcrt override library");
    println!("cargo:rustc-link-search=native={}", output.display());
}
