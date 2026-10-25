//! Explicit owned-window native court. Owner: PRD 28 / PRD 32.
//! Never part of an ordinary unattended test run; no AX/global input or WebView.

#[cfg(target_os = "macos")]
mod macos {
    use std::{cell::RefCell, rc::Rc, time::Instant};

    use minicon_platform::window_host::{
        LogicalSize, PixelWindow, PixelWindowApplication, PixelWindowDirective, PixelWindowError,
        PixelWindowEvent, PixelWindowOptions, WindowHalf, XrgbPixelFrame, run_pixel_window,
    };
    use objc2_app_kit::NSView;
    use objc2_foundation::{MainThreadMarker, NSRect, NSSize};

    const TOLERANCE_POINTS: f64 = 1.0;

    #[derive(Clone, Copy)]
    struct Sample {
        stage: &'static str,
        visible: [f64; 4],
        expected: [f64; 4],
        actual: [f64; 4],
    }

    fn rect(value: NSRect) -> [f64; 4] {
        [
            value.origin.x,
            value.origin.y,
            value.size.width,
            value.size.height,
        ]
    }

    fn close(actual: [f64; 4], expected: [f64; 4]) -> bool {
        actual
            .into_iter()
            .zip(expected)
            .all(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() <= TOLERANCE_POINTS)
    }

    fn half(visible: [f64; 4], right: bool) -> [f64; 4] {
        [
            visible[0] + if right { visible[2] / 2.0 } else { 0.0 },
            visible[1],
            visible[2] / 2.0,
            visible[3],
        ]
    }

    struct App {
        result: Rc<RefCell<Option<Vec<Sample>>>>,
    }

    impl PixelWindowApplication for App {
        fn opened(&mut self, host: &PixelWindow) -> Result<PixelWindowDirective, PixelWindowError> {
            let _main_thread = MainThreadMarker::new().ok_or_else(|| {
                PixelWindowError::failed("snap_court_not_main_thread", "main thread required")
            })?;
            let address = host
                .embed_parent()
                .ok_or_else(|| PixelWindowError::unsupported("owned AppKit NSView unavailable"))?;
            let address = usize::try_from(address).map_err(|_| {
                PixelWindowError::failed("snap_court_invalid_parent", "parent identity overflow")
            })?;
            // SAFETY: the public macOS embed-parent contract supplies the live
            // owned NSView. This borrow stays inside opened on the AppKit thread,
            // while host retains its native window; nothing escapes the callback.
            let view = unsafe { (address as *const NSView).as_ref() }.ok_or_else(|| {
                PixelWindowError::failed("snap_court_invalid_parent", "null owned view")
            })?;
            let window = view
                .window()
                .ok_or_else(|| PixelWindowError::unsupported("owned NSWindow unavailable"))?;
            let screen = window
                .screen()
                .ok_or_else(|| PixelWindowError::unsupported("owned window screen unavailable"))?;
            let initial_visible = rect(screen.visibleFrame());
            let initial_frame = rect(window.frame());
            if !initial_visible.into_iter().all(f64::is_finite)
                || initial_visible[2] <= 0.0
                || initial_visible[3] <= 0.0
                || close(initial_frame, half(initial_visible, false))
                || close(initial_frame, half(initial_visible, true))
            {
                return Err(PixelWindowError::unsupported(
                    "owned fixture requires distinct initial and half-screen frames",
                ));
            }
            assert!(!window.isKeyWindow(), "owned no-activate window became key");
            let mut samples = Vec::with_capacity(3);
            let mut last_right = None;
            for (stage, side, right) in [
                ("left", WindowHalf::Left, false),
                ("right", WindowHalf::Right, true),
                ("repeat-right", WindowHalf::Right, true),
            ] {
                // Independent oracle: read the actual owning window's current
                // NSScreen before each call, not the adapter's return receipt.
                let screen = window.screen().ok_or_else(|| {
                    PixelWindowError::unsupported("current owned screen unavailable")
                })?;
                let visible = rect(screen.visibleFrame());
                let expected = half(visible, right);
                let operation = host.snap_to_half(side);
                let actual = rect(window.frame());
                // Deliberately before checking operation: removing setFrame
                // still hits this independent geometry assertion even if the
                // adapter's own readback correctly returns a typed error.
                assert!(
                    close(actual, expected),
                    "owned snap frame mismatch at {stage}: actual={actual:?}, expected={expected:?}"
                );
                assert!(operation.is_ok(), "owned snap API rejected {stage}");
                assert!(!window.isKeyWindow(), "owned snap activated the window");
                let after_screen = window.screen().ok_or_else(|| {
                    PixelWindowError::unsupported("snapped owned screen unavailable")
                })?;
                assert!(
                    close(rect(after_screen.visibleFrame()), visible),
                    "owned snap changed the current screen"
                );
                if stage == "repeat-right" {
                    assert!(
                        last_right.is_some_and(|prior| close(actual, prior)),
                        "repeat-right was not idempotent"
                    );
                }
                if right {
                    last_right = Some(actual);
                }
                samples.push(Sample {
                    stage,
                    visible,
                    expected,
                    actual,
                });
            }
            // Winit uses content constraints, which setFrame:display: does not
            // enforce. A refused operation must preserve the actual owned frame.
            let prior_minimum = unsafe { window.contentMinSize() };
            unsafe {
                window.setContentMinSize(NSSize::new(initial_visible[2] + 1.0, 0.0));
            }
            let frame_minimum = unsafe { window.minSize() };
            let content_minimum = unsafe { window.contentMinSize() };
            let before_refusal = rect(window.frame());
            let refused = host.snap_to_half(WindowHalf::Left);
            let after_refusal = rect(window.frame());
            unsafe { window.setContentMinSize(prior_minimum) };
            let (category, reason) = match &refused {
                Err(PixelWindowError::Unsupported { reason }) => {
                    let fixed_reason = match reason.as_ref() {
                        "window size constraints do not permit this screen half" => {
                            "frame-size-preflight"
                        }
                        "window content constraints do not permit this screen half" => {
                            "content-size-preflight"
                        }
                        _ => "other",
                    };
                    ("unsupported", fixed_reason)
                }
                Err(PixelWindowError::Failed { .. }) => ("failed", "other"),
                Err(_) => ("other-error", "other"),
                Ok(()) => ("not-refused", "none"),
            };
            eprintln!(
                "{{\"kind\":\"owned-size-preflight-observation\",\"frame_minimum\":{},\"content_minimum\":{},\"refusal_category\":\"{}\",\"refusal_reason\":\"{}\"}}",
                json_size(frame_minimum),
                json_size(content_minimum),
                category,
                reason
            );
            assert!(
                close(after_refusal, before_refusal),
                "content-size refusal moved the owned window: before={before_refusal:?}, after={after_refusal:?}"
            );
            assert!(
                refused.is_err(),
                "incompatible content minimum was accepted"
            );
            *self.result.borrow_mut() = Some(samples);
            Ok(PixelWindowDirective::Exit)
        }

