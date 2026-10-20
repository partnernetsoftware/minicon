//! ConDrv bootstrap, expressed in Rust against the native Windows ABI.
//!
//! Protocol reference: microsoft/terminal revision
//! 9fd41b2baec77d25b8cfb584070d5998eab7df81:
//! dep/Console/condrv.h and src/server/DeviceHandle.cpp.
//! No Microsoft host implementation is compiled or launched here.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GENERIC_ALL, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForSingleObject,
};

const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const SYNCHRONIZE: u32 = 0x0010_0000;
pub(crate) const READ_INTERRUPTED: i32 = 0x101;
enum ReadReplyKind {
    File,
    Console,
    Records,
}
const GENERIC_READ_WRITE: u32 = 0xc000_0000;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_SHARE_ALL: u32 = 7;
const IOCTL_SET_SERVER_INFORMATION: u32 = (0x50 << 16) | (7 << 2) | 3;
const IOCTL_READ_IO: u32 = (0x50 << 16) | (1 << 2) | 2;
const IOCTL_COMPLETE_IO: u32 = (0x50 << 16) | (2 << 2) | 3;
const IOCTL_READ_INPUT: u32 = (0x50 << 16) | (3 << 2) | 3;
const IOCTL_WRITE_OUTPUT: u32 = (0x50 << 16) | (4 << 2) | 3;
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct RequestId {
    low: u32,
    high: i32,
}

/// Descriptor values are supplied by the kernel, not trusted application IDs.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RequestDescriptor {
    identifier: RequestId,
    pub process: usize,
    pub object: usize,
    pub function: u32,
    pub input_size: u32,
    pub output_size: u32,
    reserved: u32,
}

/// A request received from this session's actual ConDrv endpoint.
pub struct DriverRequest<'session> {
    session: &'session DriverSession,
    pub descriptor: RequestDescriptor,
    payload: [u8; 4096],
    payload_length: usize,
    completed: bool,
}

#[repr(C)]
struct Packet {
    descriptor: RequestDescriptor,
    payload: [u8; 4096],
}

#[repr(C)]
struct BufferDescriptor {
    data: *mut c_void,
    size: u32,
    offset: u32,
}

#[repr(C)]
struct BufferOperation {
    identifier: RequestId,
    buffer: BufferDescriptor,
}

#[repr(C)]
struct Completion {
    identifier: RequestId,
    status: IoStatusBlock,
    write: BufferDescriptor,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: HANDLE,
    object_name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality_of_service: *mut c_void,
}

