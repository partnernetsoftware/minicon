use std::{path::Path, sync::OnceLock};

use ab_glyph::{Font, FontRef, ScaleFont};

use crate::contract::font::{
    FontDiscovery, FontError, FontFileCandidate, FontMetrics, FontRequest, OpaqueWindowHandle,
    RasterGlyph,
};

fn intern_string(value: &str) -> &'static str {
    Box::leak(value.to_owned().into_boxed_str())
}

fn path_to_candidate(name: &str, path: &Path) -> Option<FontFileCandidate> {
    if !path.is_file() {
        return None;
    }
    let components: Vec<&'static str> = path
        .iter()
        .filter_map(|component| component.to_str().map(intern_string))
        .collect();
    if components.is_empty() {
        return None;
    }
    let components = Box::leak(components.into_boxed_slice());
    Some(FontFileCandidate {
        name: intern_string(name),
        components,
    })
}

/// Queries fontconfig for the system monospace face (`fc-match monospace`).
///
/// Returns `None` when `fc-match` is missing, exits non-zero, or resolves to a
/// path that is not a readable font file.
fn fontconfig_monospace() -> Option<FontFileCandidate> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{family}\n%{file}\n", "monospace"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let family = lines.next()?.trim();
    let file = lines.next()?.trim();
    if family.is_empty() || file.is_empty() {
        return None;
    }
    path_to_candidate(family, Path::new(file))
}

/// Hard-coded Debian/Ubuntu-style paths used when fontconfig is unavailable or
/// does not resolve to a readable file.
const HARDCODED_CANDIDATES: &[FontFileCandidate] = &[
    FontFileCandidate {
        name: "DejaVu Sans Mono",
        components: &[
            "usr",
            "share",
            "fonts",
            "truetype",
            "dejavu",
            "DejaVuSansMono.ttf",
        ],
    },
    FontFileCandidate {
        name: "Liberation Mono",
        components: &[
            "usr",
            "share",
            "fonts",
            "truetype",
            "liberation",
            "LiberationMono-Regular.ttf",
        ],
    },
    FontFileCandidate {
        name: "Liberation Mono",
        components: &[
            "usr",
            "share",
            "fonts",
            "truetype",
            "liberation2",
            "LiberationMono-Regular.ttf",
        ],
    },
    FontFileCandidate {
        name: "Noto Sans Mono",
        components: &[
            "usr",
            "share",
            "fonts",
            "truetype",
            "noto",
            "NotoSansMono-Regular.ttf",
        ],
    },
    FontFileCandidate {
        name: "Noto Sans Mono CJK",
        components: &[
            "usr",
            "share",
            "fonts",
            "opentype",
            "noto",
            "NotoSansCJK-Regular.ttc",
        ],
    },
];

fn dedupe_candidates(candidates: Vec<FontFileCandidate>) -> Vec<FontFileCandidate> {
    let mut unique = Vec::new();
    for candidate in candidates {
        let path = candidate.absolute_path();
        if unique
            .iter()
            .any(|existing: &FontFileCandidate| existing.absolute_path() == path)
        {
            continue;
        }
        unique.push(candidate);
    }
    unique
}

fn resolved_candidates() -> &'static [FontFileCandidate] {
    static CANDIDATES: OnceLock<&'static [FontFileCandidate]> = OnceLock::new();
    CANDIDATES.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Some(fontconfig) = fontconfig_monospace() {
            candidates.push(fontconfig);
        }
        candidates.extend(HARDCODED_CANDIDATES.iter().copied());
        Box::leak(dedupe_candidates(candidates).into_boxed_slice())
    })
}

/// Font files probed in order, by absolute path.
///
/// The first entry is resolved through fontconfig (`fc-match monospace`) when
/// available; the remainder follow the Debian/Ubuntu layout for hosts without
/// fontconfig or with non-standard install paths.
pub(crate) fn candidates() -> &'static [FontFileCandidate] {
    resolved_candidates()
}

