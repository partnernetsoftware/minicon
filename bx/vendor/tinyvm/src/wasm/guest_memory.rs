//! Bounds-checked access to a guest's `(ptr, len)` windows.
//!
//! A host callback bound through [`Module::bind_import_typed`] receives the
//! selected linear memory as a bare `&mut [u8]`, and the guest names a region
//! of it with two `i32` values it chose. Turning that pair into a slice is the
//! one place in an embedding where a mistake reads or writes host memory, and
//! it is the same three questions every time: does `ptr + len` overflow, does
//! the window end inside the memory, and — for text — is it UTF-8. This module
//! answers them once so no embedder answers them again.
//!
//! Pointers and lengths are reinterpreted as unsigned, which is what standard
//! Wasm means by an `i32` address: `-1` is address 4294967295, not an error in
//! its own right, and it fails the bounds check like any other address past the
//! end. Every window that is not wholly inside the memory is rejected with a
//! [`WasmError`]; nothing is clamped, truncated or panicked on.
//!
//! ```
//! use tinyvm::{guest_str, guest_write};
//! # use tinyvm::WasmError;
//! # fn host_call(memory: &mut [u8], args: &[i32]) -> Result<(), WasmError> {
//! let name = guest_str(memory, args[0], args[1])?;
//! let reply = name.len() as u8;
//! guest_write(memory, args[2], args[3], &[reply])?;
//! # Ok(())
//! # }
//! ```
//!
//! [`Module::bind_import_typed`]: crate::WasmModule::bind_import_typed

use core::ops::Range;
use core::str;

use crate::wasm::WasmError;

/// The window is not wholly inside the memory: it ends past the last byte, or
/// `ptr + len` is not even representable.
const WINDOW: WasmError = WasmError::Trap("guest memory window");
/// The window is in range but its bytes are not valid UTF-8.
const UTF8: WasmError = WasmError::Trap("guest memory utf-8");
/// The window is in range but shorter than the bytes the host wants to write.
const OVERRUN: WasmError = WasmError::Trap("guest memory window too small");

/// Resolve a guest `(ptr, len)` pair into a byte range of a memory of
/// `memory_len` bytes.
///
/// This is the primitive the rest of the module is built from; call it
/// directly when a window has to be resolved before the memory can be
/// borrowed, or when two disjoint windows must be held at once.
///
/// A zero-length window is legal exactly where a zero-length bulk-memory
/// operation is: at any address up to and including `memory_len`, and nowhere
/// beyond it.
pub fn guest_window(memory_len: usize, ptr: i32, len: i32) -> Result<Range<usize>, WasmError> {
    // Wasm i32 addresses are unsigned; a "negative" pointer is simply a high
    // one. On a 32-bit host the sum can leave `usize`, so it is checked.
    let start = ptr as u32 as usize;
    let count = len as u32 as usize;
    let end = start.checked_add(count).ok_or(WINDOW)?;
    if end > memory_len {
        return Err(WINDOW);
    }
    Ok(start..end)
}

/// Read a guest `(ptr, len)` window as bytes.
pub fn guest_bytes(memory: &[u8], ptr: i32, len: i32) -> Result<&[u8], WasmError> {
    let window = guest_window(memory.len(), ptr, len)?;
    Ok(&memory[window])
}

/// Borrow a guest `(ptr, len)` window for in-place mutation.
pub fn guest_bytes_mut(memory: &mut [u8], ptr: i32, len: i32) -> Result<&mut [u8], WasmError> {
    let window = guest_window(memory.len(), ptr, len)?;
    Ok(&mut memory[window])
}

/// Read a guest `(ptr, len)` window as text.
///
/// Invalid UTF-8 is reported separately from an out-of-range window, because
/// the two mean different things about the guest: one is a bad string, the
/// other is a bad pointer.
pub fn guest_str(memory: &[u8], ptr: i32, len: i32) -> Result<&str, WasmError> {
    str::from_utf8(guest_bytes(memory, ptr, len)?).map_err(|_| UTF8)
}

/// Copy `source` into the guest `(ptr, len)` window and report how many bytes
/// were written.
///
/// `len` is the capacity the guest offered, not the number of bytes to write:
/// a shorter `source` writes only its own bytes and leaves the tail of the
/// window untouched. A `source` longer than the window is refused outright —
/// the window is never partially filled, so a guest that mis-sized its buffer
/// cannot observe half a result.
pub fn guest_write(
    memory: &mut [u8],
    ptr: i32,
    len: i32,
    source: &[u8],
) -> Result<usize, WasmError> {
    let window = guest_window(memory.len(), ptr, len)?;
    if source.len() > window.len() {
        return Err(OVERRUN);
    }
    let start = window.start;
    memory[start..start + source.len()].copy_from_slice(source);
    Ok(source.len())
}
