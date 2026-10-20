//! Native connection-owned histories. Released buffers remain bounded and reusable.
use super::*;
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

const MAX_BUFFERS: usize = 64;
const MAX_COMMANDS: usize = 1024;
const MAX_UNITS: usize = 262144;
const NO_MEMORY: i32 = 0xc000_0017_u32 as i32;
const OVERFLOW: i32 = 0xc007_006f_u32 as i32;

struct History {
    owner: Option<usize>,
    exe: Vec<u16>,
    capacity: usize,
    commands: Vec<Vec<u16>>,
    last: Option<usize>,
    reset: bool,
}
impl History {
    fn matches(&self, exe: &[u16]) -> bool {
        // SAFETY: both bounded UTF-16 slices remain live for this ordinal comparison.
        unsafe {
            CompareStringOrdinal(
                self.exe.as_ptr(),
                self.exe.len() as i32,
                exe.as_ptr(),
                exe.len() as i32,
                1,
            ) == CSTR_EQUAL
        }
    }
    fn reset(&mut self) {
        self.last = self.commands.len().checked_sub(1);
        self.reset = true;
    }
    fn resize(&mut self, capacity: usize) {
        if self.capacity != capacity {
            // Native resize retains the oldest prefix, unlike append eviction.
            self.commands.truncate(capacity);
            self.capacity = capacity;
            self.reset();
        }
    }
    fn remove(&mut self, index: usize) {
        self.commands.remove(index);
        self.last = self.last.and_then(|last| {
            if last == index {
                None
            } else {
                Some(last - usize::from(last > index))
            }
        });
    }
}
pub(super) struct Histories {
    size: usize,
    buffers: usize,
    flags: u32,
    entries: VecDeque<History>,
}
impl Default for Histories {
    fn default() -> Self {
        Self {
            size: 50,
            buffers: 4,
            flags: 0,
            entries: VecDeque::new(),
        }
    }
}
impl Histories {
    pub(super) fn attach(&mut self, owner: usize, exe: Vec<u16>) {
        let same = self
            .entries
            .iter()
            .position(|h| h.owner.is_none() && h.matches(&exe));
        let candidate = same.or_else(|| {
            if self.entries.len() < self.buffers {
                None
            } else {
                self.entries
                    .iter()
                    .rposition(|h| h.owner.is_none() && h.commands.is_empty())
                    .or_else(|| self.entries.iter().rposition(|h| h.owner.is_none()))
            }
        });
        let history = if let Some(index) = candidate {
            let mut history = self.entries.remove(index).unwrap();
            if same.is_none() {
                history.exe = exe;
                history.commands.clear();
                history.reset();
            }
            history.owner = Some(owner);
            history
        } else if self.entries.len() < self.buffers {
            History {
                owner: Some(owner),
                exe,
                capacity: self.size,
                commands: Vec::new(),
                last: None,
                reset: true,
            }
        } else {
            return;
        };
        self.entries.push_front(history);
    }
    pub(super) fn detach(&mut self, owner: usize) {
        if let Some(history) = self.entries.iter_mut().find(|h| h.owner == Some(owner)) {
            history.owner = None;
        }
    }
    fn configure(&mut self, size: usize, buffers: usize, flags: u32) -> i32 {
        if size > MAX_COMMANDS || buffers > MAX_BUFFERS {
            return NO_MEMORY;
        }
        if flags & !1 != 0 {
            return INVALID_PARAMETER;
        }
        self.size = size;
        self.buffers = buffers;
        self.flags = flags;
        for history in &mut self.entries {
            history.resize(size);
        }
        SUCCESS
    }
    fn find_exe(&self, exe: &[u16]) -> Option<usize> {
        self.entries
            .iter()
            .position(|h| h.owner.is_some() && h.matches(exe))
    }
    fn units(&self) -> usize {
        self.entries
            .iter()
            .flat_map(|h| &h.commands)
            .map(Vec::len)
            .sum()
    }
    pub(super) fn record(&mut self, owner: usize, line: &[u16]) {
        if line.is_empty() {
            return;
        }
        let Some(index) = self.entries.iter().position(|h| h.owner == Some(owner)) else {
            return;
        };
        let used = self.units();
        let history = &mut self.entries[index];
        if history.capacity == 0 {
            return;
        }
        if history.commands.last().is_some_and(|last| last == line) {
            history.reset = true;
            return;
        }
        let duplicate = if self.flags & 1 != 0 {
            history.commands.iter().rposition(|item| item == line)
        } else {
            None
        };
        let remove_first =
            history.commands.len() - usize::from(duplicate.is_some()) >= history.capacity;
        let removed = duplicate.map_or(0, |i| history.commands[i].len())
            + if remove_first {
                history.commands[usize::from(duplicate == Some(0))].len()
            } else {
                0
            };
        // History is convenience state: allocation pressure cannot fail the
        // caller's already accepted console read. Keep prior records intact.
        if used - removed + line.len() > MAX_UNITS {
            return;
        }
        if let Some(index) = duplicate {
            history.remove(index);
        }
        if remove_first {
            history.remove(0);
        }
        history.commands.push(line.to_vec());
        if !history
            .last
            .and_then(|i| history.commands.get(i))
            .is_some_and(|item| item == line)
        {
            history.reset();
        }
        history.reset = true;
    }
    pub(super) fn reset(&mut self, owner: usize) {
        if let Some(history) = self.entries.iter_mut().find(|h| h.owner == Some(owner)) {
            history.reset();
        }
    }
    pub(super) fn recall(&mut self, owner: usize, previous: bool) -> Option<Vec<u16>> {
        let history = self.entries.iter_mut().find(|h| h.owner == Some(owner))?;
        let last = history.last?;
        let index = if previous {
            if history.reset {
                history.reset = false;
                last
            } else {
                last.checked_sub(1)?
            }
        } else {
            if last + 1 >= history.commands.len() {
                return None;
            }
            last + 1
        };
        history.last = Some(index);
        history.commands.get(index).cloned()
    }
}

