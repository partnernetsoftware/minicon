# Vendored 	inyvm core

Provenance: https://github.com/partnernetsoftware/tinyvm at commit f476cd2c1ea500c60fbde152b02681b744eea6d5
(copied 2026-11-17 by the bx P1 migration, see prd/PRD_02_44_bx_tinyvm_absorption.md).

Rules while vendored:

- This is an owned copy, not a crates.io dependency. Keep LICENSE-MIT /
  LICENSE-APACHE and the module docs intact.
- Do not hand-edit behavior here without recording why in the bx PRD; the
  canonical history stays in the tinyvm repository until the retirement gate.
- 	ests/ is not vendored. bx proves the core through its own suite and the
  300-case differential, and later through the core's upstream smoke scripts
  moved over in P3.
- Only the std feature is enabled by bx today. staticcore, simd,
  cartridge/replay/iOS features are carried for the later absorption phases.

Upstream product sentence: standard WebAssembly interpretation, no JIT, bounded
decode/validate/execute, host imports behind a versioned boundary.
