//! Windows developer fixture only. No product/ConApp entry point.
#[cfg(windows)]
use minicon_web::http_url_at;
#[cfg(windows)]
#[path = "windows_cookie_lifetime/host.rs"]
mod host;

fn main() {
    #[cfg(windows)]
    let code = host::main_entry();
    #[cfg(not(windows))]
    let code = 69;
    std::process::exit(code);
}
