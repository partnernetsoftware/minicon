//! Console object identities, access and share checks, separate from API state.
use super::*;

pub(super) const READ: u32 = 0x8000_0000;
pub(super) const WRITE: u32 = 0x4000_0000;
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
                && h.process == process
                && kind.is_none_or(|kind| kind == h.kind)
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
        } else if descriptor.input_size < 12 || self.handles.len() >= 1024 {
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
            let kind = match kind {
                1 => Some(HandleKind::Input),
                2 => Some(HandleKind::Output),
                3 => {
                    receipt.status = NOT_SUPPORTED;
                    None
                } // separate screen buffers remain explicit
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
                    .filter(|handle| handle.kind == kind)
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
                            process: descriptor.process,
                            kind,
                            access,
                            share,
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
