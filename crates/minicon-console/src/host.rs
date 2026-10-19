//! Initial user-mode console API service; no system console host is used.
use crate::DriverRequest;
use std::collections::BTreeMap;
use std::io::{self, Write};
use windows_sys::Win32::Globalization::{GetOEMCP, GetUserDefaultLangID, MultiByteToWideChar};

const SUCCESS: i32 = 0;
const INVALID_PARAMETER: i32 = 0xc000_000d_u32 as i32;
const INVALID_HANDLE: i32 = 0xc000_0008_u32 as i32;
const NOT_SUPPORTED: i32 = 0xc000_00bb_u32 as i32;

#[derive(Clone, Copy)]
enum HandleKind {
    Input,
    Output,
}

struct ClientHandle {
    process: usize,
    kind: HandleKind,
}

/// Observed dispatch outcome. Unsupported calls remain distinguishable.
#[derive(Clone, Copy, Debug)]
pub struct DispatchReceipt {
    pub function: u32,
    pub api: Option<u32>,
    pub status: i32,
}

/// Experimental console state. Currently supports attachment and a small
/// output/mode/code-page subset; unsupported APIs return NOT_SUPPORTED.
pub struct ConsoleHost {
    session_id: Option<u64>,
    processes: BTreeMap<usize, u32>,
    handles: BTreeMap<usize, ClientHandle>,
    next_token: usize,
    input_mode: u32,
    output_mode: u32,
    input_code_page: u32,
    output_code_page: u32,
    pending_high_surrogate: Option<u16>,
    screen: vt100::Parser,
    original_title: Vec<u16>,
    title: Vec<u16>,
}

impl Default for ConsoleHost {
    fn default() -> Self {
        // SAFETY: GetOEMCP reads the system code page without pointers or handles.
        let code_page = unsafe { GetOEMCP() };
        Self {
            session_id: None,
            processes: BTreeMap::new(),
            handles: BTreeMap::new(),
            next_token: 1,
            input_mode: 7,
            output_mode: 3,
            input_code_page: code_page,
            output_code_page: code_page,
            pending_high_surrogate: None,
            screen: vt100::Parser::new(30, 120, 0),
            original_title: Vec::new(),
            title: Vec::new(),
        }
    }
}

