# minicon.com — cosmocc launcher over six-cell payloads

Exploration only. Does **not** recompile MiniCon with cosmopolitan libc.
GUI/PTY/Win32/Cocoa/Wayland stay in the already-qualified Rust cells.

```
minicon.com          ← cosmocc fat APE (ISAx2 host trampoline)
 └── /zip/cells/     ← ZipOS overlay
      osx-aarch64/minicon
      osx-x86_64/minicon
      lnx-aarch64/minicon
      lnx-x86_64/minicon
      win-aarch64/minicon.exe
      win-x86_64/minicon.exe
```

On start the trampoline picks `{os}-{isa}`, copies the blob out of ZipOS
(or `MINICON_COM_CELLS`), and **exec**s it. In-process mmap (rust-ape style)
is the wrong join for a native GUI terminal.

Payload source: `target-six/builds/current/…/release-fast/minicon` (fallback
`debug/` if release-fast missing). Those binaries remain the six-cell gate's
job; this directory only packs and dispatches.

## Preflight / pack

```bash
# repo root
./research/minicon-com-loader/preflight.sh
./research/minicon-com-loader/pack.sh          # host-cc dispatcher if no cosmocc
# after ~/cosmocc exists:
COSMOCC_DIR=~/cosmocc ./research/minicon-com-loader/pack.sh
./research/minicon-com-loader/dist/minicon.com --status
```

`--status` of one `minicon.com` after rebuilding payloads from this tree (`0.1.2`):

| cell | court | `--status` |
|------|-------|------------|
| osx-aarch64 | host | PASS `minicon 0.1.2` `unix-pty` |
| osx-x86_64 | Rosetta | PASS `minicon 0.1.2` `unix-pty` |
| lnx-aarch64 | Lima vz | PASS `minicon 0.1.2` `unix-pty` |
| lnx-x86_64 | Lima qemu | PASS `minicon 0.1.2` `unix-pty` |
| win-aarch64 | UTM + `.exe` + Start-Process | PASS `conpty` (0.1.0 pack; 0.1.2 not re-run) |
| win-x86_64 | UTM TCG | FAIL — GUI PE holds redirected files; QGA/PowerShell unusable. Raw `minicon.exe` same. Not an APE-only bug. |

Loader: runtime `IsWindows()` + `posix_spawn`/`waitpid`, extract to `/C/Users/Public/minicon-payload.exe`. Never `#ifdef _WIN32` in cosmocc fat.

Not v0.1.3: win-x86_64 evidence, `release.yml` still three archives.

## Non-goals

- Linking MiniCon crates against cosmocc
- Replacing `scripts/six-cell-qualify.sh`
- Byte-level compatibility with moltbaby zig-ape/rust-ape `APE!` tables
