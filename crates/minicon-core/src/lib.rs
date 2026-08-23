//! Host-neutral MiniCon logic.
//!
//! What lives here is decided by a single rule: **no platform, no operating
//! system, no window, no terminal backend, and no `cfg` on architecture or
//! OS.** Everything in this crate is ordinary data and arithmetic, so it
//! compiles and behaves identically on every target MiniCon supports and on
//! every target a consumer supports.
//!
//! That rule is what makes the crate usable from another product. MiniCon's
//! own binary depends on a platform layer; a consumer that only wants the
//! editing rules or the codec should not have to take that layer with it, and
//! a dependency on it here would force exactly that.
//!
//! The rule is enforced by a test rather than by intent — see
//! `no_platform_dependency_creeps_in`. It is the kind of boundary that erodes
//! one convenient import at a time, and a boundary nothing checks is a
//! comment.
//!
//! Deliberately *not* here yet: the tab tree, the chrome geometry and the
//! palette. Each is pure in substance but currently reaches into
//! `agenterm-ui-core` for one helper, and a dependency on that crate would
//! reintroduce the coupling this crate exists to avoid — worse, it would make
//! the crate unusable from `agenterm` itself, which owns that crate and would
//! then resolve two copies of it. Moving those helpers here is the next step,
//! and it is a deliberate one rather than an oversight.

pub mod composer;
pub mod json;

#[cfg(test)]
mod boundary_tests {
    /// The crate's whole value is what it does *not* depend on, and that is a
    /// property no compiler error announces: adding a platform dependency
    /// would build cleanly and quietly make the crate unusable from a consumer
    /// that cannot take a platform layer.
    ///
    /// Reading the manifest is the only way to assert it, because the property
    /// is about the dependency graph rather than about any line of code.
    #[test]
    fn no_platform_dependency_creeps_in() {
        let manifest = include_str!("../Cargo.toml");
        // Bounded at the next section header, so this checks the production
        // graph only. A dev-dependency cannot reach a consumer, and failing on
        // one would forbid the very oracles that keep this code honest.
        let dependencies = manifest
            .split("\n[dependencies]\n")
            .nth(1)
            .expect("a dependencies section")
            .split("\n[")
            .next()
            .expect("a bounded section");
        for forbidden in [
            "agenterm-platform",
            "agenterm-ui-core",
            "windows-sys",
            "winit",
            "softbuffer",
            "vt100",
            "libc",
        ] {
            assert!(
                !dependencies.contains(forbidden),
                "{forbidden} would tie this crate to a host; it belongs above this layer"
            );
        }
    }

    /// A `cfg` on architecture or OS is the other way the boundary erodes:
    /// the manifest stays clean while the code stops behaving identically
    /// everywhere, which is the property a consumer is actually relying on.
    ///
    /// Only the modules are scanned. This file names the patterns in order to
    /// forbid them, so scanning it would fail against itself.
    #[test]
    fn the_crate_behaves_identically_on_every_target() {
        for (name, source) in [
            ("composer.rs", include_str!("composer.rs")),
            ("json.rs", include_str!("json.rs")),
        ] {
            for forbidden in [
                "cfg(windows)",
                "cfg(unix)",
                "cfg(target_os",
                "cfg(target_arch",
                "cfg(target_pointer_width",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{name} branches on {forbidden}; this crate must behave the same everywhere"
                );
            }
        }
    }
}
