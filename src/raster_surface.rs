//! Bounded clipped XRGB raster target.
//!
//! Product paint policy stays in the caller; this leaf owns only pixel bounds,
//! rectangle fill, and glyph-mask blending.

use agenterm_ui_core::PixelRect;

use crate::font;
use crate::palette::Rgb;

#[derive(Clone, Copy)]
pub(super) struct CellRect {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
}

/// The pixel target for one frame: the buffer and its dimensions, which always
/// travel together. Bundling them keeps the drawing calls readable — the free
/// functions this replaced took nine positional arguments, most of them the
/// same three values threaded through every call.
pub(super) struct Surface<'a> {
    pub(super) pixels: &'a mut [u32],
    pub(super) width: u32,
    pub(super) height: u32,
    clip: PixelRect,
}

impl<'a> Surface<'a> {
    #[cfg(test)]
    pub(super) fn new(pixels: &'a mut [u32], width: u32, height: u32) -> Surface<'a> {
        Self::with_clip(pixels, width, height, PixelRect::full_frame(width, height))
    }

    pub(super) fn with_clip(
        pixels: &'a mut [u32],
        width: u32,
        height: u32,
        clip: PixelRect,
    ) -> Surface<'a> {
        Self {
            pixels,
            width,
            height,
            clip: clip.clip(width, height),
        }
    }

    fn clipped_rect(&self, x: u32, y: u32, w: u32, h: u32) -> PixelRect {
        let rect = PixelRect::from_xywh(x, y, w, h).clip(self.width, self.height);
        let left = rect.left.max(self.clip.left);
        let top = rect.top.max(self.clip.top);
        let right = rect.right.min(self.clip.right).max(left);
        let bottom = rect.bottom.min(self.clip.bottom).max(top);
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }

    pub(super) fn intersects_rect(&self, x: u32, y: u32, w: u32, h: u32) -> bool {
        !self.clipped_rect(x, y, w, h).is_empty()
    }

