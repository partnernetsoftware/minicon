//! The pseudo-console window the rust-console provider reports through
//! `GetConsoleWindow`, as ConPTY does. Console runtimes such as MSYS2/Cygwin's
//! treat a failed `GetConsoleWindow` as "no real console" and silently stop
//! writing, so every session gets one window that exists. It is a
//! message-only window (`HWND_MESSAGE` parent): `EnumWindows` never lists it,
//! so nothing that finds MiniCon's window by process mistakes it for the
//! terminal, and a client's `ShowWindow` cannot surface it. Its own thread
//! pumps it so a cross-process message can never hang the caller.

use std::io;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WNDCLASSW,
};

const CLASS_NAME: &str = "MiniConPseudoConsoleWindow";

/// Owns one pseudo-console window and its pump thread; dropping it closes the
/// window and joins the thread.
pub(super) struct PseudoWindow {
    hwnd: HWND,
    thread: Option<JoinHandle<()>>,
}

// SAFETY: the HWND is only used as an opaque value and as a PostMessageW
// target, both of which Win32 permits from any thread.
unsafe impl Send for PseudoWindow {}

impl PseudoWindow {
    pub(super) fn create() -> io::Result<Self> {
        let (ready, receive) = mpsc::sync_channel::<io::Result<usize>>(1);
        let thread = std::thread::Builder::new()
            .name("minicon-pseudo-console-window".to_owned())
            .spawn(move || pump(&ready))?;
        let hwnd = receive
            .recv()
            .map_err(|_| io::Error::other("pseudo-console window thread exited early"))??;
        Ok(Self {
            hwnd: hwnd as HWND,
            thread: Some(thread),
        })
    }

    /// The handle value `GetConsoleWindow` reports.
    pub(super) fn handle_value(&self) -> u64 {
        self.hwnd as usize as u64
    }
}

impl Drop for PseudoWindow {
    fn drop(&mut self) {
        // SAFETY: posting to a window this object created; a stale handle only
        // makes the post fail, which leaves the thread to exit with the process.
        let posted = unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) } != 0;
        if let Some(thread) = self.thread.take()
            && posted
        {
            let _ = thread.join();
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pump(ready: &mpsc::SyncSender<io::Result<usize>>) {
    let class = wide(CLASS_NAME);
    // SAFETY: plain Win32 window creation on this thread with a static window
    // procedure; every pointer outlives the call that receives it.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let mut window_class: WNDCLASSW = std::mem::zeroed();
        window_class.lpfnWndProc = Some(window_procedure);
        window_class.hInstance = instance;
        window_class.lpszClassName = class.as_ptr();
        // A second registration in the same process fails with
        // ERROR_CLASS_ALREADY_EXISTS, which is fine: the class is identical.
        let _ = RegisterClassW(&window_class);
        let title = wide("");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            let _ = ready.send(Err(io::Error::last_os_error()));
            return;
        }
        let _ = ready.send(Ok(hwnd as usize));
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: default handling for everything else, including WM_CLOSE,
        // which destroys the window on this thread.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