/// Fonts consulted only for glyphs the primary face does not have, so CJK and
/// emoji do not render as blank cells. Never chosen as the primary face.
///
/// Noto Sans CJK is already a primary candidate above, but only as a last
/// resort — on a system that found DejaVu first it is never reached, which is
/// exactly the gap this list closes.
pub(crate) fn fallback_candidates() -> &'static [FontFileCandidate] {
    &[
        FontFileCandidate {
            name: "Noto Sans CJK",
            components: &[
                "usr",
                "share",
                "fonts",
                "opentype",
                "noto",
                "NotoSansCJK-Regular.ttc",
            ],
        },
        FontFileCandidate {
            name: "WenQuanYi Micro Hei",
            components: &[
                "usr",
                "share",
                "fonts",
                "truetype",
                "wqy",
                "wqy-microhei.ttc",
            ],
        },
        FontFileCandidate {
            name: "Noto Color Emoji",
            components: &[
                "usr",
                "share",
                "fonts",
                "truetype",
                "noto",
                "NotoColorEmoji.ttf",
            ],
        },
    ]
}

pub(crate) fn probe() -> FontDiscovery {
    let mut available_families = Vec::new();
    for candidate in candidates() {
        if candidate.exists() && !available_families.contains(&candidate.name) {
            available_families.push(candidate.name);
        }
    }
    FontDiscovery {
        primary_family: available_families.first().copied(),
        available_families,
    }
}

pub(crate) fn primary_family_name() -> Result<&'static str, FontError> {
    probe().primary_family.ok_or(FontError::Unavailable)
}

pub(crate) fn primary_metrics(size_px: u16) -> Result<FontMetrics, FontError> {
    let candidate = candidates()
        .iter()
        .copied()
        .find(|candidate| candidate.exists())
        .ok_or(FontError::Unavailable)?;
    let data = std::fs::read(candidate.absolute_path()).map_err(|_| FontError::MetricsFailed)?;
    let font = FontRef::try_from_slice(&data).map_err(|_| FontError::MetricsFailed)?;
    let size_px = size_px.clamp(8, 72);
    let scaled = font.as_scaled(f32::from(size_px));
    let ascent = scaled.ascent();
    let cell_width = scaled.h_advance(scaled.glyph_id('M'));
    let cell_height = scaled.height();
    if ![ascent, cell_width, cell_height]
        .into_iter()
        .all(|metric| metric.is_finite() && metric > 0.0)
    {
        return Err(FontError::MetricsFailed);
    }
    Ok(FontMetrics {
        family: Some(candidate.name),
        size_px,
        cell_width,
        cell_height,
        ascent,
    })
}

/// Face report with half/full-width advances so `--status` can say whether the
/// grid's double-width invariant holds, matching the Windows report shape.
///
/// ASCII advance comes from the primary mono face. Full-width advance prefers
/// a glyph the primary actually covers; when the primary is Latin-only (e.g.
/// DejaVu Sans Mono maps 中 onto a single-cell `.notdef`/fallback), measure
/// the probe on the first CJK fallback face so the report reflects the grid
/// the rasterizer will paint, not a missing-glyph stub.
pub(crate) fn primary_face_report(
    size_px: u16,
) -> Result<crate::font::PrimaryFaceReport, FontError> {
    let candidate = candidates()
        .iter()
        .copied()
        .find(|candidate| candidate.exists())
        .ok_or(FontError::Unavailable)?;
    let data = std::fs::read(candidate.absolute_path()).map_err(|_| FontError::MetricsFailed)?;
    let font = FontRef::try_from_slice_and_index(&data, 0).map_err(|_| FontError::MetricsFailed)?;
    let size_px = size_px.clamp(8, 72);
    let scaled = font.as_scaled(f32::from(size_px));
    let ascii = scaled.h_advance(scaled.glyph_id('M'));
    let cell_height = scaled.height();
    if ![ascii, cell_height]
        .into_iter()
        .all(|metric| metric.is_finite() && metric > 0.0)
    {
        return Err(FontError::MetricsFailed);
    }
    let ascii_u = crate::numeric::ceil_f32(ascii).max(1.0) as u32;
    let full_u = full_width_advance_px(&data, size_px, ascii).or_else(|| {
        fallback_candidates().iter().copied().find_map(|fallback| {
            if !fallback.exists() {
                return None;
            }
            let bytes = std::fs::read(fallback.absolute_path()).ok()?;
            full_width_advance_px(&bytes, size_px, ascii)
        })
    });
    Ok(crate::font::PrimaryFaceReport {
        face: candidate.name.to_owned(),
        cell_width: ascii_u,
        cell_height: crate::numeric::ceil_f32(cell_height).max(1.0) as u32,
        ascii_advance: Some(ascii_u),
        full_width_advance: full_u,
    })
}

