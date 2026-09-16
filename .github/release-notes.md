MiniCon is a standalone terminal in a single executable — no installer, no
bundled language runtime, and no Visual C++ redistributable. It uses only the
operating system's own desktop libraries.

## Highlights

- **Signed and notarized.** The Windows executables and `minicon.com` are
  Authenticode-signed (Azure Artifact Signing), and the macOS build is
  Developer ID-signed and Apple-notarized — all as PARTNERNET SOFTWARE PTY LTD,
  with an RFC 3161 timestamp.
- A two-tool header (New terminal + Settings) with a settings panel gathering
  interface language (English / 简 / 繁), font size, theme, and the shortcut
  list. Open it from the header or with `Ctrl+Shift+,`.
- Three built-in themes (Neutral Ink, Docs Ink, Paper Ink), switchable from the
  panel swatches or `Ctrl+Shift+P`. Each theme also recolors the terminal body
  (background, foreground, cursor, and the 16 ANSI colors); the 256-color cube
  stays standard and a program's own explicit colors are never overridden.
- A grid crosshair (`Ctrl+Shift+G`) and a bottom status bar showing the
  cursor's `L###:C###`.
- The composer keeps its own line, separate from the output: `Enter` inserts a
  newline, `Ctrl+O` sends the draft. The window stays open on a greeting page
  after the last tab closes.
- Runs on older Windows (Server 2016 / Windows 10 1607), and on Linux the
  runtime-only `libxkbcommon-x11.so.0` starts without a `-dev` package.
- **Correct CJK / double-width text on legacy Windows.** On Windows without
  ConPTY (Server 2016 / 1607), Chinese, Japanese, Korean and other double-width
  characters now render aligned — no stray spaces between glyphs and no drift on
  lines that mix CJK with ASCII.

## Downloads

Pick your platform and run the file — there is no installer.

| Platform | Download |
| --- | --- |
| Windows x64 | `minicon-VERSION-windows-x86_64.zip` |
| Windows ARM64 | `minicon-VERSION-windows-arm64.zip` |
| macOS (Apple Silicon + Intel) | `minicon-VERSION-macos-universal.dmg` |
| Linux x64 | `minicon-VERSION-linux-x86_64.tar.gz` |
| Linux ARM64 | `minicon-VERSION-linux-arm64.tar.gz` |
<!-- OPTIONAL_APE_START -->
| Advanced: one signed launcher, all platforms | `minicon.com` |
<!-- OPTIONAL_APE_END -->

The macOS `.dmg` is a signed, notarized, stapled universal binary; a `.tar.gz`
of the same binary is also provided. Every archive has a SHA-256 beside it.

Linux archives use the distribution runtime libraries `libxkbcommon0` and
`libwayland-client0` (no `-dev` packages required) and bundle the runtime-only
X11 bridge `libxkbcommon-x11.so.0`, so slim X11 systems start MiniCon without
extra packages.

Verify a download before running it:

```
sha256sum -c minicon-VERSION-linux-x86_64.tar.gz.sha256
```

## Older Windows

**Windows Server 2016 and Windows 10 version 1607** are supported. Those builds
have no pseudoconsole, so MiniCon hosts the shell in a hidden console instead.
See [Running on old Windows](https://minicon.agenterm.work/old-windows.html).

## Reporting a problem

Run `minicon --status` and include the output. It reports the build, the
console backend this machine selected and why, the font the system resolved to
with its measured cell width, and where MiniCon writes when something fails —
without opening a window.

Signing details:
<https://github.com/partnernetsoftware/minicon/blob/main/CODE_SIGNING_POLICY.md>

MIT OR Apache-2.0.
