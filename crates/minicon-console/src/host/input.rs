//! Deferred console input. Requests retain their originating driver's lifetime.
use super::*;
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;
use windows_sys::Win32::Globalization::WideCharToMultiByte;

const MAX_EVENTS: usize = 4096;
const MAX_PENDING: usize = 64;
const MAX_LINE: usize = 32768;
// Console handle closing/interruption uses STATUS_ALERTED in the native
// protocol, not the generic I/O STATUS_CANCELLED (see pinned readData*.cpp).
const READ_INTERRUPTED: i32 = 0x101;

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
    ready: VecDeque<u16>,
    ready_event_suppressed: bool,
    ansi_tail: BTreeMap<usize, VecDeque<u8>>,
    pending: VecDeque<PendingRead<'session>>,
}

impl InputState<'_> {
    pub(super) fn flush_keyboard_input(&mut self) {
        self.events.clear();
        // A completed cooked read can retain text independently of keyboard
        // storage. Native Break resets readiness without discarding that text.
        self.ready_event_suppressed = !self.ready.is_empty();
        self.ansi_tail.clear();
    }
}

enum ReadKind {
    Text {
        unicode: bool,
        raw: bool,
        control_z: bool,
        wakeup: u32,
    },
    Records {
        flags: u16,
    },
}

struct PendingRead<'session> {
    request: DriverRequest<'session>,
    body: Vec<u8>,
    kind: ReadKind,
    line: Vec<u16>,
    controls: u32,
}