impl ConsoleHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connected_clients(&self) -> usize {
        self.processes.len()
    }

    /// Service one real driver request and write terminal bytes to the caller's
    /// bounded output path. No display window or platform host is created.
    pub fn dispatch(
        &mut self,
        request: DriverRequest<'_>,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let mut receipt = DispatchReceipt {
            function: descriptor.function,
            api: None,
            status: SUCCESS,
        };
        if self.session_id.is_some_and(|id| id != request.session_id()) {
            receipt.status = INVALID_HANDLE;
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        self.session_id = Some(request.session_id());
        match descriptor.function {
            1 => {
                if self.processes.len() >= 64
                    || self.handles.len() > 1021
                    || descriptor.process == 0
                    || u32::try_from(descriptor.process).is_err()
                    || descriptor.input_size < 1340
                    || descriptor.output_size < (3 * size_of::<usize>()) as u32
                {
                    receipt.status = INVALID_PARAMETER;
                    request.complete(receipt.status, 0, &[])?;
                } else {
                    if self.processes.is_empty() {
                        let connect = request.read_input(0, 1340)?;
                        let length = u16::from_le_bytes([connect[34], connect[35]]) as usize;
                        if length > 520 || !length.is_multiple_of(2) {
                            receipt.status = INVALID_PARAMETER;
                            request.complete(receipt.status, 0, &[])?;
                            return Ok(receipt);
                        }
                        self.title = connect[36..36 + length]
                            .chunks_exact(2)
                            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                            .collect();
                        self.original_title = self.title.clone();
                    }
                    let process = self.token()?;
                    let input = self.token()?;
                    let output_handle = self.token()?;
                    let reply: Vec<u8> = [process, input, output_handle]
                        .into_iter()
                        .flat_map(usize::to_ne_bytes)
                        .collect();
                    request.complete(SUCCESS, reply.len(), &reply)?;
                    self.processes.insert(process, descriptor.process as u32);
                    self.handles.insert(
                        input,
                        ClientHandle {
                            process,
                            kind: HandleKind::Input,
                        },
                    );
                    self.handles.insert(
                        output_handle,
                        ClientHandle {
                            process,
                            kind: HandleKind::Output,
                        },
                    );
                }
            }
            2 => {
                if self.processes.remove(&descriptor.process).is_none() {
                    receipt.status = INVALID_HANDLE;
                }
                self.handles
                    .retain(|_, handle| handle.process != descriptor.process);
                request.complete(receipt.status, 0, &[])?;
            }
            4 => {
                if !self.valid_handle(descriptor.process, descriptor.object)
                    || self.handles.remove(&descriptor.object).is_none()
                {
                    receipt.status = INVALID_HANDLE;
                }
                request.complete(receipt.status, 0, &[])?;
            }
            5 => {
                if !self.output_handle(descriptor.process, descriptor.object) {
                    receipt.status = INVALID_HANDLE;
                    request.complete(receipt.status, 0, &[])?;
                } else {
                    let bytes = request.read_input(0, descriptor.input_size as usize)?;
                    self.write_ansi(&bytes, output)?;
                    request.complete(SUCCESS, bytes.len(), &[])?;
                }
            }
            7 => return self.dispatch_api(request, output),
            _ => {
                receipt.status = NOT_SUPPORTED;
                request.reject_unsupported()?;
            }
        }
        Ok(receipt)
    }

    fn token(&mut self) -> io::Result<usize> {
        let token = self.next_token;
        self.next_token = token
            .checked_add(1)
            .ok_or_else(|| io::Error::other("console token space exhausted"))?;
        Ok(token)
    }

    fn valid_handle(&self, process: usize, object: usize) -> bool {
        self.processes.contains_key(&process)
            && self
                .handles
                .get(&object)
                .is_some_and(|handle| handle.process == process)
    }

    fn output_handle(&self, process: usize, object: usize) -> bool {
        self.valid_handle(process, object)
            && matches!(
                self.handles.get(&object).map(|h| h.kind),
                Some(HandleKind::Output)
            )
    }

    fn dispatch_api(
        &mut self,
        request: DriverRequest<'_>,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let inline = request.inline_payload();
        let api = read_u32(inline, 0);
        let body_size = read_u32(inline, 4);
        let mut receipt = DispatchReceipt {
            function: 7,
            api,
            status: SUCCESS,
        };
        let Some((api, body_size)) = api.zip(body_size) else {
            receipt.status = INVALID_PARAMETER;
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        };
        if !self.processes.contains_key(&descriptor.process) {
            receipt.status = INVALID_HANDLE;
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        if body_size > 4096 - 8
            || body_size > descriptor.input_size.saturating_sub(8)
            || body_size > descriptor.output_size
            || inline.len() < body_size as usize + 8
        {
            receipt.status = INVALID_PARAMETER;
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        let mut body = inline[8..8 + body_size as usize].to_vec();
        let mut information = 0;
        match api {
            0x0100_0000 if body.len() >= 8 => {
                let code_page = if body[4] != 0 {
                    self.output_code_page
                } else {
                    self.input_code_page
                };
                body[..4].copy_from_slice(&code_page.to_le_bytes());
            }
            0x0100_0001 | 0x0100_0002 if body.len() >= 4 => {
                if !self.valid_handle(descriptor.process, descriptor.object) {
                    receipt.status = INVALID_HANDLE;
                } else {
                    let mode = match self.handles[&descriptor.object].kind {
                        HandleKind::Input => &mut self.input_mode,
                        HandleKind::Output => &mut self.output_mode,
                    };
                    if api == 0x0100_0001 {
                        body[..4].copy_from_slice(&mode.to_le_bytes());
                    } else {
                        *mode = read_u32(&body, 0).unwrap();
                    }
                }
            }
            0x0100_0006 if body.len() >= 8 => {
                if !self.output_handle(descriptor.process, descriptor.object) {
                    receipt.status = INVALID_HANDLE;
                } else {
                    let offset = body_size + 8;
                    let length = descriptor.input_size - offset;
                    let bytes = request.read_input(offset, length as usize)?;
                    if body[4] != 0 {
                        if !bytes.len().is_multiple_of(2) {
                            receipt.status = INVALID_PARAMETER;
                        } else {
                            self.write_utf16(
                                bytes
                                    .chunks_exact(2)
                                    .map(|b| u16::from_le_bytes([b[0], b[1]])),
                                output,
                            )?;
                        }
                    } else {
                        self.write_ansi(&bytes, output)?;
                    }
                    if receipt.status == SUCCESS {
                        information = bytes.len();
                        body[..4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
                    }
                }
            }
            0x0100_0008 if body.len() >= 2 => {
                // SAFETY: reads the current user's default language ID.
                body[..2].copy_from_slice(&unsafe { GetUserDefaultLangID() }.to_le_bytes());
            }
            0x0200_0007 if body.len() >= 92 => {
                if !self.output_handle(descriptor.process, descriptor.object) {
                    receipt.status = INVALID_HANDLE;
                } else {
                    let (rows, cols) = self.screen.screen().size();
                    let (row, col) = self.screen.screen().cursor_position();
                    body.fill(0);
                    put_coord(&mut body, 0, cols, rows);
                    put_coord(&mut body, 4, col, row);
                    body[12..14].copy_from_slice(&7_u16.to_le_bytes());
                    // The Win32 client converts dimensions to inclusive bounds.
                    put_coord(&mut body, 14, cols, rows);
                    put_coord(&mut body, 18, cols, rows);
                    body[22..24].copy_from_slice(&0xf5_u16.to_le_bytes());
                    const COLORS: [u32; 16] = [
                        0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080,
                        0xc0c0c0, 0x808080, 0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff,
                        0x00ffff, 0xffffff,
                    ];
                    for (slot, color) in body[28..92].chunks_exact_mut(4).zip(COLORS) {
                        slot.copy_from_slice(&color.to_le_bytes());
                    }
                }
            }
            0x0200_0014 if body.len() >= 8 && body[4] != 0 => {
                let title = if body[5] != 0 {
                    &self.original_title
                } else {
                    &self.title
                };
                let capacity = (descriptor.output_size - body_size) as usize / 2;
                let written = title.len().min(capacity.saturating_sub(1));
                let bytes: Vec<u8> = title[..written]
                    .iter()
                    .flat_map(|unit| unit.to_le_bytes())
                    .collect();
                request.write_output(body_size, &bytes)?;
                body[..4].copy_from_slice(&(title.len() as u32).to_le_bytes());
                information = bytes.len();
            }
            0x0200_0015 if !body.is_empty() && body[0] != 0 => {
                let offset = body_size + 8;
                let bytes =
                    request.read_input(offset, (descriptor.input_size - offset) as usize)?;
                if !bytes.len().is_multiple_of(2) || bytes.len() > 65534 {
                    receipt.status = INVALID_PARAMETER;
                } else {
                    self.title = bytes
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]]))
                        .collect();
                }
            }
            _ => receipt.status = NOT_SUPPORTED,
        }
        request.complete(receipt.status, information, &body)?;
        Ok(receipt)
    }

    fn write_ansi(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        if self.output_code_page == 65001 {
            return self.emit(bytes, output);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let mut wide = vec![0_u16; bytes.len()];
        // SAFETY: input/output point to live, disjoint buffers and lengths fit
        // i32 because read_input bounds every request to at most one MiB.
        let length = unsafe {
            MultiByteToWideChar(
                self.output_code_page,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                wide.as_mut_ptr(),
                wide.len() as i32,
            )
        };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        self.write_utf16(wide[..length as usize].iter().copied(), output)
    }

    fn write_utf16(
        &mut self,
        units: impl IntoIterator<Item = u16>,
        output: &mut impl Write,
    ) -> io::Result<()> {
        for unit in units {
            if let Some(high) = self.pending_high_surrogate.take() {
                if (0xdc00..=0xdfff).contains(&unit) {
                    let value =
                        0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00;
                    self.emit_char(char::from_u32(value).unwrap(), output)?;
                    continue;
                }
                self.emit_char(char::REPLACEMENT_CHARACTER, output)?;
            }
            if (0xd800..=0xdbff).contains(&unit) {
                self.pending_high_surrogate = Some(unit);
            } else {
                self.emit_char(
                    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER),
                    output,
                )?;
            }
        }
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        output.write_all(bytes)?;
        self.screen.process(bytes);
        Ok(())
    }

    fn emit_char(&mut self, character: char, output: &mut impl Write) -> io::Result<()> {
        let mut buffer = [0; 4];
        self.emit(character.encode_utf8(&mut buffer).as_bytes(), output)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn put_coord(body: &mut [u8], offset: usize, x: u16, y: u16) {
    body[offset..offset + 2].copy_from_slice(&x.to_le_bytes());
    body[offset + 2..offset + 4].copy_from_slice(&y.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_host_preserves_a_surrogate_pair_across_writes() {
        let mut host = ConsoleHost::new();
        let mut output = Vec::new();
        host.write_utf16([0x4e2d, 0xd83d], &mut output).unwrap();
        assert_eq!(output, "中".as_bytes());
        host.write_utf16([0xde00, 0x6587], &mut output).unwrap();
        assert_eq!(output, "中😀文".as_bytes());
        assert_eq!(host.screen.screen().cursor_position(), (0, 6));
    }

    #[test]
    fn rust_console_host_invalid_surrogate_does_not_swallow_the_next_character() {
        let mut host = ConsoleHost::new();
        let mut output = Vec::new();
        host.write_utf16([0xd800], &mut output).unwrap();
        host.write_utf16([b'A' as u16, 0xdc00], &mut output)
            .unwrap();
        assert_eq!(output, "�A�".as_bytes());
    }

    #[test]
    fn rust_console_host_propagates_output_failure_without_advancing_the_screen() {
        struct FailedOutput;
        impl Write for FailedOutput {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut host = ConsoleHost::new();
        let result = host.write_utf16([b'A' as u16], &mut FailedOutput);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(host.screen.screen().cursor_position(), (0, 0));
    }
}
