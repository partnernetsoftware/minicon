//! Incremental UTF-16 mechanism shared by independent terminal producers.

/// Incremental decoder for one terminal text producer. Independent producers
/// must own independent decoders. An unmatched high surrogate is retained
/// until the next unit or explicit discard; invalid sequences yield U+FFFD.
#[derive(Default)]
pub struct Utf16Decoder {
    high: Option<u16>,
}

impl Utf16Decoder {
    /// Decode one unit into zero, one or two owned scalars. The returned
    /// iterator does not borrow the decoder, allowing immediate publication.
    pub fn push(&mut self, unit: u16) -> impl Iterator<Item = char> + use<> {
        let mut decoded = [None; 2];
        if let Some(high) = self.high.take() {
            if (0xdc00..=0xdfff).contains(&unit) {
                let value = 0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00;
                decoded[0] = char::from_u32(value);
                return decoded.into_iter().flatten();
            }
            decoded[0] = Some(char::REPLACEMENT_CHARACTER);
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.high = Some(unit);
        } else {
            decoded[1] =
                Some(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        decoded.into_iter().flatten()
    }

    /// Discard an unpaired high surrogate, returning whether one was present.
    /// A line editor can delete an undisplayed unit without erasing a cell.
    pub fn discard_pending(&mut self) -> bool {
        self.high.take().is_some()
    }
}
