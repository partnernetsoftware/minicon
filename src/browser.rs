//! Tab-strip adapter for the browser content panel.
//!
//! The stable API lives in [`minicon_web`] (`crates/minicon-web`). This module
//! re-exports it so the GUI keeps one import path. Core `minicon` does not
//! link wry or tao. There is no system tray. Chrome sent-history stays outside
//! this module.

// The GUI calls a subset. The rest is the same adapter surface (`PageHost`,
// `host_reply`) so callers in this binary do not import `minicon_web` directly.
#[allow(unused_imports)]
pub use minicon_web::{
    BrowserPanel, BrowserProxy, BrowserProxyConfig, BrowserTab, PageHost, PageLoad,
    ProcessPageHost, host_reply, http_url_at, oauth_authorize_load, page_host_request,
    page_load_from_exit, resolve_proxy, resolve_proxy_from_process, spawn_page_host,
    spawn_page_host_with_proxy, webkit_proxy_clear, webkit_proxy_env, wry_proxy_endpoint,
};
