use agenterm_platform::window_host::*;
use std::io::Write;
use std::time::Instant;

struct Probe { rendered: bool, ready: bool, next_frame: Option<Instant> }
impl PixelWindowApplication for Probe {
    fn opened(&mut self, window: &PixelWindow) -> Result<PixelWindowDirective, PixelWindowError> {
        window.set_ime_cursor_area(LogicalRect::new(20.0, 20.0, 10.0, 20.0))?;
        window.request_redraw();
        Ok(PixelWindowDirective::Continue)
    }
    fn event(&mut self, _: &PixelWindow, _: PixelWindowEvent) -> Result<PixelWindowDirective, PixelWindowError> {
        Ok(PixelWindowDirective::Continue)
    }
    fn render(&mut self, window: &PixelWindow, frame: &mut XrgbPixelFrame<'_>) -> Result<PixelWindowDirective, PixelWindowError> {
        let metrics = window.metrics()?;
        let width = metrics.physical_width as usize;
        for (i, pixel) in frame.pixels_mut().iter_mut().enumerate() {
            *pixel = if ((i % width) / 32 + (i / width) / 32) & 1 == 1 { 0x00304050 } else { 0x00102030 };
        }
        frame.commit(PixelFrameWrite::Full)
            .map_err(|error| PixelWindowError::failed("probe_frame", error))?;
        self.rendered = true;
        eprintln!("FRAME {}x{}", metrics.physical_width, metrics.physical_height);
        Ok(PixelWindowDirective::Continue)
    }
    fn about_to_wait(&mut self, window: &PixelWindow, now: Instant) -> Result<PixelWindowDirective, PixelWindowError> {
        if self.rendered && !self.ready {
            println!("READY");
            std::io::stdout().flush().expect("probe stdout");
            self.ready = true;
        }
        if let Some(deadline) = self.next_frame {
            let next = if now >= deadline {
                window.request_redraw();
                now + std::time::Duration::from_millis(500)
            } else { deadline };
            self.next_frame = Some(next);
            return Ok(PixelWindowDirective::WaitUntil(next));
        }
        Ok(PixelWindowDirective::Wait)
    }
}
fn main() {
    let options = PixelWindowOptions::new("Memory research", LogicalSize::new(960.0, 600.0))
        .with_no_activate(true).with_ime_allowed(true);
    run_pixel_window(options, Box::new(Probe { rendered: false, ready: false, next_frame: std::env::var_os("PROBE_TICK").map(|_| Instant::now() + std::time::Duration::from_millis(500)) })).expect("pixel probe");
}
