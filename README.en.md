**English** · [简体中文](README.zh.md)

# MiniCon™

`one file · no install · nothing left behind`

**One file is the whole terminal.** Copy one file and run it — a fast terminal
for Windows, macOS and Linux, with an input line of its own and a control
command your scripts can read. No installer, no bundled runtime, nothing left
behind.

![The MiniCon window: a tab tree, a terminal, and a separate input line at the bottom.](docs/assets/minicon-neutral.png)

| | |
| --- | --- |
| Runs on | Windows, Linux, macOS — x86_64 and aarch64 (Windows back to Server 2016) |
| Packaging | one executable per platform; release archives include SHA-256 checksums |
| Depends on | native OS libraries; browser pages use WebView2 (Windows), WebKit (macOS), or GTK 3 / WebKitGTK 4.1 (Linux) |
| Licence | MIT OR Apache-2.0 |

## Your input has its own line.

Most terminals make you type where the program is printing, so a command and a
burst of output tangle together. MiniCon gives input a line of its own.

- **Write while output scrolls.** Nothing you type interleaves with what a
  program prints.
- **Recall what you sent.** Up and Down walk your sent lines; half-typed text is
  kept while you browse.
- **A panel lists what you sent.** The right-side panel — the header's panel
  toggle or `Ctrl+Shift+H` — shows the newest sent lines; clicking one puts it
  back in the input area through that same recall, so a half-typed draft
  survives.
- **It names the target tab.** In a tree of tabs you always know where a command
  is headed.

`Enter` makes a new line; `Shift+Enter` or `Ctrl+O` sends the draft.

## Finding your way.

The tab tree has a fixed **@0 Home** above your terminal and Web tabs. It is a
quiet starting screen; it does not create a tab or start an agent. The two New
buttons open a terminal or the Web page directly. The robot button opens the
Harness overview, while Settings holds account and connection changes.
Terminal output and the input area each have their own visible scrollbar.

## Your scripts can see into it.

Sending a keystroke is easy — but blind. The control command reads back what the
window actually shows — text, screenshots, focus, tabs — so automation waits for
a real result instead of guessing with a sleep. Control stays local, with no
server. Network access follows your browser navigation, sign-in or manual update check.

```
# start a session with a local control endpoint
minicon --control unix:/tmp/minicon-build/control.sock
# Windows: minicon --control pipe:\\.\pipe\build

# run something, then wait for it — no sleeps
minicon cli --control unix:/tmp/minicon-build/control.sock send-text "cargo test\r"
minicon cli --control unix:/tmp/minicon-build/control.sock wait-text --timeout-ms 120000 "test result"

# read the result as text, or as a picture
minicon cli --control unix:/tmp/minicon-build/control.sock capture-pane
minicon cli --control unix:/tmp/minicon-build/control.sock screenshot-pane --output run.png
```

Programs inside a Console tab inherit this MiniCon instance's local control
endpoint, even on an ordinary launch. Run `minicon mux list-panes` there, then
address another terminal by its stable `@ID`. `--control` remains available
when an outside script needs a known endpoint.

`ui-snapshot` returns focus, input-method state, pending work and layout as JSON;
`wait-text` and `wait-tab-exit` block until a condition holds or times out;
`send-keys`, `send-mouse`, `send-wheel` and `send-ui-ime` type, click, scroll and
compose CJK through the same paths a person's input takes.
→ [docs/control-cli.html](docs/control-cli.html)

## Small, open, signed — on purpose.

- **One executable.** No installer or bundled browser engine. Artifact sizes
  and platform measurements are maintained in [the delivery requirements](prd/PRD_02_27_con_delivery.md).
- **Permanently open source.** MIT OR Apache-2.0. No telemetry, no required MiniCon account, no
  auto-updater — the settings panel's update check is manual, per-press, and
  never runs on its own. The source is the product.
- **Updates are manual, and reversible.** The panel can check the release page
  and replace the one file in place; the copy it replaces is kept beside it as
  `minicon.exe.backup` on Windows and `minicon.backup` elsewhere, so a build
  that will not start is one rename away from being undone — rename the backup
  back over the executable (and on Linux, if you ever end up with a `minicon`
  that will not run, `chmod +x minicon` or re-download). Nothing else is
  left behind: a download that was never installed is cleared the next time
  MiniCon starts.
