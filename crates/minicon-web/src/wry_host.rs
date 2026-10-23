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

pub fn load(url: &str, smoke: bool) -> Result<(), &'static str> {
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
    let window = WindowBuilder::new()
        .with_title("MiniCon")
        .with_visible(!smoke)
        .build(&event_loop)
        .map_err(|_| "webkit-host-not-linked")?;
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
