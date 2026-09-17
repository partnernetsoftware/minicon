# PRD 02.30 — `foundry` software-production-line horizon

Status: **portfolio horizon, not MiniCon scope**. This records a cross-product
direction the owner has decided on (初心: 抽象与复用 — abstraction and reuse), so
the blueprint is not lost. It is **not** near-term MiniCon work, does **not**
authorize creating a repository yet, and does **not** change any MiniCon
workflow, receipt, or contract. MiniCon remains the reference consumer whose
real release chain is the only proven source of the parts inventoried below.

The canonical home for this blueprint is the private
`partnernetsoftware/dev-lifecycle-playbooks` repository (methodology layer). This
file is a MiniCon-side mirror so the decision is discoverable from the product
that seeded it.

## Why this exists

MiniCon, AgenTerm, and more software queued behind them each re-implement the
same build / test / sign / release machinery. That repetition is the waste the
owner wants removed. The goal is a **software production line** (软件生产线 /
工坊) — a reusable line that mass-produces trustworthy releases — named
**`foundry`** (a chip-foundry metaphor: designs come in,规模化 signed products go
out). It pairs, by metaphor, with the existing `utm-court` (裁决场, the
test-environment engine): **court judges, foundry casts**.

## What `foundry` is — and is not

`foundry` is **not** a new build system or a new CI runner. Those already exist
as *capabilities*. `foundry` is the **assembly line**: the correct assembly
order, the quality gates, the safety red lines, and an AI-friendly driver that
turns those capabilities into a "confidently mass-produce" line.

```text
foundry ≈  GitHub-CI executors (build / sign / release runners)      [capability, exists]
         + utm-court test-environment engine (health / AV courts)     [capability, exists]
         + assembly craft: order + gates + red lines                  [foundry's own asset]
         + AI-friendly driver: one-command runs, structured receipts  [foundry's own asset]
```

The craft is the reusable body. Machines can be swapped per product; the order
and the gates carry over. Everything below marks each part as **[generic]**
already product-neutral, **[parameterize]** generic once a value is lifted out,
or **[minicon]** product-specific and must not be hoisted from one sample.

## Product outcome tree

```text
foundry software production line
├── user problem
│   └── every new product re-implements build/test/sign/release; effort wasted
├── outcome
│   └── one reusable line produces trustworthy releases for many products
├── the four layers
│   ├── github-ci executors                                   [generic capability]
│   ├── utm-court test-environment engine                     [generic capability]
│   ├── assembly craft (order + gates + red lines)            [the reusable asset]
│   └── AI-friendly driver (one-command, structured receipts) [the reusable asset]
├── layer boundaries (do not merge)
│   ├── dev-lifecycle-playbooks  = why (methodology, paper)
│   ├── foundry                  = how, executable (this)
│   ├── company-dev-hub/skills   = how an agent invokes foundry
│   └── utm-court / github-ci    = the machines foundry calls
├── entry gate (do not build the repo until all hold)
│   ├── ≥ 2 real consumers exist (MiniCon + a genuinely different 2nd, e.g. Android client)
│   ├── the release chain has been stable across several versions
│   └── each hoisted part has a named parameter, not a copied constant
└── non-goals
    ├── no repository before the entry gate holds
    ├── no abstraction straightened from a single sample (MiniCon alone)
    └── no weakening of any existing MiniCon gate or red line to fit the line
```

## Parts inventory — the MiniCon release chain, marked for reuse

The order below is the real assembly order; each step's genericness is marked.

1. **six-cell qualify** (`{win,lnx,osx}×{x86_64,aarch64}` fmt/clippy/test/build)
   — **[parameterize]** the cell matrix and per-product ceilings; the harness is
   generic.
2. **one-pack build** (all artifacts from one source state) — **[parameterize]**
   artifact list; **[minicon]** the `minicon.com` APE representation.
3. **dual signing**, Windows Trusted Signing (OIDC) + macOS Developer ID +
   notarization, both `mode=required` — **[generic]** the flow; **[parameterize]**
   signing identities and profiles (must stay out of receipts/logs).
4. **candidate seal** — freeze exact bytes, bind `GITHUB_SHA == source_sha`.
   **[generic]** and load-bearing: never rebuild/re-sign to promote.
5. **court health / AV scan** — native-ISA Defender court (not emulated),
   produces a receipt. **[generic]** via utm-court; **[parameterize]** which
   courts a product requires.
6. **weld receipt → qualification** — `reputation_court.py qualify` converts the
   court receipt into the qualification kind reputation consumes. **[generic]**
   gate; a raw receipt must never be fed to reputation.
7. **reputation** — **[generic]**; **[parameterize]** `mode` (e.g. `defender`).
8. **release** — dry-run then publish; publish *promotes* the sealed candidate,
   never rebuilds. **[generic]**.

### Red lines the line must enforce (all [generic])

- Never bulk-kill the product process during a run (shares the user session's
  kill-on-close job object).
- Never relax/skip the AV scan or edit assertions to force green.
- Never rebuild/re-sign to promote — promotion moves the sealed candidate only.
- Never run exploratory VMs in the release court instance during a release.
- Never commit to `main` during a release (dispatch `--ref` must resolve to the
  candidate's `source_sha`); use a throwaway branch at the SHA if `main` moved.
- Keep signing identifiers out of receipts, logs, and docs.

## New-product onboarding checklist (what the line asks of a consumer)

- Declare the cell matrix and per-cell size/latency/memory ceilings.
- Declare the artifact list and any product-specific representation.
- Provide signing identities/profiles by reference (never inline).
- Declare which courts are required and their native-ISA targets.
- Provide a machine-readable release policy (signing mode, reputation mode).
- Nothing product-specific may leak into the shared craft; if it must, it is a
  parameter, not a fork.

## Deferred decisions

- Which parts graduate from **[parameterize]** to **[generic]** — decided only
  when the 2nd consumer exercises them, not before.
- The driver's interface shape (single binary vs. script set vs. workflow).
- Whether `company-dev-hub/skills` call `foundry` directly or through playbooks.
- Numeric per-product ceilings, fixed per consumer at onboarding.
