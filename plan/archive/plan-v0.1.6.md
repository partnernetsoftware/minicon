# MiniCon v0.1.6 delivery plan

Status: **archived — v0.1.6 shipped; this is historical execution evidence**
Outcome: publish the same unsigned native six-cell set plus `minicon.com`
from one exact source that includes the multiline paste fix. v0.1.5 remains
immutable.

## 1. Product ruling

Users paste multiline text into the composer and the terminal paste-review
dialog. v0.1.5 folded those breaks onto one painted line. v0.1.6 keeps
visible soft breaks; Send remains the only submission. Policy stays
unsigned native archives plus experimental `minicon.com`.

```json
{
  "version": "0.1.6",
  "assets": {
    "native_archives": true,
    "minicon_com": true
  },
  "signing": {
    "mode": "off"
  }
}
```

### Governing invariants

- v0.1.5's tag, assets, manifests and receipts are never edited or backfilled.
- One clean build emits six native payloads and one APE; test runners compile
  nothing.
- Candidate packages and seals exact bytes. Promotion only downloads, verifies
  and republishes those bytes.
- Unsigned means `signing.mode=off`.
- Every Windows-facing final executable is independently reputation-qualified:
  `minicon.com`, native Windows x86_64, and native Windows arm64.
- A failed, missing or stale court blocks v0.1.6. v0.1.5 remains the public
  fallback.

## 2. Markdown-tree DAG

```text
[V16] v0.1.6 — multiline paste + unsigned six-cell delivery
├── [S0] scope and source identity
│   ├── version = 0.1.6 in Cargo, policy, payloads and runtime output
│   ├── release-policy: native_archives=true · minicon_com=true · signing=off
│   ├── paste: composer stores LF; review displays CRLF; PTY delivery uses CR
│   └── v0.1.5 is immutable and remains the rollback release
├── [B1] one build owner
│   ├── six Rust payloads + one Cosmopolitan loader
│   └── pinned Rust/Zig/xwin/cosmocc tools + source/build receipt
├── [R3] six execute-only APE courts
├── [N4] native release parity (5 archives cover 6 cells)
├── [C5] exact Candidate, no compilation
├── [D6] exact-byte reputation court (three Windows-facing objects)
├── [P7] no-rebuild Promotion
│   └── explicit human authority: version 0.1.6 + publish-v0.1.6
└── [-] no signing claim, no v0.1.5 mutation, no UI redesign
```

## 3. Gate ledger

| Gate | Owner | Pass evidence | Safe failure |
|---|---|---|---|
| G0 policy/version | `release-policy.json`, Cargo | all identities say 0.1.6; paste unit tests green | stop before build |
| G1 one build | `minicon-com.yml` | clean exact SHA; six payloads + APE | discard build run |
| G3 APE six-grid | execute-only native runners | six unique same-digest GUI/control receipts | Candidate forbidden |
| G4 native parity | package/runtime courts | five archives cover six cells | Candidate forbidden |
| G5 size/manifest | Candidate sealer | raw APE ≤ 9 MiB; sidecars bound | do not raise ceiling |
| G6 reputation | Defender court | three final Windows-facing objects clean | no Promotion |
| G7 Promotion | release workflow + human | exact run IDs and `publish-v0.1.6` | no tag/Release mutation |

## 4. This increment

- [x] G0 source: paste fix `f817f04`; identities bumped to `a000565`.
- [x] G1 one build: `minicon-com.yml` run `33966670567` success.
- [x] G3/G4/G5 Candidate: `candidate.yml` run `33967601388` success (~1m36s).
- [x] G6 Defender court: clean on minicon.com + both Windows PEs
  (engine `1.1.26080.3`, signatures `1.459.55.0`); qualification dispatched
  as `reputation.yml` run `33968769535`.
- [x] G7 Promotion: run `33976278521`, tag `v0.1.6` at `a000565`,
  public Release neither draft nor pre-release.
  Ledger: `prd/archive/v0.1.6-release-history.md`.
