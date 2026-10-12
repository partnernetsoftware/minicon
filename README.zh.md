[English](README.en.md) · **简体中文**

# MiniCon™

`一个文件 · 免安装 · 删掉不留痕`

**一个文件就是整个终端。** 拷一个文件就能跑——一个面向 Windows、macOS、Linux
的快速终端，带独立的输入行，还有一个脚本能读取的控制命令。免安装、免捆绑运行
时、删掉不留痕。

![MiniCon 窗口：标签树、终端，以及底部独立的输入行。](docs/assets/minicon-neutral.png)

| | |
| --- | --- |
| 支持 | Windows、Linux、macOS × x86_64、aarch64（Windows 最低到 Server 2016） |
| 体积 | 单文件：Windows 约 660 KB、macOS 约 1.4 MB、Linux 约 5 MB——实测 |
| 内存 | 单个空闲标签页的宿主内存（实测）：Linux 约 12 MB、Windows 约 21 MB——我们守着的预算是 10 MB |
| 依赖 | 仅操作系统库 |
| 许可 | MIT 或 Apache-2.0 |

## 输入有自己的一行。

多数终端让你在「程序正在输出的那块区域」里打字，于是一条命令和一屏输出搅成一
团。MiniCon 给输入单独一行。

- **一边滚动一边写。** 你敲的字不会和程序打印的内容搅在一起。
- **发出去的命令能翻回来。** 上下键走一遍发过的内容，翻历史时敲到一半的字会被
  保留。
- **它写着去向。** 标签上标明文字发往哪个标签页，在标签树里你始终知道命令去哪。

`Enter` 换行，`Ctrl+O` 发送草稿。

## 脚本能「看进去」。

发一个按键很简单，但那是「瞎发」。控制命令能把窗口实际显示的内容读回来——文字、
截图、焦点、标签页——让自动化等一个真实结果，而不是靠 sleep 瞎猜。全程本地，
无服务、无网络。

```
# 用本地控制端点开一个会话
minicon --control unix:/tmp/minicon-build/control.sock
# Windows：minicon --control pipe:\\.\pipe\build

# 跑点东西，然后等它 —— 不用 sleep
minicon cli --control unix:/tmp/minicon-build/control.sock send-text "cargo test\r"
minicon cli --control unix:/tmp/minicon-build/control.sock wait-text --timeout-ms 600000 "test result"

# 结果可以读成文字，也可以截成图
minicon cli --control unix:/tmp/minicon-build/control.sock capture-pane
minicon cli --control unix:/tmp/minicon-build/control.sock screenshot-pane --output run.png
```

`ui-snapshot` 以 JSON 返回焦点、输入法状态、待处理工作与布局；`wait-text` 和
`wait-tab-exit` 阻塞到条件成立或超时；`send-keys`、`send-mouse`、`send-wheel`、
`send-ui-ime` 敲键、点击、滚动和中日韩组字，走的都是真人输入的同一条路径。
→ [docs/control-cli.html](docs/control-cli.html)

## 迷你、开源、已签名——都是刻意为之。

- **真的迷你。** Windows 约 660 KB、macOS 约 1.4 MB、Linux 约 5 MB——实测，非估
  算。免安装，不捆绑运行时。
- **永久开源。** MIT 或 Apache-2.0。无遥测、无账号、无自动更新——源码就是产品。
- **已签名并公证。** Windows 与 macOS，主体 PARTNERNET SOFTWARE PTY LTD。只用
  系统自带的库。
- **极省资源。** 只有一个进程，没有后台服务；关窗后什么都不再运行。低内存是目标，不是
  副产品：单个空闲标签页的宿主内存预算为 10 MB，实测 Linux 约 12 MB、Windows 约
  21 MB。（macOS 的下限更高，来自系统自身的窗口栈。）

三款内置主题——Neutral、Docs Ink、Paper Ink——窗框与终端一起换色，`Ctrl+Shift+P`
循环。

## 旧版 Windows 上照样跑。

新一点的终端需要较新的 Windows 版本，在老机器上根本起不来。MiniCon 在运行期解析
伪终端，没有就退回隐藏控制台，因此一路支持到 Windows Server 2016。
→ [docs/old-windows.html](docs/old-windows.html)

