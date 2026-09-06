//! Research-only init RSS hook. Enabled by MINICON_INIT_TRACE=path.
//! Cheap: K32GetProcessMemoryInfo + GetModuleHandleW presence. Not a full QWS.
//! Also records foreground HWND/focus. Optional post-present SetForegroundWindow
//! + English SendInput. Does not write the PTY via control.

use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    io::Write,
    mem,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
    },
    time::Instant,
};

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, HMODULE},
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
    UI::{
        Input::KeyboardAndMouse::{
            GetFocus, GetKeyboardLayout, GetKeyboardLayoutList, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
            KEYEVENTF_KEYUP, SendInput, SetFocus, VK_A, VK_B, VK_C,
        },
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow},
    },
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
type GetCurrentThreadIdFn = unsafe extern "system" fn() -> u32;
type AttachThreadInputFn = unsafe extern "system" fn(u32, u32, i32) -> i32;

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
static GET_TID: OnceLock<Option<GetCurrentThreadIdFn>> = OnceLock::new();
static ATTACH: OnceLock<Option<AttachThreadInputFn>> = OnceLock::new();
static FIRST_PRESENT: AtomicBool = AtomicBool::new(false);
static LAST_TIF: AtomicU32 = AtomicU32::new(0);
static HWND_STORE: AtomicIsize = AtomicIsize::new(0);
static ACTIVATE_DONE: AtomicBool = AtomicBool::new(false);

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

fn current_tid() -> u32 {
    let fun = GET_TID.get_or_init(|| {
        let k32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let module = unsafe { GetModuleHandleW(k32.as_ptr()) };
        if module.is_null() {
            return None;
        }
        let name = b"GetCurrentThreadId\0";
        let addr = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        addr.map(|f| unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, GetCurrentThreadIdFn>(f)
        })
    });
    fun.map(|f| unsafe { f() }).unwrap_or(0)
}

fn attach_thread_input(id_attach: u32, id_attach_to: u32, attach: i32) -> i32 {
    let fun = ATTACH.get_or_init(|| {
        let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let module = unsafe { GetModuleHandleW(user32.as_ptr()) };
        if module.is_null() {
            return None;
        }
        let name = b"AttachThreadInput\0";
        let addr = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        addr.map(|f| unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, AttachThreadInputFn>(f)
        })
    });
    fun.map(|f| unsafe { f(id_attach, id_attach_to, attach) }).unwrap_or(0)
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

pub fn set_hwnd(hwnd: HWND) {
    HWND_STORE.store(hwnd as isize, Ordering::Release);
}

fn focus_fields() -> (isize, isize, isize, u8, u8) {
    let ours = HWND_STORE.load(Ordering::Acquire);
    let fg = unsafe { GetForegroundWindow() } as isize;
    let focus = unsafe { GetFocus() } as isize;
    let fg_ours = u8::from(ours != 0 && fg == ours);
    let focus_ours = u8::from(ours != 0 && focus == ours);
    (ours, fg, focus, fg_ours, focus_ours)
}

