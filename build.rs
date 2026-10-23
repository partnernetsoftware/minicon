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
    emit_window_icon(ICON);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Keep the standard window controls without macOS 26's more expensive
        // design initialization. Apple ignores this temporary compatibility
        // key for SDK 27+ builds; it is a measured reduction, not the RSS goal.
        // Embed the plist so the native executable remains a single file.
        const PLIST: &str = "assets/macos-info.plist";
        println!("cargo:rerun-if-changed={PLIST}");
        println!("cargo:rustc-link-arg-bin=minicon=-Wl,-sectcreate,__TEXT,__info_plist,{PLIST}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must provide arch");
        println!("cargo:rerun-if-env-changed=MINICON_BROWSER_EMBEDDED_HOST");
        let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
        let helper = if let Some(path) = std::env::var_os("MINICON_BROWSER_EMBEDDED_HOST") {
            println!(
                "cargo:rerun-if-changed={}",
                std::path::Path::new(&path).display()
            );
            let bytes = std::fs::read(path).expect("canonical Linux browser helper is missing");
            assert!(
                bytes.len() >= 64
                    && bytes.starts_with(b"\x7fELF\x02\x01\x01")
                    && bytes.len() <= 32 * 1024 * 1024,
                "Linux browser helper must be a little-endian ELF64 within 32 MiB"
            );
            let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
            let expected_machine = match arch.as_str() {
                "x86_64" => 62,
                "aarch64" => 183,
                _ => panic!("unsupported embedded Linux browser architecture: {arch}"),
            };
            assert_eq!(
                machine, expected_machine,
                "Linux browser helper ISA mismatch"
            );
            bytes
        } else {
            Vec::new()
        };
        std::fs::write(output.join("minicon-web-host.bin"), helper)
            .expect("stage embedded browser helper");
        for (environment, soname) in [
            ("MINICON_BUNDLED_XKB_X11_PATH", "libxkbcommon-x11.so.0"),
            ("MINICON_BUNDLED_XCB_XKB_PATH", "libxcb-xkb.so.1"),
        ] {
            let vendor = format!("vendor/linux/{arch}/{soname}");
            println!("cargo:rerun-if-changed={vendor}");
            let bytes = std::fs::read(&vendor).unwrap_or_else(|error| {
                panic!("missing bundled Linux XKB library at {vendor}: {error}")
            });
            assert!(
                !bytes.is_empty(),
                "bundled Linux XKB library at {vendor} is empty"
            );
            println!("cargo:rustc-env={environment}={vendor}");
        }
    }
    // Build scripts compile for the host, so `#[cfg(windows)]` would silently
    // remove this target policy from macOS/Linux cross-builds. Cargo's target
    // environment is the only authority here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if std::env::var("PROFILE").as_deref() == Ok("windows-release")
            && std::env::var("MINICON_RELEASE_PATHS_REDACTED").as_deref() != Ok("1")
        {
            panic!(
                "windows-release requires MINICON_RELEASE_PATHS_REDACTED=1 and a home-path remap"
            );
        }
        override_msvcrt_defaultlib();
        // The MSVC runtime is three separable pieces: the C startup files, the
        // VC runtime, and the Universal CRT. They normally have to share a
        // linkage model, with one documented exception -- startup and VC
        // runtime static, UCRT dynamic. WebView2's static loader pulls
        // `libcmt.lib(utility.obj)`, whose private `__acrt_*` initialization
        // symbols are absent from the dynamic UCRT import library. Since the
        // loader must stay inside this one-file executable, select the static
        // UCRT as well. `startup.rs` initializes both runtimes explicitly.
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
            "ucrt.lib",
            "libucrtd.lib",
        ] {
            println!("cargo:rustc-link-arg=/NODEFAULTLIB:{library}");
        }
        for library in ["libcmt.lib", "libvcruntime.lib", "libucrt.lib"] {
            println!("cargo:rustc-link-arg=/DEFAULTLIB:{library}");
        }
        println!("cargo:rustc-link-arg-bin=minicon=/ENTRY:minicon_entry");
        // `startup.rs` explicitly walks XI/XC/XP/XT while the PE loader owns
        // XL through the TLS Directory. The default CRT entry is intentionally
        // absent, so link.exe cannot infer that every `.CRT` family is handled.
        println!("cargo:rustc-link-arg-bin=minicon=/IGNORE:4210");
        // Declare the exe per-monitor DPI-aware in its manifest so Windows never
        // bitmap-stretches the window on a scaled display (the cause of blurry
        // text next to a crisp cmd.exe). `dpiAware` (true = system aware) is
        // honored back to Vista/7; `dpiAwareness` (PerMonitorV2) is honored on
        // Windows 10+. The runtime also declares awareness dynamically for APE
        // builds and as a belt-and-braces fallback.
        const MINICON_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(ICON)
            .set("ProductName", "MiniCon")
            .set("FileDescription", "MiniCon standalone terminal")
            .set("OriginalFilename", "minicon.exe")
            .set("InternalName", "minicon")
            .set_manifest(MINICON_MANIFEST);
        resource
            .compile()
            .expect("failed to embed minicon resources");
    }
}

