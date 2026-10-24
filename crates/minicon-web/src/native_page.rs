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
fn macos_proxy_endpoint(
    proxy: &BrowserProxy,
    available: bool,
) -> Result<Option<(String, String)>, String> {
    let parse = |value: &Option<String>| {
        value
            .as_ref()
            .map(|raw| {
                crate::parse_http_proxy_endpoint(raw)
                    .ok_or_else(|| "browser-proxy-invalid".to_owned())
            })
            .transpose()
    };
    let http = parse(&proxy.http)?;
    let https = parse(&proxy.https)?;
    let endpoint = match (http, https) {
        (Some(http), Some(https)) if http != https => {
            return Err("browser-proxy-unsupported".to_owned());
        }
        (Some(one), _) | (_, Some(one)) => Some(one),
        (None, None) => None,
    };
    if endpoint.is_some() && !available {
        return Err("browser-proxy-unsupported".to_owned());
    }
    Ok(endpoint)
}

#[cfg(target_os = "macos")]
fn macos_proxy_available() -> bool {
    use objc2::{
        class, msg_send,
        runtime::{AnyObject, Bool},
        sel,
    };
    // WebKit's proxyConfigurations setter is available on macOS 14+. Check
    // before wry calls the newer Network HTTP-CONNECT proxy functions.
    let store: *mut AnyObject = unsafe { msg_send![class!(WKWebsiteDataStore), defaultDataStore] };
    if store.is_null() {
        return false;
    }
    let available: Bool =
        unsafe { msg_send![store, respondsToSelector: sel!(setProxyConfigurations:)] };
    available.as_bool()
}

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

