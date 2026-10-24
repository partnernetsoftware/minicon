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
use std::{cell::RefCell, rc::Rc};

#[cfg(target_os = "macos")]
mod macos_shortcuts;

#[derive(Clone, Debug, Default)]
pub struct NativePageState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub changed: bool,
    pub error: Option<String>,
    pub shortcuts: Vec<char>,
}

const HOME_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
<title>MiniCon Browser</title>
<style>
:root { color-scheme: light dark; font: 16px -apple-system, BlinkMacSystemFont, sans-serif; }
body { margin: 0; min-height: 100vh; background: #f5f2eb; color: #262421; }
main { padding: 2rem; }
h1 { font-size: 1.1rem; margin: 0 0 .5rem; font-weight: 500; }
p { line-height: 1.55; color: #615d56; }
code { color: #275f55; }
@media (prefers-color-scheme: dark) {
  body { background: #1e211f; color: #f3efe6; }
  p { color: #bdb8ae; }
  code { color: #8fc7b8; }
}
</style>
</head>
<body><main><h1>Browse beside your terminal</h1><p>Enter a URL above, or <code>Ctrl+click</code> a link in a terminal.</p></main></body>
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
    // Remove the AppKit monitor before dropping its child view.
    #[cfg(target_os = "macos")]
    _shortcuts: macos_shortcuts::ShortcutMonitor,
    webview: WebView,
    state: Rc<RefCell<NativePageState>>,
    local_origin: Rc<RefCell<Option<String>>>,
}

impl crate::PageSession for NativePageHost {
    fn dispatch(
        &mut self,
        command: crate::PageCommand,
    ) -> Result<crate::PageView, crate::PageError> {
        use crate::{PageCommand, PageError, PageState, PageView};
        let result = match command {
            PageCommand::Navigate(url) => self.load_url(&url),
            PageCommand::Back => self.back(),
            PageCommand::Forward => self.forward(),
            PageCommand::Reload => self.reload(),
            PageCommand::Inspect => Ok(()),
            PageCommand::Click(_) => {
                return Err(PageError::new(
                    "unsupported_operation",
                    "native WebView semantic actions are unavailable",
                ));
            }
        };
        result.map_err(|message| PageError::new("page_operation_failed", message))?;
        let state = self
            .take_state()
            .unwrap_or_else(|| self.state.borrow().clone());
        if let Some(message) = state.error {
            return Err(PageError::new("page_load_failed", message));
        }
        Ok(PageView {
            state: PageState {
                url: state.url,
                title: state.title,
                loading: state.loading,
                can_back: state.can_back,
                can_forward: state.can_forward,
            },
            nodes: None,
            truncated: false,
        })
    }
}

impl NativePageHost {
    /// Create the visible, bounded first-party home surface.
    pub fn new(
        slot: PanelSlot,
        proxy: &BrowserProxy,
        wake: impl Fn() + 'static,
    ) -> Result<Self, String> {
        let parent = native_parent(slot)?;
        let state = Rc::new(RefCell::new(NativePageState::default()));
        let title_state = state.clone();
        let load_state = state.clone();
        let wake = Rc::new(wake);
        let title_wake = wake.clone();
        let navigation_wake = wake.clone();
        let navigation_state = state.clone();
        let local_origin = Rc::new(RefCell::new(None::<String>));
        let navigation_origin = local_origin.clone();
        let native_wake = wake.clone();
        let mut builder = WebViewBuilder::new()
            .with_html(HOME_HTML)
            .with_bounds(bounds(slot))
            .with_focused(false)
            .with_document_title_changed_handler(move |title| {
                let mut state = title_state.borrow_mut();
                state.title = title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect();
                state.changed = true;
                title_wake();
            })
            .with_on_page_load_handler(move |event, url| {
                let mut state = load_state.borrow_mut();
                if url.len() <= 8192 {
                    state.url = url;
                }
                state.loading = matches!(event, wry::PageLoadEvent::Started);
                state.changed = true;
                wake();
            })
            .with_navigation_handler(move |next| {
                let explicitly_local = navigation_origin
                    .borrow()
                    .as_ref()
                    .is_some_and(|origin| next.starts_with(origin));
                if crate::webview_must_bypass_proxy(&next) && !explicitly_local {
                    let _ = crate::deliver_loopback_get(&next);
                    false
                } else {
                    let mut state = navigation_state.borrow_mut();
                    state.error = None;
                    state.loading = true;
                    if next.len() <= 8192 {
                        state.url.clone_from(&next);
                    }
                    state.changed = true;
                    navigation_wake();
                    true
                }
            });
        #[cfg(target_os = "macos")]
        if let Some((host, port)) = crate::wry_proxy_endpoint(proxy) {
            builder = builder
                .with_proxy_config(wry::ProxyConfig::Http(wry::ProxyEndpoint { host, port }));
        }
        #[cfg(windows)]
        if let Some(arguments) = windows_proxy_arguments(proxy)? {
            use wry::WebViewBuilderExtWindows;
            builder = builder.with_additional_browser_args(arguments);
        }
        let webview = builder
            .build_as_child(&parent)
            .map_err(|_| "browser-native-create-failed".to_owned())?;
        #[cfg(windows)]
        install_windows_events(&webview, state.clone(), native_wake)
            .map_err(|_| "browser-native-events-failed".to_owned())?;
        #[cfg(target_os = "macos")]
        let shortcuts =
            macos_shortcuts::ShortcutMonitor::install(&webview, state.clone(), native_wake)?;
        Ok(Self {
            #[cfg(target_os = "macos")]
            _shortcuts: shortcuts,
            webview,
            state,
            local_origin,
        })
    }

    pub fn take_state(&self) -> Option<NativePageState> {
        let mut state = self.state.borrow_mut();
        if !state.changed {
            return None;
        }
        state.changed = false;
        state.can_back = self.webview.can_go_back().unwrap_or(false);
        state.can_forward = self.webview.can_go_forward().unwrap_or(false);
        let result = state.clone();
        state.shortcuts.clear();
        Some(result)
    }

    pub fn back(&self) -> Result<(), String> {
        self.webview
            .go_back()
            .map_err(|_| "browser-native-back-failed".to_owned())
    }

    pub fn forward(&self) -> Result<(), String> {
        self.webview
            .go_forward()
            .map_err(|_| "browser-native-forward-failed".to_owned())
    }

    pub fn reload(&self) -> Result<(), String> {
        self.webview
            .reload()
            .map_err(|_| "browser-native-reload-failed".to_owned())
    }

    pub fn focus(&self) {
        let _ = self.webview.focus();
    }

    pub fn focus_parent(&self) {
        let _ = self.webview.focus_parent();
    }

    pub fn load_url(&self, url: &str) -> Result<(), String> {
        let canonical = crate::http_url_at(url, 0).ok_or_else(|| "not-http".to_owned())?;
        if canonical != url {
            return Err("not-http".to_owned());
        }
        self.state.borrow_mut().error = None;
        *self.local_origin.borrow_mut() = if crate::webview_must_bypass_proxy(url) {
            url.strip_prefix("http://").map(|rest| {
                let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
                format!("http://{authority}/")
            })
        } else {
            None
        };
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

#[cfg(windows)]
fn install_windows_events(
    webview: &WebView,
    state: Rc<RefCell<NativePageState>>,
    wake: Rc<impl Fn() + 'static>,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
    use webview2_com::{AcceleratorKeyPressedEventHandler, NavigationCompletedEventHandler};
    use wry::WebViewExtWindows;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetKeyState(key: i32) -> i16;
    }
    let key_state = state.clone();
    let key_wake = wake.clone();
    let mut token = 0;
    unsafe {
        webview
            .controller()
            .add_AcceleratorKeyPressed(
                &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut kind = Default::default();
                    let mut key = 0;
                    args.KeyEventKind(&mut kind)?;
                    args.VirtualKey(&mut key)?;
                    let control = GetKeyState(0x11) < 0;
                    let shift = GetKeyState(0x10) < 0;
                    let reserved = control
                        && ((shift
                            && matches!(
                                key,
                                0x54 | 0x4F
                                    | 0x44
                                    | 0x4E
                                    | 0x57
                                    | 0x42
                                    | 0x48
                                    | 0x49
                                    | 0x50
                                    | 0x47
                                    | 0xBC
                                    | 0xDB
                                    | 0xDD
                            ))
                            || key == 0x4C);
                    if reserved {
                        args.SetHandled(true)?;
                        if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                            let mut state = key_state.borrow_mut();
                            if state.shortcuts.len() < 16 {
                                state.shortcuts.push(match key {
                                    0xDB => '[',
                                    0xDD => ']',
                                    0xBC => ',',
                                    _ => char::from_u32(key).unwrap_or(' '),
                                });
                                state.changed = true;
                                key_wake();
                            }
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|_| "browser-native-shortcuts-failed".to_owned())?;
        webview
            .webview()
            .add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut success = Default::default();
                    args.IsSuccess(&mut success)?;
                    let mut state = state.borrow_mut();
                    state.error = if success.as_bool() {
                        None
                    } else {
                        let mut status = Default::default();
                        args.WebErrorStatus(&mut status)?;
                        Some(format!("browser-navigation-error-{}", status.0))
                    };
                    state.loading = false;
                    state.changed = true;
                    wake();
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|_| "browser-native-load-events-failed".to_owned())?;
    }
    Ok(())
}

#[cfg(windows)]
fn windows_proxy_arguments(proxy: &BrowserProxy) -> Result<Option<String>, String> {
    let endpoint = |raw: &Option<String>| -> Result<Option<String>, String> {
        let Some(raw) = raw else { return Ok(None) };
        let parsed = crate::wry_proxy_endpoint(&BrowserProxy {
            http: Some(raw.clone()),
            ..BrowserProxy::default()
        });
        let (host, port) = parsed.ok_or_else(|| "browser-proxy-invalid".to_owned())?;
        if !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
            || port.parse::<u16>().ok().is_none_or(|port| port == 0)
        {
            return Err("browser-proxy-invalid".to_owned());
        }
        Ok(Some(format!("http://{host}:{port}")))
    };
    let http = endpoint(&proxy.http)?;
    let https = endpoint(&proxy.https)?;
    let server = match (http, https) {
        (None, None) => return Ok(None),
        (Some(one), None) | (None, Some(one)) => one,
        (Some(http), Some(https)) => format!("http={http};https={https}"),
    };
    // This API replaces wry's default arguments, so retain its feature flags.
    let mut arguments = format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required --proxy-server={server}"
    );
    if let Some(bypass) = &proxy.no_proxy {
        let rules = bypass
            .split([',', ';'])
            .map(str::trim)
            .filter(|rule| !rule.is_empty());
        let mut accepted = Vec::new();
        for rule in rules {
            if rule != "<local>"
                && !rule
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_:*[]/".contains(&byte))
            {
                return Err("browser-proxy-invalid".to_owned());
            }
            accepted.push(rule);
        }
        if !accepted.is_empty() {
            arguments.push_str(" --proxy-bypass-list=");
            arguments.push_str(&accepted.join(";"));
        }
    }
    Ok(Some(arguments))
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
        assert!(HOME_HTML.contains("Enter a URL above"));
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

    #[cfg(windows)]
    #[test]
    fn windows_proxy_preserves_scheme_overrides_and_bypass_rules() {
        let args = windows_proxy_arguments(&BrowserProxy {
            http: Some("http://127.0.0.1:8001".into()),
            https: Some("http://127.0.0.1:8002".into()),
            no_proxy: Some("localhost, *.example.test; 127.0.0.1".into()),
        })
        .unwrap()
        .unwrap();
        assert!(
            args.contains("--proxy-server=http=http://127.0.0.1:8001;https=http://127.0.0.1:8002")
        );
        assert!(args.contains("--proxy-bypass-list=localhost;*.example.test;127.0.0.1"));
        let fallback = windows_proxy_arguments(&BrowserProxy {
            http: Some("http://127.0.0.1:8001".into()),
            ..BrowserProxy::default()
        })
        .unwrap()
        .unwrap();
        assert!(fallback.ends_with("--proxy-server=http://127.0.0.1:8001"));
        assert!(
            windows_proxy_arguments(&BrowserProxy::default())
                .unwrap()
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_proxy_refuses_malformed_endpoints_and_browser_switches() {
        for raw in [
            "http://host --flag:80",
            "http://host:0",
            "http://host:65536",
            "socks5://host:80",
        ] {
            assert!(
                windows_proxy_arguments(&BrowserProxy {
                    http: Some(raw.into()),
                    ..BrowserProxy::default()
                })
                .is_err(),
                "{raw}"
            );
        }
        assert!(
            windows_proxy_arguments(&BrowserProxy {
                http: Some("http://127.0.0.1:8001".into()),
                no_proxy: Some("host --flag".into()),
                ..BrowserProxy::default()
            })
            .is_err()
        );
    }
}
