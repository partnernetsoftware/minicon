//! A staged variable-length result, for the two-pass return every embedder
//! ends up needing.
//!
//! A host callback holds `&mut` on guest linear memory for its whole body, so
//! it cannot re-enter the guest to call an exported allocator. That is a
//! structural property of this VM, not an accident of one embedding: there is
//! no point in the callback at which the host can ask the guest "give me
//! `n` bytes" and then fill them. So a host that must return a result whose
//! size it only learns while computing it cannot return the bytes at all —
//! only a status.
//!
//! The answer is two passes, and it is always the same protocol:
//!
//! 1. the guest calls the operation; the host computes the result, **stages**
//!    it here, and returns a status plus the length;
//!
//! 2. the guest allocates a buffer of that length with its own allocator, and
//!    calls back asking the host to **copy the staged bytes out** into it.
//!
//! This module is that mechanism and nothing else. It names no import, defines
//! no status code and knows nothing about what the bytes mean; those are the
//! embedder's ABI. What it does own is the part that is easy to get wrong:
//! the staged buffer is bounded and grown fallibly, a result is delivered at
//! most once, and a destination too small to hold it is reported as its own
//! condition — with the size actually needed — while leaving the staged bytes
//! in place so the guest can retry with a larger buffer.
//!
//! ```
//! use tinyvm::{PendingResult, PendingResultError};
//!
//! let mut pending = PendingResult::new(4096);
//! // Pass one: the host stages what it computed and tells the guest the size.
//! assert_eq!(pending.stage(b"variable length").unwrap(), 15);
//!
//! // Pass two, with a buffer the guest sized wrong: nothing is delivered and
//! // nothing is lost.
//! let mut small = [0u8; 4];
//! assert_eq!(
//!     pending.copy_out(&mut small),
//!     Err(PendingResultError::DestinationTooSmall { needed: 15 })
//! );
//!
//! // Pass two again, sized correctly.
//! let mut buffer = [0u8; 15];
//! assert_eq!(pending.copy_out(&mut buffer).unwrap(), 15);
//! assert_eq!(&buffer, b"variable length");
//! assert!(!pending.is_staged());
//! ```

use alloc::vec::Vec;

use crate::wasm::{WasmError, guest_window};

/// Why a staged result could not be produced or delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingResultError {
    /// Nothing is staged. The guest asked for a result it never requested, or
    /// asked for the same one twice.
    NotStaged,
    /// A result is already staged. Staging a second one would silently drop a
    /// result the guest is still entitled to collect.
    AlreadyStaged,
    /// The result is larger than this buffer's configured ceiling.
    OverBudget,
    /// The allocator refused to grow the staging buffer.
    AllocationFailed,
    /// The destination cannot hold the staged result. The staged bytes are
    /// kept, so the guest may allocate `needed` bytes and ask again.
    DestinationTooSmall { needed: usize },
}

impl From<PendingResultError> for WasmError {
    fn from(error: PendingResultError) -> Self {
        WasmError::Trap(match error {
            PendingResultError::NotStaged => "pending result missing",
            PendingResultError::AlreadyStaged => "pending result already staged",
            PendingResultError::OverBudget => "pending result budget",
            PendingResultError::AllocationFailed => "pending result allocation",
            PendingResultError::DestinationTooSmall { .. } => "pending result destination",
        })
    }
}

/// One staged, bounded, variable-length result awaiting collection.
///
/// The buffer keeps its allocation between results, so a steady-state
/// embedding stages and delivers without allocating after the first result of
/// each size.
pub struct PendingResult {
    bytes: Vec<u8>,
    staged: bool,
    max_bytes: usize,
}

