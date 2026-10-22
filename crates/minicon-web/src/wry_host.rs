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

pub fn load(url: &str) -> Result<(), &'static str> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("MiniCon")
        .build(&event_loop)
        .map_err(|_| "webkit-host-not-linked")?;
    let builder = WebViewBuilder::new().with_url(url);
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
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _keep = &webview;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
    Ok(())
}
