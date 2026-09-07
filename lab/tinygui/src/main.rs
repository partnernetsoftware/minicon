#![cfg_attr(windows, windows_subsystem = "windows")]

//! MiniCon-shaped empty pixel window.
//!
//! Same `run_pixel_window` host as MiniCon: unix `portable-pixel-window`
//! (winit + vendored softbuffer), Windows `native-pixel-window`. No PTY,
//! font, IME, blink, control, or MiniCon UI. One full present, then `Wait`.

use agenterm_platform::window_host::{
    LogicalSize, PixelFrameWrite, PixelWindow, PixelWindowApplication, PixelWindowDirective,
    PixelWindowError, PixelWindowEvent, PixelWindowOptions, XrgbPixelFrame, run_pixel_window,
};

struct TinyGui {
    presented: bool,
}

impl PixelWindowApplication for TinyGui {
    fn opened(&mut self, window: &PixelWindow) -> Result<PixelWindowDirective, PixelWindowError> {
        window.request_redraw();
        Ok(PixelWindowDirective::Continue)
    }

    fn event(
        &mut self,
        _window: &PixelWindow,
        event: PixelWindowEvent,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        match event {
            PixelWindowEvent::CloseRequested => Ok(PixelWindowDirective::Exit),
            _ => Ok(PixelWindowDirective::Continue),
        }
    }

    fn render(
        &mut self,
        _window: &PixelWindow,
        frame: &mut XrgbPixelFrame<'_>,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        for pixel in frame.pixels_mut().iter_mut() {
            *pixel = 0x0018_1818;
        }
        frame
            .commit(PixelFrameWrite::Full)
            .map_err(|error| PixelWindowError::failed("tinygui_frame", error.to_string()))?;
        self.presented = true;
        Ok(PixelWindowDirective::Continue)
    }

    fn about_to_wait(
        &mut self,
        _window: &PixelWindow,
        _now: std::time::Instant,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        Ok(PixelWindowDirective::Wait)
    }
}

fn main() {
    let options = PixelWindowOptions::new("tinygui", LogicalSize::new(960.0, 600.0))
        .with_no_activate(true)
        .with_ime_allowed(false);
    run_pixel_window(options, Box::new(TinyGui { presented: false }))
        .expect("tinygui pixel window");
}