impl<'session> ConsoleHost<'session> {
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
        if receipt.status == SUCCESS {
            if body[4] == 0 {
                receipt.status = NOT_SUPPORTED;
            } else if !length.is_multiple_of(20)
                || length / 20 > MAX_EVENTS.saturating_sub(self.input.events.len())
            {
                receipt.status = INVALID_PARAMETER;
            }
        }
        let mut events = Vec::new();
        if receipt.status == SUCCESS {
            let bytes = request.read_input(body.len() as u32 + 8, length)?;
            for record in bytes.chunks_exact(20) {
                let word = |at| u16::from_le_bytes([record[at], record[at + 1]]);
                let signed = |at| word(at) as i16;
                let event = match word(0) {
                    1 => ConsoleInput::Key {
                        down: read_u32(record, 4).unwrap() != 0,
                        repeat: word(8),
                        virtual_key: word(10),
                        scan_code: word(12),
                        character: word(14),
                        controls: read_u32(record, 16).unwrap(),
                    },
                    2 => ConsoleInput::Mouse {
                        x: signed(4),
                        y: signed(6),
                        buttons: read_u32(record, 8).unwrap(),
                        controls: read_u32(record, 12).unwrap(),
                        flags: read_u32(record, 16).unwrap(),
                    },
                    4 => ConsoleInput::Resize {
                        columns: signed(4),
                        rows: signed(6),
                    },
                    16 => ConsoleInput::Focus(read_u32(record, 4).unwrap() != 0),
                    _ => {
                        receipt.status = NOT_SUPPORTED;
                        break;
                    }
                };
                events.push(event);
            }
        }
        if receipt.status == SUCCESS {
            if body[5] != 0 {
                self.input.events.extend(events.iter().copied());
            } else {
                for event in events.iter().rev() {
                    self.input.events.push_front(*event);
                }
            }
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
        let pending = std::mem::take(&mut self.input.pending);
        let count = pending.len();
        // Publish native readiness before any blocked SDK caller resumes.
        let mut failure = self.update_input_event().err();
        for mut read in pending {
            match read.kind {
                ReadKind::Text { raw: false, .. } => read.body[12..20].fill(0),
                ReadKind::Records { .. } => read.body[..4].fill(0),
                _ => {}
            }
            if let Err(error) = read.request.complete(READ_INTERRUPTED, 0, &read.body) {
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
        let previous = self.input.pending.len();
        self.input.pending.push_back(PendingRead {
            request,
            body,
            kind: ReadKind::Text {
                unicode,
                raw,
                control_z,
                wakeup,
            },
            line,
            controls: 0,
        });
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
        receipt.status = self.handle_status(
            request.descriptor.process,
            request.descriptor.object,
            Some(HandleKind::Input),
            objects::READ,
        );
        if receipt.status != SUCCESS {
            // Preserve the native handle/access result before buffer validation.
        } else if flags & !3 != 0
            || request
                .descriptor
                .output_size
                .saturating_sub(body.len() as u32)
                < 20
            || self.input.pending.len() >= MAX_PENDING
        {
            receipt.status = INVALID_PARAMETER;
        } else if body[6] == 0 {
            receipt.status = NOT_SUPPORTED;
        }
        if receipt.status != 0 {
            request.complete(receipt.status, 0, &[])?;
            return Ok(receipt);
        }
        if flags & 2 != 0 {
            // A NOWAIT request cannot queue behind a blocked cooked read.
            let capacity = (request.descriptor.output_size as usize - body.len()) / 20;
            let count = capacity.min(self.input.events.len());
            let bytes: Vec<u8> = self
                .input
                .events
                .iter()
                .take(count)
                .flat_map(record_bytes)
                .collect();
            if flags & 1 == 0 {
                self.input.events.drain(..count);
            }
            let mut body = body;
            body[..4].copy_from_slice(&(count as u32).to_le_bytes());
            request.write_output(body.len() as u32, &bytes)?;
            request.complete(SUCCESS, bytes.len(), &body)?;
            self.update_input_event()?;
            return Ok(receipt);
        }
        let previous = self.input.pending.len();
        self.input.pending.push_back(PendingRead {
            request,
            body,
            kind: ReadKind::Records { flags },
            line: Vec::new(),
            controls: 0,
        });
        receipt.pending = self.service_pending(output)? <= previous;
        Ok(receipt)
    }

    fn service_pending(&mut self, output: &mut impl Write) -> io::Result<usize> {
        let mut completed = 0;
        while let Some(mut read) = self.input.pending.pop_front() {
            let data = match read.kind {
                ReadKind::Records { flags } => {
                    if self.input.events.is_empty() && flags & 2 == 0 {
                        None
                    } else {
                        let capacity =
                            (read.request.descriptor.output_size as usize - read.body.len()) / 20;
                        let count = capacity.min(self.input.events.len());
                        let bytes = self
                            .input
                            .events
                            .iter()
                            .take(count)
                            .flat_map(record_bytes)
                            .collect();
                        if flags & 1 == 0 {
                            self.input.events.drain(..count);
                        }
                        read.body[..4].copy_from_slice(&(count as u32).to_le_bytes());
                        Some(bytes)
                    }
                }
                ReadKind::Text {
                    unicode,
                    control_z,
                    wakeup,
                    ..
                } => {
                    let has_tail = self
                        .input
                        .ansi_tail
                        .get(&read.request.descriptor.object)
                        .is_some_and(|tail| !tail.is_empty());
                    if self.input.ready.is_empty() && !has_tail {
                        self.cook_text(&mut read, wakeup, output)?;
                    }
                    if self.input.ready.is_empty() && !has_tail {
                        None
                    } else if !has_tail
                        && control_z
                        && self.input_mode & 1 != 0
                        && self.input.ready.front() == Some(&26)
                    {
                        self.input.ready.clear();
                        Some(Vec::new())
                    } else {
                        Some(self.encode_ready(&read, unicode)?)
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
            self.update_input_event()?;
            read.request.complete(SUCCESS, bytes.len(), &read.body)?;
            completed += 1;
        }
        self.update_input_event()?;
        Ok(completed)
    }

    fn cook_text(
        &mut self,
        read: &mut PendingRead<'_>,
        wakeup: u32,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let line_mode = self.input_mode & 2 != 0;
        let echo = self.input_mode & 4 != 0;
        let unicode = matches!(read.kind, ReadKind::Text { unicode: true, .. });
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
            read.controls = controls;
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
                        self.input.ready_event_suppressed = false;
                        self.input.ready.extend(read.line.drain(..));
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
            if line_mode && character == 8 {
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
                        let width = char::from_u32(removed)
                            .and_then(UnicodeWidthChar::width)
                            .unwrap_or(1);
                        for _ in 0..width {
                            self.emit(b"\x08 \x08", output)?;
                        }
                    }
                }
                continue;
            }
            if line_mode && character == 13 {
                read.line.extend([13, 10]);
                if echo {
                    self.write_utf16([13, 10], output)?;
                }
                self.input.ready_event_suppressed = false;
                self.input.ready.extend(read.line.drain(..));
                return Ok(());
            }
            if read.line.len() >= MAX_LINE {
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "console input line budget exhausted",
                ));
            }
            read.line.push(character);
            if echo {
                self.write_utf16([character], output)?;
            }
            if !line_mode && !unicode && (0xd800..=0xdbff).contains(&character) {
                // Byte reads must encode a complete supplementary character,
                // even when its two UTF-16 input records arrive separately.
                continue;
            }
            if !line_mode || (character < 32 && wakeup & (1_u32 << character) != 0) {
                self.input.ready_event_suppressed = false;
                self.input.ready.extend(read.line.drain(..));
                return Ok(());
            }
        }
        Ok(())
    }

