//! Thread-local allocation traffic, not a wall-clock performance threshold.
use minicon_console::terminal::TerminalPresentation;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Default)]
struct Traffic {
    allocations: usize,
    bytes: usize,
}
thread_local! { static TRACK: Cell<Option<Traffic>> = const { Cell::new(None) }; }
struct Counting;
fn account(bytes: usize) {
    let _ = TRACK.try_with(|track| {
        if let Some(mut traffic) = track.get() {
            traffic.allocations += 1;
            traffic.bytes += bytes;
            track.set(Some(traffic));
        }
    });
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        account(layout.size());
        // SAFETY: forward the caller's unchanged allocation request.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        account(layout.size());
        // SAFETY: forward the caller's unchanged allocation request.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: every allocation above belongs to the same System allocator.
        unsafe {
            System.dealloc(ptr, layout);
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        account(size);
        // SAFETY: preserve the original allocation and requested replacement size.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn measure(work: impl FnOnce()) -> (Traffic, u128) {
    let start = Instant::now();
    TRACK.with(|track| track.set(Some(Traffic::default())));
    work();
    let traffic = TRACK.with(|track| track.replace(None).unwrap());
    (traffic, start.elapsed().as_nanos())
}

#[test]
fn synchronized_presentation_history_workload_bounds_allocation_traffic_and_keeps_frozen_cells() {
    let mut t = TerminalPresentation::new(24, 80, 4096, Duration::from_secs(1), 65536);
    let source = b"history\r\n".repeat(3200);
    t.process(&source, Duration::ZERO);
    let history = t.active_screen().scrollback_depth();
    assert!(history >= 3000);
    let _ = t.take_damage();
    let (ascii, ascii_ns) = measure(|| {
        for _ in 0..500 {
            t.process(b"\r*", Duration::ZERO);
            let _ = t.take_damage();
        }
    });
    assert_eq!((ascii.allocations, ascii.bytes), (0, 0));
    let (csi, csi_ns) = measure(|| {
        for _ in 0..500 {
            t.process(b"\x1b[1G*", Duration::ZERO);
            let _ = t.take_damage();
        }
    });
    assert_eq!((csi.allocations, csi.bytes), (0, 0));
    let mut raw = vt100::Parser::new(24, 80, 4096);
    raw.process(&source);
    let (raw_traffic, raw_ns) = measure(|| {
        for _ in 0..500 {
            raw.process(b"\x1b[1G*");
            let _ = raw.take_damage();
        }
    });
    assert_eq!((raw_traffic.allocations, raw_traffic.bytes), (0, 0));
    let (snapshot, snapshot_ns) = measure(|| {
        t.process(b"\x1b[?2026h", Duration::ZERO);
    });
    assert!(t.is_holding());
    assert!(
        snapshot.allocations <= 6,
        "snapshot allocations={};bytes={}",
        snapshot.allocations,
        snapshot.bytes
    );
    assert!(
        snapshot.bytes <= 256 * 1024,
        "history cell arrays were copied: {}",
        snapshot.bytes
    );
    assert_eq!(t.presented_screen().cell(0, 0).unwrap().contents(), "h");
    let (edit, edit_ns) = measure(|| {
        t.process(b"\x1b[1;1H\x1b[2KZ", Duration::ZERO);
    });
    assert!(edit.allocations <= 6 && edit.bytes <= 32 * 1024);
    assert_eq!(t.active_screen().cell(0, 0).unwrap().contents(), "Z");
    assert_eq!(t.presented_screen().cell(0, 0).unwrap().contents(), "h");
    t.process(b"\x1b[?2026l", Duration::ZERO);
    assert_eq!(t.presented_screen().cell(0, 0).unwrap().contents(), "Z");
    assert!(t.take_damage().needs_full_raster());
    t.process(b"\x1b]2;", Duration::ZERO);
    t.process(&vec![b'x'; 32768], Duration::ZERO);
    let (osc, osc_ns) = measure(|| {
        t.process(b"more", Duration::ZERO);
    });
    assert!(osc.allocations <= 2 && osc.bytes <= 256);
    t.process(b"\x07", Duration::ZERO);
    struct Controls(usize);
    impl vte::Perform for Controls {
        fn csi_dispatch(
            &mut self,
            params: &vte::Params,
            intermediates: &[u8],
            ignore: bool,
            action: char,
        ) {
            if !ignore
                && intermediates == b"?"
                && action == 'h'
                && params.iter().any(|p| p == [2026])
            {
                self.0 += 1;
            }
        }
    }
    let grammar = vt100::Parser::new(1, 1, 0);
    let mut long = b"\x1b]2;".to_vec();
    long.extend_from_slice(&vec![b'x'; 32768]);
    long.extend_from_slice(b"\x07\x1b[?2026h");
    let mut controls = Controls(0);
    let (inspected, inspected_ns) = measure(|| {
        grammar.inspect_control_sequences(&long, &mut controls);
    });
    assert_eq!(controls.0, 1);
    // std Vec may grow geometrically within the bounded scratch span; the
    // no_std array needs no heap. Keep both variants under the same byte cap.
    assert!(
        inspected.allocations <= 8 && inspected.bytes <= 256,
        "inspection allocations={} bytes={}",
        inspected.allocations,
        inspected.bytes
    );
    eprintln!(
        "presentation-cost {}",
        serde_json::json!({
            "debug":cfg!(debug_assertions),"history_rows":history,"rows":24,"cols":80,"iterations":500,
            "ascii":{"allocations":ascii.allocations,"bytes":ascii.bytes,"ns":ascii_ns},
            "csi":{"allocations":csi.allocations,"bytes":csi.bytes,"ns":csi_ns},
            "raw_csi":{"allocations":raw_traffic.allocations,"bytes":raw_traffic.bytes,"ns":raw_ns},
            "snapshot":{"allocations":snapshot.allocations,"bytes":snapshot.bytes,"ns":snapshot_ns},
            "row_edit":{"allocations":edit.allocations,"bytes":edit.bytes,"ns":edit_ns},
            "osc_tail":{"allocations":osc.allocations,"bytes":osc.bytes,"ns":osc_ns},
            "inspection":{"allocations":inspected.allocations,"bytes":inspected.bytes,"ns":inspected_ns}
        })
    );
}
