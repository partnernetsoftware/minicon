# Owned-window placement shortcuts

Parent: [workspace](PRD_02_25_con_workspace.md). Mechanisms: [reuse map](PRD_02_28_shared_core.md).
Status: `[-]` development integration after urgent owner request, 2026-10-25; selected for the current development version.

## Outcome tree DAG

```text
Outcome: move the focused MiniCon window to a predictable half of its current screen
├── [-] GEOMETRY — fixed left/right halves of the owning screen's usable frame
│   invariant: retain screen origin; exclude menu/Dock; use AppKit points exactly once
│   evidence: WS-01 negative origins/odd widths/invalid geometry and native frame readback
│   safe failure: invalid/no screen/fullscreen/nonresizable refuses; no guessed primary monitor
│   dependency: shared PixelWindow owned operation; non-goal: foreign-window management or AX permission
├── [-] INPUT — exact Command+Left/Right before local editor dispatch
│   invariant: press acts once; release is consumed; repeated presses are idempotent
│   evidence: WS-02 exact modifiers and unchanged text/session state
│   safe failure: report placement refusal through existing host notice, never terminate the session
│   dependency: GEOMETRY; non-goal: changing Command+Shift/Option/Control combinations
├── [-] PAGE — native WebView responder forwards the same window action
│   invariant: same-window/key-window/page-descendant filtering and bounded shortcut queue remain
│   evidence: WS-03 distinct physical arrow mappings and shared host action
│   safe failure: invalid focus/queue limits do not operate on another window
│   dependency: INPUT and existing native local monitor; non-goal: browser history or global hotkeys
└── [-] QUALIFY — separate mechanism, real adapter and physical-input evidence
    invariant: source/target/input provenance accompanies each claim
    evidence: WS-04 actual macOS native owned-window court and selected GUI/native journeys
    safe failure: unavailable native evidence is BLOCKED, not a passing Windows test
    dependency: preceding leaves; non-goal: migration of AgenTerm's full shortcut system
```

## Memory palace

```mermaid
flowchart LR
    HOST[Host key event] --> EXACT{Command only + Left/Right?}
    PAGE[Native WK responder / same key window] --> QUEUE[Existing bounded local shortcut queue]
    QUEUE --> ACTION[One MiniCon window action]
    EXACT -->|yes| ACTION
    EXACT -->|other modifiers| EDIT[Existing editor/browser/terminal path]
    ACTION --> SCREEN[Owned NSWindow.screen.visibleFrame]
    SCREEN --> CHECK{Window and geometry usable?}
    CHECK -->|yes| FRAME[Set fixed outer frame; read back actual frame]
    CHECK -->|no| NOTICE[Typed failure / existing host notice]
    FRAME --> KEEP[Same tabs, sessions and input focus]
    FUT[Future minicon-winmgr macOS product] -. reuse shared mechanism .-> SCREEN
```

## Behavior and boundary

Command+Left and Command+Right are window-level shortcuts inside MiniCon on
macOS, including Console, Settings and a focused native Web page. These exact
chords intentionally take precedence over editor line-start/line-end movement.
Additional Shift, Option or Control retains its existing meaning. Releases do
not reach text/PTY input. Repeating a shortcut selects the same half; it does not
cycle through thirds or move to another display.

Use the actual owned NSWindow's screen and its visible frame, in AppKit's own
coordinate system. Do not apply the Retina factor or mix Quartz top-left
coordinates into it. Placement concerns the outer window, leaving tabs, PTYs,
page state and editing drafts intact. No external-window AX path, Accessibility
prompt, system-wide shortcut registration or persistent window layout is added.

Fullscreen, a missing screen, invalid geometry, a nonresizable window or an
incompatible minimum/maximum size is a typed refusal before placement. If the
system clamps a requested frame, report the observed failure; do not claim that
no movement occurred after a native mutation. Future minicon-winmgr migration
is a separate product decision and must not create an AgenTerm product-state
or evidence dependency.

## Evidence frontier

Windows development evidence on the ca8861c source plus this uncommitted
shortcut overlay: `target-six/v0.2.23-window-half/initial-gates/receipt.json`
records fmt, default/native-auth-court all-target strict Clippy, two geometry
consumers and one exact-chord consumer passing. The initial test constructor
was corrected by source review before compilation; no compiler failure is
claimed as a counterexample. `pure-controls/receipt.json` in the same directory
records three actual original-assertion RED controls (right origin, invalid
geometry admission and Shift exclusion), each followed by exact restoration
and a passing original consumer. These are narrow WS-01/02 pure facts.

Run `40208688490` at exact `bea033d1e0b1e32516efee728b23157f9aeb12b6`
subsequently compiled and executed both macOS ISAs. Actual owned-window
left/right/repeat and incompatible-content-minimum refusal passed. Removing
setFrame and changing the page physical-arrow mapping each reached their
original assertion; exact restored consumers passed. The isolated-content
preflight mutant instead passed the entire example, so both complete courts
correctly failed. The exact rejecting getter/path was not observed; it remains
UNKNOWN. Mac ordinary suites after that step were skipped. Raw audit:
`target-six/v0.2.23-window-half/ci-bea033d/macos-half-verification-index.json`.
The enclosing scoped six-cell audit is `verification-index.json` beside it;
its Linux/Windows passes do not qualify the skipped Mac suites.

The next source court removes both related frame/content size-preflights as
one boundary and records finite getter/refusal observations. It does not
retroactively relabel the old NOTRED or prove each size guard independently.
WS-02 public input/state, WS-03 actual native page focus and the full WS-04 join
remain open. The example establishes adapter placement without AX/global
input; physical keyboard and focused WK responder ownership remain distinct.

## WS-02 public CLI host-route court amendment

Add a fresh owned `--no-activate` macOS phase to the existing browser workspace
smoke. Public `send-ui-keys Cmd+Left`, `Command+Right`, and `Meta+Right` must
produce left, right and repeated-right actual window bounds while preserving
tab identities, the active Browser, composer and address drafts/selection,
loaded URL/title and the fixture's live DOM/history report. A malformed final
chord rejects the whole batch with geometry and retained input unchanged.

The observer reads only the exact child's unique layer-zero CG window and the
single primary NSScreen's public frame/visibleFrame metadata. AppKit visible
frame is converted once to CG's main-display top-left coordinates; Retina scale
is not multiplied. Multiple displays, ambiguous windows or unavailable metadata
are BLOCKED, never silently qualified. The source control bypasses only the
ConApp snap call, requiring the original public geometry assertion to fail;
exact restoration must pass the same journey again. This establishes CLI host
routing on the selected single-display court. Hardware keys, a focused WK
responder and multi-display public observation remain open. Source integration
and two Windows pure-parser controls are now implemented; their evidence is
owned by [PRD26](PRD_02_26_con_control_cli.md#ui-only-commandmeta-injection-amendment).
The new macOS public journey and joint size-preflight control remain UNRUN
until the next exact-source native court executes.
