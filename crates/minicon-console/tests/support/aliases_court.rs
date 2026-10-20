//! Shared scenario selection; SDK implementation remains independent of host code.
pub fn kinds(system: bool) -> Vec<&'static str> {
    let mut kinds = vec![
        "table",
        "wide",
        "ansi",
        "multiline-wide",
        "multiline-ansi",
        "split-wide",
        "split-ansi",
        "silent",
        "leading",
        "raw",
        "literal",
        "arguments",
        "cmd",
    ];
    if !system {
        kinds.extend(["budget", "bounds", "counts", "units"]);
    } else {
        // Installed build 20348 loses LF in CP65001 multiline ANSI reads
        // returns a wide multiline count above the caller's capacity, and
        // expands aliases after leading whitespace (unlike the pinned host).
        // Keep owned assertions intact; the blocked native observations live
        // in PRD 02.34, rather than accepting truncated data as compatibility.
        kinds.retain(|kind| {
            !matches!(
                *kind,
                "multiline-ansi" | "split-ansi" | "split-wide" | "leading"
            )
        });
    }
    kinds
}
