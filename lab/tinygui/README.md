# tinygui — MiniCon-shaped empty pixel window

```text
Host-floor RSS/CPU for MiniCon's actual window stack
├── Artifact: one Rust binary, same pin as MiniCon (745f52b2)
│   ├── unix: portable-pixel-window (winit + vendored softbuffer)
│   ├── windows: native-pixel-window
│   ├── invariant: 960×600, --no-activate, IME off, one present, then Wait
│   └── non-goal: PTY, font, blink, MiniCon UI, six-cell MiniCon claim
├── Link: same six-cell toolchain as payload-build
│   ├── osx: cargo --profile release-fast
│   ├── lnx: cargo zigbuild --profile release
│   └── win: cargo xwin --profile windows-release
└── Execute: exact artifact, no guest Cargo
    ├── osx-aarch64: this host
    ├── osx-x86_64: Rosetta (no UTM row)
    └── lnx/win × {aarch64,x86_64}: utm-court, one lease at a time
```

`lab/hello-window` remains the Win32/QVM size court. It is not this lab.
A Darwin-only AppKit stub is not MiniCon's host; this binary is.

```sh
./lab/tinygui/build-six.sh
./lab/tinygui/court-six.sh
```

Receipts: `lab/tinygui/target/court-six-receipts.jsonl` and `RESULTS.md`.
Missing court is `BLOCKED`. One cell is not six-cell. Do not copy tinygui
RSS into a MiniCon product cell or the reverse.

`hello-cocoa.m` is an optional local AppKit floor only; it is not a six-cell
artifact and is not measured by `court-six.sh`.
