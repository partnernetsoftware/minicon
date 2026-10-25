//! Optional wry page load for `minicon-web-host`.
//!
//! Compiled only with `--features wry-host`. Failure is `webkit-host-not-linked`
//! so the process exits 69. This module does not call the OS browser.

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

#[derive(Debug)]
enum HostEvent {
    Loaded,
    Changed(bool, String),
    Title(String),
    Command,
    #[cfg(target_os = "linux")]
    Failed,
}

fn local_origin(url: &str) -> Option<String> {
    crate::webview_must_bypass_proxy(url).then(|| {
        let authority = url
            .strip_prefix("http://")
            .unwrap_or("")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        format!("http://{authority}/")
    })
}

fn emit(value: serde_json::Value) {
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    let _ = writeln!(output, "{value}");
    let _ = output.flush();
}

pub fn probe() -> Result<(), &'static str> {
    wry::webview_version()
        .map(|_| ())
        .map_err(|_| "webkit-host-not-linked")
}

pub fn load(
    url: &str,
    smoke: bool,
    slot: Option<crate::PanelSlot>,
    redirect_smoke: Option<&str>,
) -> Result<(), &'static str> {
    load_inner(url, smoke, slot, redirect_smoke, None)
}

#[cfg(target_os = "linux")]
pub fn load_with_context(
    url: &str,
    slot: Option<crate::PanelSlot>,
    context: &crate::PageContext,
) -> Result<(), crate::PageContextError> {
    context.validate()?;
    load_inner(url, false, slot, None, Some(context))
        .map_err(|_| crate::PageContextError::NativeCreateFailed)
}

