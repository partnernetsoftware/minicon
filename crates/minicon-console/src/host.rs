//! Initial user-mode console API service; no system console host is used.
use crate::{DriverRequest, DriverSession};
mod buffers;
mod cell_codec;
mod cursor;
mod encoding;
mod font;
mod geometry;
mod indexed;
mod input;
mod objects;
mod output;
mod readback;
mod rectangle;
mod screen;
mod scroll;
mod stream;
mod title;
pub use input::ConsoleInput;
use input::InputState;
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use windows_sys::Win32::Globalization::{GetOEMCP, GetUserDefaultLangID, IsValidCodePage};

const SUCCESS: i32 = 0;
const INVALID_PARAMETER: i32 = 0xc000_000d_u32 as i32;
const INVALID_HANDLE: i32 = 0xc000_0008_u32 as i32;
const NOT_SUPPORTED: i32 = 0xc000_00bb_u32 as i32;
const MAX_CLIENTS: usize = 64;
const MAX_DEFERRED_OUTPUT: usize = 64;

#[derive(Clone, Copy, Eq, PartialEq)]
enum HandleKind {
    Input,
    Output,
}

struct ClientHandle {
    lifetime: HandleLifetime,
    kind: HandleKind,
    access: u32,
    share: u32,
    buffer: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HandleLifetime {
    // CONNECT's default handles are owned by that console connection.
    Connection(usize),
    // CREATE_OBJECT files are retained until the driver's final CLOSE_OBJECT.
    FileObject,
}

/// Observed dispatch outcome. Unsupported calls remain distinguishable.
#[derive(Clone, Copy, Debug)]
pub struct DispatchReceipt {
    pub function: u32,
    pub api: Option<u32>,
    pub status: i32,
    pub pending: bool,
}

/// Experimental console state. Currently supports attachment and a small
/// output/mode/code-page subset; unsupported APIs return NOT_SUPPORTED.
pub struct ConsoleHost<'session> {
    session: Option<&'session DriverSession>,
    input: InputState<'session>,
    session_id: Option<u64>,
    processes: BTreeMap<usize, u32>,
    control: Option<crate::control::ControlScope<'session>>,
    handles: BTreeMap<usize, ClientHandle>,
    next_token: usize,
    input_mode: u32,
    output_mode: u32,
    cursor_size: u32,
    input_code_page: u32,
    output_code_page: u32,
    output_decoder: crate::terminal::Utf16Decoder,
    previous_output_cr: bool,
    terminal: crate::terminal::TerminalStream,
    viewport: Option<crate::terminal::TerminalViewport>,
    buffers: BTreeMap<usize, buffers::BufferState>,
    active_buffer: usize,
    current_buffer: usize,
    parked_cells: usize,
    original_title: Vec<u16>,
    title: Vec<u16>,
    deferred_output: VecDeque<DriverRequest<'session>>,
}

impl Default for ConsoleHost<'_> {
    fn default() -> Self {
        // SAFETY: GetOEMCP reads the system code page without pointers or handles.
        let code_page = unsafe { GetOEMCP() };
        Self {
            session: None,
            input: InputState::default(),
            session_id: None,
            processes: BTreeMap::new(),
            control: None,
            handles: BTreeMap::new(),
            next_token: 1,
            input_mode: 7,
            output_mode: 3,
            cursor_size: 25,
            input_code_page: code_page,
            output_code_page: code_page,
            output_decoder: crate::terminal::Utf16Decoder::default(),
            previous_output_cr: false,
            terminal: crate::terminal::TerminalStream::new(30, 120, 0, 65536),
            viewport: None,
            buffers: BTreeMap::new(),
            active_buffer: 0,
            current_buffer: 0,
            parked_cells: 0,
            original_title: Vec::new(),
            title: Vec::new(),
            deferred_output: VecDeque::new(),
        }
    }
}