pub fn sample(label: &str) {
    let Some(trace) = state() else {
        return;
    };
    let seq = trace.seq.fetch_add(1, Ordering::Relaxed);
    let ms = trace.start.elapsed().as_secs_f64() * 1000.0;
    let ws = working_set();
    let depth = create_depth();
    let paints = paint_during_create();
    let tif = module_loaded("TextInputFramework.dll");
    let msctf = module_loaded("MSCTF.dll");
    let coremsg = module_loaded("CoreMessaging.dll");
    let coreui = module_loaded("CoreUIComponents.dll");
    let imm32 = module_loaded("imm32.dll");
    let prev_tif = LAST_TIF.swap(u32::from(tif), Ordering::AcqRel);
    let edge = if prev_tif == 0 && tif == 1 { 1 } else { 0 };
    let (hwnd, fg, focus, fg_ours, focus_ours) = focus_fields();
    let line = format!(
        "seq={seq} t_ms={ms:.3} label={label} ws={ws} create_depth={depth} paint_during_create={paints} tif={tif} tif_edge={edge} msctf={msctf} coremsg={coremsg} coreui={coreui} imm32={imm32} hwnd=0x{hwnd:x} fg=0x{fg:x} focus=0x{focus:x} fg_ours={fg_ours} focus_ours={focus_ours}\n"
    );
    if let Ok(mut file) = trace.file.lock() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

pub fn sample_stretch(which: &str, side: &str) {
    sample(&format!("StretchDIBits_{which}_{side}"));
    if side == "after" && create_depth() == 0 {
        if !FIRST_PRESENT.swap(true, Ordering::AcqRel) {
            sample("first_recorded_present");
        }
    }
}

pub fn skip_focus() -> bool {
    std::env::var_os("MINICON_INIT_SKIP_FOCUS").is_some()
}

fn activate_after_present() -> bool {
    std::env::var_os("MINICON_INIT_ACTIVATE_AFTER_PRESENT").is_some()
}

fn key_input(vk: u16, up: bool) -> INPUT {
    let mut input: INPUT = unsafe { mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        wScan: 0,
        dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

fn force_foreground(hwnd: HWND) -> (i32, u32, i32) {
    let fg = unsafe { GetForegroundWindow() };
    let mut fg_pid = 0u32;
    let fg_tid = unsafe { GetWindowThreadProcessId(fg, &mut fg_pid) };
    let our_tid = current_tid();
    let attached = if fg_tid != 0 && our_tid != 0 && fg_tid != our_tid {
        attach_thread_input(fg_tid, our_tid, 1)
    } else {
        0
    };
    let fg_ok = unsafe { SetForegroundWindow(hwnd) };
    let fg_err = unsafe { GetLastError() };
    let _focus = unsafe { SetFocus(hwnd) };
    if attached != 0 {
        attach_thread_input(fg_tid, our_tid, 0);
    }
    (fg_ok, fg_err, attached)
}

fn send_english_abc() -> u32 {
    let inputs = [
        key_input(VK_A, false),
        key_input(VK_A, true),
        key_input(VK_B, false),
        key_input(VK_B, true),
        key_input(VK_C, false),
        key_input(VK_C, true),
    ];
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<INPUT>() as i32,
        )
    }
}

fn chinese_layout_probe() -> (u32, u32, u8) {
    let mut layouts: [*mut core::ffi::c_void; 16] = [core::ptr::null_mut(); 16];
    let n = unsafe { GetKeyboardLayoutList(layouts.len() as i32, layouts.as_mut_ptr()) }.max(0) as usize;
    let n = n.min(layouts.len());
    let current = unsafe { GetKeyboardLayout(0) } as usize;
    let mut has_zh = 0u8;
    for layout in layouts.iter().take(n) {
        let langid = (*layout as usize as u32) & 0xffff;
        if langid & 0x3ff == 0x04 {
            has_zh = 1;
            break;
        }
    }
    (n as u32, (current & 0xffff) as u32, has_zh)
}

/// After first present (not during WM_PAINT): real activate, English keys,
/// then Chinese IME probe. Never writes the PTY via control.
pub fn maybe_activate_after_present(hwnd: HWND) {
    if !activate_after_present() {
        return;
    }
    if !FIRST_PRESENT.load(Ordering::Acquire) {
        return;
    }
    if ACTIVATE_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    set_hwnd(hwnd);
    sample("before_activate_after_present");
    let (fg_ok, fg_err, attached) = force_foreground(hwnd);
    sample(&format!(
        "after_activate_after_present_fg_ok={fg_ok}_fg_err={fg_err}_attached={attached}"
    ));
    let fg_ours = focus_fields().3;
    let sent = send_english_abc();
    sample(&format!(
        "after_english_sendinput_sent={sent}_pty_control_write=0_fg_ours={fg_ours}"
    ));
    let (n_layouts, current_lang, has_zh) = chinese_layout_probe();
    sample(&format!(
        "chinese_ime_probe_n_layouts={n_layouts}_current_langid=0x{current_lang:x}_has_zh={has_zh}"
    ));
    if has_zh == 0 {
        sample("chinese_ime_BLOCKED_no_zh_keyboard_layout");
        sample("after_input_steady_rss_tif");
        return;
    }
    // A zh layout is not compose/commit. Do not SendInput Unicode CJK as a
    // stand-in, and do not write the PTY via control.
    sample("chinese_ime_BLOCKED_no_real_tsf_compose_commit_in_utm_job");
    sample("after_input_steady_rss_tif");
}

pub fn warmup() {
    sample("probe_warmup_a");
    sample("probe_warmup_b");
}
