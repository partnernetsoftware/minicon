//! DirectWrite's grayscale text blend, applied to a coverage mask.
//!
//! A rasteriser reports geometric coverage: how much of a pixel the glyph
//! outline covers. Blending that linearly in sRGB -- `bg*(1-a) + fg*a` -- makes
//! the partially covered pixels at a stroke's edge too faint, and on a dark
//! background light text reads thin, grey and washed out. It is the usual
//! reason one terminal looks worse than another with the same font at the
//! same size.
//!
//! DirectWrite corrects coverage before blending with an enhanced-contrast
//! curve and a gamma-1.8 alpha correction, both dependent on the text colour.
//! This is that correction, ported from Windows Terminal's AtlasEngine, which
//! reproduces DirectWrite's native grayscale blend in a pixel shader:
//! `src/renderer/atlas/dwrite_helpers.hlsl`, `DWrite_GrayscaleBlend`.
//! Copyright (c) Microsoft Corporation, MIT licence.
//!
//! The correction depends only on (coverage, foreground colour), so it is
//! computed once per colour as a 256-entry table and applied by lookup; the
//! blend kernel itself is unchanged.

use std::cell::RefCell;

/// `DWrite_GetGammaRatios` for gamma 1.8, DirectWrite's default.
const GAMMA_RATIOS: [f32; 4] = [0.148_054_42, -0.894_594_55, 1.475_908, -0.324_668_26];

/// `IDWriteRenderingParams1::GetGrayscaleEnhancedContrast` default.
const GRAYSCALE_ENHANCED_CONTRAST: f32 = 1.0;

/// Colours a frame draws text in are few -- a palette -- but a truecolour
/// program can emit many. Past this many the cache starts over rather than
/// grow without bound.
const MAX_CACHED_COLOURS: usize = 64;

/// The corrected coverage for every input coverage, for text in `foreground`
/// (`0x00RRGGBB`).
pub(crate) fn grayscale_table(foreground: u32) -> [u8; 256] {
    let red = ((foreground >> 16) & 0xff) as f32 / 255.0;
    let green = ((foreground >> 8) & 0xff) as f32 / 255.0;
    let blue = (foreground & 0xff) as f32 / 255.0;

    // DWrite_ApplyLightOnDarkContrastAdjustment: light text gets less of the
    // contrast boost, since it is the gamma correction below that thickens it.
    let lightness = 0.30 * red + 0.59 * green + 0.11 * blue;
    let contrast = GRAYSCALE_ENHANCED_CONTRAST * (3.0 - 4.0 * lightness).clamp(0.0, 1.0);
    // DWrite_CalcColorIntensity.
    let intensity = 0.25 * red + 0.5 * green + 0.25 * blue;
    let [g0, g1, g2, g3] = GAMMA_RATIOS;

    let mut table = [0_u8; 256];
    for (coverage, slot) in table.iter_mut().enumerate() {
        let alpha = coverage as f32 / 255.0;
        // DWrite_EnhanceContrast.
        let contrasted = alpha * (contrast + 1.0) / (alpha * contrast + 1.0);
        // DWrite_ApplyAlphaCorrection.
        let corrected = contrasted
            + contrasted
                * (1.0 - contrasted)
                * ((g0 * intensity + g1) * contrasted + (g2 * intensity + g3));
        *slot = (corrected.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    table
}

thread_local! {
    static TABLES: RefCell<Vec<(u32, [u8; 256])>> = const { RefCell::new(Vec::new()) };
}

/// `grayscale_table`, computed once per colour and reused.
pub(crate) fn with_table<R>(foreground: u32, use_table: impl FnOnce(&[u8; 256]) -> R) -> R {
    let foreground = foreground & 0x00ff_ffff;
    let cached = TABLES.try_with(|tables| {
        let mut tables = tables.borrow_mut();
        if let Some((_, table)) = tables.iter().find(|(colour, _)| *colour == foreground) {
            return *table;
        }
        if tables.len() >= MAX_CACHED_COLOURS {
            tables.clear();
        }
        let table = grayscale_table(foreground);
        tables.push((foreground, table));
        table
    });
    use_table(&cached.unwrap_or_else(|_| grayscale_table(foreground)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(table: &[u8; 256]) -> [u8; 6] {
        [0, 32, 64, 128, 192, 255].map(|i| table[i])
    }

    /// Pinned against an independent implementation of the same shader math
    /// (computed separately, not from this code), so a transcription error in
    /// the port cannot pass by agreeing with itself.
    #[test]
    fn the_table_matches_directwrite_grayscale_blend() {
        assert_eq!(
            samples(&grayscale_table(0x00ff_ffff)),
            [0, 62, 110, 178, 220, 255]
        );
        assert_eq!(
            samples(&grayscale_table(0x0000_0000)),
            [0, 34, 60, 118, 185, 255]
        );
        assert_eq!(
            samples(&grayscale_table(0x00c0_c0c0)),
            [0, 51, 92, 153, 201, 255]
        );
        assert_eq!(
            samples(&grayscale_table(0x0000_cd00)),
            [0, 61, 98, 154, 205, 255]
        );
    }

    /// The property the owner could see: light text on a dark background --
    /// a terminal's default -- was too thin. Half-covered edge pixels of white
    /// text must come out clearly heavier than linear coverage.
    #[test]
    fn light_text_edges_are_heavier_than_linear_coverage() {
        let white = grayscale_table(0x00ff_ffff);
        assert!(white[128] >= 170, "half coverage became {}", white[128]);
        for (coverage, corrected) in white.iter().enumerate() {
            assert!(
                usize::from(*corrected) >= coverage,
                "white text lost weight at coverage {coverage}"
            );
        }
    }

    /// Empty stays empty and full stays full for every colour, and more
    /// coverage never means less ink -- anything else would put halos or holes
    /// into glyphs.
    #[test]
    fn every_table_fixes_its_endpoints_and_never_decreases() {
        for colour in [
            0x0000_0000,
            0x00ff_ffff,
            0x00ff_0000,
            0x0000_00ff,
            0x0080_8080,
            0x00c0_c000,
        ] {
            let table = grayscale_table(colour);
            assert_eq!(table[0], 0, "{colour:06x}");
            assert_eq!(table[255], 255, "{colour:06x}");
            assert!(
                table.windows(2).all(|pair| pair[0] <= pair[1]),
                "{colour:06x} is not monotonic"
            );
        }
    }

    #[test]
    fn the_cache_returns_the_same_table_and_stays_bounded() {
        let direct = grayscale_table(0x0012_3456);
        assert_eq!(with_table(0x0012_3456, |table| *table), direct);
        // The top byte is not part of the colour.
        assert_eq!(with_table(0xff12_3456, |table| *table), direct);
        for colour in 0..(MAX_CACHED_COLOURS as u32 * 3) {
            with_table(colour, |_| ());
        }
        TABLES.with(|tables| assert!(tables.borrow().len() <= MAX_CACHED_COLOURS));
    }
}