    pub(super) fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let rect = self.clipped_rect(x, y, w, h);
        if rect.is_empty() {
            return;
        }
        agenterm_ui_core::pixel::fill_xrgb_rect(
            self.pixels,
            self.width,
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            color,
        );
    }

    /// Alpha-blends `color` over the existing pixels in the rect (0.0 = keep,
    /// 1.0 = replace). Used for translucent overlays like the grid crosshair,
    /// which must sit over terminal content without erasing it.
    pub(super) fn blend_rect(
        &mut self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        color: crate::palette::Rgb,
        alpha: f32,
    ) {
        let rect = self.clipped_rect(x, y, w, h);
        if rect.is_empty() {
            return;
        }
        let a = alpha.clamp(0.0, 1.0);
        let inv = 1.0 - a;
        let (cr, cg, cb) = (f32::from(color.0), f32::from(color.1), f32::from(color.2));
        for row in rect.top..rect.bottom {
            let base = (row * self.width) as usize;
            for col in rect.left..rect.right {
                let idx = base + col as usize;
                let px = self.pixels[idx];
                let dr = f32::from(((px >> 16) & 0xff) as u8);
                let dg = f32::from(((px >> 8) & 0xff) as u8);
                let db = f32::from((px & 0xff) as u8);
                let nr = (dr * inv + cr * a).round().clamp(0.0, 255.0) as u32;
                let ng = (dg * inv + cg * a).round().clamp(0.0, 255.0) as u32;
                let nb = (db * inv + cb * a).round().clamp(0.0, 255.0) as u32;
                self.pixels[idx] = (nr << 16) | (ng << 8) | nb;
            }
        }
    }

    /// Blits a rasterized glyph into a cell, clipped to that cell.
    ///
    /// `shear` slants the glyph for faux italic: a per-row horizontal offset
    /// proportional to height above the baseline. Synthesizing the slant beats
    /// loading a real italic face, which would have a different advance width
    /// and break the fixed cell grid.
    pub(super) fn blit_glyph(
        &mut self,
        glyph: &font::RasterGlyph,
        cell: CellRect,
        fg: Rgb,
        shear: f32,
    ) {
        let clip = self.clipped_rect(cell.x, cell.y, cell.w, cell.h);
        if clip.is_empty() {
            return;
        }
        let start_x = i64::from(cell.x) + i64::from(glyph.offset_x);
        let start_y = i64::from(cell.y) + i64::from(glyph.offset_y);
        let clip_x0 = i64::from(clip.left);
        let clip_y0 = i64::from(clip.top);
        let clip_x1 = i64::from(clip.right);
        let clip_y1 = i64::from(clip.bottom);

        for gy in 0..glyph.height {
            let py = start_y + i64::from(gy);
            if py < clip_y0 || py >= clip_y1 || py < 0 || py >= i64::from(self.height) {
                continue;
            }
            // Rows nearer the top lean further right, pivoting on the bottom
            // of the cell so the glyph stays seated on its baseline.
            let slant = if shear == 0.0 {
                0
            } else {
                minicon_core::numeric::round_f32((clip_y1 - py) as f32 * shear) as i64
            };
            let row_start_x = start_x + slant;
            let source_x_start = (clip_x0 - row_start_x).max(0).min(i64::from(u32::MAX)) as u32;
            let source_x_end = glyph
                .width
                .min((clip_x1 - row_start_x).max(0).min(i64::from(u32::MAX)) as u32);
            if source_x_start >= source_x_end {
                continue;
            }
            let destination_x = row_start_x + i64::from(source_x_start);
            if destination_x < 0 || destination_x >= i64::from(self.width) {
                continue;
            }
            let count = usize::try_from(source_x_end - source_x_start).unwrap_or(0);
            let Some(row_start) = usize::try_from(py)
                .ok()
                .and_then(|row| row.checked_mul(self.width as usize))
            else {
                continue;
            };
            let Some(destination_start) = row_start.checked_add(destination_x as usize) else {
                continue;
            };
            let Some(destination_end) = destination_start.checked_add(count) else {
                continue;
            };
            let Some(source_start) = usize::try_from(gy)
                .ok()
                .and_then(|row| row.checked_mul(glyph.width as usize))
                .and_then(|row| row.checked_add(source_x_start as usize))
            else {
                continue;
            };
            let Some(source_end) = source_start.checked_add(count) else {
                continue;
            };
            let Some(destination) = self.pixels.get_mut(destination_start..destination_end) else {
                continue;
            };
            let Some(source) = glyph.alpha.get(source_start..source_end) else {
                continue;
            };
            agenterm_ui_core::pixel::blend_mask_xrgb(destination, source, fg.to_xrgb());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make<'a>(pixels: &'a mut [u32], w: u32, h: u32, clip: PixelRect) -> Surface<'a> {
        Surface::with_clip(pixels, w, h, clip)
    }

    /// `with_clip` must intersect the requested clip with the frame, so a clip
    /// larger than the buffer cannot let a fill run past it.
    #[test]
    fn the_surface_clip_is_intersected_with_the_frame() {
        let mut pixels = vec![0u32; 4 * 4];
        let grew = make(&mut pixels, 4, 4, PixelRect::from_xywh(0, 0, 100, 100));
        assert_eq!(grew.clip, PixelRect::full_frame(4, 4));

        let mut pixels = vec![0u32; 4 * 4];
        let partial = make(&mut pixels, 4, 4, PixelRect::from_xywh(1, 1, 2, 2));
        assert_eq!(partial.clip, PixelRect::from_xywh(1, 1, 2, 2));
    }

    /// `clipped_rect` intersects the request with the frame and the clip, and
    /// collapses an out-of-clip request to an empty rect (`right <= left`)
    /// rather than a malformed one with `right < left`.
    #[test]
    fn clipped_rect_intersects_and_never_inverts() {
        let mut pixels = vec![0u32; 8 * 8];
        let surface = make(&mut pixels, 8, 8, PixelRect::from_xywh(2, 2, 4, 4));

        // Inside the clip: unchanged.
        assert_eq!(
            surface.clipped_rect(3, 3, 2, 2),
            PixelRect::from_xywh(3, 3, 2, 2)
        );
        // Straddling the clip's left edge: trimmed to the clip.
        assert_eq!(
            surface.clipped_rect(0, 0, 4, 4),
            PixelRect::from_xywh(2, 2, 2, 2)
        );
        // Entirely left of the clip: empty, and still a valid half-open rect.
        let outside = surface.clipped_rect(0, 0, 1, 1);
        assert!(
            outside.is_empty(),
            "an out-of-clip rect must be empty: {outside:?}"
        );
        assert!(outside.right >= outside.left && outside.bottom >= outside.top);
    }

    #[test]
    fn intersects_rect_reports_only_overlap() {
        let mut pixels = vec![0u32; 8 * 8];
        let surface = make(&mut pixels, 8, 8, PixelRect::from_xywh(4, 4, 4, 4));
        assert!(surface.intersects_rect(4, 4, 1, 1));
        assert!(surface.intersects_rect(3, 3, 4, 4), "an overlap counts");
        assert!(
            !surface.intersects_rect(0, 0, 4, 4),
            "touching is not overlapping"
        );
        assert!(
            !surface.intersects_rect(20, 20, 1, 1),
            "past the frame is empty"
        );
        assert!(!surface.intersects_rect(0, 0, 0, 0), "a zero rect is empty");
    }

    /// A fill outside the clip must write nothing; a fill straddling the clip
    /// must touch only the clipped pixels, leaving the rest as the sentinel.
    #[test]
    fn fill_rect_writes_only_inside_the_clip() {
        const SENTINEL: u32 = 0x00AB_CDEF;
        const INK: u32 = 0x0011_2233;

        let mut pixels = vec![SENTINEL; 4 * 4];
        let mut surface = make(&mut pixels, 4, 4, PixelRect::from_xywh(1, 1, 2, 2));
        surface.fill_rect(0, 0, 4, 4, INK);
        // The fill request covers the frame, but the clip is 2x2 at (1,1), so
        // exactly those four pixels are inked and the rest keep the sentinel.
        for y in 0..4 {
            for x in 0..4 {
                let inside = (1..3).contains(&x) && (1..3).contains(&y);
                assert_eq!(
                    pixels[y * 4 + x],
                    if inside { INK } else { SENTINEL },
                    "pixel ({x},{y}) is {} the clip",
                    if inside { "inside" } else { "outside" }
                );
            }
        }

        let mut pixels = vec![SENTINEL; 4 * 4];
        let mut surface = make(&mut pixels, 4, 4, PixelRect::from_xywh(1, 1, 2, 2));
        surface.fill_rect(0, 0, 1, 1, INK);
        assert_eq!(
            pixels,
            vec![SENTINEL; 16],
            "a fill outside the clip writes nothing"
        );

        let mut pixels = vec![SENTINEL; 4 * 4];
        let mut surface = make(&mut pixels, 4, 4, PixelRect::from_xywh(1, 1, 2, 2));
        surface.fill_rect(1, 1, 1, 1, INK);
        // Only the top-left pixel of the 2x2 clip is inked.
        assert_eq!(pixels[0], SENTINEL);
        assert_eq!(pixels[4 + 1], INK);
        assert_eq!(
            pixels.iter().filter(|pixel| **pixel == INK).count(),
            1,
            "exactly one pixel must be inked"
        );
    }

    /// A full-alpha glyph: every pixel of the mask is opaque, so a blit marks
    /// exactly the mask cells that land inside the clip.
    fn solid_glyph(width: u32, height: u32) -> font::RasterGlyph {
        font::RasterGlyph {
            alpha: vec![255; (width * height) as usize],
            width,
            height,
            offset_x: 0,
            offset_y: 0,
        }
    }

    fn cell(x: u32, y: u32, w: u32, h: u32) -> CellRect {
        CellRect { x, y, w, h }
    }

    /// A glyph smaller than its cell inks exactly its own pixels at the cell
    /// origin; nothing outside the mask is touched.
    #[test]
    fn blit_glyph_inks_the_mask_within_the_cell() {
        const SENTINEL: u32 = 0x0000_0001;
        let fg = Rgb(0xFF, 0xFF, 0xFF);
        let mut pixels = vec![SENTINEL; 8 * 8];
        let mut surface = make(&mut pixels, 8, 8, PixelRect::full_frame(8, 8));
        let glyph = solid_glyph(2, 2);
        surface.blit_glyph(&glyph, cell(3, 4, 4, 4), fg, 0.0);

        // The 2x2 mask sits at (3,4)..(5,6).
        for y in 0..8 {
            for x in 0..8 {
                let inked = (3..5).contains(&x) && (4..6).contains(&y);
                assert_eq!(
                    pixels[y * 8 + x] != SENTINEL,
                    inked,
                    "pixel ({x},{y}) inked should be {inked}"
                );
            }
        }
    }

    /// The blit is clipped to the cell, so a glyph wider than its cell must not
    /// spill into the neighbouring cell's columns even though the mask covers
    /// them.
    #[test]
    fn blit_glyph_never_spills_past_its_cell() {
        const SENTINEL: u32 = 0x0000_0001;
        let fg = Rgb(0xFF, 0xFF, 0xFF);
        let mut pixels = vec![SENTINEL; 8 * 8];
        let mut surface = make(&mut pixels, 8, 8, PixelRect::full_frame(8, 8));
        // A 6-wide glyph in a 2-wide cell at (2,2).
        let glyph = solid_glyph(6, 1);
        surface.blit_glyph(&glyph, cell(2, 2, 2, 4), fg, 0.0);

        for x in 0..8 {
            let inked = pixels[2 * 8 + x] != SENTINEL;
            let should = (2..4).contains(&x);
            assert_eq!(inked, should, "column {x} inked should be {should}");
        }
    }

    /// Shear slants the glyph: rows nearer the top shift further right,
    /// pivoting on the bottom of the cell. A single-column mask becomes a
    /// diagonal staircase.
    #[test]
    fn blit_glyph_shears_rows_further_right_toward_the_top() {
        const SENTINEL: u32 = 0x0000_0001;
        let fg = Rgb(0xFF, 0xFF, 0xFF);
        // Wide enough that the slant stays inside the surface.
        let mut pixels = vec![SENTINEL; 16 * 8];
        let mut surface = make(&mut pixels, 16, 8, PixelRect::full_frame(16, 8));
        // A 1x4 vertical bar in a 4-cell-wide, 4-tall cell at (2,2).
        let glyph = solid_glyph(1, 4);
        surface.blit_glyph(&glyph, cell(2, 2, 10, 4), fg, 0.5);

        // The bottom row (py = clip_y1 - 1) has slant 0; each row up gains 0.5
        // rounded, so the column shifts right as y decreases.
        let column_at = |y: usize| (0..16).find(|x| pixels[y * 16 + *x] != SENTINEL);
        let bottom = column_at(5).expect("bottom row inked");
        let top = column_at(2).expect("top row inked");
        assert!(
            top > bottom,
            "the top row must lean right of the bottom: top={top:?} bottom={bottom:?}"
        );
    }

    /// A cell entirely outside the clip inks nothing, so a glyph on a hidden
    /// cell cannot reach the visible ones.
    #[test]
    fn blit_glyph_outside_the_clip_inks_nothing() {
        let fg = Rgb(0xFF, 0xFF, 0xFF);
        let mut pixels = vec![0u32; 8 * 8];
        let mut surface = make(&mut pixels, 8, 8, PixelRect::from_xywh(4, 4, 4, 4));
        surface.blit_glyph(&solid_glyph(2, 2), cell(0, 0, 2, 2), fg, 0.0);
        assert!(
            pixels.iter().all(|pixel| *pixel == 0),
            "a glyph on a cell outside the clip must write nothing"
        );
    }
}
