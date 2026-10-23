//! Linux anonymous executable carrier for the isolated page adapter (PRD 27/28).
use std::{
    fs::File,
    io::Write,
    os::fd::{AsRawFd, FromRawFd},
    path::PathBuf,
};

pub struct EmbeddedPageProgram(File);

impl EmbeddedPageProgram {
    pub fn new(bytes: &[u8]) -> Result<Self, String> {
        // Only an ELF64 little-endian executable for this payload's ISA is a
        // usable helper. Reject a mismatched build before allocating memory.
        let machine = u16::from_le_bytes(
            bytes
                .get(18..20)
                .and_then(|b| b.try_into().ok())
                .unwrap_or([0; 2]),
        );
        let executable = u16::from_le_bytes(
            bytes
                .get(16..18)
                .and_then(|b| b.try_into().ok())
                .unwrap_or([0; 2]),
        );
        if bytes.len() < 64
            || !bytes.starts_with(b"\x7fELF\x02\x01\x01")
            || !matches!(executable, 2 | 3)
            || bytes.len() > 32 * 1024 * 1024
            || !matches!(
                (std::env::consts::ARCH, machine),
                ("x86_64", 62) | ("aarch64", 183)
            )
        {
            return Err("browser-embedded-host-unavailable".to_owned());
        }
        #[cfg(target_arch = "x86_64")]
        const MEMFD_CREATE: std::ffi::c_long = 319;
        #[cfg(target_arch = "aarch64")]
        const MEMFD_CREATE: std::ffi::c_long = 279;
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        const MEMFD_CREATE: std::ffi::c_long = -1;
        unsafe extern "C" {
            fn syscall(number: std::ffi::c_long, ...) -> std::ffi::c_long;
        }
        // CLOEXEC closes the descriptor after exec has opened /proc/self/fd/N.
        let fd = unsafe { syscall(MEMFD_CREATE, c"minicon-web-host".as_ptr(), 1u32) };
        if fd < 0 {
            return Err("browser-executable-memory-unavailable".to_owned());
        }
        let mut file = unsafe { File::from_raw_fd(fd as i32) };
        file.write_all(bytes)
            .map_err(|_| "browser-embedded-host-write-failed".to_owned())?;
        Ok(Self(file))
    }

    pub fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.0.as_raw_fd()))
    }
}
