//! Optional wry page load for `minicon-web-host`.
//!
//! Compiled only with `--features wry-host`. Failure is `webkit-host-not-linked`
//! so the process exits 69. This module does not call the OS browser.

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

pub fn probe() -> Result<(), &'static str> {
    wry::webview_version()
        .map(|_| ())
        .map_err(|_| "webkit-host-not-linked")
}

pub fn load(url: &str, smoke: bool, slot: Option<crate::PanelSlot>) -> Result<(), &'static str> {
    // The parent installed MINICON_BROWSER_*. WebKit reads http_proxy.
    let proxy = crate::resolve_proxy_from_process(&crate::BrowserProxyConfig::default());
    // The host process is single-threaded until the event loop starts.
    for key in crate::webkit_proxy_clear(&proxy) {
        unsafe { std::env::remove_var(key) };
    }
    for (key, value) in crate::webkit_proxy_env(&proxy) {
        unsafe { std::env::set_var(key, value) };
    }
    let event_loop = EventLoop::new();
    let page_loaded = event_loop.create_proxy();
    let mut builder_window = WindowBuilder::new()
        .with_title("MiniCon")
        .with_decorations(slot.is_none())
        .with_visible(!smoke);
    #[cfg(target_os = "linux")]
    if slot.is_some() {
        use tao::platform::unix::WindowBuilderExtUnix;
        builder_window = builder_window
            .with_default_vbox(false)
            .with_skip_taskbar(true);
    }
    let window = builder_window
        .build(&event_loop)
        .map_err(|_| "webkit-host-not-linked")?;
    if let Some(slot) = slot {
        embed_into_parent(&window, slot);
    }
    let mut builder = WebViewBuilder::new().with_url(url);
    if smoke {
        builder = builder.with_on_page_load_handler(move |event, _loaded| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                let _ = page_loaded.send_event(());
            }
        });
    }
    if let Some((host, port)) = crate::wry_proxy_endpoint(&proxy) {
        builder =
            builder.with_proxy_config(wry::ProxyConfig::Http(wry::ProxyEndpoint { host, port }));
    }
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder
            .build_gtk(window.gtk_window())
            .map_err(|_| "webkit-host-not-linked")?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder
        .build(&window)
        .map_err(|_| "webkit-host-not-linked")?;
    // `EventLoop::run` does not return. Close ends the process.
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _keep = &webview;
        if smoke {
            if let Event::UserEvent(()) = event {
                *control_flow = ControlFlow::Exit;
            }
        }
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(target_os = "linux")]
fn embed_into_parent(window: &tao::window::Window, slot: crate::PanelSlot) {
    use gtk::glib::ObjectType;
    use gtk::prelude::WidgetExt;
    use tao::platform::unix::WindowExtUnix;
    let Some(gdk_window) = window.gtk_window().window() else {
        return;
    };
    let Some(child) = gdk_x11_xid(gdk_window.as_ptr().cast()) else {
        return;
    };
    unsafe {
        x_reparent(child, slot);
    }
}

#[cfg(not(target_os = "linux"))]
fn embed_into_parent(_window: &tao::window::Window, _slot: crate::PanelSlot) {}

#[cfg(target_os = "linux")]
fn gdk_x11_xid(gdk_window: *mut std::ffi::c_void) -> Option<u64> {
    use std::ffi::{CStr, c_void};
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
    }
    let symbol = CStr::from_bytes_with_nul(b"gdk_x11_window_get_xid\0").ok()?;
    let symbol = unsafe { dlsym(std::ptr::null_mut(), symbol.as_ptr()) };
    if symbol.is_null() {
        return None;
    }
    let get_xid: unsafe extern "C" fn(*mut std::ffi::c_void) -> u64 =
        unsafe { std::mem::transmute(symbol) };
    let xid = unsafe { get_xid(gdk_window) };
    if xid == 0 { None } else { Some(xid) }
}

#[cfg(target_os = "linux")]
unsafe fn x_reparent(child: u64, slot: crate::PanelSlot) {
    #[link(name = "X11")]
    unsafe extern "C" {
        fn XOpenDisplay(name: *const i8) -> *mut std::ffi::c_void;
        fn XReparentWindow(
            display: *mut std::ffi::c_void,
            window: u64,
            parent: u64,
            x: i32,
            y: i32,
        ) -> i32;
        fn XMoveResizeWindow(
            display: *mut std::ffi::c_void,
            window: u64,
            x: i32,
            y: i32,
            width: u32,
            height: u32,
        ) -> i32;
        fn XMapWindow(display: *mut std::ffi::c_void, window: u64) -> i32;
        fn XFlush(display: *mut std::ffi::c_void) -> i32;
        fn XCloseDisplay(display: *mut std::ffi::c_void) -> i32;
    }
    let display = unsafe { XOpenDisplay(std::ptr::null()) };
    if display.is_null() {
        return;
    }
    unsafe {
        XReparentWindow(display, child, slot.parent, slot.x, slot.y);
        XMoveResizeWindow(display, child, slot.x, slot.y, slot.width, slot.height);
        XMapWindow(display, child);
        XFlush(display);
        XCloseDisplay(display);
    }
}
