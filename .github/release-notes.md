MiniCon is a standalone terminal in a single executable: no installer or
bundled language runtime, and no Visual C++ redistributable. It uses the
operating system's desktop libraries listed below.

## Highlights

- Adds aligned New Terminal, language, zoom and Help controls; Enter inserts a
  soft newline and Ctrl+O sends the complete composer draft.
- Keeps the window open on a greeting page after the final tab closes.
- Makes the Linux X11 runtime boundary explicit: runtime-only
  `libxkbcommon-x11.so.0` starts normally without a `-dev` package, while a
  missing package produces an actionable error instead of a Rust panic.
- Uses the QVM-qualified Windows release profile selected by the controlled
  false-positive experiment; no packer or evasive byte mutation is used.
- Prioritizes screenshot completion while terminals produce sustained output.
- Makes Windows control requests replay-safe across named-pipe disconnects
  without repeating mutations.
- Qualifies the same product behavior across the Windows, Linux and macOS
  x86_64/ARM64 test grid; the downloadable macOS client remains universal.

## Downloads

Five platform archives cover all six OS/ISA cells, with a SHA-256 beside each.

| Platform | Archive |
| --- | --- |
| Windows x86_64 | `minicon-VERSION-windows-x86_64.zip` |
| Windows ARM64 | `minicon-VERSION-windows-arm64.zip` |
| Linux x86_64 | `minicon-VERSION-linux-x86_64.tar.gz` |
| Linux ARM64 | `minicon-VERSION-linux-arm64.tar.gz` |
| macOS — Apple Silicon and Intel | `minicon-VERSION-macos-universal.tar.gz` |
<!-- OPTIONAL_APE_START -->
| Experimental unsigned six-cell APE launcher | `minicon.com` |
<!-- OPTIONAL_APE_END -->

The macOS build is a universal binary: the same file runs on both
architectures.

Linux archives use the distribution runtime libraries `libxkbcommon0` and
`libwayland-client0`; development packages are not required. They bundle the
runtime-only X11 bridge `libxkbcommon-x11.so.0` and its XCB-XKB dependency, so
slim X11 systems do not need the `libxkbcommon-x11-0` or `libxcb-xkb1` package
merely to start MiniCon.

Verify a download before running it:

```
sha256sum -c minicon-VERSION-linux-x86_64.tar.gz.sha256
```

## Old Windows

**Windows Server 2016 and Windows 10 version 1607** are supported. Those builds
have no pseudoconsole, so MiniCon hosts the shell in a hidden console instead.
See [Running on old Windows](https://minicon.agenterm.work/old-windows.html).

## Reporting a problem

Run `minicon --status` and include the output. It reports the build, the PTY
backend this machine selected and why, the font face the system actually
resolved to with its measured cell width, and where MiniCon writes when
something fails — none of which can be answered by reading the source.

MIT OR Apache-2.0.
