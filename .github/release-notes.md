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
- **Crisp text on scaled Windows displays.** MiniCon now declares per-monitor
  DPI awareness, so on a display scaled above 100% Windows no longer
  bitmap-stretches the window — text is sharp at the correct size instead of
  blurry.
- **Mouse-wheel scrolling in Windows terminal apps.** The wheel now carries the
  pointer position, so scrolling inside a full-screen terminal program, and over
  the tab sidebar, works on Windows the same as on macOS and Linux.
- **No more overlap between the terminal and the input box on scaled Windows
  displays.** The terminal's bottom row was overrunning the composer's top edge
  once per-monitor DPI scaling was active; the terminal now ends exactly where
  the composer begins.
- **Clicking a terminal program's bottom input line now works on Windows.**
  Because of that same overlap, a click on the very bottom row of a mouse-aware
  full-screen program (its input line) could be captured by the composer instead
  of reaching the program. The click now reaches the program as it does on macOS.
- **macOS now ships `MiniCon.app`.** The `.dmg` carries a signed, notarized and
  *stapled* application bundle — drag it to Applications and double-click, with
  no Gatekeeper prompt, online or offline. A bare command-line binary cannot
  carry a stapled notarization ticket, which is why earlier downloads could warn
  that the app "could not be verified" even though they were properly signed.
- **The tab sidebar collapses.** A toggle joins the sidebar header, to the right
  of Settings, and `Ctrl+Shift+B` does the same from the keyboard. Collapsed, the
  sidebar becomes a narrow rail: each tab shows its `@ID` — the same handle the
  control CLI uses — and hovering a row floats the full title beside it. The
  terminal actually gains the column rather than the chrome merely getting
  thinner.
- **MiniCon.app has an icon.** Running it from the `.dmg` used to leave a blank
  square in the Dock.
- **The window can be detached and reattached, and MiniCon can start without
  one.** `minicon cli --control … detach-gui` releases the window while the
  process, its sessions and the control endpoint keep running; `attach-gui`
  brings a window back with the scrollback intact. `minicon --headless` starts
  with no window at all, which is useful when something else — a script, an
  agent, eventually a phone — is going to attach to the session later. This
  narrows one line of the product boundary and nothing else: the endpoint's
  lifetime is now the process rather than the window, and there is still no
  server, daemon, persistent workspace, session sharing or background start.
- **Windows text is drawn in a real terminal font.** MiniCon used to pick
  新宋体 (NSimSun) for everything on any Windows with East Asian fonts
  installed, so every Latin letter was drawn in a Song face. Latin text now
  uses Cascadia Mono when it is installed and Consolas otherwise, and Chinese
  still comes from NSimSun, sized so that each character spans exactly two
  cells. `minicon --status` shows which face is in use. Rows are a little
  taller than before (18 px instead of 15 at the default size), because
  NSimSun's line spacing was unusually tight.
- **Crisper, heavier text on dark backgrounds.** Glyph edges are now blended
  the way DirectWrite blends them (the correction is ported from Windows
  Terminal), so light text on a dark theme no longer looks thin and washed
  out. For each font size, MiniCon measures GDI and DirectWrite against the
  font's true outline and uses whichever is closer: DirectWrite at the default
  size, GDI from 18 px up. `AGENTERM_FONT_RASTERIZER=gdi` or `=directwrite`
  forces one of them.
- **Selecting with the mouse copies, even inside full-screen programs.**
  Programs that take over the mouse, such as Claude Code, vim or htop,
  receive every drag, so MiniCon used to copy nothing. It now copies the text
  under the drag when the button is released, then passes the release on to
  the program.
- **The status bar shows how much text is on the clipboard**, left of the
  cursor position, and the count updates when anything is copied, in MiniCon
  or in another program.
- **The macOS input-method indicator follows the Chinese/English toggle.**
  macOS offers no way for an application to read an input method's internal
  mode, so the indicator used to say "native" whatever was selected. MiniCon
  now works it out from what the input method does with the next letter you
  type.

  Detaching does **not** measurably reduce memory on macOS: 84,432 KB attached
  against 84,512 KB detached, release build, one shell session. That was one of
  the reasons for building it, and it is withdrawn rather than repeated.
- **Pasting non-English text works again on macOS.** Every paste containing a
  non-ASCII character — a typographic apostrophe, Chinese, an emoji — failed
  with "clipboard read failed: invalid utf-8 sequence", in both the terminal
  area and the composer. The macOS clipboard helper encodes its output in the
  text encoding implied by the environment, and an app launched from Finder
  inherits almost no environment, so on a system whose language is not English
  the bytes came back in that language's legacy encoding. MiniCon now pins a
  UTF-8 locale for the helper instead of inheriting whatever the launcher
  provided. Launching from a terminal masked the bug, which is why it survived
  this long.
- **Mouse works in Windows terminal programs, by default.** Clicks never reached
  a program running inside MiniCon, while the same program in a plain `cmd.exe`
  window handled them fine. Windows has two ways a program can ask for the
  mouse and no terminal can serve both: MiniCon now hosts the shell through the
  classic Windows console, which serves the programs most people run here —
  agent CLIs and most things built on Node. `--feature conpty` switches to
  ConPTY, which serves Vim-style programs instead. The split is an open defect
  in the Windows console (microsoft/terminal#15083), inherited by every terminal
  built on ConPTY, Windows Terminal included.

  The previous release offered this as `{"console_agent": true}` in
  `minicon.json`. That key is gone; an existing config file keeps working and
  the key is simply ignored.
- **A Quit button on the greeting page.** With no tabs open the window could not
  be closed at all — the close button did nothing and the only way out was
  killing the process. The greeting page now offers an explicit exit, and the
  window close button works again.
- **The Linux download is about a quarter smaller** (6.0 MB → 4.5 MB). It was
  carrying an AT-SPI accessibility stack built for a sibling product's
  automation, which MiniCon never used. Linux automation is served by the
  control CLI instead.
- **Send (Ctrl+O) now delivers a real Enter.** The composer used to write the
  bracketed paste and its Enter as one buffer, so a terminal program received
  both in a single read. Programs that apply a paste asynchronously then handled
  the Enter first — while their input box was still empty — so the text arrived
  but was never submitted, and you had to press Enter yourself. The Enter is now
  held back briefly and written on its own, arriving as an actual key press.
  Still exactly one submission, so nothing is sent twice.
- **Cross-tab read and write accept files.** `send-text --file PATH` and
  `send-paste --file PATH` deliver a file's exact contents, so multi-line text,
  quotes and backslashes no longer have to survive shell quoting or the
  argument-length limit. `capture-pane --output PATH` writes the captured text
  to a file instead of stdout.
- **The startup diagnostic left the downloads.** It used to be copied into
  every archive, including Linux and macOS where a PowerShell script is useless.
  It is now published on the Release page on its own, under a name that says
  what it is: `diagnose-startup-windows.ps1`. You only need it if MiniCon
  refuses to start on an older Windows.
- **`minicon install-cli` puts `minicon` on your PATH.** An app bundle keeps its
  executable inside `Contents/MacOS`, so one command links it where your shell
  can find it — `install-cli`, optionally `--prefix ~/.local/bin` to avoid sudo,
  and `uninstall-cli` to undo. It only ever creates or removes a symlink; a real
  file of that name is refused, never overwritten.

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

The macOS `.dmg` contains a signed, notarized, stapled **`MiniCon.app`**
(universal — Apple Silicon and Intel). A `.tar.gz` of the bare signed binary is
also provided for command-line-only use. Every archive has a SHA-256 beside it.

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
