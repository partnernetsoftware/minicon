# v0.1.20 — Windows text that reads as well as Windows Terminal

The owner runs MiniCon on Windows daily and reports the text "isn't
good-looking", while Windows Terminal on the same machine is comfortable. This
is four separate gaps, and the first one is probably most of the felt
difference.

## 1. The face is very likely not the one we think

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

**Do:** split selection into a primary Latin face and a CJK fallback rather
than one coverage-ordered list. Prefer, in order, `Cascadia Mono`, `Consolas`
(on every Windows since Vista), then the CJK faces for the ranges they cover.
Record which family was actually resolved so a receipt can show it — today
"which one is used is decided by measuring" and nothing reports the answer.

**Verify first:** read the resolved family on the owner's machine before
changing anything. If it already says Cascadia Mono, this item is void and
items 2-3 carry the whole difference.

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

3, then 1, then 4, then 2. Item 3 is the cheapest and may be most of it; item
2 is the largest change and should be judged after the other three, against a
measurement rather than an impression.

## Measure, do not eyeball

Before and after, capture the same text at the same size and compare
programmatically — `ui-snapshot` already makes layout machine-verifiable
without a GPU. A chain of one-shot visual A/Bs is not evidence; prove the
instrument repeats before comparing.
