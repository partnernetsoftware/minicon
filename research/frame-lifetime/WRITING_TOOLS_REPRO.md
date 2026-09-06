# AppKit main-menu startup eagerly loads Writing Tools UI

Draft upstream reproduction; not submitted. Observed macOS 26.5.1 on aarch64.

## Problem

A custom pixel/text-input window with standard window controls and a complete
application menu loads WritingToolsUI during `NSApplication.finishLaunching`.
The view does not implement Writing Tools integration. Explicitly returning
`NSWritingToolsBehaviorNone` through `NSTextInputTraits`, and disabling
`NSMenu.automaticallyInsertsWritingToolsItems` on every constructed menu, does
not avoid the startup load. The latter API is documented for context menus;
this report does not assert it promises a main-menu opt-out.

## Reproduction

The checked-in research sources and runners are owned by
`plan/research-external-residency-next.md`. Run from the MiniCon repository:

```sh
mkdir -p target/frame-lifetime
WRITING_TOOLS_OUT=target/frame-lifetime/repro-menu python3 research/frame-lifetime/writing-tools-menu.py
WRITING_TOOLS_TRAITS=1 WRITING_TOOLS_OUT=target/frame-lifetime/repro-traits python3 research/frame-lifetime/writing-tools-menu.py
```

The native window preserves its first responder, input context, pixels,
standard application menu, `finishLaunching`, and event processing. Compare
default settings, the public menu flag disabled, and that flag plus custom-view
`writingToolsBehavior=None`. Keep measurements sequential on the same machine.
Use fresh output directory names for a repeat so the exclusive diagnostic files
retain previous receipts. No system settings or global preferences are changed.

## Actual behavior and call chain

All three public-API variants increase external-pager residency from
32.641 MiB to 55.219 MiB during `finishLaunching` (22.578 MiB). WritingToolsUI
is mapped in each process. A handshake immediately before that call permits a
short 1 ms `sample` run; 286 of 287 samples under `finishLaunching` traverse:

```text
NSApplication.finishLaunching
  NSApplication(NSMenuUpdating)._customizeMainMenu
    _addTextInputMenuItems:
      NSTextView(NSTextView_WritingTools)._supportsWritingTools
        WritingToolsUILibraryCore
          _sl_dlopen
            dyld dlopen_from
```

This attribution replaces an earlier, unconfirmed Objective-C class-list
hypothesis. The measured increment is mostly framework metadata residency,
not terminal scrollback, a full font read, or an additional pixel allocation.
Kernel ledger values are used for phase deltas. The separate page-to-image
census has a documented residual and is not presented as a complete ledger.

## Causal control, not a supported workaround

In a separate research executable, substituting return-NO for the private
`+[NSTextView _supportsWritingTools]` leaves the other startup steps intact.
It changes the finish-stage external increment from 22.578 to 0.156 MiB,
prevents WritingToolsUI mapping, and changes final RSS from 70.922 to
47.328 MiB. This private intervention demonstrates causality; it is not
included in MiniCon's production source or dependency pin and is not a public
API workaround.

## Expected improvement

Avoid eagerly loading Writing Tools UI for an application that does not use
Writing Tools, or provide a documented application-level opt-out that takes
effect before main-menu customization. Local context-menu/text-view settings
currently do not address this process-wide startup cost.

## Evidence

The owning report records exact source, commands, hashes, failed diagnostic
attempts and limitations. Compact receipts are under
`research/frame-lifetime/external-results.json`; full same-window traces and
vmmap receipts are under `target/frame-lifetime/`. The native experiment is
not a product RSS gate or a claim that 10 MiB has been achieved.


## Actual MiniCon confirmation

The same frozen MiniCon release was also compared with the same libobjc-only
observer in both arms. Default/checks-only idle RSS is 78.141 MiB; private-NO
is 60.500 MiB. Both arms pass the existing public GUI and RSS tests, and the
WritingToolsUI mapping disappears in the treatment. This is one paired run,
not a universal saving or a supported product workaround. Full identities and
commands are in the owning external residency report.

## Public API references

Apple documents [text-view behavior](https://developer.apple.com/documentation/appkit/nstextview/writingtoolsbehavior)
and [menu insertion control](https://developer.apple.com/documentation/appkit/nsmenu/automaticallyinsertswritingtoolsitems).
[WWDC25 Writing Tools](https://developer.apple.com/videos/play/wwdc2025/265/)
describes the menu property in the context-menu section. No documented
application-wide opt-out has been identified in this investigation.
