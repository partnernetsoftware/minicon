//! Deferred console input. Requests retain their originating driver's lifetime.
mod cache;
mod echo;
mod edit;
mod records;
use super::*;
use crate::driver::READ_INTERRUPTED;
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;
use windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED;
use windows_sys::Win32::Globalization::WideCharToMultiByte;

pub(super) const MAX_EVENTS: usize = 4096;
const MAX_PENDING: usize = 64;
const MAX_LINE: usize = 32768;

/// Native console input expressed without exposing Windows union pointers.
#[derive(Clone, Copy, Debug)]
pub enum ConsoleInput {
    Key {
        down: bool,
        repeat: u16,
        virtual_key: u16,
        scan_code: u16,
        character: u16,
        controls: u32,
    },
    Mouse {
        x: i16,
        y: i16,
        buttons: u32,
        controls: u32,
        flags: u32,
    },
    Resize {
        columns: i16,
        rows: i16,
    },
    Focus(bool),
}

#[derive(Default)]
pub(super) struct InputState<'session> {
    pub(super) events: VecDeque<ConsoleInput>,
    read_cache: BTreeMap<usize, cache::ReadCache>,
    ansi_write_lead: Option<ConsoleInput>,
    record_tail: VecDeque<ConsoleInput>,
    pending: VecDeque<PendingRead<'session>>,
    pub(super) insert_mode: Option<bool>,
}

#[derive(Clone, Copy)]
pub(super) enum ReadRetirement {
    Process(usize),
    Object(usize),
}

impl ReadRetirement {
    fn matches(self, read: &PendingRead<'_>) -> bool {
        self.matches_request(&read.request)
    }

    fn matches_request(self, request: &DriverRequest<'_>) -> bool {
        match self {
            Self::Process(process) => request.descriptor.process == process,
            Self::Object(object) => request.descriptor.object == object,
        }
    }

    fn status(self) -> i32 {
        match self {
            Self::Process(_) => 0xc000_004b_u32 as i32, // STATUS_THREAD_IS_TERMINATING
            Self::Object(_) => READ_INTERRUPTED,
        }
    }
}

impl InputState<'_> {
    pub(super) fn retains_output_after(&self, buffer: usize, retirement: ReadRetirement) -> bool {
        self.pending
            .iter()
            .any(|read| read.output_buffer == Some(buffer) && !retirement.matches(read))
    }

    fn take_retired(&mut self, retirement: ReadRetirement) -> VecDeque<PendingRead<'_>> {
        let (retired, retained) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|read| retirement.matches(read));
        self.pending = retained;
        retired
    }
    pub(super) fn retains_output(&self, buffer: usize) -> bool {
        self.pending
            .iter()
            .any(|read| read.output_buffer == Some(buffer))
    }
    fn merged_record(&self, events: &[ConsoleInput], append: bool) -> Option<ConsoleInput> {
        if append && events.len() == 1 {
            self.events
                .back()
                .and_then(|last| records::coalesced(*last, events[0]))
        } else {
            None
        }
    }
    pub(super) fn can_store(&self, events: &[ConsoleInput], append: bool) -> bool {
        let added = if self.merged_record(events, append).is_some() {
            0
        } else {
            events.len()
        };
        added <= MAX_EVENTS.saturating_sub(self.events.len())
    }
    fn store(&mut self, events: &[ConsoleInput], append: bool) {
        if let Some(merged) = self.merged_record(events, append) {
            *self.events.back_mut().unwrap() = merged;
        } else if append {
            self.events.extend(events.iter().copied());
        } else {
            for event in events.iter().rev() {
                self.events.push_front(*event);
            }
        }
    }
    fn commit_records(&mut self, batch: records::ReadBatch) {
        self.events.drain(..batch.consumed);
        self.record_tail = batch.tail;
    }
    pub(super) fn flush_keyboard_input(&mut self) {
        self.events.clear();
        // A completed cooked read can retain text independently of keyboard
        // storage. Native Break resets readiness without discarding that text.
        self.flush_cache();
    }
}

enum ReadKind {
    Text {
        cooked: bool,
        unicode: bool,
        raw: bool,
        control_z: bool,
        wakeup: u32,
    },
    Records {
        flags: u16,
        unicode: bool,
    },
}