impl PendingResult {
    /// A buffer that will never stage more than `max_bytes`.
    ///
    /// The ceiling is mandatory: the length is chosen by whatever the guest
    /// asked for, so an unbounded staging buffer is a guest-controlled host
    /// allocation.
    pub const fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            staged: false,
            max_bytes,
        }
    }

    /// The configured ceiling.
    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Whether a result is waiting to be collected.
    pub const fn is_staged(&self) -> bool {
        self.staged
    }

    /// The length of the staged result — pass one's answer. Zero when nothing
    /// is staged, which a caller distinguishes from a staged empty result with
    /// [`Self::is_staged`].
    pub fn len(&self) -> usize {
        if self.staged { self.bytes.len() } else { 0 }
    }

    /// Whether there are no staged bytes to deliver.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The staged bytes, or an empty slice when nothing is staged.
    pub fn as_slice(&self) -> &[u8] {
        if self.staged { &self.bytes } else { &[] }
    }

    /// Stage `result` and report its length — pass one.
    ///
    /// Fails rather than overwriting a result the guest has not collected, and
    /// grows fallibly: a result the allocator cannot hold is a typed error,
    /// never an abort.
    pub fn stage(&mut self, result: &[u8]) -> Result<usize, PendingResultError> {
        if self.staged {
            return Err(PendingResultError::AlreadyStaged);
        }
        if result.len() > self.max_bytes {
            return Err(PendingResultError::OverBudget);
        }
        self.bytes.clear();
        self.bytes
            .try_reserve(result.len())
            .map_err(|_| PendingResultError::AllocationFailed)?;
        self.bytes.extend_from_slice(result);
        self.staged = true;
        Ok(result.len())
    }

    /// Replace whatever is staged with `result`, without the
    /// [`PendingResultError::AlreadyStaged`] check.
    ///
    /// For an embedding whose protocol says a new request abandons the
    /// previous, uncollected result.
    pub fn restage(&mut self, result: &[u8]) -> Result<usize, PendingResultError> {
        self.staged = false;
        self.stage(result)
    }

    /// Copy the staged result into `destination` and report how many bytes were
    /// written — pass two.
    ///
    /// On success the result is delivered and the buffer becomes empty; the
    /// same result cannot be collected twice. A destination too small delivers
    /// nothing, keeps the staged bytes, and reports the size that would have
    /// been needed, so the guest can allocate again and retry. Only the staged
    /// bytes are written: a longer destination keeps its tail.
    pub fn copy_out(&mut self, destination: &mut [u8]) -> Result<usize, PendingResultError> {
        if !self.staged {
            return Err(PendingResultError::NotStaged);
        }
        let needed = self.bytes.len();
        if destination.len() < needed {
            return Err(PendingResultError::DestinationTooSmall { needed });
        }
        destination[..needed].copy_from_slice(&self.bytes);
        self.clear();
        Ok(needed)
    }

    /// [`Self::copy_out`] into a guest `(ptr, len)` window of `memory`.
    ///
    /// This is the shape a host callback actually holds: the selected linear
    /// memory as a bare `&mut [u8]` plus the two i32 values the guest passed.
    /// The window is bounds-checked exactly as [`crate::guest_bytes`] checks
    /// one, so an out-of-range pointer is refused before the staged result is
    /// touched, and — like the too-small case — leaves it collectable.
    pub fn copy_out_to_guest(
        &mut self,
        memory: &mut [u8],
        ptr: i32,
        len: i32,
    ) -> Result<usize, WasmError> {
        if !self.staged {
            return Err(PendingResultError::NotStaged.into());
        }
        let window = guest_window(memory.len(), ptr, len)?;
        let needed = self.bytes.len();
        if window.len() < needed {
            return Err(PendingResultError::DestinationTooSmall { needed }.into());
        }
        let start = window.start;
        memory[start..start + needed].copy_from_slice(&self.bytes);
        self.clear();
        Ok(needed)
    }

    /// Discard any staged result, keeping the allocation for reuse.
    ///
    /// An embedding calls this when a request is abandoned — the guest trapped,
    /// the instance was reset — so a stale result cannot be handed to the next
    /// one.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.staged = false;
    }
}
