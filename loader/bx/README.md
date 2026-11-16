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

## 本轮评审改进

| 发现 | 处理 |
|---|---|
| 打包非原子，中途失败留下半成品 | 写 `.tmp` 后 `rename` 发布 |
| 容器无法自证来源 | 新增 `__meta__` 表项 + `com-pack info`：`product` / `cell` / `version` / `source_sha` |
| Unix 上 payload 落在共享临时目录，同机其他用户可替换 | 改为私有 `minicon-com-<pid>/` 目录（0700），payload 在其中 |
| spawn 失败会遗留 payload | 失败路径同样清理文件与目录 |
| 打包器不校验 launcher | 校验 PE / ELF / Mach-O 魔数，不可执行直接拒绝 |
| GUI 产品会闪一个控制台窗口 | 新增 `--subsystem gui|console`（仅 PE）；默认不改，因为 `--version` 的输出捕获依赖控制台句柄 |
| 额外进程开销此前未量化 | 见下方「开销」 |
| 读表不校验越界，损坏文件会读越界而不是报错 | `read_table` 校验表尾/条目边界，截断文件直接拒绝 |
| 被 kill 的 launcher 会留下解包文件 | 启动时清扫 24h 前的 `minicon-com-*`；按小时节流，避免每次扫 `%TEMP%` 反而更慢 |
| 排障只能靠二进制内容 | 新增 `com-pack inspect`：launcher 格式/子系统、cell、metadata 一次看完 |

`minicon-com.yml` 的 bx lane 在六个 runner 上都会跑它：每个 runner 用自己的 `minicon-bx-<cell>.com` 执行 `bx/tools/verify.bxs`，所以跨平台行为由真实执行证明，而不是靠编译成功推断。
## bx 原生验证

`bx/tools/verify.bxs` 用 bx 自己跑端到端检查（CLI、`check`/`dis`、预算、stdin、`patch`、`spawn`、`--com-info`、`.bxb` 往返、include 归属、闭包/键值迭代冒烟），任何 OS 都能跑，产物还能自检自己：

```bash
bx run tools/verify.bxs <bx 或 minicon-bx-*.com>
minicon-bx-linux-x86_64.com run tools/verify.bxs minicon-bx-linux-x86_64.com
```
## `--com-info`

打包产物自描述：`minicon-bx.com --com-info` 由 launcher 直接输出容器里的 metadata（product / cell / version / source_sha），不解包、不执行 payload，exit 0。只有**第一个参数**会被拦截，脚本自己的 `--com-info` 仍会进 `argv`。
## 产物与覆盖矩阵

| 产物 | 壳 | cell 字节 | 总字节 | 实跑 |
|---|---|---:|---:|---|
| `minicon-bx-win-x86_64.com` | win-x86_64 | 825,344 | 1,050,350 | bx 0.2.0 / 42 / exit 3 / 套件 108 项 |
| `minicon-bx-win-aarch64.com` | win-aarch64 | 717,824 | 922,863 | 需 ARM64 主机 |
| `minicon-bx-lnx-x86_64.com` | lnx-x86_64 | 935,600 | 1,380,510 | 需 Linux（CI） |
| `minicon-bx-lnx-aarch64.com` | lnx-aarch64 | 811,296 | 1,207,815 | 需 Linux ARM（CI） |
| `minicon-win-x86_64.com` | win-x86_64 | 5,299,200 | 5,511,404 | `--version`/`--status` exit 0 |
| `minicon-win-aarch64.com` | win-aarch64 | 4,992,512 | 5,185,773 | 需 ARM64 主机 |
| `minicon-lnx-x86_64.com` | lnx-x86_64 | 7,326,688 | 7,761,004 | 需 Linux（CI） |
| `minicon-lnx-aarch64.com` | lnx-aarch64 | 6,438,816 | 6,825,293 | 需 Linux ARM（CI） |

`com-pack verify` 逐 cell 校验可执行头与 key 一致，`__meta__` 只校验可读；
`pack` 结束前自动跑一次，不匹配直接失败。

## 开销（Windows x86_64，20 次交错测量）

| 路径 | min | p50 | p90 |
|---|---:|---:|---:|
| 原生 cell（`bx.exe`） | 17.4 ms | 20.0 ms | 28.3 ms |
| 打包产物（`minicon-bx-…com`） | 43.8 ms | 48.3 ms | 57.4 ms |

实测拆解：launcher 自身（打包一个非本机 cell，因找不到 host cell 而直接报错）约
**21 ms**，与原生 cell 同量级；余下约 27 ms 是**第二次进程创建 + 825 KB 落盘**。
这是 trampoline 模型的固定成本，不是实现缺陷：`.com` 用于分发；同一台机器上反复
调用的 agent 应直接调用原生 cell。Linux 上 `execv` 可以省掉这次 fork，但那样进程
被替换、临时文件无法自清理，故未采用；解包缓存只省落盘那几毫秒，却留下文件，也
不做。

## 验证（本机实测）

```
selftest.ps1                                   -> PASS 22 项
com-pack info …\minicon-bx-win-x86_64.com      -> {"product":"minicon-bx","cell":"win-x86_64","version":"0.2.0","source_sha":…}
BX_BIN=minicon-bx-win-x86_64.com bx run bx/tests/suite.bxs -> PASS 51 checks
exit(0|1|3|42)                                 -> 0 | 1 | 3 | 42（精确透传）
bx exec(["./minicon-bx-win-x86_64.com", …])    -> {"code":0,"out":"42\n"}
拷贝到别处 + 换 cwd / 4 并发 / MINICON_COM_EXTRACT + VERBOSE -> 均通过，无残留
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
