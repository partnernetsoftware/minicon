# MiniCon

**A terminal that is one file.** No installer, no runtime, no Visual C++
redistributable — every library it loads is part of Windows. It opens a real
PTY, renders a real terminal, and a script can drive and read every part of it.

![MiniCon](docs/assets/minicon-window.png)

| | |
| --- | --- |
| Executable | current six-cell `strip=true` measurements: Windows arm64 677,376 bytes / x64 731,136; macOS arm64 1,413,408 / x64 1,455,424; Linux arm64 4,846,400 / x64 5,732,544. They are target-qualified evidence, not a universal product limit; see [Delivery](prd/PRD_02_27_con_delivery.md) for measurement context. |
| Dependencies | operating-system libraries only |
| Supported | Windows Server 2016 / Windows 10 1607 and newer; Linux; macOS |
| Licence | MIT OR Apache-2.0 |

## Why it exists

Most small terminals are small by leaving things out. MiniCon is small by not
adding a runtime: it is Rust with platform-native rendering, one executable,
and nothing installed. Deleting the file removes it completely.

Three things it does that terminals this size usually do not:

**It runs where modern terminals cannot.** Windows gained a pseudoconsole in
build 17763 (version 1809), and terminals built on it stop there. Microsoft's
ConPTY redistributable does not lower that floor — it supports the same version
as the in-box API. MiniCon resolves the pseudoconsole at run time and falls
back to hosting the shell in a hidden console, so it runs on Windows Server
2016. See [docs/old-windows.html](docs/old-windows.html).

**Input has its own area.** Most terminals make you type into the same region a
running program is printing into, so a long command and a burst of output
interleave and you lose track of what you typed. MiniCon gives input its own
band — and once input has its own area it can do more: Up and Down recall lines
you already sent, and the label names the tab the text is going to.

**A script can see into it.** The control CLI drives the real UI and reads back
what the window is actually showing — text, screenshots, focus, tab state — so
automation can wait on a condition instead of guessing with a timer. See
[docs/control-cli.html](docs/control-cli.html).

## Install

Download the executable for your platform from
[Releases](https://github.com/partnernetsoftware/minicon/releases), and run it.
There is no installer because there is nothing to install. Each archive ships a
SHA-256 beside it:

```bash
sha256sum -c minicon-0.1.2-linux-x86_64.tar.gz.sha256
```

The macOS build is a universal binary — the same file runs on Apple Silicon and
Intel.

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
| `Ctrl+Shift+[` / `]` | switch tabs |
| `Ctrl+Shift+I` | focus the input area |
| `Up` / `Down` | in the input area, recall what you sent before |

`A-` / `A+` in the header change the font size, `中` / `En` the interface
language.

### Reporting a problem

Run `minicon --status` and include its output. It reports the build, the PTY
backend this machine selected and why, the font face the system actually
resolved to with its measured cell width, and where MiniCon writes when
something fails. None of that can be answered by reading the source, because
all of it depends on the machine.

## Build

```bash
./scripts/build.sh release  # target/release/minicon; bounded local cache lifecycle
./scripts/build.sh test     # unit, GUI black-box, and gates
./scripts/six-cell-qualify.sh # one Mac: link all six cells, run available tests
```

The six-cell gate writes `target-six/receipt.json`. A missing runtime runner is
reported as `BLOCKED`; a successful cross-link is not mislabeled as a runtime
test pass. On Apple Silicon, `scripts/setup-linux-runners.sh` provisions the
Debian/glibc Lima courts used for both Linux architectures. A Windows UTM VM
with Guest Tools can use `scripts/windows-utm-runner.sh` as both Windows runner
variables; the gate transfers the exact PE/test tree before execution.

`scripts/cleanup-build-state.py` is dry-run by default. Build/package owners
invoke it with narrow scopes and protect current, receipt-owned and active
state. On macOS, `scripts/install-macos-daily-cleanup.sh` installs a per-user
LaunchAgent for 03:17 daily maintenance. VM images and cloud bodies without an
explicit verified archive receipt are never automatically deleted.

Rust 1.97. The Linux build needs `libxkbcommon0 libxkbcommon-x11-0
libwayland-client0`.

## Repository

| | |
| --- | --- |
| `src/` | the binary: terminal, chrome, control protocol |
| `crates/minicon-core/` | host-neutral logic — no platform, OS, or `cfg` on architecture |
| [`PRD.md`](PRD.md) | product tree, decision map, and links to owning module PRDs |
| `docs/` | the website at `minicon.agenterm.work` |
| `tests/` | black-box journeys and the gates below |

Two gates are worth knowing about, because each one exists where a claim
would otherwise be unchecked:

- **Import table** — the shipped executable must depend on nothing but
  operating-system modules. The exception list is empty, so a future
  redistributable dependency turns it red rather than quietly widening what a
  user has to install.
- **Alignment** — the public CLI must match the machine-readable contract in
  `alignment-contract.json`, which is in turn pinned to the PRDs.

## Relationship to AgenTerm

MiniCon began as [AgenTerm](https://github.com/partnernetsoftware/agenterm)'s
minimal console host and became its own product in August 2026. It shares
AgenTerm's platform layer and its input-area design; it deliberately has no
server, workspace persistence, multiplexer, or script runtime.

---

# MiniCon（中文）

**一个文件就是一个终端。** 免安装、免运行时、免 VC++ 可再发行组件——它载入的每一个
库都是操作系统自带的。它开真正的 PTY、画真正的终端,而且每一部分都能被脚本驱动和读取。

- 当前六格 `strip=true` 实测：Windows arm64 677,376 bytes / x64 731,136；macOS arm64 1,413,408 / x64 1,455,424；Linux arm64 4,846,400 / x64 5,732,544。它们是按目标验证的证据，不是全产品统一上限；测量语境见[交付](prd/PRD_02_27_con_delivery.md)。
- 只依赖操作系统库
- 支持 Windows Server 2016 / Windows 10 1607 及以上、Linux、macOS
- MIT 或 Apache-2.0

三件同体量终端通常做不到的事:

**能在现代终端跑不动的地方跑。** Windows 到 build 17763(1809)才有伪终端,建立在它
之上的终端就停在那里;微软的 ConPTY 可再发行包并不能往下兼容,它支持的版本和系统内建
的一样。MiniCon 在运行期解析伪终端,没有就退回到把 shell 放进隐藏控制台。

**输入有自己的区域。** 多数终端要你在"程序正在输出的同一块区域"里打字,一长串指令和
一波输出交错,你会弄不清自己打了什么。MiniCon 给输入一块自己的地方——而输入一旦有了
自己的区域,就能做更多:上下键回叫送出过的内容,标签上写着文字要送往哪个分页。

**脚本能看进去。** 控制 CLI 驱动的是真正的界面,并把窗口实际显示的内容读回来——文字、
截图、焦点、分页状态——让自动化可以等待条件成立,而不是用计时器猜。

出问题时请运行 `minicon --status` 并附上输出:它报告构建版本、这台机器选了哪个 PTY
后端及原因、系统实际解析到哪个字体与实测字宽,以及失败写在哪个文件。这些都无法通过读
源码回答,因为它们取决于机器。