fn load_inner(
    url: &str,
    smoke: bool,
    slot: Option<crate::PanelSlot>,
    redirect_smoke: Option<&str>,
    context: Option<&crate::PageContext>,
) -> Result<(), &'static str> {
    // Legacy helpers inherit their already prepared environment from spawn.
    // Explicit contexts bypass that resolver and set the native manager below.
    let proxy = context
        .map(|context| context.proxy.clone())
        .unwrap_or_else(
            || crate::resolve_proxy_from_process(&crate::BrowserProxyConfig::default()),
        );
    let event_loop = EventLoopBuilder::<HostEvent>::with_user_event().build();
    let page_loaded = event_loop.create_proxy();
    let redirect_delivered = event_loop.create_proxy();
    let ipc = if context.is_some() {
        slot.is_some()
    } else {
        std::env::var("MINICON_BROWSER_HOST_IPC").as_deref() == Ok("1")
    };
    // The reader may outpace GTK while a navigation blocks. Keep commands and
    // their wake events bounded together rather than queueing JSON in Tao.
    let (command_input, command_output) = std::sync::mpsc::sync_channel::<serde_json::Value>(8);
    if ipc {
        let commands = event_loop.create_proxy();
        std::thread::spawn(move || {
            use std::io::{BufRead, Read};
            let mut input = std::io::stdin().lock();
            loop {
                let mut bytes = Vec::new();
                match input.by_ref().take(8194).read_until(b'\n', &mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if bytes.len() > 8192 => break,
                    _ => {}
                }
                if let Ok(command) = serde_json::from_slice(&bytes) {
                    if command_input.send(command).is_err()
                        || commands.send_event(HostEvent::Command).is_err()
                    {
                        break;
                    }
                }
            }
        });
    }
    let builder_window = WindowBuilder::new()
        .with_title("MiniCon")
        .with_decorations(slot.is_none())
        // A panel helper must never appear as a floating top-level window,
        // including while GTK realizes it or an embedding step fails.
        .with_visible(!smoke && slot.is_none());
    #[cfg(target_os = "linux")]
    let builder_window = if slot.is_some() {
        use tao::platform::unix::WindowBuilderExtUnix;
        // Keep tao's default vbox. wry packs the WebView into that box with
        // expand+fill. Adding it onto the ApplicationWindow leaves WebKit's
        // X window at 1×1 inside the reparented frame.
        builder_window.with_skip_taskbar(true)
    } else {
        builder_window
    };
    let window = builder_window
        .build(&event_loop)
        .map_err(|_| "webkit-host-not-linked")?;
    if let Some(slot) = slot {
        size_embedded_view(&window, slot);
    }
    let local_origin = std::rc::Rc::new(std::cell::RefCell::new(local_origin(url)));
    let navigation_origin = local_origin.clone();
    let navigation_events = event_loop.create_proxy();
    let expected_redirect = redirect_smoke.map(str::to_owned);
    let mut native_context =
        context.map(|context| wry::WebContext::new(Some(context.profile.data_directory.clone())));
    let builder = match native_context.as_mut() {
        Some(context) => WebViewBuilder::new_with_web_context(context),
        None => WebViewBuilder::new().with_url(url),
    };
    let mut builder = builder.with_navigation_handler(move |next| {
        // ProxyConfig::Http cannot list ignore hosts. A loopback redirect
        // would otherwise leave this process and miss the OAuth bridge.
        // The initial URL remains a valid localhost page for live smoke
        // and preview use; only a later loopback navigation is a redirect.
        let explicitly_local = navigation_origin
            .borrow()
            .as_ref()
            .is_some_and(|origin| next.starts_with(origin));
        if !explicitly_local && crate::webview_must_bypass_proxy(&next) {
            let delivered = crate::deliver_loopback_get(&next).is_ok();
            if delivered && expected_redirect.as_deref() == Some(next.as_str()) {
                let _ = redirect_delivered.send_event(HostEvent::Loaded);
            }
            false
        } else {
            if ipc && next.len() <= 8192 {
                let _ = navigation_events.send_event(HostEvent::Changed(true, next));
            }
            true
        }
    });
    if smoke && redirect_smoke.is_none() {
        builder = builder.with_on_page_load_handler(move |event, _loaded| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                let _ = page_loaded.send_event(HostEvent::Loaded);
            }
        });
    }
    if ipc {
        let loads = event_loop.create_proxy();
        let titles = event_loop.create_proxy();
        builder = builder
            .with_on_page_load_handler(move |event, url| {
                if url.len() <= 8192 {
                    let _ = loads.send_event(HostEvent::Changed(
                        matches!(event, wry::PageLoadEvent::Started),
                        url,
                    ));
                }
            })
            .with_document_title_changed_handler(move |title| {
                let title = title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect();
                let _ = titles.send_event(HostEvent::Title(title));
            });
    }
    if context.is_none()
        && let Some((host, port)) = crate::wry_proxy_endpoint(&proxy)
    {
        builder =
            builder.with_proxy_config(wry::ProxyConfig::Http(wry::ProxyEndpoint { host, port }));
    }
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let built = if let Some(vbox) = window.default_vbox() {
            builder.build_gtk(vbox)
        } else {
            builder.build_gtk(window.gtk_window())
        };
        built.map_err(|_| "webkit-host-not-linked")?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder
        .build(&window)
        .map_err(|_| "webkit-host-not-linked")?;
    #[cfg(target_os = "linux")]
    if let Some(context) = context {
        use webkit2gtk::{
            NetworkProxyMode, NetworkProxySettings, WebViewExt, WebsiteDataManagerExt,
        };
        use wry::WebViewExtUnix;
        let manager = webview
            .webview()
            .website_data_manager()
            .ok_or("browser-context-profile-unavailable")?;
        let expected = context
            .profile
            .data_directory
            .canonicalize()
            .map_err(|_| "browser-context-profile-unavailable")?;
        let data = manager
            .base_data_directory()
            .map(|value| std::path::PathBuf::from(value.as_str()))
            .and_then(|path| path.canonicalize().ok());
        let cache = manager
            .base_cache_directory()
            .map(|value| std::path::PathBuf::from(value.as_str()))
            .and_then(|path| path.canonicalize().ok());
        if manager.is_ephemeral()
            || data.as_ref() != Some(&expected)
            || cache.as_ref() != Some(&expected)
        {
            return Err("browser-context-profile-unavailable");
        }
        let parts = context.proxy_parts().map_err(|error| error.code())?;
        if parts.http.is_none() && parts.https.is_none() {
            manager.set_network_proxy_settings(NetworkProxyMode::NoProxy, None);
        } else {
            let bypass: Vec<_> = parts.bypass.iter().map(String::as_str).collect();
            let mut settings = NetworkProxySettings::new(None, &bypass);
            if let Some((host, port)) = parts.http {
                settings.add_proxy_for_scheme("http", &format!("http://{host}:{port}"));
            }
            if let Some((host, port)) = parts.https {
                settings.add_proxy_for_scheme("https", &format!("http://{host}:{port}"));
            }
            manager.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut settings));
        }
        // The builder had no remote URL. No request precedes native policy.
        webview
            .load_url(url)
            .map_err(|_| "browser-context-load-failed")?;
    }
    if let Some(slot) = slot {
        // Size after the WebView is packed, then reparent the outer frame.
        size_embedded_view(&window, slot);
        embed_into_parent(&window, slot)?;
        size_embedded_view(&window, slot);
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::WidgetExt;
            use tao::platform::unix::WindowExtUnix;
            window.gtk_window().show_all();
            // Complete the initial GTK mapping before acknowledging ownership.
            // The selectors read back direct-parent, class and CARDINAL/32 PID
            // through the shared checked connection (mechanism owner: PRD 28).
            window.gtk_window().display().sync();
            let pid = std::process::id();
            if !crate::resize_embedded_page_for_process(slot, pid)
                || !crate::set_embedded_page_visible_for_process(slot.parent, pid, true)
            {
                return Err("webkit-embedding-unavailable");
            }
            if ipc {
                emit(serde_json::json!({"embedded_ready":{"pid":pid,"parent":slot.parent}}));
            }
        }
    }
    #[cfg(target_os = "linux")]
    if ipc {
        use gtk::prelude::WidgetExt;
        use tao::platform::unix::WindowExtUnix;
        use webkit2gtk::WebViewExt;
        use wry::WebViewExtUnix;
        let failures = event_loop.create_proxy();
        webview
            .webview()
            .connect_load_failed(move |_, _, _, error| {
                // Replacing an in-flight load cancels the old navigation. It is
                // not an error in the newer page and must not hide that page.
                if !error.matches(webkit2gtk::NetworkError::Cancelled) {
                    let _ = failures.send_event(HostEvent::Failed);
                }
                // MiniCon paints the failure in its own content panel. Stop
                // WebKit's stock error-page navigation from later reporting
                // Started/Finished and overwriting the typed failure.
                true
            });
        window
            .gtk_window()
            .connect_key_press_event(move |_, event| {
                let modifiers = event.state();
                let key = event.keyval().to_unicode().map(|key| match key {
                    // Shift changes these logical symbols on common
                    // layouts, while the host owns the unshifted chord.
                    '{' => '[',
                    '}' => ']',
                    '<' => ',',
                    key => key.to_ascii_uppercase(),
                });
                let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                let alt = modifiers.intersects(
                    gtk::gdk::ModifierType::MOD1_MASK | gtk::gdk::ModifierType::SUPER_MASK,
                );
                if control
                    && !alt
                    && key.is_some_and(|key| key == 'L' || (shift && "TODNWBHIPG[],".contains(key)))
                {
                    emit(serde_json::json!({"shortcut":key.unwrap().to_string()}));
                    gtk::glib::Propagation::Stop
                } else {
                    gtk::glib::Propagation::Proceed
                }
            });
    }
    // `EventLoop::run` does not return. Close ends the process.
    let mut title = String::new();
    let mut loading = true;
    let mut error: Option<&str> = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _keep = &webview;
        let _keep_context = &native_context;
        if smoke {
            if let Event::UserEvent(HostEvent::Loaded) = event {
                *control_flow = ControlFlow::Exit;
            }
        }
        if ipc {
            let changed = match &event {
                Event::UserEvent(HostEvent::Changed(active, _url)) => {
                    loading = *active;
                    if *active { error = None; }
                    true
                }
                #[cfg(target_os = "linux")]
                Event::UserEvent(HostEvent::Failed) => {
                    error = Some("browser-navigation-failed"); loading = false; true
                }
                Event::UserEvent(HostEvent::Title(next)) => {
                    title.clone_from(next);
                    true
                }
                Event::UserEvent(HostEvent::Command) => {
                    let Ok(command) = command_output.try_recv() else {
                        return;
                    };
                    let result = match command["action"].as_str() {
                        Some("load") => command["url"]
                            .as_str()
                            .filter(|url| crate::http_url_at(url, 0).as_deref() == Some(*url))
                            .map(|url| { *local_origin.borrow_mut() = self::local_origin(url); webview.load_url(url) }),
                        Some("back") => Some(webview.go_back()),
                        Some("forward") => Some(webview.go_forward()),
                        Some("reload") => Some(webview.reload()),
                        Some("focus") => {
                            #[cfg(target_os = "linux")]
                            let result = focus_page(&window, &webview);
                            #[cfg(not(target_os = "linux"))]
                            let result = webview.focus();
                            Some(result)
                        }
                        _ => None,
                    };
                    if result.is_some_and(|result| result.is_err()) {
                        emit(serde_json::json!({"error":"browser-host-command-failed"}));
                    }
                    false
                }
                _ => false,
            };
            if changed {
                emit(
                    serde_json::json!({"url":webview.url().unwrap_or_default(), "title":title,
                        "loading":loading, "error":error, "can_back":webview.can_go_back().unwrap_or(false),
                    "can_forward":webview.can_go_forward().unwrap_or(false)}),
                );
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
fn focus_page(window: &tao::window::Window, webview: &wry::WebView) -> wry::Result<()> {
    use gtk::glib::ObjectType;
    use gtk::prelude::WidgetExt;
    use std::ffi::c_void;
    use tao::platform::unix::WindowExtUnix;

    webview.focus()?;
    let Some(native) = window
        .gtk_window()
        .window()
        .filter(|native| native.is_viewable())
    else {
        return Ok(());
    };
    let Some(xid) = gdk_x11_xid(native.as_ptr().cast()) else {
        return Err(wry::Error::UnsupportedWindowHandle);
    };
    #[link(name = "gdk-3")]
    unsafe extern "C" {
        fn gdk_x11_display_get_xdisplay(display: *mut c_void) -> *mut c_void;
        fn gdk_x11_display_error_trap_push(display: *mut c_void);
        fn gdk_x11_display_error_trap_pop_ignored(display: *mut c_void);
    }
    #[link(name = "X11")]
    unsafe extern "C" {
        fn XSetInputFocus(display: *mut c_void, window: u64, revert_to: i32, time: u64) -> i32;
    }
    let display = native.display();
    let display = display.as_ptr().cast();
    // GtkWidget::grab_focus only changes the helper's internal focus owner.
    // GDK's toplevel focus uses a WM activation request, but this X window is
    // now a child of MiniCon, so direct X focus is required. Never map a hidden
    // page here; a concurrent hide may reject focus and is safely trapped.
    unsafe {
        let xdisplay = gdk_x11_display_get_xdisplay(display);
        if xdisplay.is_null() {
            return Err(wry::Error::UnsupportedWindowHandle);
        }
        gdk_x11_display_error_trap_push(display);
        XSetInputFocus(xdisplay, xid, 2, 0); // RevertToParent, CurrentTime.
        gdk_x11_display_error_trap_pop_ignored(display);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn size_embedded_view(window: &tao::window::Window, slot: crate::PanelSlot) {
    use gtk::prelude::{ContainerExt, GtkWindowExt, WidgetExt};
    use tao::platform::unix::WindowExtUnix;
    let gtk_window = window.gtk_window();
    let width = slot.width as i32;
    let height = slot.height as i32;
    window.set_inner_size(tao::dpi::PhysicalSize::new(slot.width, slot.height));
    gtk_window.set_default_size(width, height);
    gtk_window.resize(width, height);
    if let Some(vbox) = window.default_vbox() {
        vbox.set_size_request(width, height);
        let allocation = gtk::Allocation::new(0, 0, width, height);
        vbox.foreach(|child| {
            child.set_size_request(width, height);
            child.set_hexpand(true);
            child.set_vexpand(true);
            child.size_allocate(&allocation);
        });
        vbox.size_allocate(&allocation);
    }
}

#[cfg(not(target_os = "linux"))]
fn size_embedded_view(_window: &tao::window::Window, _slot: crate::PanelSlot) {}

#[cfg(target_os = "linux")]
fn embed_into_parent(
    window: &tao::window::Window,
    slot: crate::PanelSlot,
) -> Result<(), &'static str> {
    use gtk::glib::ObjectType;
    use gtk::prelude::WidgetExt;
    use tao::platform::unix::WindowExtUnix;
    window.gtk_window().realize();
    let gdk_window = window
        .gtk_window()
        .window()
        .ok_or("webkit-embedding-unavailable")?;
    let child = gdk_x11_xid(gdk_window.as_ptr().cast()).ok_or("webkit-embedding-unavailable")?;
    unsafe { x_reparent(child, slot) }
}

#[cfg(not(target_os = "linux"))]
fn embed_into_parent(
    _window: &tao::window::Window,
    _slot: crate::PanelSlot,
) -> Result<(), &'static str> {
    Err("webkit-embedding-unavailable")
}

#[cfg(target_os = "linux")]
fn gdk_x11_xid(gdk_window: *mut std::ffi::c_void) -> Option<u64> {
    use std::ffi::{CStr, c_char, c_void};
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
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
unsafe fn x_reparent(child: u64, slot: crate::PanelSlot) -> Result<(), &'static str> {
    use std::ffi::{c_char, c_ulong};
    #[link(name = "X11")]
    unsafe extern "C" {
        fn XOpenDisplay(name: *const c_char) -> *mut std::ffi::c_void;
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
        fn XSetClassHint(
            display: *mut std::ffi::c_void,
            window: u64,
            hints: *const ClassHint,
        ) -> i32;
        fn XInternAtom(
            display: *mut std::ffi::c_void,
            name: *const c_char,
            only_if_exists: i32,
        ) -> c_ulong;
        fn XChangeProperty(
            display: *mut std::ffi::c_void,
            window: u64,
            property: c_ulong,
            kind: c_ulong,
            format: i32,
            mode: i32,
            data: *const u8,
            count: i32,
        ) -> i32;
    }
    #[repr(C)]
    struct ClassHint {
        name: *const c_char,
        class: *const c_char,
    }
    let display = unsafe { XOpenDisplay(std::ptr::null()) };
    if display.is_null() {
        return Err("webkit-embedding-unavailable");
    }
    unsafe {
        let pid_atom = XInternAtom(display, c"_NET_WM_PID".as_ptr(), 0);
        if pid_atom == 0 {
            XCloseDisplay(display);
            return Err("webkit-embedding-unavailable");
        }
        // Xlib format-32 input uses native unsigned longs, including LP64.
        // This is this helper's process, not WebKit's renderer process.
        let pid = c_ulong::from(std::process::id());
        XChangeProperty(
            display,
            child,
            pid_atom,
            6, // XA_CARDINAL
            32,
            0, // PropModeReplace
            (&pid as *const c_ulong).cast(),
            1,
        );
        let hint = ClassHint {
            name: c"minicon-web-host".as_ptr(),
            class: c"minicon-web-host".as_ptr(),
        };
        XSetClassHint(display, child, &hint);
        XReparentWindow(display, child, slot.parent, slot.x, slot.y);
        XMoveResizeWindow(display, child, slot.x, slot.y, slot.width, slot.height);
        XMapWindow(display, child);
        XFlush(display);
        XCloseDisplay(display);
    }
    Ok(())
}
