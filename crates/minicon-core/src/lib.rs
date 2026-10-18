//! Host-neutral rules for MiniCon and embedding products.
//!
//! Native backends and new OS/ISA-dependent rules belong outside this crate.
//! One historical host-newline convenience API remains for compatibility;
//! new callers pass the newline contract explicitly. The boundary check names
//! that exception and rejects other direct target selectors.
//! Default features retain the complete editing/input/codec API. Consumers of
//! tree, numeric, click or version alone can disable defaults; keymap enables
//! composer explicitly. Product routing and native adapters remain outside.
//!
//! The recursive source-boundary check lives outside src so it can inspect
//! every source module, even when its feature is disabled. It checks direct
//! cfg/cfg_attr/cfg! selectors; runtime equivalence needs separate evidence.
//! Ownership and reuse contracts: `prd/PRD_02_28_shared_core.md`.

pub mod click;
#[cfg(feature = "composer")]
pub mod composer;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "keymap")]
pub mod keymap;
pub mod numeric;
pub mod tree;
pub mod version;

#[cfg(test)]
#[path = "../tests/support/source_boundary.rs"]
mod boundary_tests;
