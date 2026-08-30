# 360 QVM false-positive decisive experiment

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Status | **specified; exact v0.1.3 report not yet submitted** |
| Purpose | Decide whether 360 QVM reputation, Authenticode trust, or MiniCon's custom Windows startup is the actionable false-positive owner |
| Implementation | `lab/hello-window/` (Q0 small baseline); `research/qvm-false-positive/` (later controlled variants) |
| Read first | `PRD.md`, `prd/PRD_02_27_con_delivery.md`, `CODE_SIGNING_POLICY.md` |
| Source discipline | Only public exact Release bytes and reproducible one-variable variants; no evasion, packing, junk bytes, exclusions, or disabled protection |

This is a release-reputation experiment, not permission to weaken terminal
behavior or to promise that one antivirus verdict proves universal safety.

## §0 — settled context

1. Public v0.1.3 Windows x86_64 SHA-256 is
   `623b9e66da846cd559f2877f2c9d68d2aaf6dcb60ca9f556c6253f1ddfe880f3`;
   Windows arm64 is
   `144f5d28f1e1f0956cbb266cc98e572ec1f4d10b65d2d07d495dbee4a25fb0ad`.
2. Both exact Candidate executables passed active Microsoft Defender before
   Promotion. A different vendor/model may still flag them; verdicts do not
   inherit across engines.
3. 360 reported x86 Windows as `HEUR/QVM202.0.B951.Malware.Gen`. `HEUR` and
   `Malware.Gen` describe a generic heuristic verdict; 360 publishes no mapping
   from internal suffix `B951` to a specific byte sequence or behavior.
4. The released PE is an unsigned GUI executable with ordinary PE sections,
   embedded product/version resources, no packer, and a deliberate
   `/ENTRY:minicon_entry` that initializes the static VC runtime before Rust
   main. That unusual entry is a hypothesis, not a cause.
5. v0.1.3 stays immutable. Fixes apply to a later version or to vendor
   reputation for the exact released SHA.

## §1 — hard constraints

- Every verdict records exact SHA-256, architecture, 360 product/engine/database
  version, scan time, detection string and whether the sample was submitted.
- Compare samples on one Windows court with current definitions and no local
  allowlist. Restore the same snapshot or prove equivalent scanner state.
- A binary variant must pass `--version`, `--status`, load-portability, GUI and
  control courts before its antivirus result can influence product design.
- Trusted signing means a real timestamped provider signature. Self-signed
  rehearsal cannot stand in for publisher reputation.
- 360's official developer channel receives public binaries only. No private
  key, token, path, guest credential or unpublished source is uploaded.
- Any impulse to randomize strings, timestamps, section names, padding or code
  merely to cross a classifier boundary is the pathology this experiment
  detects. Record it as rejected evasion; never implement it.

## §2 — minimum experiment

| Variant | Fixed/moved axis | Why |
|---|---|---|
| Q0 released baseline | Exact public unsigned x86_64 v0.1.3 | Establish the reproducible detection and scanner identity |
| Q1 official rescan | Q0 bytes unchanged; submit through 360 developer false-positive channel | Isolates vendor reputation/allowlisting from code changes |
| Q2 trusted signature | Same qualified payload and behavior, provider signs through existing exact-byte workflow | Tests whether Public Trust publisher identity closes the verdict |
| Q3 conventional startup | Unsigned same-source x86_64 build using standard MSVC/Rust startup; product features retained | Tests the strongest structural hypothesis if Q1 does not close it |

The smaller `lab/hello-window` diagnostic baseline uses conventional startup
but statically links the CRT. It must not import the Visual C++ Redistributable;
otherwise a clean-machine launch failure would contaminate the antivirus test.

Q3 must publish its PE structural diff: entry RVA, imports, sections,
characteristics, resources, size and entropy. If conventional startup cannot be
built while preserving the no-redistributable and unwind contracts, record
that as the result; do not silently change more axes and call it isolated.

