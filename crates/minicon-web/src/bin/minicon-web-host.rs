//! Page-host process for one browser content panel.
//!
//! Legacy argv remains `--probe` or `--load <http-url>`. An explicit
//! `--load-context` consumes a bounded startup frame on stdin (PRD 28).
//! The default body does
//! not link wry. A WebView backend replaces this body and keeps the argv.
//! Exit 69 means the page is Unavailable. This binary never calls the OS browser.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match minicon_web::host_reply(&args) {
        Ok(()) => {}
        Err("usage") => {
            eprintln!(
                "usage: minicon-web-host --probe | --load <http-url> | --load-context <http-url> | --smoke <http-url> | --oauth-smoke <http-url> <loopback-url>"
            );
            std::process::exit(64);
        }
        Err(reason) => {
            eprintln!("unavailable {reason}");
            std::process::exit(69);
        }
    }
}