## 安装

从 [Releases](https://github.com/partnernetsoftware/minicon/releases) 下载对应平台
的可执行文件，直接运行。每个压缩包都附一份 SHA-256；macOS 的 `.dmg` 是已签名、已
公证的通用二进制。

```bash
sha256sum -c minicon-<version>-linux-x86_64.tar.gz.sha256
```

仅 macOS，把 `minicon` 放进 `PATH`：
`/Applications/MiniCon.app/Contents/MacOS/minicon install-cli`。

## 使用

```
minicon                          # 一个终端，用你的默认 shell
minicon -e cmd.exe /k build.bat  # 跑一条命令，而不是开 shell
minicon --status                 # 这台机器给了它什么
minicon --help                   # 其余全部
```

| 按键 | |
| --- | --- |
| `Ctrl+Shift+T` | 新建终端 |
| `Ctrl+Shift+N` | 在当前标签页下方新建终端 |
| `Ctrl+Shift+W` | 关闭当前终端；子节点会被提升 |
| `Ctrl+Shift+B` | 把标签侧栏收成细条，或再展开 |
| `Ctrl+Shift+[` / `]` | 切换标签页 |
| `Ctrl+Shift+I` | 聚焦输入区 |
| `Ctrl+Shift+P` | 循环切换配色主题（Neutral / Docs / Paper） |
| `Ctrl+Shift+,` | 打开或关闭设置面板 |
| `Ctrl+Shift+G` | 开关网格准星 |
| `Enter` | 在输入区插入软换行，不发送 |
| `Ctrl+O` | 发送输入区的完整草稿 |
| `Up` / `Down` | 在输入区内逐行移动；在首行或末行则回叫之前发过的内容 |
| `Alt+Up` / `Down` | 在输入区任意行回叫之前发过的内容 |
| `Ctrl+Left` / `Right` | 在输入区按整词移动光标 |
| `Ctrl+Home` / `End` | 在输入区把光标移到草稿开头或结尾 |
| `Ctrl+Backspace` / `Delete` | 在输入区删除光标前或后的一个词 |

## 配置

用户配置目录下可选的 `minicon.json`：`font_size`、`cols`、`rows`。

**Windows 鼠标：** 对多数程序开箱即用。MiniCon 默认用经典控制台宿主，因为
Windows 自带的 ConPTY 会把一次点击变成游离按键，打扰读控制台事件的程序（大多数
agent CLI）；`--feature conpty` 可切到它，供 Vim 类程序使用。

## 构建

```bash
./scripts/build.sh release     # target/release/minicon
./scripts/build.sh test        # 单元 + GUI 黑盒 + 各道门禁
./scripts/six-cell-qualify.sh  # 六种 OS/ISA 二进制
```

Rust 1.97。Linux 需要 `libxkbcommon.so.0` 和 `libwayland-client.so.0`（无需
`-dev` 包）。见 [PRD.md](PRD.md) 与 `plan/` 下的笔记。

## 仓库结构

| | |
| --- | --- |
| `src/` | 二进制本体：终端、宿主界面、控制协议 |
| `crates/minicon-core/` | 与宿主无关的逻辑——没有平台或架构的 `cfg` |
| `PRD.md` | 产品树与各模块 PRD |
| `docs/` | 位于 `minicon.agenterm.work` 的网站 |

## 更多

- **AgenTerm。** [AgenTerm](https://github.com/partnernetsoftware/agenterm) 是同一
  平台层上的 Agent 时代工作台；MiniCon 是它的一文件本机终端。MiniCon 源自 AgenTerm
  的控制台宿主，2026 年 8 月独立成产品。
- **代码签名。** 自 v0.1.9 起，每个公开发布都双重签名——Windows Authenticode 与
  macOS Developer ID + 公证，主体为 PARTNERNET SOFTWARE PTY LTD，并带 RFC 3161
  时间戳。用 `scripts/inspect-authenticode.ps1` 验证 Windows 下载。细节见
  [CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md)。

---

MIT 或 Apache-2.0。MiniCon™ 是 PartnerNet Software 的商标。
