# v0.1.20 — Windows text that reads as well as Windows Terminal

The owner runs MiniCon on Windows daily and reports the text "isn't
good-looking", while Windows Terminal on the same machine is comfortable. This
is four separate gaps, and the first one is probably most of the felt
difference.

## Confirmed on the owner's machine, 2026-09-21

```
minicon 0.1.19
  font           新宋体 (8x15 cells)
                 half/full width correct
```

`新宋体` is NSimSun. Item 1 below is not a hypothesis any more: it is the
main cause, and the order at the end of this document is revised for it.

Note what is already right: **half/full width correct**. The cell grid and the
CJK double-width relationship are sound. Whatever replaces the Latin face has
to keep that property — a prettier face that breaks the grid is a worse
terminal.

## 1. The face is not the one we think — CONFIRMED

`agenterm-platform`'s Windows raster adapter carries a coverage-ordered wish
list, and its own comment says `CreateFontW` never fails on a missing family:

```rust
const RASTER_FAMILIES: &[&str] = &[
    "NSimSun", "SimSun", "Sarasa Fixed SC", "Cascadia Mono", "Consolas", ...
];
```

`NSimSun`/`SimSun` are first and ship with stock Windows; `Cascadia Mono` is
fourth and **ships with nothing** (it arrives with the Windows Terminal
package). So Latin text on a stock machine is being drawn in a Song-style CJK
face whose Latin glyphs are thin and loosely fitted at terminal sizes.
Windows Terminal draws Cascadia Mono.

The list is ordered for *coverage*, which is the right goal for CJK and the
wrong goal for the Latin text that fills most of a terminal.

`select_primary` returns the first family whose full-width advance measures
exactly double its ASCII advance. NSimSun is index 0 and is a true dual-width
CJK face, so it wins on any Windows carrying East Asian fonts and Cascadia
Mono at index 3 is never reached. The rule is not wrong — a terminal grid does
need that relationship — it is just being asked to choose one face for two
jobs.

**Do:** select two faces instead of one.

- A **Latin face** that sets the cell: `Cascadia Mono` when present, else
  `Consolas`, which has shipped on every Windows since Vista and is the
  safe floor. The cell width comes from this face's ASCII advance.
- A **CJK face** for the wide ranges, kept as today's measured dual-width
  choice (NSimSun), rasterised so its full-width glyph spans exactly two
  cells.

**The hard part, stated rather than discovered later:** the two faces do not
agree on advance at the same pixel size. NSimSun's half-width advance at size
N is not Cascadia Mono's advance at size N, so the CJK face needs its own size
chosen such that its full-width advance is exactly `2 * cell_width`, or its
glyphs need scaling into the double cell. Solve that explicitly and assert it
— `--status` already reports `half/full width correct`, and that line must
keep saying so after the change. It is the regression test.

`minicon --status` already reports the resolved face via
`primary_face_report`; keep it reporting both faces once there are two.

## 2. GDI rasterisation, not DirectWrite

Today: `CreateFontW(..., ANTIALIASED_QUALITY)` plus
`GetGlyphOutlineW(GGO_GRAY8_BITMAP | GGO_GLYPH_INDEX)`, which returns 65
coverage levels that the adapter rescales to 0..255.

Windows Terminal uses DirectWrite through its AtlasEngine. Beyond the
rasteriser itself that brings subpixel glyph positioning and the modern
hinting/stem-darkening path; GDI's grayscale outline path predates all of it.

**Do:** evaluate `IDWriteFactory` + `IDWriteGlyphRunAnalysis` behind the same
`agenterm_platform::font::rasterize` contract, so the product side and its
glyph cache do not change. Keep GDI as the fallback for hosts where
DirectWrite is unavailable.

## 3. No gamma-correct blending — the most underrated of the four

Coverage from the rasteriser is being alpha-blended linearly. On a dark
background that makes stems read thin, grey and washed out, and it is the
usual reason a terminal "looks worse" than Windows Terminal with the *same*
font at the *same* size. DirectWrite blends through a text gamma ramp (1.8 by
default) with enhanced contrast applied first.

**Do:** apply gamma correction and a contrast boost to the coverage value
before blending. This is a small, self-contained change to the blend, testable
without changing the rasteriser, and worth doing **before** item 2 because it
is cheap and may carry most of the remaining difference.

## 4. Box-drawing and Powerline glyphs

Windows Terminal defaults `font.builtinGlyphs` to true and draws those ranges
procedurally so they tile the cell grid exactly, whatever the font does.
Leaving them to the font produces the familiar hairline gaps and overhangs in
borders.

**Do:** draw `U+2500`-`U+259F` and the Powerline private-use range from cell
geometry rather than from glyphs.

## Windows Terminal's configuration, for reference

`profiles.defaults`, documented defaults:

```jsonc
"font": { "face": "Cascadia Mono", "size": 12, "weight": "normal",
          "builtinGlyphs": true },
"antialiasingMode": "grayscale"   // not ClearType: subpixel AA fringes on dark
```

AtlasEngine has been the default renderer since 1.16. `font.features` and
`font.axes` expose OpenType features and variable-font axes (Cascadia is
variable, with a `wght` axis). Note the antialiasing default: grayscale, not
ClearType — the win is in the gamma and contrast handling, not in subpixel
rendering.

## Order

**1, then 3, then 4, then 2.**

Item 1 first: it is confirmed, it is most of the felt difference, and a
Song-style face is the wrong shape for Latin terminal text no matter how well
the rest of the pipeline is tuned. Item 3 next because it is cheap and is the
usual reason stems still read thin on a dark background once the face is
right. Item 2 is the largest change and should be judged after the other
three, against a measurement rather than an impression.

## Measure, do not eyeball

Before and after, capture the same text at the same size and compare
programmatically — `ui-snapshot` already makes layout machine-verifiable
without a GPU. A chain of one-shot visual A/Bs is not evidence; prove the
instrument repeats before comparing.