#[derive(Clone, Copy)]
enum ReadCancellation {
    Administrative,
    Keyboard { is_break: bool },
}

impl ReadCancellation {
    fn text(self, cooked: bool) -> bool {
        match self {
            Self::Administrative => true,
            Self::Keyboard { is_break } => is_break || cooked,
        }
    }

    fn records(self) -> bool {
        matches!(self, Self::Administrative)
    }
}

fn cooked_input_mode(mode: u32) -> bool {
    mode & 2 != 0 && mode & 0x200 == 0
}

struct PendingRead<'session> {
    request: DriverRequest<'session>,
    body: Vec<u8>,
    kind: ReadKind,
    line: Vec<u16>,
    controls: u32,
    echo_decoder: crate::terminal::Utf16Decoder,
    editor: edit::Editor,
    editing: bool,
    echo: Option<echo::Echo>,
    output_buffer: Option<usize>,
}

impl PendingRead<'_> {
    fn interrupt(self, status: i32) -> io::Result<()> {
        complete_read_interruption(self.request, status)
    }
}

fn complete_read_interruption(request: DriverRequest<'_>, status: i32) -> io::Result<()> {
    // This is a reply to one retiring request. A recipient already cancelled
    // by ConDrv is benign; event-update, output and other transport errors are
    // outside this exception and still propagate through their own operations.
    match request.interrupt_read(status) {
        Err(error) if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) => Ok(()),
        result => result,
    }
}

impl<'session> ConsoleHost<'session> {
    pub(super) fn flush_input(&mut self, process: usize, object: usize) -> io::Result<i32> {
        let status = self.handle_status(process, object, Some(HandleKind::Input), objects::WRITE);
        if status == SUCCESS {
            // Queue flush is also used by keyboard Break, but native Flush
            // does not interrupt readers or send a signal. Completed UTF-16
            // and an in-progress cooked line are independent of this queue.
            self.input.flush_keyboard_input();
            self.update_input_event()?;
        }
        Ok(status)
    }

    pub(super) fn write_input_records(
        &mut self,
        request: DriverRequest<'session>,
        mut body: Vec<u8>,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let descriptor = request.descriptor;
        let mut receipt = DispatchReceipt {
            function: 7,
            api: Some(0x0200_0010),
            status: self.handle_status(
                descriptor.process,
                descriptor.object,
                Some(HandleKind::Input),
                objects::WRITE,
            ),
            pending: false,
        };
        let length = descriptor.input_size.saturating_sub(body.len() as u32 + 8) as usize;
        if receipt.status == SUCCESS && (!length.is_multiple_of(20) || length / 20 > MAX_EVENTS) {
            receipt.status = INVALID_PARAMETER;
        }
        let mut events = Vec::new();
        let mut lead = self.input.ansi_write_lead;
        if receipt.status == SUCCESS {
            let bytes = request.read_input(body.len() as u32 + 8, length)?;
            match records::decode(&bytes) {
                Ok(decoded) => events = decoded,
                Err(status) => receipt.status = status,
            }
        }
        if receipt.status == SUCCESS && body[4] == 0 {
            (events, lead) = records::ansi(self.input_code_page, &events, lead);
        }
        let append = body[5] != 0;
        if receipt.status == SUCCESS && !self.input.can_store(&events, append) {
            receipt.status = INVALID_PARAMETER;
        }
        if receipt.status == SUCCESS {
            self.input.ansi_write_lead = lead;
            self.input.store(&events, append);
            self.service_pending(output)?;
            body[..4].copy_from_slice(&(events.len() as u32).to_le_bytes());
        } else {
            body[..4].fill(0);
        }
        request.complete(
            receipt.status,
            if receipt.status == SUCCESS { length } else { 0 },
            &body,
        )?;
        Ok(receipt)
    }

    pub fn pending_reads(&self) -> usize {
        self.input.pending.len()
    }