/// Shadows rustc's hard-coded `msvcrt.lib` directive with an empty COFF
/// object. `/NODEFAULTLIB` alone is insufficient on newer MSVC toolsets: the
/// unresolved default can still make `libcmt` contribute startup objects that
/// require private static-UCRT symbols. The WebView2 static loader now requires
/// that complete static UCRT boundary; this override still excludes the
/// incompatible dynamic startup object.
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

/// Writes the icon pixels the X11 and macOS hosts need as a generated Rust
/// source, decoded from the same committed `.ico` the Windows resource uses.
///
/// Doing it here rather than shipping a second icon keeps `assets/minicon.ico`
/// the single source of truth for every platform: there is no committed
/// derived blob that can drift from it, and no PNG decoder in the shipped
/// binary. Windows never reads this — it gets the icon from the PE resource
/// `winresource` embeds — so the generated file is only compiled in on unix
/// hosts, where neither a PE resource nor a desktop file can carry the icon.
fn emit_window_icon(icon: &str) {
    let (width, height, rgba) = decode_largest_ico_face(icon);
    let mut source = String::from("// @generated by build.rs from assets/minicon.ico\n");
    source.push_str(&format!("pub const WINDOW_ICON_WIDTH: u32 = {width};\n"));
    source.push_str(&format!("pub const WINDOW_ICON_HEIGHT: u32 = {height};\n"));
    source.push_str(&format!(
        "pub const WINDOW_ICON_RGBA: [u8; {}] = [\n",
        rgba.len()
    ));
    for chunk in rgba.chunks(16) {
        source.push_str("    ");
        for byte in chunk {
            source.push_str(&format!("{byte},"));
        }
        source.push('\n');
    }
    source.push_str("];\n");
    let destination =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"))
            .join("window_icon.rs");
    std::fs::write(destination, source).expect("failed to write the generated window icon");
}

/// The largest face of an `.ico` as `(edge, edge, rgba8)`.
///
/// The faces this repository ships are PNG-compressed, which `png` decodes.
/// Any other encoding is refused loudly instead of silently producing wrong
/// pixels: the generator in `assets/icon/` is ours, so an unexpected face is a
/// change to review, not a format to guess at.
fn decode_largest_ico_face(path: &str) -> (u32, u32, Vec<u8>) {
    let bytes = std::fs::read(path).expect("minicon icon is missing");
    assert!(
        bytes.len() > 6 && bytes[2] == 1,
        "{path} is not an icon file"
    );
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    let mut best: Option<(u32, u32, usize, usize)> = None;
    for index in 0..count {
        let entry = 6 + index * 16;
        let edge = |offset: usize| -> u32 {
            let raw = bytes[entry + offset];
            if raw == 0 { 256 } else { u32::from(raw) }
        };
        let (width, height) = (edge(0), edge(1));
        assert_eq!(width, height, "{path} faces are square");
        let length = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(bytes[entry + 12..entry + 16].try_into().unwrap()) as usize;
        if best.is_none_or(|(area, ..)| width * height > area) {
            best = Some((width * height, width, offset, length));
        }
    }
    let (_, edge, offset, length) = best.expect("minicon icon has no faces");
    let face = &bytes[offset..offset + length];
    assert_eq!(
        &face[..8],
        b"\x89PNG\r\n\x1a\n",
        "{path} faces are expected to be PNG"
    );
    let mut reader = png::Decoder::new(face)
        .read_info()
        .expect("failed to read an icon face");
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .expect("failed to decode an icon face");
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "{path} faces are RGBA"
    );
    assert_eq!(
        info.bit_depth,
        png::BitDepth::Eight,
        "{path} faces are 8-bit"
    );
    assert_eq!(
        info.width, edge,
        "{path} face size matches its directory entry"
    );
    buffer.truncate(info.buffer_size());
    (edge, edge, buffer)
}