- **Signed and notarized.** Windows and macOS, as PARTNERNET SOFTWARE PTY LTD.
  Browser pages require the native runtime listed above; a missing runtime
  leaves the terminal usable and reports a browser error.
- **Low on purpose.** No background service or daemon; close the window and its
  owned terminal/browser processes stop. Keeping idle memory small is a goal we work
  on, not a side effect.

Three built-in themes — Neutral, Docs Ink and Paper Ink — recolour chrome and
terminal together; cycle with `Ctrl+Shift+P`.

## Runs on older Windows, too.

Newer terminals need a recent Windows build and will not start on older
machines. MiniCon resolves the pseudoconsole at run time and falls back to a
hidden console, so it runs back to Windows Server 2016.
→ [docs/old-windows.html](docs/old-windows.html)

## Install

Download the executable for your platform from
[Releases](https://github.com/partnernetsoftware/minicon/releases) and run it.
Every archive ships a SHA-256 beside it; the macOS `.dmg` is a signed,
notarized universal binary.

```bash
sha256sum -c minicon-<version>-linux-x86_64.tar.gz.sha256
```

macOS only, to put `minicon` on `PATH`:
`/Applications/MiniCon.app/Contents/MacOS/minicon install-cli`.

## Use

```
minicon                          # a terminal, with your default shell
minicon -e cmd.exe /k build.bat  # run one command instead of a shell
minicon --status                 # what this machine gave it
minicon --help                   # everything else
```

Choose **New → Browser** to preview a local URL beside your terminal. If the
service is unavailable, start it in the terminal and choose **Retry**; use
**Edit address** to correct the URL. Click a terminal tab to resume your draft.

| Key | |
| --- | --- |
| `Ctrl+Shift+T` | new terminal |
| `Ctrl+Shift+O` | open or return to the browser tab |
| `Ctrl+Shift+N` | new terminal below the active tab |
| `Ctrl+Shift+W` | close the active tab; children are promoted |
| `Ctrl+Shift+B` | collapse the tab sidebar to a rail, or expand it again |
| `Ctrl+Shift+[` / `]` | switch tabs |
| `Ctrl+Shift+I` | focus terminal input, or the browser address field |
| `Ctrl+L` / `Cmd+L` | in the browser tab, edit its address; Enter navigates and Escape returns to the page |
| `Cmd+Left` / `Right` | on macOS, move this window to the current screen's left/right half |
| `Ctrl+Shift+P` | cycle the color theme (Neutral / Docs / Paper) |
| `Cmd+[` / `Cmd+]` / `Cmd+R` | on macOS, browser Back / Forward / Reload |
| `Ctrl+Shift+,` | open or close the settings panel |
| `Ctrl+Shift+G` | toggle the grid crosshair |
| `Ctrl+Shift+H` | open or close the sent-history panel |
| `Enter` | in the input area, insert a soft newline without sending |
| `Shift+Enter` | send the complete input-area draft |
| `Ctrl+O` | send the complete input-area draft |
| `Up` / `Down` | in the input area, move between lines; at the first or last line, recall what you sent before |
| `Alt+Up` / `Down` | in the input area, recall what you sent before from any line |
| `Ctrl+Left` / `Right` | in the input area, move the caret by whole words |
| `Ctrl+Home` / `End` | in the input area, move the caret to the start or end of the draft |
| `Ctrl+Backspace` / `Delete` | in the input area, delete the word before or after the caret |

## Configuration

Optional `minicon.json` in your user config directory: `font_size`, `cols`,
`rows`, and `backend` (`rust-console`, `console-agent` or `conpty`; Windows).

**Windows backend:** since 0.2.20 MiniCon hosts shells in its own console
(`rust-console`): one process per tab instead of three, no extra `conhost.exe`,
and mouse delivered as console records. If a program misbehaves,
`--feature console-agent` (or `"backend": "console-agent"`) returns to the
classic console; `--feature conpty` selects ConPTY for Vim-style programs.
`minicon --status` shows which one a run will use.

**Alternate screen and resize:** full-screen TUIs that use the alternate screen
do not follow a window resize on Windows. After a resize they redraw at the old
size and the layout is misplaced. The system console cannot change a child's
visible size while the alternate screen is active (unrelated to MiniCon; the
classic console and ConPTY backends behave the same). Keep the program off the
alternate screen to avoid it, e.g. `grok --no-alt-screen` (or its
`[terminal] alt_screen` setting); MiniCon then reflows the interface correctly.

## Build

```bash
./scripts/build.sh release     # target/release/minicon
./scripts/build.sh dev         # target/debug/minicon
./scripts/build.sh test        # unit + GUI black-box + gates
./scripts/six-cell-qualify.sh  # all six OS/ISA binaries (needs a macOS host; CI runs these as .github/workflows/minicon-com.yml)
```

On macOS, the canonical dev build also assembles the unsigned local-test wrapper
`target/debug/MiniCon 0.2.26 Dev.app`; its version follows `Cargo.toml` and it has a distinct development bundle id
so it cannot be confused with an installed release during UI qualification.
The executable under test remains `target/debug/minicon`.

Rust 1.97. Linux needs `libxkbcommon.so.0` and `libwayland-client.so.0` (no
`-dev` packages). See [PRD.md](PRD.md) and the `plan/` notes.

## Repository

| | |
| --- | --- |
| `src/` | the binary: terminal, host UI, control protocol |
| `crates/minicon-core/` | host-neutral logic — no platform or architecture `cfg` |
| `PRD.md` | product tree and module PRDs |
| `AGENTS.md` | the agent working agreement: build, test, qualify, release |
| `docs/` | the website at `minicon.agenterm.work` |

## More

- **AgenTerm.** [AgenTerm](https://github.com/partnernetsoftware/agenterm) is the
  Agent-era workbench on the same platform layer; MiniCon is its one-file local
  terminal. MiniCon began as AgenTerm's console host and became its own product
  in August 2026.
- **Code signing.** Since v0.1.9 every public release is dual-signed — Windows
  Authenticode and macOS Developer ID + notarization, as PARTNERNET SOFTWARE PTY
  LTD, with an RFC 3161 timestamp. Verify a Windows download with
  `scripts/inspect-authenticode.ps1`. Details in
  [CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md).

The editable proxy dropdown beside a Browser address accepts an HTTP proxy
(for example `http://127.0.0.1:8890`). Enter applies and reloads the page,
resetting its session/history; Escape cancels editing. Manage saved choices
in **Settings → Web**; **Console** keeps the terminal settings. Select
**Use configured proxy** to clear the override.
Adding a choice in **Settings → Web** does not activate it for the current page;
select it beside the Browser address, or enter an endpoint there and press Enter.

The 0.2.23 line adds experimental **Settings → Harness** for optional provider
accounts and an explicitly started local model API. Its connection steps and
current qualification limits are owned by the
[Harness requirements](prd/PRD_02_31_01_provider_gateway.md).
Authorization pages use the account's **Login proxy**, independently of the
ordinary Browser proxy. Set it in **Settings → Harness → Account pool** before
continuing OpenCode / Google sign-in; an empty field means Direct. The separate
**Service proxy** controls later model requests.
Use the robot button to the left of **Settings** to view the
[Harness dashboard](prd/PRD_02_36_harness_workspace.md). Configure accounts,
client allocations, usage monitoring and the local API in **Settings → Harness**.
Opening either view does not start the model API or run a task.
Complete real multi-account and external-client acceptance is still pending;
see the [experimental scope](prd/PRD_02_31_01_provider_gateway.md#0223-experimental-release-amendment-2026-10-26).

In 0.2.24, click inside a tab's inline rename field to place the caret without
closing the editor. Drag a tab row to reorder or reparent its branch; release
to apply, or press Escape to cancel. The status bar labels clipboard content
as text, image, mixed, empty or other when native formats are available, and
shows a character count for readable text.

---

MIT OR Apache-2.0. MiniCon™ is a trademark of PartnerNet Software.