// IO_STATUS_BLOCK begins with a union of NTSTATUS and PVOID. uintptr-sized
// storage preserves that union's size and alignment on each Windows ISA.
#[repr(C)]
struct IoStatusBlock {
    status_or_pointer: usize,
    information: usize,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtOpenFile(
        file: *mut HANDLE,
        access: u32,
        attributes: *const ObjectAttributes,
        status: *mut IoStatusBlock,
        share: u32,
        options: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

/// One private driver server, input event and reference handle.
///
/// Handles are non-inheritable. Dropping the session releases all three; it
/// neither attaches to nor terminates an existing user's console. Clients are
/// launched explicitly through this session's owned reference.
pub struct DriverSession {
    id: u64,
    _reference: OwnedHandle,
    server: OwnedHandle,
    _input_available: OwnedHandle,
}

impl DriverSession {
    pub(crate) fn client_reference(&self) -> HANDLE {
        self._reference.as_raw_handle()
    }
    pub fn open() -> io::Result<Self> {
        let server = open_device(r"\Device\ConDrv\Server", null_mut(), GENERIC_ALL, 0)?;
        // SAFETY: null security attributes make a non-inheritable unnamed
        // event; no borrowed pointers are retained by CreateEventW.
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful CreateEventW transfers one unique handle.
        let input_available = unsafe { OwnedHandle::from_raw_handle(event) };
        let input_event = input_available.as_raw_handle();
        control(
            &server,
            IOCTL_SET_SERVER_INFORMATION,
            (&input_event as *const HANDLE).cast(),
            size_of::<HANDLE>() as u32,
            null_mut(),
            0,
        )?;
        let reference = open_device(
            r"\Reference",
            server.as_raw_handle(),
            GENERIC_READ_WRITE | SYNCHRONIZE,
            FILE_SYNCHRONOUS_IO_NONALERT,
        )?;
        Ok(Self {
            id: NEXT_SESSION_ID
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .map_err(|_| io::Error::other("console session identity space exhausted"))?,
            _reference: reference,
            server,
            _input_available: input_available,
        })
    }

    /// Receive one kernel request with a bounded wait and fully settled I/O.
    /// No request is accepted or reported successful by receiving it.
    pub fn receive(&self, timeout: Duration) -> io::Result<DriverRequest<'_>> {
        let milliseconds = u32::try_from(timeout.as_millis())
            .ok()
            .filter(|value| *value < u32::MAX)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "finite timeout required")
            })?;
        // SAFETY: unnamed non-inheritable completion event.
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateEventW transferred ownership.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let mut operation = OVERLAPPED {
            hEvent: event.as_raw_handle(),
            ..Default::default()
        };
        let mut packet = Packet {
            descriptor: RequestDescriptor::default(),
            payload: [0; 4096],
        };
        let mut bytes = 0;
        // SAFETY: packet and operation remain live until the request has been
        // completed, including the cancellation path below. The server was
        // opened for asynchronous I/O and the completion event is unique.
        let immediate = unsafe {
            DeviceIoControl(
                self.server.as_raw_handle(),
                IOCTL_READ_IO,
                null(),
                0,
                (&mut packet as *mut Packet).cast(),
                size_of::<Packet>() as u32,
                &mut bytes,
                &mut operation,
            )
        };
        if immediate == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            // SAFETY: the event and timeout are valid; no pointer escapes.
            let wait = unsafe { WaitForSingleObject(event.as_raw_handle(), milliseconds) };
            let wait_error = if wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT {
                Some(io::Error::last_os_error())
            } else {
                None
            };
            if wait != WAIT_OBJECT_0 {
                // SAFETY: cancel only this operation, then await completion so
                // the kernel cannot retain pointers into a dropped stack.
                unsafe {
                    CancelIoEx(self.server.as_raw_handle(), &operation);
                }
            }
            // SAFETY: the operation remains alive and was completed or cancelled.
            let completed = unsafe {
                GetOverlappedResult(self.server.as_raw_handle(), &operation, &mut bytes, 1)
            };
            if completed == 0 {
                let error = io::Error::last_os_error();
                if wait == WAIT_TIMEOUT
                    && error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "ConDrv receive timed out",
                    ));
                }
                return Err(wait_error.unwrap_or(error));
            }
            if let Some(error) = wait_error {
                return Err(error);
            }
        }
        if bytes < size_of::<RequestDescriptor>() as u32 || bytes > size_of::<Packet>() as u32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid ConDrv descriptor length",
            ));
        }
        Ok(DriverRequest {
            session: self,
            descriptor: packet.descriptor,
            payload: packet.payload,
            payload_length: bytes as usize - size_of::<RequestDescriptor>(),
            completed: false,
        })
    }

    pub(crate) fn set_input_available(&self, available: bool) -> io::Result<()> {
        // SAFETY: this session owns the live event registered with ConDrv.
        let result = unsafe {
            if available {
                SetEvent(self._input_available.as_raw_handle())
            } else {
                ResetEvent(self._input_available.as_raw_handle())
            }
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl<'session> DriverRequest<'session> {
    pub(crate) fn origin(&self) -> &'session DriverSession {
        self.session
    }
    pub(crate) fn session_id(&self) -> u64 {
        self.session.id
    }

    /// Explicitly reject a request while its API semantics are unimplemented.
    /// This consumes its receipt; it must not be completed a second time.
    pub fn reject_unsupported(self) -> io::Result<()> {
        self.complete(0xc000_00bb_u32 as i32, 0, &[])
    }

    pub(crate) fn complete(
        mut self,
        status: i32,
        information: usize,
        reply: &[u8],
    ) -> io::Result<()> {
        if reply.len() > self.descriptor.output_size as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reply exceeds client output buffer",
            ));
        }
        let completion = Completion {
            identifier: self.descriptor.identifier,
            status: IoStatusBlock {
                status_or_pointer: status as u32 as usize,
                information,
            },
            write: BufferDescriptor {
                data: if reply.is_empty() {
                    null_mut()
                } else {
                    reply.as_ptr().cast_mut().cast()
                },
                size: reply.len() as u32,
                offset: 0,
            },
        };
        control(
            &self.session.server,
            IOCTL_COMPLETE_IO,
            (&completion as *const Completion).cast(),
            size_of::<Completion>() as u32,
            null_mut(),
            0,
        )?;
        self.completed = true;
        Ok(())
    }

    pub(crate) fn read_input(&self, offset: u32, length: usize) -> io::Result<Vec<u8>> {
        // Bound allocation before touching client-supplied lengths. Requests
        // beyond this initial transport budget are explicitly unsupported.
        if length > 1024 * 1024
            || length > self.descriptor.input_size.saturating_sub(offset) as usize
            || offset > self.descriptor.input_size
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid or excessive client input range",
            ));
        }
        let mut bytes = vec![0_u8; length];
        if length != 0 {
            let operation = BufferOperation {
                identifier: self.descriptor.identifier,
                buffer: BufferDescriptor {
                    data: bytes.as_mut_ptr().cast(),
                    size: length as u32,
                    offset,
                },
            };
            control(
                &self.session.server,
                IOCTL_READ_INPUT,
                (&operation as *const BufferOperation).cast(),
                size_of::<BufferOperation>() as u32,
                null_mut(),
                0,
            )?;
        }
        Ok(bytes)
    }

    pub(crate) fn write_output(&self, offset: u32, bytes: &[u8]) -> io::Result<()> {
        if offset > self.descriptor.output_size
            || bytes.len() > self.descriptor.output_size.saturating_sub(offset) as usize
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid client output range",
            ));
        }
        if !bytes.is_empty() {
            let operation = BufferOperation {
                identifier: self.descriptor.identifier,
                buffer: BufferDescriptor {
                    data: bytes.as_ptr().cast_mut().cast(),
                    size: bytes.len() as u32,
                    offset,
                },
            };
            control(
                &self.session.server,
                IOCTL_WRITE_OUTPUT,
                (&operation as *const BufferOperation).cast(),
                size_of::<BufferOperation>() as u32,
                null_mut(),
                0,
            )?;
        }
        Ok(())
    }
    /// Raw inline bytes for future validated, per-operation decoding.
    pub fn inline_payload(&self) -> &[u8] {
        &self.payload[..self.payload_length]
    }

    pub(crate) fn is_console_read(&self) -> bool {
        self.read_reply_kind().is_some()
    }

    pub(crate) fn is_console_text_read(&self) -> bool {
        matches!(
            self.read_reply_kind(),
            Some(ReadReplyKind::File | ReadReplyKind::Console)
        )
    }

    fn read_reply_kind(&self) -> Option<ReadReplyKind> {
        match self.descriptor.function {
            6 => Some(ReadReplyKind::File),
            7 if self.payload_length >= 4 => {
                match u32::from_le_bytes(self.payload[..4].try_into().unwrap()) {
                    0x0100_0004 => Some(ReadReplyKind::Records),
                    0x0100_0005 => Some(ReadReplyKind::Console),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn read_interruption_reply(&self) -> Option<([u8; 20], usize)> {
        let needed = match self.read_reply_kind()? {
            ReadReplyKind::File => 0,
            ReadReplyKind::Console => 20,
            ReadReplyKind::Records => 8,
        };
        let mut reply = [0; 20];
        if needed != 0 {
            if self.payload_length < needed + 8 || self.descriptor.output_size < needed as u32 {
                return None;
            }
            reply[..needed].copy_from_slice(&self.payload[8..8 + needed]);
            if needed == 8 {
                reply[..4].fill(0);
            } else {
                reply[12..20].fill(0);
            }
        }
        Some((reply, needed))
    }

    pub(crate) fn interrupt_read(self, status: i32) -> io::Result<()> {
        if !self.is_console_read() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if let Some((reply, length)) = self.read_interruption_reply() {
            self.complete(status, 0, &reply[..length])
        } else {
            // Malformed unadmitted reads still get an explicit API rejection.
            self.complete(0xc000_000d_u32 as i32, 0, &[])
        }
    }
}

impl Drop for DriverRequest<'_> {
    fn drop(&mut self) {
        if !self.completed {
            let (mut reply, reply_len, status) = self
                .read_interruption_reply()
                .map(|(reply, length)| (reply, length, READ_INTERRUPTED as u32 as usize))
                .unwrap_or(([0; 20], 0, 0xc000_0120));
            let completion = Completion {
                identifier: self.descriptor.identifier,
                status: IoStatusBlock {
                    status_or_pointer: status,
                    information: 0,
                },
                write: BufferDescriptor {
                    data: if reply_len == 0 {
                        null_mut()
                    } else {
                        reply.as_mut_ptr().cast()
                    },
                    size: reply_len as u32,
                    offset: 0,
                },
            };
            // A dropped/failed handler must release the actual waiting client.
            // The borrow guarantees the driver endpoint still exists here.
            let _ = control(
                &self.session.server,
                IOCTL_COMPLETE_IO,
                (&completion as *const Completion).cast(),
                size_of::<Completion>() as u32,
                null_mut(),
                0,
            );
        }
    }
}

fn open_device(name: &str, root: HANDLE, access: u32, options: u32) -> io::Result<OwnedHandle> {
    let mut name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut unicode = UnicodeString {
        length: ((name.len() - 1) * 2) as u16,
        maximum_length: (name.len() * 2) as u16,
        buffer: name.as_mut_ptr(),
    };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: root,
        object_name: &mut unicode,
        attributes: OBJ_CASE_INSENSITIVE,
        security_descriptor: null_mut(),
        security_quality_of_service: null_mut(),
    };
    let mut handle = null_mut();
    let mut status = IoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    // SAFETY: all native structs have repr(C), name storage lives through the
    // synchronous call, and the output handle is uniquely acquired on success.
    let result = unsafe {
        NtOpenFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            FILE_SHARE_ALL,
            options,
        )
    };
    if result < 0 {
        // SAFETY: this pure conversion accepts every NTSTATUS value.
        return Err(io::Error::from_raw_os_error(unsafe {
            RtlNtStatusToDosError(result) as i32
        }));
    }
    if handle.is_null() {
        return Err(io::Error::other("NtOpenFile returned a null handle"));
    }
    // SAFETY: NtOpenFile succeeded and transferred a unique live handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn control(
    handle: &OwnedHandle,
    code: u32,
    input: *const c_void,
    input_len: u32,
    output: *mut c_void,
    output_len: u32,
) -> io::Result<u32> {
    let mut written = 0;
    // SAFETY: internal callers supply live buffers of the specified lengths,
    // or null for zero length. No pointers escape this synchronous call.
    let success = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            code,
            input,
            input_len,
            output,
            output_len,
            &mut written,
            null_mut(),
        )
    };
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};

    #[test]
    fn rust_console_driver_registers_noninheritable_server_and_reference() {
        let session = DriverSession::open().expect("establish a private ConDrv server");
        for handle in [
            &session.server,
            &session._reference,
            &session._input_available,
        ] {
            let mut flags = 0;
            // SAFETY: each field holds a live owned handle; flags is writable.
            let success = unsafe { GetHandleInformation(handle.as_raw_handle(), &mut flags) };
            assert_ne!(success, 0, "driver/event handle must actually exist");
            assert_eq!(
                flags & HANDLE_FLAG_INHERIT,
                0,
                "no ambient handle inheritance"
            );
        }
    }

    #[test]
    fn rust_console_driver_rejects_a_non_console_handle() {
        // SAFETY: unnamed event with default security; uniquely owned below.
        let handle = unsafe { CreateEventW(null(), 1, 0, null()) };
        assert!(!handle.is_null());
        // SAFETY: CreateEventW returned a new live event handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let input_event = handle.as_raw_handle();
        let result = control(
            &handle,
            IOCTL_SET_SERVER_INFORMATION,
            (&input_event as *const HANDLE).cast(),
            size_of::<HANDLE>() as u32,
            null_mut(),
            0,
        );
        assert!(
            result.is_err(),
            "an event cannot register as a console driver server"
        );
    }

    #[test]
    fn rust_console_driver_rejects_truncated_registration() {
        let server = open_device(r"\Device\ConDrv\Server", null_mut(), GENERIC_ALL, 0)
            .expect("open a fresh server");
        let result = control(
            &server,
            IOCTL_SET_SERVER_INFORMATION,
            null(),
            0,
            null_mut(),
            0,
        );
        assert!(
            result.is_err(),
            "registration must contain an actual event handle"
        );
    }

    #[test]
    fn rust_console_driver_receive_timeout_cancels_and_settles_io() {
        let session = DriverSession::open().unwrap();
        let start = std::time::Instant::now();
        let result = session.receive(Duration::from_millis(20));
        assert!(matches!(result, Err(ref error) if error.kind() == io::ErrorKind::TimedOut));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "pending receive must cancel promptly"
        );
        // A second receive must work after cancellation; this detects a stale
        // pending operation holding the endpoint rather than a settled one.
        let result = session.receive(Duration::from_millis(20));
        assert!(matches!(result, Err(ref error) if error.kind() == io::ErrorKind::TimedOut));
    }

    mod bootstrap;
}
