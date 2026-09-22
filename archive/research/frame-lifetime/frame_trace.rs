use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct State {
    started: Instant,
    next: u64,
    live: BTreeMap<usize, (u64, usize)>,
}
static TRACE: OnceLock<Option<Mutex<State>>> = OnceLock::new();
unsafe extern "C" {
    fn getpagesize() -> i32;
    fn mincore(address: *const std::ffi::c_void, length: usize, vector: *mut i8) -> i32;
}
fn state() -> Option<&'static Mutex<State>> {
    TRACE.get_or_init(|| {
        if std::env::var_os("MINICON_FRAME_TRACE").is_none() { return None; }
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if let Some(Some(lock)) = TRACE.get() {
                    if let Ok(state) = lock.lock() {
                        for (&address, &(id, bytes)) in &state.live {
                            emit(&state, "sample", address, id, bytes);
                        }
                    }
                }
            }
        });
        Some(Mutex::new(State { started: Instant::now(), next: 0, live: BTreeMap::new() }))
    }).as_ref()
}
fn emit(state: &State, event: &str, address: usize, id: u64, bytes: usize) {
    let page = unsafe { getpagesize() } as usize;
    let mut pages = vec![0_i8; bytes.div_ceil(page)];
    let result = unsafe { mincore(address as *const _, bytes, pages.as_mut_ptr()) };
    let resident = if result == 0 { pages.iter().filter(|p| **p & 1 != 0).count() * page } else { 0 };
    let paged_out = pages.iter().filter(|p| **p & 0x20 != 0).count() * page;
    let copied = pages.iter().filter(|p| **p & 0x40 != 0).count() * page;
    let modified = pages.iter().filter(|p| **p & 4 != 0).count() * page;
    eprintln!("FRAME t_us={} event={} id={} ptr={:#x} bytes={} mincore_rc={} resident={} paged_out={} copied={} modified={} live={}", state.started.elapsed().as_micros(), event, id, address, bytes, result, resident, paged_out, copied, modified, state.live.len());
}
pub fn allocated(address: usize, bytes: usize) {
    if let Some(lock) = state() {
        if let Ok(mut state) = lock.lock() {
            state.next += 1;
            let id = state.next;
            state.live.insert(address, (id, bytes));
            emit(&state, "allocate", address, id, bytes);
        }
    }
}
pub fn event(event: &str, address: usize) {
    if let Some(lock) = state() {
        if let Ok(state) = lock.lock() {
            if let Some(&(id, bytes)) = state.live.get(&address) { emit(&state, event, address, id, bytes); }
        }
    }
}
pub fn releasing(address: usize) {
    if let Some(lock) = state() {
        if let Ok(mut state) = lock.lock() {
            if let Some((id, bytes)) = state.live.remove(&address) { emit(&state, "release", address, id, bytes); }
        }
    }
}
