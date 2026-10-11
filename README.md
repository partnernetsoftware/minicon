# MiniCon

**A terminal that is one file.** One native executable per platform — no
installer, no bundled runtime, only the operating system's own libraries. It
opens a real PTY and renders a real terminal on Windows, Linux and macOS, and
every part of it can be driven and read by a script.

![MiniCon](docs/assets/minicon-neutral.png)

| | |
| --- | --- |
| Runs on | Windows, Linux, macOS — x86_64 and aarch64 (Windows back to Server 2016) |
| Size | one file: ~0.7 MB (Windows), ~1.4 MB (macOS), ~4.5 MB (Linux) |
| Depends on | operating-system libraries only |
| Licence | MIT OR Apache-2.0 |

## Why it exists

Most small terminals are small by leaving things out. MiniCon is small by not
adding a runtime: Rust, platform-native rendering, one file. Delete the file and
it is gone. Three things it does that terminals this size usually do not:

- **It runs where modern terminals cannot.** Windows only gained a pseudoconsole
  in build 17763, and terminals built on it stop there — Microsoft's ConPTY
  redistributable does not lower that floor either. MiniCon resolves the
  pseudoconsole at run time and falls back to a hidden console, so it runs on
  Windows Server 2016. → [docs/old-windows.html](docs/old-windows.html)
- **Input has its own area.** Most terminals make you type into the region a
  program is printing into, so a command and a burst of output interleave. The
  input band is separate — and once it is, Up/Down recall what you sent before.
- **A script can see into it.** The control CLI drives the real UI and reads
  back what the window is actually showing — text, screenshots, focus, tab
  state — so automation waits on a condition instead of guessing with a timer.
  → [docs/control-cli.html](docs/control-cli.html)

## Install

Download the executable for your platform from
[Releases](https://github.com/partnernetsoftware/minicon/releases) and run it.
Every archive ships a SHA-256 beside it; the macOS `.dmg` is a signed,
notarized universal binary.

```bash
sha256sum -c minicon-<version>-linux-x86_64.tar.gz.sha256
```

macOS only: to put `minicon` on `PATH` (the bundle keeps it off by design):
`/Applications/MiniCon.app/Contents/MacOS/minicon install-cli`.

## Use

```
minicon                          # a terminal, with your default shell
minicon -e cmd.exe /k build.bat  # run one command instead of a shell
minicon --status                 # what this machine gave it
minicon --help                   # everything else
```

| Key | |
| --- | --- |
| `Ctrl+Shift+T` | new terminal |
| `Ctrl+Shift+N` | new terminal below the active tab |
| `Ctrl+Shift+W` | close the active terminal; children are promoted |
| `Ctrl+Shift+B` | collapse the tab sidebar to a rail, or expand it again |
| `Ctrl+Shift+[` / `]` | switch tabs |
| `Ctrl+Shift+I` | focus the input area |
| `Ctrl+Shift+P` | cycle the color theme (Neutral / Docs / Paper) |
| `Ctrl+Shift+,` | open or close the settings panel |
| `Ctrl+Shift+G` | toggle the grid crosshair |
| `Enter` | in the input area, insert a soft newline without sending |
| `Ctrl+O` | send the complete input-area draft |
| `Up` / `Down` | in the input area, move between lines; at the first or last line, recall what you sent before |
| `Alt+Up` / `Down` | in the input area, recall what you sent before from any line |
| `Ctrl+Left` / `Right` | in the input area, move the caret by whole words |
| `Ctrl+Home` / `End` | in the input area, move the caret to the start or end of the draft |
| `Ctrl+Backspace` / `Delete` | in the input area, delete the word before or after the caret |

## Configuration

Optional `minicon.json` in your user config directory; every key is optional:
`font_size`, `cols`, `rows`.

**Windows mouse:** works out of the box for the programs most people run.
MiniCon uses the classic console host by default because the ConPTY copy
Windows ships turns a click into stray keystrokes for console-event readers
(most agent CLIs). `--feature conpty` selects it anyway, which is what
Vim-style programs want:

```
minicon                    # default: clicks reach agent CLIs
minicon --feature conpty   # instead: clicks reach Vim-style programs
```

## Build

```bash
./scripts/build.sh release     # target/release/minicon
./scripts/build.sh test        # unit + GUI black-box + gates
./scripts/six-cell-qualify.sh  # all six OS/ISA binaries
```

Rust 1.97. Linux needs the desktop runtime libraries `libxkbcommon.so.0` and
`libwayland-client.so.0` (no `-dev` packages). See
[PRD.md](PRD.md) and the `plan/` notes for the full build, packaging and
qualification process.

## Repository

| | |
| --- | --- |
| `src/` | the binary: terminal, host UI, control protocol |
| `crates/minicon-core/` | host-neutral logic — no platform, OS, or architecture `cfg` |
| `PRD.md` | product tree, decision map, links to owning module PRDs |
| `docs/` | the website at `minicon.agenterm.work` |

## Relationship to AgenTerm

[AgenTerm](https://github.com/partnernetsoftware/agenterm) is the Agent-era
workbench on the same platform layer. MiniCon is the one-file **local
terminal**; AgenTerm adds the longer-lived server identity, Fleet, cross-instance
multiplexing, persistence and agent permission policy. `minicon mux` speaks
`tmux`'s verbs against the tabs of **one** running instance, and `minicon
harness` hands a model one narrowly scoped local task — both live and die with
the window, exactly like `--control`. MiniCon began as AgenTerm's minimal
console host and became its own product in August 2026.

## Code signing

Since v0.1.9 every public release is dual-signed — Windows Authenticode and
macOS Developer ID + notarization, as PARTNERNET SOFTWARE PTY LTD, with an
RFC 3161 timestamp. Verify a Windows download with
`scripts/inspect-authenticode.ps1`. Details in
[CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md).

---

# MiniCon（中文）

**一个文件就是一个终端。** 每个平台一个原生可执行文件，免安装、免捆绑运行时，只依赖
操作系统库。它在 Windows、Linux、macOS 上开真正的 PTY、画真正的终端，每一部分都能被
脚本驱动和读取。

| | |
| --- | --- |
| 支持 | Windows、Linux、macOS × x86_64、aarch64（Windows 最低到 Server 2016） |
| 体积 | 单文件：Windows ~0.7 MB、macOS ~1.4 MB、Linux ~4.5 MB |
| 依赖 | 仅操作系统库 |
| 许可 | MIT 或 Apache-2.0 |

它的核心就三点：

- **能在现代终端跑不动的地方跑。** Windows 到 build 17763 才有伪终端，基于它的终端就
  停在那里；MiniCon 运行期解析伪终端，没有就退回隐藏控制台，因此支持 Server 2016。
- **输入有自己的区域。** 不再和程序输出挤在同一块地方；输入独立成条，上下键还能回叫
  送出过的内容。
- **脚本能看进去。** 控制 CLI 驱动真正的界面并把窗口实际显示读回来，自动化可以等条件
  而不是靠定时器猜。

安装：从 [Releases](https://github.com/partnernetsoftware/minicon/releases) 下载对应平台
的可执行文件直接运行即可。Windows 鼠标开箱即用；默认用经典控制台宿主，`--feature conpty`
可切到 ConPTY（Vim 类程序需要）。构建：`./scripts/build.sh release`。
AgenTerm 关系、签名政策与更多细节见上文英文部分与 [PRD.md](PRD.md)。