fn start_navigation(state: &mut NativePageState, url: &str) {
    if url.len() <= 8192 {
        state.url = url.to_owned();
    }
    state.title.clear();
    state.error = None;
    state.loading = true;
    state.changed = true;
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
        #[cfg(windows)]
        let windows_arguments = windows_proxy_arguments(proxy)?;
        #[cfg(windows)]
        let mut windows_context = windows_arguments
            .as_deref()
            .map(windows_proxy_context)
            .transpose()?;
        #[cfg(windows)]
        let builder = match windows_context.as_mut() {
            Some(context) => WebViewBuilder::new_with_web_context(context),
            None => WebViewBuilder::new(),
        };
        #[cfg(not(windows))]
        let builder = WebViewBuilder::new();
        let mut builder = builder
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
                    start_navigation(&mut state, &next);
                    navigation_wake();
                    true
                }
            });
        #[cfg(target_os = "macos")]
        if let Some((host, port)) = macos_proxy_endpoint(proxy, macos_proxy_available())? {
            builder = builder
                // A fresh private store prevents the old view's proxy from
                // contaminating a later view after the field is cleared.
                .with_incognito(true)
                .with_proxy_config(wry::ProxyConfig::Http(wry::ProxyEndpoint { host, port }));
        }
        #[cfg(windows)]
        if let Some(arguments) = windows_arguments {
            use wry::WebViewBuilderExtWindows;
            builder = builder
                .with_additional_browser_args(arguments)
                .with_incognito(true);
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
        start_navigation(&mut self.state.borrow_mut(), url);
        *self.local_origin.borrow_mut() = if crate::webview_must_bypass_proxy(url) {
            url.strip_prefix("http://").map(|rest| {
                let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
                format!("http://{authority}/")
            })
        } else {
            None
        };
        self.webview.load_url(url).map_err(|_| {
            let reason = "browser-native-load-failed".to_owned();
            let mut state = self.state.borrow_mut();
            state.loading = false;
            state.error = Some(reason.clone());
            state.changed = true;
            reason
        })
    }

    pub fn set_slot(&self, slot: PanelSlot) -> Result<(), String> {
        self.webview
            .set_bounds(bounds(slot))
            .map_err(|_| "browser-native-resize-failed".to_owned())
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), String> {
        let result = apply_visibility(
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
        );
        #[cfg(target_os = "macos")]
        if visible && result.is_ok() {
            use wry::WebViewExtMacOS;
            let view: objc2::rc::Retained<objc2_app_kit::NSView> =
                self.webview.webview().into_super().into_super();
            // Keep a replacement WK child above the parent software surface.
            // A loaded page otherwise stays blank until a tab hide/show cycle.
            // SAFETY: the GUI thread owns this live child and its parent
            // window; neither can be removed during this synchronous call.
            if let Some(parent) = unsafe { view.superview() } {
                parent.addSubview_positioned_relativeTo(
                    &view,
                    objc2_app_kit::NSWindowOrderingMode::Above,
                    None,
                );
            }
            view.setNeedsDisplay(true);
        }
        result
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
                &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut success = Default::default();
                    args.IsSuccess(&mut success)?;
                    let mut state = state.borrow_mut();
                    state.error = if success.as_bool() {
                        // Read the committed document as well as title-change
                        // notifications: redirects may deliver those before the
                        // final navigation-start callback resets stale state.
                        if let Some(sender) = sender {
                            let mut title = Default::default();
                            if sender.DocumentTitle(&mut title).is_ok() {
                                state.title = webview2_com::take_pwstr(title)
                                    .chars()
                                    .filter(|c| !c.is_control())
                                    .take(256)
                                    .collect();
                            }
                        }
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
fn windows_proxy_context(arguments: &str) -> Result<wry::WebContext, String> {
    use sha2::Digest;
    // WebView2 environments with different options cannot share a data folder,
    // even while the previous view's browser process is still shutting down.
    // Key all native arguments, including bypass rules; never put endpoints in
    // directory names. Equal options reuse one cache rather than accumulating
    // a new folder on every apply. Product lifetime/choice rules: PRD 25.01.
    let key = crate::base64url(&sha2::Sha256::digest(arguments.as_bytes()));
    let path = std::env::temp_dir()
        .join("minicon-webview2-proxy")
        .join(key);
    std::fs::create_dir_all(&path).map_err(|_| "browser-native-profile-unavailable".to_owned())?;
    Ok(wry::WebContext::new(Some(path)))
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_proxy_validation_fails_closed_when_unavailable_invalid_or_split() {
        let proxy =
            crate::proxy_override(&BrowserProxy::default(), "http://127.0.0.1:8890").unwrap();
        assert_eq!(
            macos_proxy_endpoint(&proxy, true).unwrap(),
            Some(("127.0.0.1".into(), "8890".into()))
        );
        assert_eq!(
            macos_proxy_endpoint(&proxy, false),
            Err("browser-proxy-unsupported".into())
        );
        assert_eq!(
            macos_proxy_endpoint(&BrowserProxy::default(), false).unwrap(),
            None
        );
        assert_eq!(
            macos_proxy_endpoint(
                &BrowserProxy {
                    http: Some("invalid".into()),
                    ..BrowserProxy::default()
                },
                true
            ),
            Err("browser-proxy-invalid".into())
        );
        assert_eq!(
            macos_proxy_endpoint(
                &BrowserProxy {
                    http: Some("http://one.test:80".into()),
                    https: Some("http://two.test:80".into()),
                    no_proxy: None
                },
                true
            ),
            Err("browser-proxy-unsupported".into())
        );
    }

    #[test]
    fn new_navigation_replaces_stale_error_title_and_url_before_a_native_callback() {
        let mut state = NativePageState {
            url: "http://localhost/old".into(),
            title: "Earlier page".into(),
            error: Some("browser-navigation-error-7".into()),
            ..Default::default()
        };
        start_navigation(&mut state, "http://localhost/retry");
        assert_eq!(state.url, "http://localhost/retry");
        assert!(state.title.is_empty());
        assert!(state.error.is_none());
        assert!(state.loading && state.changed);
    }

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
