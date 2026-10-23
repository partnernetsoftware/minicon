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
    BrowserPanel, BrowserProxy, BrowserProxyConfig, BrowserTab, OAuthBridge, PageHost, PageLoad,
    PanelSlot, ProcessPageHost, deepseek_oauth_authorize, deepseek_oauth_authorize_redirect,
    embedded_page_class, host_reply, http_url_at, oauth_authorize_load, oauth_code_from_query,
    page_host_embed_args, page_host_program, page_host_request, page_load_from_exit,
    resize_embedded_page, resolve_proxy, resolve_proxy_from_process, set_embedded_page_visible,
    spawn_page_host, spawn_page_host_in_panel, spawn_page_host_with_proxy, webkit_proxy_clear,
    webkit_proxy_env, wry_proxy_endpoint,
};
