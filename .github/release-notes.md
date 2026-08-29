MiniCon is a standalone terminal in a single executable: no installer, no
runtime, and no Visual C++ redistributable.

## Highlights

- Separates Send and Newline so external input can be composed and submitted
  deliberately.
- Prioritizes screenshot completion while terminals produce sustained output.
- Makes Windows control requests replay-safe across named-pipe disconnects
  without repeating mutations.
- Qualifies the same product behavior across the Windows, Linux and macOS
  x86_64/ARM64 test grid; the downloadable macOS client remains universal.

## Downloads

Five platform archives cover all six OS/ISA cells, with a SHA-256 beside each.
The experimental `minicon.com` launcher contains all six payloads in one file.

| Platform | Archive |
| --- | --- |
| Windows x86_64 | `minicon-VERSION-windows-x86_64.zip` |
| Windows ARM64 | `minicon-VERSION-windows-arm64.zip` |
| Linux x86_64 | `minicon-VERSION-linux-x86_64.tar.gz` |
| Linux ARM64 | `minicon-VERSION-linux-arm64.tar.gz` |
| macOS — Apple Silicon and Intel | `minicon-VERSION-macos-universal.tar.gz` |
| Six-cell APE launcher | `minicon.com` |

The macOS build is a universal binary: the same file runs on both
architectures.

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
