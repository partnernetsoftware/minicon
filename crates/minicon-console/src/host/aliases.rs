//! Windows executable-scoped aliases and native message adaptation.
use super::*;

const MAX_EXES: usize = 64;
const MAX_ALIASES: usize = 1024;
const MAX_UNITS: usize = 262144;
const MAX_EXPANSION_UNITS: usize = 32768;
const NO_MEMORY: i32 = 0xc000_0017_u32 as i32;
const MISSING: i32 = 0xc000_0001_u32 as i32;
const SMALL: i32 = 0xc000_0023_u32 as i32;
const LIST_SMALL: i32 = 0xc007_006f_u32 as i32;

#[derive(Default)]
pub(super) struct Aliases {
    entries: BTreeMap<Vec<u16>, BTreeMap<Vec<u16>, Vec<u16>>>,
}
fn key(units: &[u16]) -> Vec<u16> {
    units
        .iter()
        .map(|unit| {
            char::from_u32(u32::from(*unit))
                .and_then(|c| c.to_lowercase().next())
                .and_then(|c| u16::try_from(u32::from(c)).ok())
                .unwrap_or(*unit)
        })
        .collect()
}
impl Aliases {
    fn set(&mut self, exe: Vec<u16>, source: Vec<u16>, target: Vec<u16>) -> i32 {
        if source.is_empty() {
            return INVALID_PARAMETER;
        }
        let (exe, source) = (key(&exe), key(&source));
        if target.is_empty() {
            if let Some(table) = self.entries.get_mut(&exe) {
                table.remove(&source);
            }
            return SUCCESS;
        }
        let old = self.entries.get(&exe).and_then(|table| table.get(&source));
        let count: usize = self.entries.values().map(BTreeMap::len).sum();
        let used: usize = self
            .entries
            .iter()
            .map(|(exe, table)| {
                exe.len()
                    + table
                        .iter()
                        .map(|(source, target)| source.len() + target.len())
                        .sum::<usize>()
            })
            .sum();
        let extra = target.len()
            + if old.is_some() { 0 } else { source.len() }
            + if self.entries.contains_key(&exe) {
                0
            } else {
                exe.len()
            };
        if (!self.entries.contains_key(&exe) && self.entries.len() >= MAX_EXES)
            || (old.is_none() && count >= MAX_ALIASES)
            || used + extra - old.map_or(0, Vec::len) > MAX_UNITS
        {
            return NO_MEMORY;
        }
        self.entries.entry(exe).or_default().insert(source, target);
        SUCCESS
    }
    fn get(&self, exe: &[u16], source: &[u16]) -> Option<&[u16]> {
        self.entries
            .get(&key(exe))?
            .get(&key(source))
            .map(Vec::as_slice)
    }
    fn list(&self, exe: Option<&[u16]>) -> Vec<u16> {
        let mut units = Vec::new();
        if let Some(exe) = exe {
            if let Some(table) = self.entries.get(&key(exe)) {
                for (source, target) in table {
                    units.extend(source);
                    units.push(61);
                    units.extend(target);
                    units.push(0);
                }
            }
        } else {
            for exe in self.entries.keys() {
                units.extend(exe);
                units.push(0);
            }
        }
        units
    }
    pub(super) fn expand(
        &self,
        exe: &[u16],
        line: &[u16],
    ) -> Result<Option<(Vec<u16>, bool)>, i32> {
        // Native alias arguments split on ASCII space, without shell quoting.
        let mut args = Vec::new();
        let mut start = 0;
        while start < line.len() && args.len() < 10 {
            let end = line[start..]
                .iter()
                .position(|u| *u == 32)
                .map_or(line.len(), |n| start + n);
            if start == end {
                break;
            }
            args.push(start..end);
            start = end;
            while start < line.len() && line[start] == 32 {
                start += 1;
            }
        }
        let Some(first) = args.first() else {
            return Ok(None);
        };
        let Some(target) = self.get(exe, &line[first.clone()]) else {
            return Ok(None);
        };
        let mut result = Vec::new();
        let mut multiline = false;
        let mut i = 0;
        while i < target.len() {
            let unit = target[i];
            i += 1;
            if unit != 36 || i == target.len() {
                result.push(unit);
            } else {
                let escaped = target[i];
                i += 1;
                match escaped {
                    49..=57 => {
                        if let Some(range) = args.get((escaped - 48) as usize) {
                            result.extend_from_slice(&line[range.clone()]);
                        }
                    }
                    42 => {
                        if let Some(range) = args.get(1) {
                            result.extend_from_slice(&line[range.start..]);
                        }
                    }
                    76 | 108 => result.push(60),
                    71 | 103 => result.push(62),
                    66 | 98 => result.push(124),
                    84 | 116 => {
                        result.extend([13, 10]);
                        multiline = true;
                    }
                    _ => result.extend([36, escaped]),
                }
            }
            if result.len() > MAX_EXPANSION_UNITS - 2 {
                return Err(NO_MEMORY);
            }
        }
        result.extend([13, 10]);
        Ok(Some((result, multiline)))
    }
}

