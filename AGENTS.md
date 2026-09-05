# MiniCon agent guide

Start at `PRD.md`. It is the compact product index and memory palace; follow
its links to the owning `prd/PRD_*.md` module for detailed behavior and
evidence. Machine truth lives in `alignment-contract.json`, public evidence
identities in `evidence-registry.json`, and release selection in
`release-policy.json`.

## Planning method: tree DAG + memory palace

Use both views for material product planning; neither replaces the other.

1. Write a Markdown tree DAG first. Begin with one user outcome, split it into
   capability owners, and give every delivery leaf its invariant, observable
   evidence, safe failure, dependency and explicit non-goal.
2. Draw one Mermaid flowchart memory palace for relationships that the tree
   cannot show well: shared prerequisites, exact-artifact flow, parallel
   courts, authority boundaries, decision gates and kill paths.
3. Keep one owner per fact. `PRD.md` is the compact index; an owning PRD module
   holds current product truth; `plan/` holds sequencing; `plan/archive/` and
   `prd/archive/` preserve superseded decisions and release history.
4. Upsert accepted scope and status into the owning PRD before archiving a
   completed plan. Link instead of copying. `[x]` requires named evidence;
   unavailable evidence is `BLOCKED`, never silently skipped.
5. Before implementation, identify shared prerequisites and hot files. Work
   independent leaves in parallel only when their file ownership and evidence
   are independent; integrate and run final gates serially.

## Product boundary

MiniCon is a one-file local terminal. Preserve these exclusions: no server,
persistent workspace, Fleet, mux, MCP, script runtime, plugin host or Agent
permission policy. Its `--control` endpoint lives only as long as its GUI.

Preserve these invariants:

- Child exit retains its tab, final screen and exit status until explicit close.
- Closing a parent promotes direct children; parent cycles are rejected.
- Closing the final tab leaves the window open on its greeting state.
- Composer and terminal input have one focus owner; Enter inserts a newline and
  Ctrl+O sends the complete draft.
- Native callbacks never unwind across FFI; bounded failures remain local.
- One platform's evidence never becomes a six-cell claim.

Local UTM courts are not MiniCon product code. Lifecycle, guest adapters and
image recipes live in sibling `utm-court` (`partnernetsoftware/utm-court`).
MiniCon calls that CLI from `scripts/*-utm-runner.sh`. Missing court is a
locator failure or `BLOCKED`, never a skipped PASS. Sequencing:
`plan/plan-utm-court-extract.md`.

## Development and delivery

From the repository root:

```bash
./scripts/build.sh dev
./scripts/build.sh release
./scripts/build.sh test
./scripts/six-cell-qualify.sh
```

Do not pick a product binary by modification time. Canonical local outputs are
documented in `README.md`. Do not commit generated binaries; local outputs stay
under ignored `target*/`, `dist/` or research output directories.

Release is exact-source Candidate followed by no-rebuild Promotion. Signing is
a `release-policy.json` choice, not a fallback inferred from credentials.
Public Promotion always requires explicit human version and publish authority.

## Change and documentation rules

- Put cross-platform mechanisms in the shared platform crates and product
  meaning in MiniCon-owned code. Do not make MiniCon depend on AgenTerm product
  state or evidence.
- Exercise observable behavior through public CLI/GUI black boxes.
- Keep `README.md` brief and human-facing; keep operational agent rules here.
- Preserve user changes in a dirty tree and stage only exact reviewed paths.
- In public documents use repo-relative paths for this clone and `~/...` for
  locations below a user home. Never write real emails, hostnames, IPs,
  credentials, cloud identifiers or expanded home paths.
