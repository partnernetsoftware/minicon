//! Same-process native page surface for the GUI-owned content panel.
//!
//! The parent is an AppKit `NSView` or Win32 `HWND` supplied by `minicon-platform`. All wry
//! calls remain inside this crate; callers see only [`PanelSlot`] geometry and
//! bounded navigation methods.

#[cfg(windows)]
use std::num::NonZeroIsize;
#[cfg(target_os = "macos")]
use std::{ffi::c_void, ptr::NonNull};

#[cfg(target_os = "macos")]
use wry::raw_window_handle::AppKitWindowHandle;
#[cfg(windows)]
use wry::raw_window_handle::Win32WindowHandle;
use wry::{
    Rect, WebView, WebViewBuilder,
    dpi::{PhysicalPosition, PhysicalSize},
    raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle},
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

#[cfg(target_os = "macos")]
struct NativeParent(NonNull<c_void>);
#[cfg(windows)]
struct NativeParent(NonZeroIsize);

impl HasWindowHandle for NativeParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        #[cfg(target_os = "macos")]
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
        #[cfg(windows)]
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // SAFETY: `PixelWindow` owns this live NSView/HWND for longer than the
        // returned borrow and the child WebView. The GUI drops the WebView
        // before releasing or replacing its native window.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(target_os = "macos")]
fn native_parent(slot: PanelSlot) -> Result<NativeParent, String> {
    usize::try_from(slot.parent)
        .ok()
        .and_then(|value| NonNull::new(value as *mut c_void))
        .map(NativeParent)
        .ok_or_else(|| "browser-parent-unavailable".to_owned())
}

#[cfg(windows)]
fn native_parent(slot: PanelSlot) -> Result<NativeParent, String> {
    isize::try_from(slot.parent)
        .ok()
        .and_then(NonZeroIsize::new)
        .map(NativeParent)
        .ok_or_else(|| "browser-parent-unavailable".to_owned())
}

/// One native WebView child owned by MiniCon's active browser content panel.
pub struct NativePageHost {
    webview: WebView,
}

impl NativePageHost {
    /// Create the visible, bounded first-party home surface.
    pub fn new(slot: PanelSlot, proxy: &BrowserProxy) -> Result<Self, String> {
        let parent = native_parent(slot)?;
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
        apply_visibility(
            visible,
            || {
                self.webview
                    .focus_parent()
                    .map_err(|_| "browser-native-focus-release-failed".to_owned())
            },
            |show| {
                self.webview
                    .set_visible(show)
                    .map_err(|_| "browser-native-visibility-failed".to_owned())
            },
        )
    }
}

fn apply_visibility(
    visible: bool,
    focus_parent: impl FnOnce() -> Result<(), String>,
    set_visible: impl FnOnce(bool) -> Result<(), String>,
) -> Result<(), String> {
    if !visible {
        // A hidden WebView must release keyboard focus. Win32 SetFocus returns
        // the *previous* HWND, so wry can report an error even after focus
        // moved. Hiding must still happen in that case.
        let _ = focus_parent();
    }
    set_visible(visible)
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
    fn focus_error_does_not_skip_hiding_the_child() {
        let calls = std::cell::RefCell::new(Vec::new());
        apply_visibility(
            false,
            || {
                calls.borrow_mut().push("focus");
                Err("no previous HWND".to_owned())
            },
            |visible| {
                calls
                    .borrow_mut()
                    .push(if visible { "show" } else { "hide" });
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls.into_inner(), ["focus", "hide"]);
    }

    #[test]
    fn showing_the_child_does_not_move_focus_to_the_parent() {
        apply_visibility(
            true,
            || panic!("show must not focus the parent"),
            |visible| {
                assert!(visible);
                Ok(())
            },
        )
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn win32_parent_is_typed_and_rejects_missing_hwnd() {
        let slot = PanelSlot {
            parent: 0,
            x: 0,
            y: 0,
            width: 320,
            height: 200,
        };
        assert!(native_parent(slot).is_err());
        let parent = native_parent(PanelSlot { parent: 7, ..slot }).unwrap();
        let handle = parent.window_handle().unwrap();
        assert!(matches!(handle.as_raw(), RawWindowHandle::Win32(raw) if raw.hwnd.get() == 7));
    }
}
