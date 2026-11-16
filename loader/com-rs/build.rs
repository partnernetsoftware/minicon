//! Embed a VERSIONINFO resource into the Windows launcher so a packed `.com`
//! carries MiniCon's identity before it is signed: the company-signing
//! preflight requires a non-empty resource directory, `ProductName == MiniCon`
//! and an exact `ProductVersion`. The icon is the product's own, kept under a
//! 16 KiB budget; the version comes from `MINICON_COM_VERSION` (falling back
//! to the crate version) so CI can stamp the release version at pack time.
//!
//! Non-Windows targets and cross-checks build nothing here.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let crate_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let version = std::env::var("MINICON_COM_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or(crate_version);

    // Keep the icon path stable from the com-rs directory regardless of the
    // workspace the build runs from.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = std::path::Path::new(&manifest_dir)
        .join("../../assets/minicon.ico")
        .canonicalize()
        .expect("assets/minicon.ico must exist relative to loader/com-rs");
    let icon = icon.to_str().expect("icon path must be UTF-8");
    println!("cargo:rerun-if-changed={icon}");

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(icon)
        .set("ProductName", "MiniCon")
        .set("FileDescription", "MiniCon single-file launcher")
        .set("InternalName", "minicon-com")
        .set("ProductVersion", &version);
    resource
        .compile()
        .expect("failed to embed launcher resources");
}
