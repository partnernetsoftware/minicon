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

## Two lanes (one pack, six executes)

Never compile MiniCon or cosmocc on a guest/runner matrix.

**Local accelerated** (this Mac packs; host/Lima/UTM only execute)

```bash
./research/minicon-com-loader/local-accelerated.sh
```

`local-qualify.sh` is a compatibility alias. Lima instances this script starts
are stopped on EXIT. Receipt lane is `local-accelerated` (`source_sha`,
`source_dirty`, `source_tree_digest`, com digest, per-cell runner + log digest).
A dirty tree must not be treated as `source_sha == HEAD` alone.

**GitHub CI** (`.github/workflows/minicon-com.yml`)

- `workflow_dispatch` only (no per-push six-runner bill).
- **one** `macos-15` job: payloads + cosmocc pack; upload `minicon.com` + sha256.
- **six** native runners download that artifact and run `--version`, unknown
  flag (must be nonzero), and `--status`. No `cargo`, no cosmocc, no checkout.
- `aggregate` requires exactly six unique cells, the same `minicon_com_sha256`
  and `source_sha`, one `run_id`, all `job_status=success`.
- cosmocc zip SHA-256 and `bin/cosmocc` digest are pinned; install always
  builds a complete NEXT tree then `rename`s it over DEST (same parent).
  Failure before commit restores PREV; `install-cosmocc-swap-test.sh`
  injects a mid-swap abort and checks the old bin digest. Zig 0.16.0 is
  the same idea (verified tar → PREFIX, mv-fail restores). `--version`
  of cosmocc is GCC 14.1.0 and is not the release pin. Cache key hashes
  workflow + loader/pack/install scripts. GUI+control black-box is a later
  candidate gate, not this smoke.

Windows CI uses `Start-Process -Redirect*` (same as `release.yml`). Local
Windows UTM uses `utm-win-ape-status.sh` (job agent). Both consume the same
packed file.

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
| win-x86_64 | UTM TCG disposable + `job.ready` agent | PASS `minicon 0.1.2` `conpty` build 26200 |

Loader: per-invocation private temp dir (`mkdtemp /tmp/minicon.com.<pid>.XXXXXX`,
mode 0700 + `.minicon-extract` owner marker), atomic `.payload.tmp` → rename,
`posix_spawn` + `waitpid` (EINTR retry), then loader-owned unlink/rmdir.
`reap_stale_extracts` does not treat name+dead-pid as delete authority: `lstat`
the path, reject symlink/non-dir, require `st_uid==geteuid` and mode 0700,
require matching marker, then `open(O_NOFOLLOW|O_DIRECTORY)` / `unlinkat`.
Four cases in `reaper-tests.sh`: fake-symlink, foreign-owner, active-pid,
stale-owned. Never `#ifdef _WIN32` in cosmocc fat.

Not a product Release: `release.yml` still ships three archives. These two
lanes only qualify `minicon.com`.

## Non-goals

- Linking MiniCon crates against cosmocc
- Replacing `scripts/six-cell-qualify.sh`
- Byte-level compatibility with moltbaby zig-ape/rust-ape `APE!` tables