fn full_width_advance_px(data: &[u8], size_px: u16, ascii: f32) -> Option<u32> {
    const FULL_WIDTH_PROBE: char = '\u{4e2d}';
    // Noto CJK is a .ttc. Index 0 is not always the face that covers 中, and
    // try_from_slice rejects collections outright.
    for index in 0..32u32 {
        let Ok(font) = FontRef::try_from_slice_and_index(data, index) else {
            break;
        };
        let scaled = font.as_scaled(f32::from(size_px));
        let glyph = scaled.glyph_id(FULL_WIDTH_PROBE);
        // .notdef (id 0) or a Latin mono stub at ~1 cell is not a real CJK advance.
        if glyph.0 == 0 {
            continue;
        }
        let full = scaled.h_advance(glyph);
        if full.is_finite() && full > ascii * 1.25 {
            return Some(crate::numeric::ceil_f32(full).max(1.0) as u32);
        }
    }
    None
}

pub(crate) fn probe_capability() -> Result<(), FontError> {
    primary_metrics(14).map(|_| ())
}

pub(crate) fn rasterizer_name() -> Result<String, FontError> {
    crate::selected::portable_font_raster::rasterizer_name(candidates, fallback_candidates)
}

pub(crate) fn rasterize(ch: char, size_px: u16) -> Result<Option<RasterGlyph>, FontError> {
    crate::selected::portable_font_raster::rasterize(candidates, fallback_candidates, ch, size_px)
}

pub(crate) fn create_terminal_font(
    _window: OpaqueWindowHandle,
    _request: FontRequest<'_>,
) -> Result<(isize, FontMetrics), FontError> {
    Err(FontError::Unsupported)
}

pub(crate) fn destroy_terminal_font(_raw: isize) {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn discovery_never_claims_a_missing_candidate() {
        let facts = probe();
        assert_eq!(
            facts.primary_family,
            facts.available_families.first().copied()
        );
        assert!(facts.available_families.iter().all(|family| {
            candidates()
                .iter()
                .any(|candidate| candidate.name == *family && candidate.exists())
        }));
    }

    #[test]
    fn fontconfig_monospace_is_primary_when_available() {
        let output = std::process::Command::new("fc-match")
            .args(["-f", "%{family}\n%{file}\n", "monospace"])
            .output();
        let Ok(output) = output else {
            return;
        };
        if !output.status.success() {
            return;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines();
        let expected_family = lines.next().unwrap_or("").trim();
        let expected_file = lines.next().unwrap_or("").trim();
        if expected_family.is_empty() || !Path::new(expected_file).is_file() {
            return;
        }

        let facts = probe();
        let primary = facts
            .primary_family
            .expect("fontconfig resolved a monospace file on this host");
        assert_eq!(primary, expected_family);

        let primary_path = candidates()
            .iter()
            .find(|candidate| candidate.name == primary && candidate.exists())
            .expect("primary family must map to an existing candidate")
            .absolute_path();
        assert_eq!(primary_path, PathBuf::from(expected_file));
    }

    #[test]
    fn cjk_fallback_reports_a_wider_advance_than_ascii() {
        if !fallback_candidates()
            .iter()
            .any(|candidate| candidate.exists())
        {
            return;
        }
        let report = primary_face_report(16).expect("primary face");
        let ascii = report.ascii_advance.expect("ascii advance");
        let full = report
            .full_width_advance
            .expect("CJK fallback must measure U+4E2D");
        assert!(
            full > ascii,
            "full {full} ascii {ascii} face {}",
            report.face
        );
    }
}
