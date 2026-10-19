//! Read-only virtual font metadata for a headless cell transport.
//! This identity describes no installed face or frontend pixel allocation.

pub(super) fn size_bytes() -> [u8; 4] {
    let mut bytes = [0; 4];
    bytes[..2].copy_from_slice(&8_i16.to_le_bytes());
    bytes[2..].copy_from_slice(&16_i16.to_le_bytes());
    bytes
}

/// FontIndex through FaceName; MaximumWindow and its alignment stay in caller.
pub(super) fn current_bytes() -> [u8; 80] {
    let mut bytes = [0; 80];
    bytes[4..8].copy_from_slice(&size_bytes());
    bytes[8..12].copy_from_slice(&0x36_u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&400_u32.to_le_bytes());
    for (slot, unit) in bytes[16..]
        .chunks_exact_mut(2)
        .zip("MiniCon Virtual".encode_utf16())
    {
        slot.copy_from_slice(&unit.to_le_bytes());
    }
    bytes
}