        fn event(
            &mut self,
            _window: &PixelWindow,
            _event: PixelWindowEvent,
        ) -> Result<PixelWindowDirective, PixelWindowError> {
            Ok(PixelWindowDirective::Exit)
        }

        fn render(
            &mut self,
            _window: &PixelWindow,
            _frame: &mut XrgbPixelFrame<'_>,
        ) -> Result<PixelWindowDirective, PixelWindowError> {
            Ok(PixelWindowDirective::Exit)
        }

        fn about_to_wait(
            &mut self,
            _window: &PixelWindow,
            _now: Instant,
        ) -> Result<PixelWindowDirective, PixelWindowError> {
            Ok(PixelWindowDirective::Exit)
        }
    }

    fn json_size(value: NSSize) -> String {
        if value.width.is_finite() && value.height.is_finite() {
            format!("[{},{}]", value.width, value.height)
        } else {
            "null".to_owned()
        }
    }

    fn json_rect(value: [f64; 4]) -> String {
        format!("[{},{},{},{}]", value[0], value[1], value[2], value[3])
    }

    pub fn run() -> i32 {
        let result = Rc::new(RefCell::new(None));
        let app = App {
            result: Rc::clone(&result),
        };
        let options = PixelWindowOptions::new(
            "MiniCon owned half-screen court",
            LogicalSize::new(520.0, 340.0),
        )
        .with_no_activate(true);
        match run_pixel_window(options, Box::new(app)) {
            Ok(()) => {}
            Err(PixelWindowError::Unsupported { .. }) => {
                println!(
                    "{{\"status\":\"BLOCKED\",\"reason\":\"owned-native-capability-unavailable\"}}"
                );
                return 2;
            }
            Err(_) => {
                println!("{{\"status\":\"FAIL\",\"reason\":\"owned-native-window-error\"}}");
                return 1;
            }
        }
        let result = result.borrow();
        let Some(samples) = result.as_ref().filter(|samples| samples.len() == 3) else {
            println!("{{\"status\":\"FAIL\",\"reason\":\"owned-sequence-incomplete\"}}");
            return 1;
        };
        let rows = samples
            .iter()
            .map(|sample| {
                format!(
                    "{{\"stage\":\"{}\",\"visible\":{},\"expected\":{},\"actual\":{},\"pass\":true}}",
                    sample.stage,
                    json_rect(sample.visible),
                    json_rect(sample.expected),
                    json_rect(sample.actual)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        // Emit PASS only after the event loop returned normally and the owned
        // window/surface cleanup path has completed.
        println!(
            "{{\"status\":\"PASS\",\"scope\":\"owned-macos-adapter\",\"steps\":3,\"content_constraint_refused\":true,\"tolerance_points\":1,\"no_activate\":true,\"samples\":[{rows}]}}"
        );
        0
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    std::process::exit(macos::run());
    #[cfg(not(target_os = "macos"))]
    {
        println!("{{\"status\":\"UNSUPPORTED\",\"reason\":\"macos-owned-adapter-only\"}}");
        std::process::exit(2);
    }
}