    fn encode_ready(&mut self, read: &PendingRead<'_>, unicode: bool) -> io::Result<Vec<u8>> {
        let capacity = read.request.descriptor.output_size as usize - read.body.len();
        if unicode {
            if self
                .input
                .ansi_tail
                .get(&read.request.descriptor.object)
                .is_some_and(|tail| !tail.is_empty())
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "switching to Unicode in the middle of an ANSI byte is not qualified",
                ));
            }
            let count = (capacity / 2).min(self.input.ready.len());
            return Ok(self
                .input
                .ready
                .drain(..count)
                .flat_map(u16::to_le_bytes)
                .collect());
        }
        self.encode_ansi_ready(read.request.descriptor.object, capacity)
    }

    fn encode_ansi_ready(&mut self, object: usize, capacity: usize) -> io::Result<Vec<u8>> {
        let mut tail = self.input.ansi_tail.remove(&object).unwrap_or_default();
        let mut bytes = Vec::new();
        while bytes.len() < capacity {
            if let Some(byte) = tail.pop_front() {
                bytes.push(byte);
                continue;
            }
            let Some(first) = self.input.ready.front().copied() else {
                break;
            };
            let mut units = [first, 0];
            let count = if (0xd800..=0xdbff).contains(&first)
                && self
                    .input
                    .ready
                    .get(1)
                    .is_some_and(|u| (0xdc00..=0xdfff).contains(u))
            {
                units[1] = self.input.ready[1];
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
            self.input.ready.drain(..count as usize);
            tail.extend(&encoded[..length as usize]);
        }
        if !tail.is_empty() {
            self.input.ansi_tail.insert(object, tail);
        }
        Ok(bytes)
    }

    pub(super) fn update_input_event(&self) -> io::Result<()> {
        if let Some(session) = self.session {
            session.set_input_available(
                !self.input.events.is_empty()
                    || (!self.input.ready.is_empty() && !self.input.ready_event_suppressed)
                    || !self.input.ansi_tail.is_empty(),
            )?;
        }
        Ok(())
    }

    pub(super) fn cancel_process_reads(&mut self, process: usize) {
        // Waiting operations belong to their caller; shared file conversion
        // state belongs to the file object and can outlive its creator.
        self.input
            .pending
            .retain(|read| read.request.descriptor.process != process);
    }

    pub(super) fn retire_input_object(&mut self, object: usize) {
        self.input.ansi_tail.remove(&object);
        self.input
            .pending
            .retain(|read| read.request.descriptor.object != object);
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