impl ConsoleHost<'_> {
    pub(super) fn history_api(
        &mut self,
        request: &DriverRequest<'_>,
        api: u32,
        body: &mut [u8],
    ) -> io::Result<(i32, usize)> {
        if matches!(api, 0x0300_002a | 0x0300_002b) {
            if body.len() < 12 {
                return Ok((INVALID_PARAMETER, 0));
            }
            if api == 0x0300_002b {
                return Ok((
                    self.histories.configure(
                        read_u32(body, 0).unwrap() as usize,
                        read_u32(body, 4).unwrap() as usize,
                        read_u32(body, 8).unwrap(),
                    ),
                    0,
                ));
            }
            body[..4].copy_from_slice(&(self.histories.size as u32).to_le_bytes());
            body[4..8].copy_from_slice(&(self.histories.buffers as u32).to_le_bytes());
            body[8..12].copy_from_slice(&self.histories.flags.to_le_bytes());
            return Ok((SUCCESS, 0));
        }
        let boolean = if api == 0x0300_0018 { 0 } else { 4 };
        if body.len() <= boolean {
            return Ok((INVALID_PARAMETER, 0));
        }
        let unicode = body[boolean] != 0;
        let offset = body.len() as u32 + 8;
        let length = request.descriptor.input_size.saturating_sub(offset) as usize;
        if length > 1024 * 1024 {
            return Ok((NO_MEMORY, 0));
        }
        let data = request.read_input(offset, length)?;
        if unicode && !data.len().is_multiple_of(2) {
            return Ok((INVALID_PARAMETER, 0));
        }
        let exe = if unicode {
            data.chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect()
        } else {
            encoding::decode(self.input_code_page, &data)?
        };
        let found = self.histories.find_exe(&exe);
        if api == 0x0300_0018 {
            if let Some(index) = found {
                let history = &mut self.histories.entries[index];
                history.commands.clear();
                history.reset();
            }
            return Ok((SUCCESS, 0));
        }
        if api == 0x0300_0019 {
            let size = read_u32(body, 0).unwrap() as usize;
            if size > MAX_COMMANDS {
                return Ok((NO_MEMORY, 0));
            }
            if let Some(index) = found {
                let mut history = self.histories.entries.remove(index).unwrap();
                history.resize(size);
                self.histories.entries.push_front(history);
            }
            return Ok((SUCCESS, 0));
        }
        let units: Vec<_> = found
            .into_iter()
            .flat_map(|i| self.histories.entries[i].commands.iter())
            .flat_map(|line| line.iter().copied().chain([0]))
            .collect();
        let bytes: Vec<_> = if unicode {
            units.into_iter().flat_map(u16::to_le_bytes).collect()
        } else {
            encoding::encode(self.input_code_page, &units, usize::MAX)?.unwrap()
        };
        body[..4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        if api == 0x0300_001a {
            return Ok((SUCCESS, 0));
        }
        let capacity = request.descriptor.output_size as usize - body.len();
        if bytes.len() > capacity {
            return Ok((OVERFLOW, 0));
        }
        request.write_output(body.len() as u32, &bytes)?;
        Ok((SUCCESS, bytes.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_history_total_budget_preserves_other_connections_and_existing_records() {
        let mut histories = Histories::default();
        assert_eq!(histories.configure(1024, 4, 0), SUCCESS);
        histories.attach(1, vec![97]);
        histories.attach(2, vec![98]);
        // Eight maximum-length input lines fill the aggregate content budget.
        // The next process cannot take that storage or erase the first owner's
        // records. Native reads themselves do not fail for convenience-history OOM.
        for ch in 97..105 {
            histories.record(1, &vec![ch; 32768]);
        }
        histories.record(2, &[120]);
        assert_eq!(histories.units(), MAX_UNITS);
        assert_eq!(
            histories
                .entries
                .iter()
                .find(|h| h.owner == Some(1))
                .unwrap()
                .commands
                .len(),
            8
        );
        assert!(
            histories
                .entries
                .iter()
                .find(|h| h.owner == Some(2))
                .unwrap()
                .commands
                .is_empty()
        );
        assert_eq!(histories.recall(1, true), Some(vec![104; 32768]));
        assert_eq!(histories.configure(1025, 4, 0), NO_MEMORY);
        assert_eq!(histories.size, 1024);
        assert_eq!(histories.recall(1, true), Some(vec![103; 32768]));
    }
}