impl<'session> ConsoleHost<'session> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable control delivery only to clients admitted from this private Job.
    /// Attach before servicing any CONNECT requests.
    pub fn for_child(child: &'session crate::ConsoleChild) -> io::Result<Self> {
        let mut host = Self::default();
        host.control = Some(crate::control::ControlScope::new(child)?);
        Ok(host)
    }

    /// Request Ctrl+C delivery to live attached clients in the owned Job.
    /// The count reports attempted targets, not handler execution or completion.
    pub fn send_ctrl_c(&mut self) -> io::Result<usize> {
        self.interrupt_control(crate::control::ControlEvent::CtrlC)
    }

    fn interrupt_control(&mut self, event: crate::control::ControlEvent) -> io::Result<usize> {
        if self.control.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no owned control scope",
            ));
        }
        if matches!(event, crate::control::ControlEvent::CtrlBreak) {
            self.input.flush_keyboard_input();
        }
        let cancelled = self.cancel_pending_reads();
        let count = self.control.as_ref().unwrap().send(event, 0);
        cancelled?;
        Ok(count)
    }

    /// Retained signal-target identities; zero when no control scope is bound.
    pub fn control_targets(&self) -> usize {
        self.control.as_ref().map_or(0, |scope| scope.targets())
    }

    /// Update the owned screen dimensions, optionally queueing a native resize
    /// event when the client has enabled ENABLE_WINDOW_INPUT.
    pub fn resize(&mut self, columns: u16, rows: u16, output: &mut impl Write) -> io::Result<()> {
        if self.viewport.is_some() {
            return self.resize_projected_parent(columns, rows, output);
        }
        if columns == 0
            || rows == 0
            || columns > geometry::MAX_AXIS
            || rows > geometry::MAX_AXIS
            || !self.buffer_size_fits(rows, columns)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "console dimensions exceed screen budget",
            ));
        }
        if self.input_mode & 8 != 0 {
            self.enqueue_input(
                &[ConsoleInput::Resize {
                    columns: columns as i16,
                    rows: rows as i16,
                }],
                output,
            )?;
        }
        self.terminal.screen_mut().set_size(rows, columns);
        Ok(())
    }

    pub fn input_mode(&self) -> u32 {
        self.input_mode
    }

    pub fn connected_clients(&self) -> usize {
        self.processes.len()
    }

    /// Live server handle records, useful alongside connected_clients when
    /// observing native close/connection cleanup without exposing identities.
    pub fn open_handles(&self) -> usize {
        self.handles.len()
    }

    fn accept_origin(&mut self, request: &DriverRequest<'session>) -> bool {
        if self.session_id.is_some_and(|id| id != request.session_id()) {
            return false;
        }
        self.session_id = Some(request.session_id());
        self.session = Some(request.origin());
        true
    }

    /// Dispatch queries and controls while terminal output is backpressured.
    /// Calls which may emit or change output interpretation retain their real
    /// kernel request, in order, without copying client payloads. The adapter
    /// resumes one only when its bounded output staging is empty.
    pub fn dispatch_with_output_ready(
        &mut self,
        request: DriverRequest<'session>,
        ready: bool,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let api = (descriptor.function == 7)
            .then(|| read_u32(request.inline_payload(), 0))
            .flatten();
        // Pin the transport before retaining its first CONNECT. Foreign calls
        // must not occupy the queue or bind the host through a later bypass.
        if !self.accept_origin(&request) {
            return self.dispatch(request, output);
        }
        // Fail closed for future API additions: only known non-emitting calls
        // bypass ordered output. Input reads may cook and echo queued keys.
        let bypass = descriptor.function == 3
            || descriptor.function == 8
            || (descriptor.function == 1 && !self.processes.is_empty())
            || (matches!(descriptor.function, 2 | 4) && !self.close_may_publish(descriptor))
            || (descriptor.function == 7
                && matches!(
                    api,
                    Some(
                        0x0200_0001
                            | 0x0200_0003
                            | 0x0100_0003
                            | 0x0100_0000
                            | 0x0100_0001
                            | 0x0100_0008
                            | 0x0200_0005
                            | 0x0200_0007
                            | 0x0200_000b
                            | 0x0200_000f
                            | 0x0200_0013
                            | 0x0200_0014
                            | 0x0300_0003
                            | 0x0300_0004
                            | 0x0300_0029
                    )
                ));
        if !bypass && (!ready || !self.deferred_output.is_empty()) {
            let mut receipt = DispatchReceipt {
                function: descriptor.function,
                api,
                status: SUCCESS,
                pending: true,
            };
            let limit = if matches!(descriptor.function, 2 | 4) {
                // Kernel finalizers must retain capacity even when ordinary
                // emitting calls exhaust their queue. At most one final close
                // per retained file object and one disconnect per connection.
                MAX_DEFERRED_OUTPUT + objects::MAX_HANDLES + MAX_CLIENTS
            } else {
                MAX_DEFERRED_OUTPUT
            };
            if self.deferred_output.len() >= limit {
                receipt.status = 0xc000_009a_u32 as i32; // STATUS_INSUFFICIENT_RESOURCES
                receipt.pending = false;
                request.complete(receipt.status, 0, &[])?;
            } else {
                self.deferred_output.push_back(request);
            }
            return Ok(receipt);
        }
        self.dispatch(request, output)
    }

    /// Resume at most one queued emitting call; the caller must have drained
    /// the preceding call's terminal bytes before invoking this method.
    pub fn resume_output(
        &mut self,
        output: &mut impl Write,
    ) -> io::Result<Option<DispatchReceipt>> {
        self.deferred_output
            .pop_front()
            .map(|request| self.dispatch(request, output))
            .transpose()
    }

    /// Number of retained kernel calls waiting for terminal output capacity.
    pub fn pending_output_requests(&self) -> usize {
        self.deferred_output.len()
    }

    /// Service one real driver request and write terminal bytes to the caller's
    /// bounded output path. No display window or platform host is created.
    pub fn dispatch(
        &mut self,
        request: DriverRequest<'session>,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let activation =
            descriptor.function == 7 && read_u32(request.inline_payload(), 0) == Some(0x0200_0002);
        let target = self
            .handles
            .get(&descriptor.object)
            .filter(|handle| {
                matches!(descriptor.function, 5 | 7)
                    && !activation
                    && handle.kind == HandleKind::Output
                    && handle.buffer != self.active_buffer
            })
            .map(|handle| handle.buffer);
        if let Some(target) = target {
            let stream = descriptor.function == 5
                || (descriptor.function == 7
                    && read_u32(request.inline_payload(), 0) == Some(0x0100_0006));
            if stream {
                return self.with_buffer(target, |host| host.dispatch_inner(request, output));
            }
            return self.with_buffer(target, |host| host.dispatch_inner(request, &mut io::sink()));
        }
        self.dispatch_inner(request, output)
    }

    fn dispatch_inner(
        &mut self,
        request: DriverRequest<'session>,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let mut receipt = DispatchReceipt {
            function: descriptor.function,
            api: None,
            status: SUCCESS,
            pending: false,
        };
        if !self.accept_origin(&request) {
            receipt.status = INVALID_HANDLE;
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        self.update_input_event()?;
        match descriptor.function {
            1 => {
                if self.processes.len() >= MAX_CLIENTS
                    || self.handles.len() > objects::MAX_HANDLES - 3
                    || descriptor.process == 0
                    || u32::try_from(descriptor.process).is_err()
                    || descriptor.input_size < 1340
                    || descriptor.output_size < (3 * size_of::<usize>()) as u32
                {
                    receipt.status = INVALID_PARAMETER;
                    request.complete(receipt.status, 0, &[])?;
                } else {
                    let initial_title = if self.processes.is_empty() {
                        let connect = request.read_input(0, 1340)?;
                        let length = u16::from_le_bytes([connect[34], connect[35]]) as usize;
                        if length > 520 || !length.is_multiple_of(2) {
                            receipt.status = INVALID_PARAMETER;
                            request.complete(receipt.status, 0, &[])?;
                            return Ok(receipt);
                        }
                        Some(
                            connect[36..36 + length]
                                .chunks_exact(2)
                                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                                .collect::<Vec<u16>>(),
                        )
                    } else {
                        None
                    };
                    let process = self.token()?;
                    let input = self.token()?;
                    let output_handle = self.token()?;
                    let reply: Vec<u8> = [process, input, output_handle]
                        .into_iter()
                        .flat_map(usize::to_ne_bytes)
                        .collect();
                    if let Some(control) = &mut self.control {
                        let connect = request.read_input(0, 1340)?;
                        if control
                            .admit(
                                process,
                                descriptor.process as u32,
                                connect[32] != 0,
                                read_u32(&connect, 28).unwrap(),
                            )
                            .is_err()
                        {
                            receipt.status = 0xc000_0022_u32 as i32;
                            request.complete(receipt.status, 0, &[])?;
                            return Ok(receipt);
                        }
                    }
                    // Admit before publication; a rejected identity must not
                    // change title metadata or send a frontend title event.
                    if let Some(title) = &initial_title
                        && let Err(error) = self.publish_native_title(title, output)
                    {
                        if let Some(control) = &mut self.control {
                            control.remove(process);
                        }
                        return Err(error);
                    }
                    request.complete(SUCCESS, reply.len(), &reply)?;
                    if let Some(title) = initial_title {
                        self.original_title = title.clone();
                        self.title = title;
                    }
                    self.processes.insert(process, descriptor.process as u32);
                    self.handles.insert(
                        input,
                        ClientHandle {
                            lifetime: HandleLifetime::Connection(process),
                            kind: HandleKind::Input,
                            access: objects::READ | objects::WRITE,
                            share: 3,
                            buffer: 0,
                        },
                    );
                    self.handles.insert(
                        output_handle,
                        ClientHandle {
                            lifetime: HandleLifetime::Connection(process),
                            kind: HandleKind::Output,
                            access: objects::READ | objects::WRITE,
                            share: 3,
                            buffer: self.active_buffer,
                        },
                    );
                }
            }
            2 => {
                self.deferred_output.retain(|pending| {
                    pending.descriptor.function == 4
                        || pending.descriptor.process != descriptor.process
                });
                self.cancel_process_reads(descriptor.process);
                if let Some(control) = &mut self.control {
                    control.remove(descriptor.process);
                }
                if self.processes.remove(&descriptor.process).is_none() {
                    receipt.status = INVALID_HANDLE;
                }
                let defaults: Vec<_> = self
                    .handles
                    .iter()
                    .filter(|(_, handle)| {
                        handle.lifetime == HandleLifetime::Connection(descriptor.process)
                    })
                    .map(|(id, _)| *id)
                    .collect();
                for object in defaults {
                    self.retire_input_object(object);
                    self.handles.remove(&object);
                }
                self.update_input_event()?;
                self.collect_buffers(output)?;
                request.complete(receipt.status, 0, &[])?;
            }
            3 => return self.create_object(request),
            4 => {
                self.deferred_output
                    .retain(|pending| pending.descriptor.object != descriptor.object);
                // This is a kernel object finalizer, including closes after
                // process disconnect; it need not carry a live process token.
                self.retire_input_object(descriptor.object);
                self.handles.remove(&descriptor.object);
                self.collect_buffers(output)?;
                self.update_input_event()?;
                request.complete(receipt.status, 0, &[])?;
            }
            5 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status != SUCCESS {
                    request.complete(receipt.status, 0, &[])?;
                } else {
                    let bytes = request.read_input(0, descriptor.input_size as usize)?;
                    receipt.status = output::write_status(self.write_ansi(&bytes, output))?;
                    request.complete(
                        receipt.status,
                        if receipt.status == SUCCESS {
                            bytes.len()
                        } else {
                            0
                        },
                        &[],
                    )?;
                }
            }
            6 => return self.defer_text_read(request, Vec::new(), true, output),
            7 => return self.dispatch_api(request, output),
            8 => {
                receipt.status = self.flush_input(descriptor.process, descriptor.object)?;
                request.complete(receipt.status, 0, &[])?;
            }
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

    fn dispatch_api(
        &mut self,
        request: DriverRequest<'session>,
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
            pending: false,
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
            0x0200_0003 => {
                receipt.status = self.flush_input(descriptor.process, descriptor.object)?;
            }
            0x0300_0029 if body.len() >= 4 => {
                // Native connection registry only; neither global process
                // enumeration nor caller-sized allocation belongs here.
                let mut ids: Vec<_> = self.processes.values().copied().collect();
                ids.sort_unstable();
                ids.dedup();
                body[..4].copy_from_slice(&(ids.len() as u32).to_le_bytes());
                let capacity = (descriptor.output_size - body_size) as usize / 4;
                if ids.len() <= capacity {
                    let bytes: Vec<_> = ids.into_iter().flat_map(u32::to_le_bytes).collect();
                    request.write_output(body_size, &bytes)?;
                    information = bytes.len();
                }
            }
            0x0200_0002 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    receipt.status =
                        self.activate_buffer(self.handles[&descriptor.object].buffer, output)?;
                }
            }
            0x0200_0000 if body.len() >= 16 => {
                let requested = read_u32(&body, 12).unwrap();
                body[12..16].fill(0);
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    let kind = read_u32(&body, 4).unwrap();
                    let element = u16::from_le_bytes([body[8], body[9]]);
                    let column = u16::from_le_bytes([body[0], body[1]]);
                    let row = u16::from_le_bytes([body[2], body[3]]);
                    match self.fill_output(
                        kind,
                        element,
                        row,
                        column,
                        requested as usize,
                        output,
                    )? {
                        Ok(written) => {
                            body[12..16].copy_from_slice(&(written as u32).to_le_bytes())
                        }
                        Err(status) => receipt.status = status,
                    }
                }
            }
            0x0200_0001 if body.len() >= 8 => {
                let event = match read_u32(&body, 0).unwrap() {
                    0 => Some(crate::control::ControlEvent::CtrlC),
                    1 => Some(crate::control::ControlEvent::CtrlBreak),
                    _ => None,
                };
                let group = read_u32(&body, 4).unwrap();
                receipt.status = match (event, &self.control) {
                    (None, _) => INVALID_PARAMETER,
                    (Some(_), None) => NOT_SUPPORTED,
                    (Some(event), Some(control)) if control.has_group(group) => {
                        // API generation posts a signal; unlike keyboard input,
                        // it does not flush input or terminate console reads.
                        control.send(event, group);
                        SUCCESS
                    }
                    _ => INVALID_PARAMETER,
                };
            }
            0x0100_0003 if body.len() >= 4 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Input),
                    objects::READ,
                );
                if receipt.status == SUCCESS {
                    body[..4].copy_from_slice(&(self.input.events.len() as u32).to_le_bytes());
                }
            }
            0x0100_0004 if body.len() >= 8 => return self.defer_record_read(request, body, output),
            0x0100_0005 if body.len() >= 20 => {
                return self.defer_text_read(request, body, false, output);
            }
            0x0200_0010 if body.len() >= 8 => {
                return self.write_input_records(request, body, output);
            }
            0x0100_0000 if body.len() >= 8 => {
                let code_page = if body[4] != 0 {
                    self.output_code_page
                } else {
                    self.input_code_page
                };
                body[..4].copy_from_slice(&code_page.to_le_bytes());
            }
            0x0200_0004 if body.len() >= 8 => {
                let code_page = read_u32(&body, 0).unwrap();
                // SAFETY: checks a numeric code-page ID without borrowed buffers.
                if unsafe { IsValidCodePage(code_page) } == 0 {
                    receipt.status = INVALID_PARAMETER;
                } else if body[4] != 0 {
                    self.output_code_page = code_page;
                } else {
                    self.input_code_page = code_page;
                }
            }
            0x0100_0001 | 0x0100_0002 if body.len() >= 4 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    None,
                    if api == 0x0100_0001 {
                        objects::READ
                    } else {
                        objects::WRITE
                    },
                );
                if receipt.status == SUCCESS {
                    let mode = match self.handles[&descriptor.object].kind {
                        HandleKind::Input => &mut self.input_mode,
                        HandleKind::Output => &mut self.output_mode,
                    };
                    if api == 0x0100_0001 {
                        body[..4].copy_from_slice(&mode.to_le_bytes());
                    } else {
                        let requested = read_u32(&body, 0).unwrap();
                        if matches!(self.handles[&descriptor.object].kind, HandleKind::Input)
                            && requested & 4 != 0
                            && requested & 2 == 0
                        {
                            receipt.status = INVALID_PARAMETER;
                        } else {
                            *mode = requested;
                            if matches!(self.handles[&descriptor.object].kind, HandleKind::Output) {
                                self.set_output_mode(requested, output)?;
                            }
                        }
                    }
                }
            }
            0x0100_0006 if body.len() >= 8 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    let offset = body_size + 8;
                    let length = descriptor.input_size - offset;
                    let bytes = request.read_input(offset, length as usize)?;
                    body[..4].fill(0);
                    if body[4] != 0 {
                        if !bytes.len().is_multiple_of(2) {
                            receipt.status = INVALID_PARAMETER;
                        } else {
                            receipt.status = output::write_status(
                                self.write_utf16(
                                    bytes
                                        .chunks_exact(2)
                                        .map(|b| u16::from_le_bytes([b[0], b[1]])),
                                    output,
                                ),
                            )?;
                        }
                    } else {
                        receipt.status = output::write_status(self.write_ansi(&bytes, output))?;
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
            0x0200_0005 if body.len() >= 8 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    // Actual 20348 server and pinned source require WRITE,
                    // despite the public GetConsoleCursorInfo READ wording.
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    body[..4].copy_from_slice(&self.cursor_size.to_le_bytes());
                    body[4] = u8::from(!self.terminal.screen().hide_cursor());
                }
            }
            0x0200_0006 if body.len() >= 8 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS
                    && let Err(status) =
                        self.set_cursor_info(read_u32(&body, 0).unwrap(), body[4] != 0, output)?
                {
                    receipt.status = status;
                }
            }
            0x0200_000a if body.len() >= 4 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    let column = u16::from_le_bytes([body[0], body[1]]);
                    let row = u16::from_le_bytes([body[2], body[3]]);
                    let (rows, columns) = self.terminal.screen().size();
                    if column >= columns || row >= rows {
                        receipt.status = INVALID_PARAMETER;
                    } else {
                        self.emit_native(
                            format!("\x1b[{};{}H", row + 1, column + 1).as_bytes(),
                            output,
                        )?;
                    }
                }
            }
            0x0200_000d if body.len() >= 2 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    let attribute = u16::from_le_bytes([body[0], body[1]]);
                    if attribute & !0xc0ff != 0 {
                        receipt.status = NOT_SUPPORTED;
                    } else {
                        self.set_text_attribute(attribute, output)?;
                    }
                }
            }
            0x0200_000b if body.len() >= 4 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                body[..4].fill(0);
                if receipt.status == SUCCESS {
                    put_coord(&mut body, 0, geometry::MAX_AXIS, geometry::MAX_AXIS);
                }
            }
            0x0200_0009 if body.len() >= 4 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS
                    && let Err(status) = self.set_buffer_size(
                        u16::from_le_bytes([body[0], body[1]]),
                        u16::from_le_bytes([body[2], body[3]]),
                        output,
                    )?
                {
                    receipt.status = status;
                }
            }
            0x0200_000e if body.len() >= 10 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                let rect =
                    std::array::from_fn(|i| i16::from_le_bytes([body[2 + i * 2], body[3 + i * 2]]));
                if receipt.status == SUCCESS
                    && let Err(status) = self.set_window_info(body[0] != 0, rect, output)?
                {
                    receipt.status = status;
                }
            }
            0x0200_0007 if body.len() >= 92 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::READ,
                );
                if receipt.status == SUCCESS {
                    let (rows, cols) = self.terminal.screen().size();
                    let (row, col) = self.terminal.screen().cursor_position();
                    body.fill(0);
                    put_coord(&mut body, 0, cols, rows);
                    put_coord(&mut body, 4, col.min(cols.saturating_sub(1)), row);
                    body[12..14].copy_from_slice(&self.text_attribute().to_le_bytes());
                    let view = self.viewport();
                    put_coord(&mut body, 8, view.area.left as u16, view.area.top as u16);
                    // The Win32 client combines origin and exclusive size.
                    put_coord(
                        &mut body,
                        14,
                        (view.area.right - view.area.left) as u16,
                        (view.area.bottom - view.area.top) as u16,
                    );
                    put_coord(
                        &mut body,
                        18,
                        cols.min(geometry::MAX_AXIS),
                        rows.min(geometry::MAX_AXIS),
                    );
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
            0x0300_0003 if body.len() >= 8 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::READ,
                );
                if receipt.status == SUCCESS {
                    if read_u32(&body, 0).unwrap() != 0 {
                        receipt.status = INVALID_PARAMETER;
                    } else {
                        body[4..8].copy_from_slice(&font::size_bytes());
                    }
                }
            }
            0x0300_0004 if body.len() >= 84 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::READ,
                );
                if receipt.status == SUCCESS {
                    body[4..84].copy_from_slice(&font::current_bytes());
                }
            }
            0x0200_000c if body.len() >= 26 => {
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS
                    && let Err(status) = self.scroll_output(&body, output)?
                {
                    receipt.status = status;
                }
            }
            0x0200_0012 if body.len() >= 12 => {
                body[8..12].fill(0);
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::WRITE,
                );
                if receipt.status == SUCCESS {
                    let offset = body_size + 8;
                    if descriptor.input_size > 1024 * 1024 {
                        receipt.status = NOT_SUPPORTED;
                    } else {
                        let bytes = request.read_input(
                            offset,
                            descriptor.input_size.saturating_sub(offset) as usize,
                        )?;
                        match self.write_output_string(
                            read_u32(&body, 4).unwrap(),
                            u16::from_le_bytes([body[0], body[1]]),
                            u16::from_le_bytes([body[2], body[3]]),
                            &bytes,
                            output,
                        )? {
                            Ok(written) => {
                                body[8..12].copy_from_slice(&(written as u32).to_le_bytes())
                            }
                            Err(status) => receipt.status = status,
                        }
                    }
                }
            }
            0x0200_0011 | 0x0200_0013 if body.len() >= 10 => {
                let region =
                    std::array::from_fn(|i| i16::from_le_bytes([body[i * 2], body[i * 2 + 1]]));
                let unicode = body[8] != 0;
                for (i, value) in [
                    region[0],
                    region[1],
                    region[0].saturating_sub(1),
                    region[1].saturating_sub(1),
                ]
                .into_iter()
                .enumerate()
                {
                    body[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
                }
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    if api == 0x0200_0011 {
                        objects::WRITE
                    } else {
                        objects::READ
                    },
                );
                if receipt.status == SUCCESS {
                    if api == 0x0200_0011 {
                        let offset = body_size + 8;
                        let length = descriptor.input_size.saturating_sub(offset) as usize;
                        if descriptor.input_size > 1024 * 1024 {
                            receipt.status = NOT_SUPPORTED;
                        } else {
                            let bytes = request.read_input(offset, length)?;
                            match self.write_output_rect(region, unicode, &bytes, output)? {
                                Ok(actual) => {
                                    for (i, value) in actual.into_iter().enumerate() {
                                        body[i * 2..i * 2 + 2]
                                            .copy_from_slice(&value.to_le_bytes());
                                    }
                                }
                                Err(status) => receipt.status = status,
                            }
                        }
                    } else if descriptor.output_size > 1024 * 1024 {
                        receipt.status = NOT_SUPPORTED;
                    } else {
                        let capacity = (descriptor.output_size - body_size) as usize;
                        match self.read_output_rect(region, unicode, capacity) {
                            Ok((actual, bytes)) => {
                                request.write_output(body_size, &bytes)?;
                                information = bytes.len();
                                for (i, value) in actual.into_iter().enumerate() {
                                    body[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
                                }
                            }
                            Err(status) => receipt.status = status,
                        }
                    }
                }
            }
            0x0200_000f if body.len() >= 12 => {
                body[8..12].fill(0);
                receipt.status = self.handle_status(
                    descriptor.process,
                    descriptor.object,
                    Some(HandleKind::Output),
                    objects::READ,
                );
                if receipt.status == SUCCESS {
                    let kind = read_u32(&body, 4).unwrap();
                    let column = u16::from_le_bytes([body[0], body[1]]);
                    let row = u16::from_le_bytes([body[2], body[3]]);
                    let capacity = (descriptor.output_size - body_size) as usize;
                    match self.read_output_string(kind, column, row, capacity) {
                        Ok(bytes) => {
                            request.write_output(body_size, &bytes)?;
                            let records = if kind == 1 {
                                bytes.len()
                            } else {
                                bytes.len() / 2
                            };
                            body[8..12].copy_from_slice(&(records as u32).to_le_bytes());
                            information = capacity;
                        }
                        Err(status) => receipt.status = status,
                    }
                }
            }
            0x0200_0014 if body.len() >= 8 => {
                let capacity = (descriptor.output_size - body_size) as usize;
                match self.title_reply(body[4] != 0, body[5] != 0, capacity) {
                    Ok((needed, bytes)) => {
                        request.write_output(body_size, &bytes)?;
                        body[..4].copy_from_slice(&(needed as u32).to_le_bytes());
                        information = bytes.len();
                    }
                    Err(status) => receipt.status = status,
                }
            }
            0x0200_0015 if !body.is_empty() => {
                let offset = body_size + 8;
                let bytes =
                    request.read_input(offset, (descriptor.input_size - offset) as usize)?;
                match self.decode_title_payload(body[0] != 0, &bytes) {
                    Ok(title) => {
                        receipt.status = output::write_status(self.set_native_title(title, output))?
                    }
                    Err(status) => receipt.status = status,
                }
            }
            _ => receipt.status = NOT_SUPPORTED,
        }
        request.complete(receipt.status, information, &body)?;
        Ok(receipt)
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
    fn rust_console_host_control_keys_require_the_native_shortcut_modifiers() {
        let mut host = ConsoleHost::new();
        let key = |down, virtual_key, character, controls| ConsoleInput::Key {
            down,
            repeat: 1,
            virtual_key,
            character,
            controls,
            scan_code: 0,
        };
        let data = [
            key(false, 3, 0, 8),
            key(true, 3, 0, 0),
            key(true, 3, 0, 9),
            key(true, 0x43, 3, 9),
        ];
        host.enqueue_input(&data, &mut Vec::new()).unwrap();
        assert_eq!(host.input.events.len(), 4);
        let error = host
            .enqueue_input(&[key(true, 0x43, 0, 8)], &mut Vec::new())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(host.input.events.len(), 4);
    }

    #[test]
    fn rust_console_host_bounds_interrupt_batch_before_attempting_delivery() {
        let mut host = ConsoleHost::new();
        let event = ConsoleInput::Key {
            down: true,
            repeat: 1,
            virtual_key: 0x43,
            scan_code: 0,
            character: 3,
            controls: 8,
        };
        let error = host
            .enqueue_input(&vec![event; 4097], &mut Vec::new())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(host.input.events.is_empty());
        assert_eq!(host.control_targets(), 0);
    }

    #[test]
    fn rust_console_host_preserves_a_surrogate_pair_across_writes() {
        let mut host = ConsoleHost::new();
        let mut output = Vec::new();
        host.write_utf16([0x4e2d, 0xd83d], &mut output).unwrap();
        assert_eq!(output, "中".as_bytes());
        host.write_utf16([0xde00, 0x6587], &mut output).unwrap();
        assert_eq!(output, "中😀文".as_bytes());
        assert_eq!(host.terminal.screen().cursor_position(), (0, 6));
    }

    #[test]
    fn rust_console_host_invalid_surrogate_does_not_swallow_the_next_character() {
        let mut host = ConsoleHost::new();
        let mut output = Vec::new();
        host.write_utf16([0xd800], &mut output).unwrap();
        host.write_utf16([b'A' as u16, 0xdc00], &mut output)
            .unwrap();
        assert_eq!(output, "�A�".as_bytes());
        assert_eq!(host.terminal.screen().contents(), "�A�");
        assert_eq!(host.terminal.screen().cursor_position(), (0, 3));
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
        assert_eq!(host.terminal.screen().cursor_position(), (0, 0));
        let result = host.set_text_attribute(0xc012, &mut FailedOutput);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(host.text_attribute(), 7);
        let result = host.fill_output(2, u16::from(b'X'), 0, 0, 1, &mut FailedOutput);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(host.terminal.screen().cell(0, 0).unwrap().contents(), "");
        host.set_output_mode(7, &mut Vec::new()).unwrap();
        host.write_ansi(b"\x1b[3", &mut Vec::new()).unwrap();
        assert!(!host.terminal.at_boundary());
        let error = host
            .emit_native(b"\x1b[2;2H", &mut FailedOutput)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        let error = host.finish_output(&mut FailedOutput).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[3");
        assert_eq!(host.terminal.screen().cursor_position(), (0, 0));
        host.write_ansi(b"1", &mut Vec::new()).unwrap();
        let error = host.write_ansi(b"m", &mut FailedOutput).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[31");
        assert!(!host.terminal.at_boundary());
        let mut output = Vec::new();
        host.write_ansi(b"mX", &mut output).unwrap();
        assert_eq!(output, b"\x1b[31mX");
        assert_eq!(
            host.terminal.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
    }
}
