# minicon-bx.com — Rust 自解压壳 + bx cell

一个文件、没有 cosmocc、没有 C：Rust 编译的 launcher 加尾部容器，容器里是
Rust 编译的 bx。启动时挑当前 `{os}-{isa}` 的 cell，落到可写目录执行，然后把
子进程的退出码原样返回。

## 组成

| 路径 | 作用 |
|---|---|
| `loader/com-rs/` | 独立 Cargo 工程（不属于 MiniCon workspace）：`com-trampoline` = 壳，`com-pack` = 打包器 |
| `loader/pack-rust.ps1` | 通用打包入口（MiniCon 与 bx 共用）：按 cell 目录产出 `<name>-<platform>.com` |
| `loader/bx/pack-bx.ps1` | bx 专用：复用已发布 `minicon.com` 的 cosmocc trampoline（旧路线，仅留作对照） |
| `loader/bx/dist/minicon-bx.com` | 交付物，1,036,360 B |

`pack-bx.ps1` 是上一条路线（借壳）的遗留；正式路线是 `pack-rust.ps1`。

## 容器格式（`loader/com-rs/src/lib.rs`）

```text
[ launcher ][ cell blobs ... ][ table ][ footer ]
                                        └ 24B: "MINICOM1" | table_off u64 | table_len u64
table entry = key[32] NUL-padded | offset u64 | length u64
```

偏移是文件绝对偏移；EOF 24 字节定表位。追加数据对 PE/ELF/Mach-O 都合法。

## 打包

```powershell
# Windows cell
pwsh -File loader/pack-rust.ps1 -Cells <dir> -Platforms win-x86_64 -OutDir loader/bx/dist

# Linux cell（rust-lld 自包含交叉，不需要 WSL）
pwsh -File loader/pack-rust.ps1 -Cells <dir> -Platforms lnx-x86_64 -OutDir loader/bx/dist
```

bx 的 cell 目录约定与 MiniCon 相同：`cells/<os-isa>/minicon[.exe]`。
bx 自己的构建：

```powershell
cargo build --release --target x86_64-unknown-linux-musl   # 需 RUSTFLAGS=-C linker=rust-lld -C link-self-contained=yes
```

## 产物与覆盖矩阵

每格一份，Rust 壳 + 同格 cell，`com-pack verify` 全部通过（8/8）：

| 产物 | 壳 | cell 字节 | 总字节 | 本机实跑 |
|---|---|---:|---:|---|
| `minicon-win-x86_64.com` | win-x86_64 | 5,299,200 | 5,510,216 | `--version`/`--status` exit 0 |
| `minicon-win-aarch64.com` | win-aarch64 | 4,992,512 | 5,185,608 | 否（需 ARM64 主机） |
| `minicon-lnx-x86_64.com` | lnx-x86_64 | 7,326,688 | 7,756,264 | 否（CI ubuntu runner） |
| `minicon-lnx-aarch64.com` | lnx-aarch64 | 6,438,816 | 6,821,480 | 否（CI ubuntu-arm runner） |
| `minicon-bx-win-x86_64.com` | win-x86_64 | 825,344 | 1,036,360 | bx 0.1.0 / 42 / exit 3 |
| `minicon-bx-win-aarch64.com` | win-aarch64 | 717,824 | 910,920 | 否（需 ARM64 主机） |
| `minicon-bx-lnx-x86_64.com` | lnx-x86_64 | 935,600 | 1,365,176 | 否（CI ubuntu runner） |
| `minicon-bx-lnx-aarch64.com` | lnx-aarch64 | 811,296 | 1,193,960 | 否（CI ubuntu-arm runner） |

macOS 两格本机无法出壳（无 Apple SDK），由 CI 的 macos-15 builder 出。

本机（Windows x86_64）出 ARM64 的具体路径，供复现：

- Windows ARM64：`cargo xwin` 的 CRT + 把 `rust-lld.exe` 复制成 `lld-link.exe`，
  `RUSTFLAGS=-C linker=<...>/lld-link.exe`，并把 `CARGO_TARGET_DIR` 放到 C: 盘
  （D: 上该输出路径曾被拒写）。
- Linux 两格：`RUSTFLAGS="-C linker=rust-lld -C link-self-contained=yes"` +
  `--target {x86_64,aarch64}-unknown-linux-musl`，不需要 WSL/zig。
## 验证（本机实测）

```
selftest.ps1                                  -> PASS（15 项：结构/表/架构/执行/退出码/捕获/无残留）
minicon-bx.com version                        -> bx 0.1.0
exit(0|1|3|42)                                -> 0 | 1 | 3 | 42      （精确，无 <<8 偏移）
bx exec(["./minicon-bx.com","eval","6*7"])     -> {"code":0,"out":"42\n"}（管道可捕获）
BX_BIN=minicon-bx.com + tests/suite.bxs       -> PASS 51 checks（5.9 s）
拷贝到别处 + 换 cwd 运行                       -> 42（路径无关）
4 个并发实例                                   -> 全部 42（按 pid 命名，无碰撞）
MINICON_COM_EXTRACT=... MINICON_COM_VERBOSE=1 -> 报告实际落盘路径，跑完无残留
```

同一机制打包真实 MiniCon（`win-x86_64` cell，5,299,200 B）：

```
minicon-win-x86_64.com --version -> minicon 0.2.23          exit 0
minicon-win-x86_64.com --status  -> pty backend rust-console exit 0
```

## 与 cosmocc trampoline 的差别

| | cosmocc trampoline | Rust launcher |
|---|---|---|
| 构建依赖 | cosmocc + C 编译器 | 只要 rustc |
| 一份文件跑六平台 | 是（polyglot） | 否：每个平台一份 |
| 非零退出码 | `(128+c)<<8`（缺陷） | 原样透传 |
| 父进程管道捕获 | 丢失（Windows） | 正常（stdio 继承） |
| 体积 | 714 KB 壳 + cells | ~211 KB 壳 + cells |

要“一个文件六个系统”，壳必须是 polyglot：要么继续用 cosmocc 编，要么走
`systems/ape` 的 `APE!` loader。Rust 路线的定位是**每平台一个单文件**，但可
复现、无 cosmocc、行为与原生 cell 一致。

## 沙箱注意

容器里的 cell 要落到可写目录。顺序：`MINICON_COM_EXTRACT` → 系统临时目录 →
launcher 所在目录 → 当前目录。在只允许写工作区的沙箱里，前三个可能被拒，最后
一个生效；`selftest.ps1` 会在当前目录检查没有残留。
