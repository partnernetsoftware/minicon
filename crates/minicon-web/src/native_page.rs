//! Same-process native page surface for the GUI-owned content panel.
//!
//! The parent is an AppKit `NSView` supplied by `minicon-platform`. All wry
//! calls remain inside this crate; callers see only [`PanelSlot`] geometry and
//! bounded navigation methods.

use std::{ffi::c_void, ptr::NonNull};

use wry::{
    Rect, WebView, WebViewBuilder,
    dpi::{PhysicalPosition, PhysicalSize},
    raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    },
};

use crate::{BrowserProxy, PanelSlot};

const HOME_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
<title>MiniCon Browser</title>
<style>
:root { color-scheme: light dark; font: 16px -apple-system, BlinkMacSystemFont, sans-serif; }
body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: #f5f2eb; color: #262421; }
main { width: min(34rem, calc(100% - 4rem)); }
h1 { font-size: 2rem; margin: 0 0 .5rem; }
p { line-height: 1.55; color: #615d56; }
code { color: #275f55; }
@media (prefers-color-scheme: dark) {
  body { background: #1e211f; color: #f3efe6; }
  p { color: #bdb8ae; }
  code { color: #8fc7b8; }
}
</style>
</head>
<body><main><h1>MiniCon Browser</h1><p>The native content panel is ready.</p><p><code>Ctrl+click</code> an http(s) link in a terminal to navigate here.</p></main></body>
</html>"#;

struct AppKitParent(NonNull<c_void>);

impl HasWindowHandle for AppKitParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
        // SAFETY: `PixelWindow` owns this live NSView for longer than the
        // returned borrow and the child WebView. The GUI drops the WebView
        // before releasing or replacing its native window.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// One WKWebView child owned by MiniCon's active browser content panel.
pub struct NativePageHost {
    webview: WebView,
}

impl NativePageHost {
    /// Create the visible, bounded first-party home surface.
    pub fn new(slot: PanelSlot, proxy: &BrowserProxy) -> Result<Self, String> {
        let pointer = usize::try_from(slot.parent)
            .ok()
            .and_then(|value| NonNull::new(value as *mut c_void))
            .ok_or_else(|| "browser-parent-unavailable".to_owned())?;
        let parent = AppKitParent(pointer);
        let mut builder = WebViewBuilder::new()
            .with_html(HOME_HTML)
            .with_bounds(bounds(slot))
            .with_navigation_handler(|next| {
                if crate::webview_must_bypass_proxy(&next) {
                    let _ = crate::deliver_loopback_get(&next);
                    false
                } else {
                    true
                }
            });
        if let Some((host, port)) = crate::wry_proxy_endpoint(proxy) {
            builder = builder
                .with_proxy_config(wry::ProxyConfig::Http(wry::ProxyEndpoint { host, port }));
        }
        let webview = builder
            .build_as_child(&parent)
            .map_err(|_| "browser-native-create-failed".to_owned())?;
        Ok(Self { webview })
    }

    pub fn load_url(&self, url: &str) -> Result<(), String> {
        let canonical = crate::http_url_at(url, 0).ok_or_else(|| "not-http".to_owned())?;
        if canonical != url {
            return Err("not-http".to_owned());
        }
        self.webview
            .load_url(url)
            .map_err(|_| "browser-native-load-failed".to_owned())
    }

    pub fn set_slot(&self, slot: PanelSlot) -> Result<(), String> {
        self.webview
            .set_bounds(bounds(slot))
            .map_err(|_| "browser-native-resize-failed".to_owned())
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), String> {
        if !visible {
            // A hidden WKWebView must not remain AppKit's first responder;
            // otherwise host Ctrl+Shift shortcuts disappear into an invisible
            // page after the user clicks a terminal tab.
            self.webview
                .focus_parent()
                .map_err(|_| "browser-native-focus-release-failed".to_owned())?;
        }
        self.webview
            .set_visible(visible)
            .map_err(|_| "browser-native-visibility-failed".to_owned())
    }
}

fn bounds(slot: PanelSlot) -> Rect {
    Rect {
        position: PhysicalPosition::new(slot.x, slot.y).into(),
        size: PhysicalSize::new(slot.width, slot.height).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_a_bounded_first_party_smoke_surface() {
        assert!(HOME_HTML.len() < 4096);
        assert!(HOME_HTML.contains("MiniCon Browser"));
        assert!(HOME_HTML.contains("native content panel is ready"));
        assert!(HOME_HTML.contains("default-src 'none'"));
        assert!(!HOME_HTML.contains("http://"));
        assert!(!HOME_HTML.contains("https://"));
    }

    #[test]
    fn panel_slot_maps_to_physical_child_bounds() {
        let rect = bounds(PanelSlot {
            parent: 7,
            x: 101,
            y: 43,
            width: 640,
            height: 360,
        });
        assert_eq!(rect.position, PhysicalPosition::new(101, 43).into());
        assert_eq!(rect.size, PhysicalSize::new(640, 360).into());
    }

    #[test]
    fn native_page_is_a_child_and_releases_focus_before_hiding() {
        let source = include_str!("native_page.rs");
        assert!(source.contains(".build_as_child(&parent)"));
        assert!(source.contains("self.webview\n                .focus_parent()"));
        let top_level_build = [".build", "(&parent)"].concat();
        assert!(!source.contains(&top_level_build));
    }
}