    /// Queue input atomically, respecting the bounded event inventory. Text
    /// becomes key-down records; CR/LF spellings each become one Enter key.
    pub fn enqueue_text(&mut self, text: &str, output: &mut impl Write) -> io::Result<usize> {
        let mut events = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(character) = chars.next() {
            let character = if character == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                '\r'
            } else if character == '\n' {
                '\r'
            } else {
                character
            };
            let mut encoded = [0; 2];
            for unit in character.encode_utf16(&mut encoded).iter().copied() {
                events.push(ConsoleInput::Key {
                    down: true,
                    repeat: 1,
                    virtual_key: if matches!(unit, 8 | 9 | 13 | 27) {
                        unit
                    } else {
                        0
                    },
                    scan_code: 0,
                    character: unit,
                    controls: 0,
                });
                if events.len() > MAX_EVENTS {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
            }
        }
        self.enqueue_input(&events, output)
    }

    pub fn enqueue_input(
        &mut self,
        events: &[ConsoleInput],
        output: &mut impl Write,
    ) -> io::Result<usize> {
        if events.len() > MAX_EVENTS {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        let processed = self.input_mode & 1 != 0;
        let control_event = |event: &ConsoleInput| {
            let ConsoleInput::Key {
                down: true,
                repeat: 1..,
                virtual_key,
                character,
                controls,
                ..
            } = event
            else {
                return None;
            };
            if controls & 3 != 0 {
                return None;
            } // Alt excludes console shortcuts.
            if *virtual_key == 3 && controls & 0xc != 0 {
                Some(crate::control::ControlEvent::CtrlBreak)
            } else if processed
                && (*character == 3 || (*virtual_key == 0x43 && controls & 0xc != 0))
            {
                Some(crate::control::ControlEvent::CtrlC)
            } else {
                None
            }
        };
        let queued = events
            .iter()
            .filter(|event| control_event(event).is_none())
            .count();
        // Keep controls and multi-event batch boundaries on their established
        // path. A single ordinary producer uses the same admission as the SDK.
        if events.len() == 1 && queued == 1 {
            if !self.input.can_store(events, true) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.input.store(events, true);
            return self.service_pending(output);
        }
        if queued > MAX_EVENTS.saturating_sub(self.input.events.len()) {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        for event in events {
            if let Some(event) = control_event(event) {
                self.interrupt_control(event)?;
            } else {
                self.input.events.push_back(*event);
            }
        }
        self.service_pending(output)
    }

    /// Wake blocked reads with the console interruption status (STATUS_ALERTED).
    pub fn cancel_pending_reads(&mut self) -> io::Result<usize> {
        self.cancel_reads(ReadCancellation::Administrative)
    }

    pub(super) fn cancel_keyboard_reads(&mut self, is_break: bool) -> io::Result<usize> {
        self.cancel_reads(ReadCancellation::Keyboard { is_break })
    }

    fn cancel_reads(&mut self, cancellation: ReadCancellation) -> io::Result<usize> {
        // Queued and accepted reads retain the same native reply shape.
        // Unadmitted reads have no waiter yet; accepted reads retain the kind
        // selected at admission even if another caller changes LINE_INPUT.
        let cooked = cooked_input_mode(self.input_mode);
        let (deferred_reads, retained): (VecDeque<_>, VecDeque<_>) =
            std::mem::take(&mut self.deferred_output)
                .into_iter()
                .partition(|request| {
                    request.is_console_read()
                        && if request.is_console_text_read() {
                            cancellation.text(cooked)
                        } else {
                            cancellation.records()
                        }
                });
        self.deferred_output = retained;
        let (pending, retained): (VecDeque<_>, VecDeque<_>) =
            std::mem::take(&mut self.input.pending)
                .into_iter()
                .partition(|read| match read.kind {
                    ReadKind::Text { cooked, .. } => cancellation.text(cooked),
                    ReadKind::Records { .. } => cancellation.records(),
                });
        self.input.pending = retained;
        let count = pending.len() + deferred_reads.len();
        // Inactive state needs no publication. An orphaned active buffer stays
        // selected until resume_output admits its transactional fallback.
        self.discard_unused_buffers();
        // Publish native readiness before any blocked SDK caller resumes.
        let mut failure = self.update_input_event().err();
        for request in deferred_reads {
            if let Err(error) = complete_read_interruption(request, READ_INTERRUPTED) {
                failure.get_or_insert(error);
            }
        }
        for read in pending {
            if let Err(error) = read.interrupt(READ_INTERRUPTED) {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(count)
        }
    }

    pub(super) fn defer_text_read(
        &mut self,
        request: DriverRequest<'session>,
        body: Vec<u8>,
        raw: bool,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let mut receipt = DispatchReceipt {
            function: if raw { 6 } else { 7 },
            api: if raw { None } else { Some(0x0100_0005) },
            status: 0,
            pending: false,
        };
        let descriptor = request.descriptor;
        let unicode = !raw && body[0] != 0;
        let capacity = descriptor.output_size.saturating_sub(body.len() as u32) as usize;
        let initial = if raw {
            0
        } else {
            read_u32(&body, 4).unwrap() as usize
        };
        let exe_length = if raw {
            0
        } else {
            u16::from_le_bytes([body[2], body[3]]) as usize * 2
        };
        receipt.status = self.handle_status(
            descriptor.process,
            descriptor.object,
            Some(HandleKind::Input),
            objects::READ,
        );
        if receipt.status != SUCCESS {
            // Preserve invalid-handle versus access-denied native errors.
        } else if capacity == 0
            || capacity > 1024 * 1024
            || (unicode && capacity < 2)
            || self.input.pending.len() >= MAX_PENDING
            || initial > capacity
            || initial > MAX_LINE * 2
            || (!raw
                && exe_length + initial
                    > descriptor.input_size.saturating_sub(8 + body.len() as u32) as usize)
            || (initial != 0 && (!unicode || !initial.is_multiple_of(2)))
        {
            receipt.status = INVALID_PARAMETER;
        }
        if receipt.status != 0 {
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        let line = if initial != 0 {
            let offset = 8_usize
                .checked_add(body.len())
                .and_then(|n| n.checked_add(exe_length))
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            request
                .read_input(offset, initial)?
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect()
        } else {
            Vec::new()
        };
        let control_z = raw || body[1] != 0;
        let wakeup = if raw { 0 } else { read_u32(&body, 8).unwrap() };
        // Mode changes retire encoded record tails, independently of the
        // existing text reader/file conversion ownership.
        self.input.record_tail.clear();
        let previous = self.input.pending.len();
        // Pending completed input is served before selecting a new raw/cooked
        // waiter from current modes, matching the native per-file read state.
        let cooked =
            self.input.has_cooked_ready(descriptor.object) || cooked_input_mode(self.input_mode);
        let mut read = PendingRead {
            request,
            body,
            kind: ReadKind::Text {
                cooked,
                unicode,
                raw,
                control_z,
                wakeup,
            },
            editor: edit::Editor::new(line.len(), self.input.insert_mode.unwrap_or(true)),
            editing: cooked,
            echo: None,
            output_buffer: cooked.then_some(self.active_buffer),
            line,
            controls: 0,
            echo_decoder: crate::terminal::Utf16Decoder::default(),
        };
        // Initial caller text already has an origin at API entry. An empty
        // read deliberately discovers its origin at the first key, permitting
        // applications to publish their prompt after starting a read.
        if cooked && self.input_mode & 4 != 0 && !read.line.is_empty() {
            self.prepare_cooked_echo(&mut read)?;
        }
        self.input.pending.push_back(read);
        receipt.pending = self.service_pending(output)? <= previous;
        Ok(receipt)
    }

    pub(super) fn defer_record_read(
        &mut self,
        request: DriverRequest<'session>,
        body: Vec<u8>,
        output: &mut impl Write,
    ) -> io::Result<DispatchReceipt> {
        let mut receipt = DispatchReceipt {
            function: 7,
            api: Some(0x0100_0004),
            status: 0,
            pending: false,
        };
        let flags = u16::from_le_bytes([body[4], body[5]]);
        let unicode = body[6] != 0;
        let capacity = request
            .descriptor
            .output_size
            .saturating_sub(body.len() as u32) as usize
            / 20;
        receipt.status = self.handle_status(
            request.descriptor.process,
            request.descriptor.object,
            Some(HandleKind::Input),
            objects::READ,
        );
        if receipt.status != SUCCESS {
            // Preserve the native handle/access result before buffer validation.
        } else if flags & !3 != 0
            || (capacity != 0 && flags & 2 == 0 && self.input.pending.len() >= MAX_PENDING)
        {
            receipt.status = INVALID_PARAMETER;
        }
        if receipt.status != 0 {
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        if capacity == 0 {
            let mut body = body;
            body[..4].fill(0);
            request.complete(SUCCESS, 0, &body)?;
            return Ok(receipt);
        }
        if unicode {
            self.input.record_tail.clear();
        }
        if flags & 2 != 0 {
            // A NOWAIT request cannot queue behind a blocked cooked read.
            let batch = records::read(
                self.input_code_page,
                &self.input.events,
                &self.input.record_tail,
                unicode,
                flags & 1 != 0,
                capacity,
            )?;
            let mut body = body;
            body[..4].copy_from_slice(&((batch.bytes.len() / 20) as u32).to_le_bytes());
            let length = batch.bytes.len();
            request.write_output(body.len() as u32, &batch.bytes)?;
            self.input.commit_records(batch);
            self.update_input_event()?;
            request.complete(SUCCESS, length, &body)?;
            return Ok(receipt);
        }
        let previous = self.input.pending.len();
        self.input.pending.push_back(PendingRead {
            request,
            body,
            kind: ReadKind::Records { flags, unicode },
            line: Vec::new(),
            editor: edit::Editor::new(0, true),
            editing: false,
            echo: None,
            output_buffer: None,
            controls: 0,
            echo_decoder: crate::terminal::Utf16Decoder::default(),
        });
        receipt.pending = self.service_pending(output)? <= previous;
        Ok(receipt)
    }

    fn service_pending(&mut self, output: &mut impl Write) -> io::Result<usize> {
        let mut completed = 0;
        while let Some(mut read) = self.input.pending.pop_front() {
            let mut record_batch = None;
            let data = match read.kind {
                ReadKind::Records { flags, unicode } => {
                    let capacity =
                        (read.request.descriptor.output_size as usize - read.body.len()) / 20;
                    let mut batch = records::read(
                        self.input_code_page,
                        &self.input.events,
                        &self.input.record_tail,
                        unicode,
                        flags & 1 != 0,
                        capacity,
                    )?;
                    if batch.bytes.is_empty() && flags & 2 == 0 {
                        None
                    } else {
                        read.body[..4]
                            .copy_from_slice(&((batch.bytes.len() / 20) as u32).to_le_bytes());
                        let bytes = std::mem::take(&mut batch.bytes);
                        record_batch = Some(batch);
                        Some(bytes)
                    }
                }
                ReadKind::Text {
                    cooked,
                    unicode,
                    control_z,
                    wakeup,
                    ..
                } => {
                    let capacity = read.request.descriptor.output_size as usize - read.body.len();
                    let mut bytes = Vec::new();
                    loop {
                        let object = read.request.descriptor.object;
                        let has_tail = self.input.has_tail(object);
                        if !self.input.has_ready(object) && !has_tail {
                            let buffer = read.output_buffer.unwrap_or(self.active_buffer);
                            if buffer == self.active_buffer {
                                self.cook_text(&mut read, wakeup, output)?;
                            } else {
                                self.with_buffer(buffer, |host| {
                                    host.cook_text(&mut read, wakeup, &mut io::sink())
                                })?;
                            }
                        }
                        if !self.input.has_ready(object) && !has_tail {
                            break;
                        }
                        bytes.extend(self.encode_ready(&read, unicode, capacity - bytes.len())?);
                        // A cooked result ends with its completed line/cache. Raw
                        // reads continue through available records, never waiting
                        // for the caller's entire buffer to fill.
                        if cooked || capacity - bytes.len() < if unicode { 2 } else { 1 } {
                            break;
                        }
                    }
                    if bytes.is_empty() {
                        None
                    } else {
                        // Native EOF processing happens after filling the
                        // requested buffer. A later Ctrl+Z remains ordinary
                        // data; unconsumed records/cache stay available.
                        if control_z && self.input_mode & 1 != 0 && bytes.first() == Some(&26) {
                            bytes.clear();
                        }
                        Some(bytes)
                    }
                }
            };
            let Some(bytes) = data else {
                self.input.pending.push_front(read);
                break;
            };
            if let ReadKind::Text { raw: false, .. } = read.kind {
                read.body[12..16].copy_from_slice(&read.controls.to_le_bytes());
                read.body[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            }
            read.request.write_output(read.body.len() as u32, &bytes)?;
            if let Some(batch) = record_batch {
                self.input.commit_records(batch);
            }
            self.update_input_event()?;
            read.request.complete(SUCCESS, bytes.len(), &read.body)?;
            completed += 1;
        }
        self.update_input_event()?;
        if completed != 0 {
            self.collect_buffers(output)?;
        }
        Ok(completed)
    }

    fn cook_text(
        &mut self,
        read: &mut PendingRead<'_>,
        wakeup: u32,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let line_mode = matches!(read.kind, ReadKind::Text { cooked: true, .. });
        let echo = line_mode && self.input_mode & 4 != 0;
        let unicode = matches!(read.kind, ReadKind::Text { unicode: true, .. });
        let mut echo_dirty = false;
        while let Some(event) = self.input.events.pop_front() {
            let ConsoleInput::Key {
                down: true,
                repeat,
                character,
                controls,
                virtual_key,
                scan_code,
            } = event
            else {
                continue;
            };
            if repeat == 0 {
                continue;
            }
            if repeat > 1 {
                self.input.events.push_front(ConsoleInput::Key {
                    down: true,
                    repeat: repeat - 1,
                    character,
                    controls,
                    virtual_key,
                    scan_code,
                });
            }
            // Cooked reads report modifier state only for a wakeup character,
            // not for the ordinary key which happens to finish a line.
            if !line_mode {
                read.controls = controls;
            }
            if line_mode
                && self.input_mode & 0x200 == 0
                && !(character == 3 && self.input_mode & 1 != 0)
            {
                if !read.editing {
                    read.editor =
                        edit::Editor::new(read.line.len(), self.input.insert_mode.unwrap_or(true));
                    read.echo = None;
                }
                read.editing = true;
                if echo {
                    self.prepare_cooked_echo(read)?;
                }
                let outcome = read.editor.handle(
                    &mut read.line,
                    character,
                    virtual_key,
                    controls,
                    self.input_mode & 1 != 0,
                    wakeup,
                )?;
                match outcome {
                    edit::Outcome::Continue => {
                        echo_dirty |= echo;
                        continue;
                    }
                    edit::Outcome::Enter | edit::Outcome::Wake(_) => {
                        if echo {
                            self.redraw_cooked_echo(read, &outcome, output)?;
                        }
                        read.controls = if let edit::Outcome::Wake(modifiers) = outcome {
                            modifiers
                        } else {
                            0
                        };
                        self.input.stage_text(read);
                        return Ok(());
                    }
                }
            }
            read.editing = false;
            if character == 0 {
                if self.input_mode & 0x200 != 0 {
                    let sequence: &[u16] = match virtual_key {
                        0x25 => &[27, 91, 68],
                        0x26 => &[27, 91, 65],
                        0x27 => &[27, 91, 67],
                        0x28 => &[27, 91, 66],
                        0x24 => &[27, 91, 72],
                        0x23 => &[27, 91, 70],
                        _ => &[],
                    };
                    if read.line.len() + sequence.len() > MAX_LINE {
                        return Err(io::Error::new(
                            io::ErrorKind::OutOfMemory,
                            "console VT line budget exhausted",
                        ));
                    }
                    read.line.extend(sequence);
                    if !line_mode && !sequence.is_empty() {
                        self.input.stage_text(read);
                        return Ok(());
                    }
                }
                continue;
            }
            if character == 3 && self.input_mode & 1 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "processed Ctrl+C signal delivery is not implemented",
                ));
            }
            let wakes_line = line_mode && character < 32 && wakeup & (1_u32 << character) != 0;
            if line_mode && !wakes_line && character == 8 && self.input_mode & 1 != 0 {
                if let Some(last) = read.line.pop() {
                    let removed = if (0xdc00..=0xdfff).contains(&last)
                        && read
                            .line
                            .last()
                            .is_some_and(|u| (0xd800..=0xdbff).contains(u))
                    {
                        let high = read.line.pop().unwrap();
                        0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(last) - 0xdc00
                    } else {
                        u32::from(last)
                    };
                    if echo {
                        let width = if (0xd800..=0xdbff).contains(&removed)
                            && read.echo_decoder.discard_pending()
                        {
                            0
                        } else {
                            char::from_u32(removed)
                                .and_then(UnicodeWidthChar::width)
                                .unwrap_or(1)
                        };
                        for _ in 0..width {
                            self.emit_native(b"\x08 \x08", output)?;
                        }
                    }
                }
                continue;
            }
            if line_mode && !wakes_line && character == 13 {
                // Without processed input CR ends the read but does not add LF.
                let processed = self.input_mode & 1 != 0;
                read.line.push(13);
                if processed {
                    read.line.push(10);
                }
                read.controls = 0;
                if echo {
                    self.echo_unit(&mut read.echo_decoder, 13, output)?;
                    if processed {
                        self.echo_unit(&mut read.echo_decoder, 10, output)?;
                    }
                }
                self.input.stage_text(read);
                return Ok(());
            }
            if read.line.len() >= MAX_LINE {
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "console input line budget exhausted",
                ));
            }
            read.line.push(character);
            if wakes_line {
                read.controls = controls;
            }
            // Wakeup units belong to the API result, never the displayed line.
            // Test this before backspace/CR handling as well as ordinary echo.
            if echo && !wakes_line {
                self.echo_unit(&mut read.echo_decoder, character, output)?;
            }
            if !line_mode && !unicode && (0xd800..=0xdbff).contains(&character) {
                // Byte reads must encode a complete supplementary character,
                // even when its two UTF-16 input records arrive separately.
                let next = self.input.events.iter().find_map(|event| match event {
                    ConsoleInput::Key {
                        down: true,
                        repeat,
                        character,
                        virtual_key,
                        ..
                    } if *repeat != 0
                        && (*character != 0
                            || self.input_mode & 0x200 != 0
                                && matches!(*virtual_key, 0x23..=0x28)) =>
                    {
                        Some(*character)
                    }
                    _ => None,
                });
                if next.is_some_and(|unit| (0xdc00..=0xdfff).contains(&unit)) {
                    continue;
                }
                if next.is_none() {
                    // Keep the exact record (and metadata) for a future read
                    // when this batch has already produced earlier bytes.
                    read.line.pop();
                    self.input.events.push_front(event);
                    break;
                }
                // A following non-trail proves this unit is unpaired; the
                // existing codec supplies its code-page replacement.
            }
            if !line_mode || wakes_line {
                self.input.stage_text(read);
                return Ok(());
            }
        }
        if echo_dirty {
            self.redraw_cooked_echo(read, &edit::Outcome::Continue, output)?;
        }
        Ok(())
    }

    fn encode_ready(
        &mut self,
        read: &PendingRead<'_>,
        unicode: bool,
        capacity: usize,
    ) -> io::Result<Vec<u8>> {
        let object = read.request.descriptor.object;
        if unicode {
            if self.input.has_tail(object) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "switching to Unicode in the middle of an ANSI byte is not qualified",
                ));
            }
            let Some(cache) = self.input.read_cache.get_mut(&object) else {
                return Ok(Vec::new());
            };
            let count = (capacity / 2).min(cache.ready.len());
            let bytes = cache
                .ready
                .drain(..count)
                .flat_map(u16::to_le_bytes)
                .collect();
            self.input.prune_cache(object);
            return Ok(bytes);
        }
        self.encode_ansi_ready(object, capacity)
    }

    fn encode_ansi_ready(&mut self, object: usize, capacity: usize) -> io::Result<Vec<u8>> {
        let Some(cache) = self.input.read_cache.get_mut(&object) else {
            return Ok(Vec::new());
        };
        let mut bytes = Vec::new();
        while bytes.len() < capacity {
            if let Some(byte) = cache.bytes.pop_front() {
                bytes.push(byte);
                continue;
            }
            let Some(first) = cache.ready.front().copied() else {
                break;
            };
            let mut units = [first, 0];
            let count = if (0xd800..=0xdbff).contains(&first)
                && cache
                    .ready
                    .get(1)
                    .is_some_and(|u| (0xdc00..=0xdfff).contains(u))
            {
                units[1] = cache.ready[1];
                2
            } else {
                1
            };
            let mut encoded = [0_u8; 16];
            // SAFETY: bounded scalar input and disjoint writable codec buffer.
            let length = unsafe {
                WideCharToMultiByte(
                    self.input_code_page,
                    0,
                    units.as_ptr(),
                    count,
                    encoded.as_mut_ptr(),
                    encoded.len() as i32,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            };
            if length == 0 {
                return Err(io::Error::last_os_error());
            }
            cache.ready.drain(..count as usize);
            cache.bytes.extend(&encoded[..length as usize]);
        }
        self.input.prune_cache(object);
        Ok(bytes)
    }

    pub(super) fn update_input_event(&self) -> io::Result<()> {
        if let Some(session) = self.session {
            session.set_input_available(
                !self.input.events.is_empty() || self.input.cache_available(),
            )?;
        }
        Ok(())
    }

    fn retire_reads(&mut self, retirement: ReadRetirement) -> io::Result<()> {
        let mut failure = None;
        let (retired, retained): (VecDeque<_>, VecDeque<_>) =
            std::mem::take(&mut self.deferred_output)
                .into_iter()
                .partition(|request| {
                    request.descriptor.function != 4 && retirement.matches_request(request)
                });
        self.deferred_output = retained;
        for request in retired {
            if request.is_console_read()
                && let Err(error) = complete_read_interruption(request, retirement.status())
            {
                failure.get_or_insert(error);
            }
        }
        for read in self.input.take_retired(retirement) {
            if let Err(error) = read.interrupt(retirement.status()) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub(super) fn cancel_process_reads(&mut self, process: usize) -> io::Result<()> {
        // Waiting operations belong to their caller; shared file conversion
        // state belongs to the file object and can outlive its creator.
        self.retire_reads(ReadRetirement::Process(process))
    }

    pub(super) fn retire_input_object(&mut self, object: usize) -> io::Result<()> {
        self.input.read_cache.remove(&object);
        self.retire_reads(ReadRetirement::Object(object))
    }
}

impl Drop for ConsoleHost<'_> {
    fn drop(&mut self) {
        self.input.pending.clear(); // DriverRequest::drop uses console read interruption.
        if let Some(session) = self.session {
            let _ = session.set_input_available(false);
        }
    }
}

fn record_bytes(event: &ConsoleInput) -> [u8; 20] {
    let mut bytes = [0; 20];
    match *event {
        ConsoleInput::Key {
            down,
            repeat,
            virtual_key,
            scan_code,
            character,
            controls,
        } => {
            bytes[..2].copy_from_slice(&1_u16.to_le_bytes());
            bytes[4..8].copy_from_slice(&u32::from(down).to_le_bytes());
            for (offset, value) in [
                (8, repeat),
                (10, virtual_key),
                (12, scan_code),
                (14, character),
            ] {
                bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            }
            bytes[16..20].copy_from_slice(&controls.to_le_bytes());
        }
        ConsoleInput::Mouse {
            x,
            y,
            buttons,
            controls,
            flags,
        } => {
            bytes[..2].copy_from_slice(&2_u16.to_le_bytes());
            bytes[4..6].copy_from_slice(&x.to_le_bytes());
            bytes[6..8].copy_from_slice(&y.to_le_bytes());
            for (offset, value) in [(8, buttons), (12, controls), (16, flags)] {
                bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        ConsoleInput::Resize { columns, rows } => {
            bytes[..2].copy_from_slice(&4_u16.to_le_bytes());
            bytes[4..6].copy_from_slice(&columns.to_le_bytes());
            bytes[6..8].copy_from_slice(&rows.to_le_bytes());
        }
        ConsoleInput::Focus(focused) => {
            bytes[..2].copy_from_slice(&16_u16.to_le_bytes());
            bytes[4..8].copy_from_slice(&u32::from(focused).to_le_bytes());
        }
    }
    bytes
}