## §3 — preregistered criteria

| ID | Criterion | Nature | Pass condition |
|---|---|---|---|
| C1 | Baseline reproducibility | Boolean | Q0 produces the exact named verdict twice under one recorded scanner state |
| C2 | Official reputation effect | Boolean | Unchanged Q0 becomes clean after 360 completes its review |
| C3 | Trusted-signature effect | Boolean | Q2 is clean while the same-time Q0 control remains flagged |
| C4 | Startup-structure effect | Boolean | Pre-submission Q3 is clean twice while same-time Q0 remains flagged and behavior gates match |
| C5 | Product parity | Boolean | Any proposed changed binary passes all owning Windows behavior/portability gates |
| C6 | Diff inventory | Safety/list | Every PE difference from Q0 is named; no unexplained packer/evasion mutation exists |

One scanner verdict is categorical, not a percentage. Do not average engines
or count VirusTotal vendors as a substitute for the named 360 regression.

## §4 — decision tree, kill criterion and time-box

1. If C1 fails, stop: the report is stale or environment-dependent; obtain the
   exact screenshot/scanner identity before changing code.
2. Submit Q0 through 360's official channel. If C2 passes, rule **reputation
   route**: add exact per-release 360 submission/receipt; retain product code.
3. If C2 fails and trusted signing is available, test Q2. If C3 and C5 pass,
   rule **signing route**; signing remains a release-policy requirement.
4. If Q0 remains flagged, run Q3. If C4–C6 pass, rule **startup redesign
   candidate** and separately assess its size, Server 2016, unwind and runtime
   dependency costs before adoption.
5. If none separates, rule **unlocalized QVM heuristic**: preserve behavior,
   continue official vendor review, and do not mutate bytes blindly.

Before Q1, `lab/hello-window/` supplies a smaller diagnostic root. If that
ordinary GUI is clean, later feature bisection may grow from it toward MiniCon;
if it is flagged, split toolchain/PE/reputation below that baseline before
testing MiniCon subsystems. It informs where to start but does not replace C1.

Kill criterion: any variant loses product parity, introduces a redistributable,
or requires evasive mutation. Time-box ends when Q1 returns a vendor decision;
Q2 waits for trusted signing, and Q3 begins only if Q1 does not clean Q0.

Criteria reconciliation: C1 gates all branches; C2 precedes code changes; C3
and C4 are independent causal probes guarded by C5/C6. Every outcome exits to
reputation, signing, startup assessment, or unresolved-vendor review.

## §5 — result layout

```text
research/qvm-false-positive/
├── README.md
├── collect-pe-facts.py
└── RESULTS.md
```

Raw screenshots and vendor correspondence stay outside Git. `RESULTS.md`
contains redacted scanner metadata, exact public hashes, PE diffs, commands and
the §4 decision trace.

## §6 — excluded alternatives

| Alternative | Reason |
|---|---|
| Random byte/string/section mutation | Evasion, non-causal and invalidated by the next model update |
| UPX or another packer | 360 explicitly asks for unpacked samples; packing adds suspicion |
| Local allowlist/exclusion | Hides the customer problem and produces no distributable fix |
| Disable QVM/360 | Changes the test, not the product verdict |
| Treat Defender clean as 360 clean | Different engines and evidence identities |
| Upload every debug build | Pollutes reputation and violates exact-release evidence |

## §7 — not answered

- Whether another antivirus vendor will classify the same bytes identically.
- The proprietary feature represented by `B951`; only 360 can disclose it.
- Whether trusted signing alone prevents all SmartScreen/QVM warnings.
- Whether a future source change retains a prior version's 360 reputation.

## §8 — conclusion backfill

Not run. Fill only after Q1 returns: criterion table, exact scanner identity,
variant hashes, deviations, decision-tree path, reproducible commands and an
explicit statement that no metric or sample was changed to improve the result.
