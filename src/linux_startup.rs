//! Product-owned Linux GUI dependency preflight.
//!
//! Winit loads XKB dynamically, so the ELF import table cannot reveal this
//! runtime edge. Detect it before the event loop enters third-party code: a
//! missing desktop package must be an actionable MiniCon error, not a panic
//! containing the build machine's Cargo registry path.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};

const XKB_X11_SONAME: &str = "libxkbcommon-x11.so.0";
const XKB_X11_SYMBOLS: [&str; 4] = [
    "xkb_x11_setup_xkb_extension",
    "xkb_x11_get_core_keyboard_device_id",
    "xkb_x11_keymap_new_from_device",
    "xkb_x11_state_new_from_device",
];
const RTLD_NOW: c_int = 2;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}

struct DynamicLibrary(*mut c_void);

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `dlopen` in this module
        // and is closed exactly once by this owner.
        unsafe {
            let _ = dlclose(self.0);
        }
    }
}

pub(crate) fn preflight() -> Result<(), String> {
    let display = std::env::var_os("DISPLAY");
    let wayland = std::env::var_os("WAYLAND_DISPLAY");
    let backend = std::env::var_os("WINIT_UNIX_BACKEND");
    preflight_with(
        display.as_deref(),
        wayland.as_deref(),
        backend.as_deref(),
        probe_xkb_x11,
    )
}

fn preflight_with(
    display: Option<&OsStr>,
    wayland: Option<&OsStr>,
    backend: Option<&OsStr>,
    probe: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !requires_x11(display, wayland, backend) {
        return Ok(());
    }
    probe().map_err(|detail| {
        format!(
            "Linux X11 runtime dependency unavailable: {XKB_X11_SONAME} ({detail}). \
             Debian/Ubuntu: sudo apt-get install libxkbcommon-x11-0. \
             The -dev package is not required."
        )
    })
}

fn requires_x11(display: Option<&OsStr>, wayland: Option<&OsStr>, backend: Option<&OsStr>) -> bool {
    if backend.is_some_and(|value| value.eq_ignore_ascii_case("x11")) {
        return true;
    }
    if backend.is_some_and(|value| value.eq_ignore_ascii_case("wayland")) {
        return false;
    }
    display.is_some_and(|value| !value.is_empty())
        && !wayland.is_some_and(|value| !value.is_empty())
}

fn probe_xkb_x11() -> Result<(), String> {
    let soname = CString::new(XKB_X11_SONAME).expect("static SONAME contains no NUL");
    // SAFETY: this loads the fixed versioned system SONAME that winit will use
    // next. The returned handle is immediately placed under one RAII owner.
    let handle = unsafe { dlopen(soname.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        return Err(loader_error());
    }
    let library = DynamicLibrary(handle);
    for name in XKB_X11_SYMBOLS {
        let symbol = CString::new(name).expect("static symbol contains no NUL");
        // Clear any prior loader error before resolving this symbol.
        // SAFETY: `library` is live and the C string is NUL terminated. The
        // result is only checked for presence, never called or dereferenced.
        unsafe {
            let _ = dlerror();
            let address = dlsym(library.0, symbol.as_ptr());
            let error = dlerror();
            if address.is_null() || !error.is_null() {
                return Err(if error.is_null() {
                    format!("required symbol {name} is absent")
                } else {
                    CStr::from_ptr(error).to_string_lossy().into_owned()
                });
            }
        }
    }
    Ok(())
}

fn loader_error() -> String {
    // SAFETY: `dlerror` returns either null or a thread-local NUL-terminated
    // diagnostic valid until the next loader call on this thread. Copy now.
    unsafe {
        let error = dlerror();
        if error.is_null() {
            "dynamic loader returned no detail".to_owned()
        } else {
            CStr::from_ptr(error).to_string_lossy().into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_without_runtime_reports_the_versioned_soname_and_install_command() {
        let error = preflight_with(Some(OsStr::new(":99")), None, None, || {
            Err("not found".to_owned())
        })
        .unwrap_err();
        assert!(error.contains("libxkbcommon-x11.so.0"));
        assert!(error.contains("apt-get install libxkbcommon-x11-0"));
        assert!(error.contains("-dev package is not required"));
    }

    #[test]
    fn wayland_only_does_not_require_the_x11_bridge() {
        assert_eq!(
            preflight_with(None, Some(OsStr::new("wayland-0")), None, || Err(
                "must not probe".to_owned()
            ),),
            Ok(())
        );
    }

    #[test]
    fn explicit_backend_selection_wins_over_ambient_variables() {
        assert!(requires_x11(
            None,
            Some(OsStr::new("wayland-0")),
            Some(OsStr::new("x11"))
        ));
        assert!(!requires_x11(
            Some(OsStr::new(":0")),
            None,
            Some(OsStr::new("wayland"))
        ));
    }
}
