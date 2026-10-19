//! Bounded legacy readback views of the existing shared VT screen.
use super::{ConsoleHost, INVALID_PARAMETER, NOT_SUPPORTED};
use windows_sys::Win32::Globalization::WideCharToMultiByte;

impl ConsoleHost<'_> {
    pub(super) fn read_output_string(
        &self,
        kind: u32,
        column: u16,
        row: u16,
        capacity: usize,
    ) -> Result<Vec<u8>, i32> {
        if !(1..=4).contains(&kind) {
            return Err(INVALID_PARAMETER);
        }
        // Bound both the wire result and the UTF-16 intermediate before
        // consulting client sizes. ANSI conversion never allocates an
        // oversized expansion just to discard it afterwards.
        if capacity > 1024 * 1024 {
            return Err(NOT_SUPPORTED);
        }
        let amount = if kind == 1 { capacity } else { capacity / 2 };
        let screen = self.screen.screen();
        let (rows, columns) = screen.size();
        if row >= rows || column >= columns || amount == 0 {
            return Ok(Vec::new());
        }
        let start = usize::from(row) * usize::from(columns) + usize::from(column);
        let count = amount.min(usize::from(rows) * usize::from(columns) - start);
        let mut units = Vec::with_capacity(count);
        for offset in 0..count {
            let position = start + offset;
            let cell = screen
                .cell(
                    (position / usize::from(columns)) as u16,
                    (position % usize::from(columns)) as u16,
                )
                .unwrap();
            let clipped = (offset == 0 && cell.is_wide_continuation())
                || (offset == amount - 1 && cell.is_wide());
            if kind == 3 {
                let mut attribute = super::output::legacy_attribute(
                    cell.fgcolor(),
                    cell.bgcolor(),
                    cell.bold(),
                    cell.inverse(),
                    cell.underline(),
                );
                if !clipped {
                    attribute |= if cell.is_wide() {
                        0x100
                    } else if cell.is_wide_continuation() {
                        0x200
                    } else {
                        0
                    };
                }
                units.push(attribute);
            } else if clipped || !cell.has_contents() && !cell.is_wide_continuation() {
                units.push(u16::from(b' '));
            } else if !cell.is_wide_continuation() {
                // The pinned modern console's legacy API returns a single
                // UTF-16 unit per leading cell. Complex graphemes (including
                // supplementary scalars) use replacement, without changing
                // the full text retained by the shared screen.
                let mut characters = cell.contents().encode_utf16();
                let first = characters.next().unwrap_or(u16::from(b' '));
                units.push(if characters.next().is_some() {
                    0xfffd
                } else {
                    first
                });
            }
        }
        if kind != 1 {
            return Ok(units.into_iter().flat_map(u16::to_le_bytes).collect());
        }
        if units.is_empty() {
            return Ok(Vec::new());
        }
        // SAFETY: initialized UTF-16 input bounded above; query requires no
        // output buffer. Counts fit i32 under the one-MiB transfer limit.
        let size = unsafe {
            WideCharToMultiByte(
                self.output_code_page,
                0,
                units.as_ptr(),
                units.len() as i32,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if size == 0 {
            return Err(INVALID_PARAMETER);
        }
        // Native compatibility: an expanded string that does not fit is an
        // empty successful read, rather than a split encoded character.
        if size as usize > capacity {
            return Ok(Vec::new());
        }
        let mut bytes = vec![0; size as usize];
        // SAFETY: same live input, exact queried capacity and disjoint output.
        if unsafe {
            WideCharToMultiByte(
                self.output_code_page,
                0,
                units.as_ptr(),
                units.len() as i32,
                bytes.as_mut_ptr(),
                size,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        } != size
        {
            return Err(INVALID_PARAMETER);
        }
        Ok(bytes)
    }
}
