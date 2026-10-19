//! Native single- and double-byte cell conversion shared by block operations.
use super::{ConsoleHost, NOT_SUPPORTED};
use windows_sys::Win32::Globalization::{
    CPINFO, GetCPInfo, MultiByteToWideChar, WideCharToMultiByte,
};
impl ConsoleHost<'_> {
    pub(super) fn is_dbcs_output(&self) -> bool {
        let mut info = CPINFO::default();
        // SAFETY: numeric code-page query and initialized output record.
        unsafe { GetCPInfo(self.output_code_page, &mut info) != 0 && info.MaxCharSize == 2 }
    }

    pub(super) fn decode_double_byte_cell(&self, bytes: [u8; 2]) -> Result<char, i32> {
        let mut unit = 0;
        // SAFETY: exact initialized two-byte input and one UTF-16 output unit.
        if !self.is_dbcs_output()
            || unsafe {
                MultiByteToWideChar(self.output_code_page, 8, bytes.as_ptr(), 2, &mut unit, 1)
            } != 1
        {
            return Err(NOT_SUPPORTED);
        }
        char::from_u32(u32::from(unit)).ok_or(NOT_SUPPORTED)
    }

    pub(super) fn encode_double_byte_cell(&self, unit: u16) -> Result<[u8; 2], i32> {
        let mut bytes = [0; 2];
        // SAFETY: exact initialized UTF-16 input and two-byte output capacity.
        if !self.is_dbcs_output()
            || unsafe {
                WideCharToMultiByte(
                    self.output_code_page,
                    0,
                    &unit,
                    1,
                    bytes.as_mut_ptr(),
                    2,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            } != 2
        {
            return Err(NOT_SUPPORTED);
        }
        Ok(bytes)
    }

    pub(super) fn decode_single_byte_cell(&self, byte: u8) -> Result<char, i32> {
        if byte.is_ascii() {
            return Ok(char::from(byte));
        }
        let mut info = CPINFO::default();
        let mut unit = 0;
        // SAFETY: numeric CP query and one initialized input/output unit.
        if unsafe { GetCPInfo(self.output_code_page, &mut info) } == 0
            || info.MaxCharSize != 1
            || unsafe { MultiByteToWideChar(self.output_code_page, 0, &byte, 1, &mut unit, 1) } != 1
        {
            return Err(NOT_SUPPORTED);
        }
        char::from_u32(u32::from(unit)).ok_or(NOT_SUPPORTED)
    }

    pub(super) fn encode_single_byte_cell(&self, unit: u16) -> Result<u8, i32> {
        let mut byte = 0;
        // SAFETY: one initialized UTF-16 unit, one-byte target and no fallback pointers.
        if unsafe {
            WideCharToMultiByte(
                self.output_code_page,
                0,
                &unit,
                1,
                &mut byte,
                1,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        } != 1
        {
            return Err(NOT_SUPPORTED);
        }
        Ok(byte)
    }
}