impl ConsoleHost<'_> {
    pub(super) fn alias_api(
        &mut self,
        request: &DriverRequest<'_>,
        api: u32,
        body: &mut [u8],
    ) -> io::Result<(i32, usize)> {
        let unicode = body[if api == 0x0300_0016 {
            0
        } else if matches!(api, 0x0300_0012 | 0x0300_0013) {
            6
        } else {
            4
        }] != 0;
        let descriptor = request.descriptor;
        let offset = 8 + body.len() as u32;
        let length = descriptor.input_size.saturating_sub(offset) as usize;
        if length > 1024 * 1024 {
            return Ok((NO_MEMORY, 0));
        }
        let data = request.read_input(offset, length)?;
        if unicode && !data.len().is_multiple_of(2) {
            return Ok((INVALID_PARAMETER, 0));
        }
        let decode = |bytes: &[u8]| -> io::Result<Vec<u16>> {
            if unicode {
                if !bytes.len().is_multiple_of(2) {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                Ok(bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect())
            } else {
                encoding::decode(self.input_code_page, bytes)
            }
        };
        let capacity = descriptor.output_size as usize - body.len();
        let mut result = Vec::new();
        if matches!(api, 0x0300_0012 | 0x0300_0013) {
            let n = |i| u16::from_le_bytes([body[i], body[i + 1]]) as usize;
            let (source_len, target_len, exe_len) =
                (n(0), if api == 0x0300_0012 { n(2) } else { 0 }, n(4));
            if source_len + target_len + exe_len > data.len()
                || unicode && (source_len | target_len | exe_len) & 1 != 0
            {
                return Ok((INVALID_PARAMETER, 0));
            }
            let exe = decode(&data[..exe_len])?;
            let source = decode(&data[exe_len..exe_len + source_len])?;
            if api == 0x0300_0012 {
                let target =
                    decode(&data[exe_len + source_len..exe_len + source_len + target_len])?;
                return Ok((self.aliases.set(exe, source, target), 0));
            }
            body[2..4].fill(0);
            if capacity >= if unicode { 2 } else { 1 } {
                request.write_output(body.len() as u32, &[0, 0][..if unicode { 2 } else { 1 }])?;
            }
            let Some(target) = self.aliases.get(&exe, &source) else {
                return Ok((MISSING, 0));
            };
            result.extend_from_slice(target);
            result.push(0);
        } else {
            let exe = if matches!(api, 0x0300_0014 | 0x0300_0016) {
                Some(decode(&data)?)
            } else {
                None
            };
            result = self.aliases.list(exe.as_deref());
        }
        let bytes = if unicode {
            result.into_iter().flat_map(u16::to_le_bytes).collect()
        } else {
            encoding::encode(self.input_code_page, &result, usize::MAX)?.unwrap()
        };
        if matches!(api, 0x0300_0014 | 0x0300_0015) {
            body[..4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            return Ok((SUCCESS, 0));
        }
        if bytes.len() > capacity {
            if api == 0x0300_0013 {
                let reported = capacity * if unicode { 1 } else { 2 };
                let Ok(count) = u16::try_from(reported) else {
                    return Ok((INVALID_PARAMETER, 0));
                };
                body[2..4].copy_from_slice(&count.to_le_bytes());
                return Ok((SMALL, reported));
            }
            return Ok((LIST_SMALL, 0));
        }
        let count = bytes.len();
        request.write_output(body.len() as u32, &bytes)?;
        if api == 0x0300_0013 {
            let Ok(count) = u16::try_from(count) else {
                return Ok((0xc000_0095_u32 as i32, 0));
            };
            body[2..4].copy_from_slice(&count.to_le_bytes());
        } else {
            body[if api == 0x0300_0016 { 4 } else { 0 }..if api == 0x0300_0016 { 8 } else { 4 }]
                .copy_from_slice(&(count as u32).to_le_bytes());
        }
        Ok((SUCCESS, count))
    }
}
