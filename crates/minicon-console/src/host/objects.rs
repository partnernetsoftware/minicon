//! Console object identities, access and share checks, separate from API state.
use super::*;

pub(super) const READ: u32 = 0x8000_0000;
pub(super) const WRITE: u32 = 0x4000_0000;
pub(super) const MAX_HANDLES: usize = 1024;
const ACCESS_DENIED: i32 = 0xc000_0022_u32 as i32;
const SHARING_VIOLATION: i32 = 0xc000_0043_u32 as i32;

impl ConsoleHost<'_> {
    pub(super) fn handle_status(
        &self,
        process: usize,
        object: usize,
        kind: Option<HandleKind>,
        access: u32,
    ) -> i32 {
        let Some(handle) = self.handles.get(&object).filter(|h| {
            self.processes.contains_key(&process)
                && (h.lifetime == HandleLifetime::FileObject
                    || h.lifetime == HandleLifetime::Connection(process))
                && kind.is_none_or(|kind| kind == h.kind)
                && (h.kind == HandleKind::Input
                    || h.buffer == self.current_buffer
                    || h.buffer == self.active_buffer
                    || self.buffers.contains_key(&h.buffer))
        }) else {
            return INVALID_HANDLE;
        };
        if handle.access & access != access {
            ACCESS_DENIED
        } else {
            SUCCESS
        }
    }
}

impl<'session> ConsoleHost<'session> {
    pub(super) fn create_object(
        &mut self,
        request: DriverRequest<'session>,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let mut receipt = DispatchReceipt {
            function: 3,
            api: None,
            status: SUCCESS,
            pending: false,
        };
        if !self.processes.contains_key(&descriptor.process) {
            receipt.status = INVALID_HANDLE;
        } else if descriptor.input_size < 12 || self.handles.len() >= MAX_HANDLES {
            receipt.status = INVALID_PARAMETER;
        } else {
            let body = request.read_input(0, 12)?;
            let mut kind = read_u32(&body, 0).unwrap();
            let share = read_u32(&body, 4).unwrap();
            let access = read_u32(&body, 8).unwrap();
            // Generic CON opens choose a buffer only for one access direction.
            if kind == 4 {
                kind = match access & (READ | WRITE) {
                    READ => 1,
                    WRITE => 2,
                    _ => 4,
                };
            }
            if kind == 3 {
                if share & !3 != 0 || access & !(READ | WRITE) != 0 || descriptor.input_size < 24 {
                    receipt.status = INVALID_PARAMETER;
                } else {
                    let create = request.read_input(12, 12)?;
                    if read_u32(&create, 0).unwrap() & !1 != 0 {
                        receipt.status = INVALID_PARAMETER;
                    } else if let Some(state) = self.new_buffer()? {
                        let token = self.token()?;
                        request.complete(SUCCESS, token, &[])?;
                        self.buffers.insert(token, state);
                        self.handles.insert(
                            token,
                            ClientHandle {
                                lifetime: HandleLifetime::FileObject,
                                kind: HandleKind::Output,
                                access,
                                share,
                                buffer: token,
                            },
                        );
                        return Ok(receipt);
                    } else {
                        receipt.status = 0xc000_009a_u32 as i32;
                    }
                }
                request.complete(receipt.status, 0, &[])?;
                return Ok(receipt);
            }
            let kind = match kind {
                1 => Some(HandleKind::Input),
                2 => Some(HandleKind::Output),
                _ => {
                    receipt.status = INVALID_PARAMETER;
                    None
                }
            };
            if let Some(kind) = kind {
                if share & !3 != 0 || access & !(READ | WRITE) != 0 {
                    receipt.status = INVALID_PARAMETER;
                } else if self
                    .handles
                    .values()
                    .filter(|handle| {
                        handle.kind == kind
                            && (kind == HandleKind::Input || handle.buffer == self.active_buffer)
                    })
                    .any(|handle| {
                        (access & READ != 0 && handle.share & 1 == 0)
                            || (access & WRITE != 0 && handle.share & 2 == 0)
                            || (handle.access & READ != 0 && share & 1 == 0)
                            || (handle.access & WRITE != 0 && share & 2 == 0)
                    })
                {
                    receipt.status = SHARING_VIOLATION;
                } else {
                    let token = self.token()?;
                    // The native CREATE_OBJECT reply transfers the server token
                    // in Information, not an output byte buffer (IoDispatchers).
                    request.complete(SUCCESS, token, &[])?;
                    self.handles.insert(
                        token,
                        ClientHandle {
                            lifetime: HandleLifetime::FileObject,
                            kind,
                            access,
                            share,
                            buffer: self.active_buffer,
                        },
                    );
                    return Ok(receipt);
                }
            }
        }
        request.complete(receipt.status, 0, &[])?;
        Ok(receipt)
    }
}
