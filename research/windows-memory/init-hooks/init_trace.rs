//! Research-only init RSS hook. Enabled by MINICON_INIT_TRACE=path.
//! Cheap: K32GetProcessMemoryInfo + GetModuleHandleW presence. Not a full QWS.

use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Instant,
};

use windows_sys::Win32::{
    Foundation::HMODULE,
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
};

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

type K32Fn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut ProcessMemoryCounters, u32) -> i32;

struct TraceState {
    file: Mutex<File>,
    start: Instant,
    seq: AtomicU32,
}

thread_local! {
    static CREATE_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PAINT_DURING_CREATE: Cell<u32> = const { Cell::new(0) };
}

static TRACE: OnceLock<Option<TraceState>> = OnceLock::new();
static K32: OnceLock<Option<K32Fn>> = OnceLock::new();
static FIRST_PRESENT: AtomicBool = AtomicBool::new(false);

fn k32() -> Option<K32Fn> {
    *K32.get_or_init(|| {
        let k32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let module = unsafe { GetModuleHandleW(k32.as_ptr()) };
        if module.is_null() {
            return None;
        }
        let name = b"K32GetProcessMemoryInfo\0";
        let addr = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        addr.map(|f| unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, K32Fn>(f) })
    })
}

fn working_set() -> u64 {
    let Some(get) = k32() else {
        return 0;
    };
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    let process = (-1isize) as *mut core::ffi::c_void;
    let ok = unsafe { get(process, &mut counters, counters.cb) };
    if ok == 0 {
        0
    } else {
        counters.working_set_size as u64
    }
}

fn module_loaded(name: &str) -> u8 {
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    wide.push(0);
    let handle: HMODULE = unsafe { GetModuleHandleW(wide.as_ptr()) };
    u8::from(!handle.is_null())
}

fn state() -> Option<&'static TraceState> {
    TRACE
        .get_or_init(|| {
            let path = std::env::var_os("MINICON_INIT_TRACE")?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()?;
            Some(TraceState {
                file: Mutex::new(file),
                start: Instant::now(),
                seq: AtomicU32::new(0),
            })
        })
        .as_ref()
}

pub fn enter_create() {
    CREATE_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
}

pub fn leave_create() {
    CREATE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
}

pub fn create_depth() -> u32 {
    CREATE_DEPTH.with(Cell::get)
}

pub fn note_paint_during_create() {
    if create_depth() > 0 {
        PAINT_DURING_CREATE.with(|c| c.set(c.get().saturating_add(1)));
    }
}

pub fn paint_during_create() -> u32 {
    PAINT_DURING_CREATE.with(Cell::get)
}

pub fn sample(label: &str) {
    let Some(trace) = state() else {
        return;
    };
    let seq = trace
        .seq
        .fetch_add(1, Ordering::Relaxed);
    let ms = trace.start.elapsed().as_secs_f64() * 1000.0;
    let ws = working_set();
    let depth = create_depth();
    let paints = paint_during_create();
    let tif = module_loaded("TextInputFramework.dll");
    let msctf = module_loaded("MSCTF.dll");
    let coremsg = module_loaded("CoreMessaging.dll");
    let coreui = module_loaded("CoreUIComponents.dll");
    let imm32 = module_loaded("imm32.dll");
    let line = format!(
        "seq={seq} t_ms={ms:.3} label={label} ws={ws} create_depth={depth} paint_during_create={paints} tif={tif} msctf={msctf} coremsg={coremsg} coreui={coreui} imm32={imm32}\n"
    );
    if let Ok(mut file) = trace.file.lock() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

pub fn sample_first_present() {
    if FIRST_PRESENT.swap(true, Ordering::AcqRel) {
        return;
    }
    sample("first_present_StretchDIBits");
}

pub fn warmup() {
    sample("probe_warmup_a");
    sample("probe_warmup_b");
}
