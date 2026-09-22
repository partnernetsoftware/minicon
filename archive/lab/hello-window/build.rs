fn main() {
    const ICON: &str = "../../assets/minicon.ico";
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var_os("CARGO_FEATURE_RESOURCED").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
    {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(ICON)
        .set("ProductName", "MiniCon Lab Hello Window")
        .set("FileDescription", "MiniCon antivirus resource control")
        .set("OriginalFilename", "helloworld-resourced-x86-64.exe")
        .set("InternalName", "helloworld-resourced");
    resource
        .compile()
        .expect("failed to embed hello-window comparison resources");
}
