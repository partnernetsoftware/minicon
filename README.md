# minicon — 独立微终端

> 对标 PuTTY 路线的轻量本机终端（conhost 风格）。源于 `agenterm-con`（agenterm 的最小
> console host），2026-08 独立成仓。**产品独立，代码共享**：terminal-core 拆出后，
> agenterm 反向复用本仓核心。

## 定位

- 拥有终端窗口、渲染 cell 到像素面、把键盘输入转发给 PTY 里的 shell
- 窗口内 tab 树；**刻意不做** workspace 持久化、Fleet、mux、server、脚本运行时
- 设计优先级：**稳定性**。resize 尾沿去抖、PTY 读线程独立（不阻塞渲染路径）、
  VT 解析复用产品终端已硬化的同一 parser

## 结构（两阶段演进）

```
阶段 1（当前）: minicon ──git依赖──> agenterm(锁 revision, platform/ui-core)
阶段 2（目标）: terminal-core(pty/vt100/raster/font/ime) 下沉本仓
              agenterm ──git依赖──> minicon 的 core(复用)
```

## 产品文档

PRD 根：[`prd/PRD_02_23_minicon.md`](prd/PRD_02_23_minicon.md)（终端渲染 / workspace 输入 /
控制协议 CLI / 打包交付四个子模块）。机器契约：[`alignment-contract.json`](alignment-contract.json)
与 [`evidence-registry.json`](evidence-registry.json)，由 `cargo test --test minicon_alignment` 强制。

## 构建

```bash
cargo build --release        # 产物 target/release/minicon
cargo test                   # 全量(含黑盒/吞吐门)
```

## 已知状态（2026-08-23）

- 依赖 agenterm 的 `agenterm-platform` / `agenterm-ui-core`，以及 agenterm 内 vendored 的
  `vt100` / `softbuffer` fork，全部按同一 revision 钉死。vt100 fork 不是可选项：产品用到
  `take_damage` / `ScreenDamage` / `RowRange` / cursor shape+blink / `scrollback_len`，
  上游 0.16 都没有——行级 VT damage 是渲染路径的整个设计。
- `cargo test`：130 单元 + alignment 契约 + 21 条公共黑盒中 20 条通过。
  `controlled_resize_storm_reports_successful_frames_and_exits_cleanly` 在本机 macOS 红
  （36 帧 > 非 Windows 分支的 24 帧预算）；同一条测试在 agenterm 的 `agenterm-con` 上
  **同样红、同样 36 帧**，是既有的本机预算问题，与迁出无关。
- 体积门与独立 CI 未随迁出带过来，见 [交付子模块](prd/PRD_02_27_con_delivery.md)。

## 本仓规则

- 沿用 agenterm 纪律：测试优先、文档脱敏（路径只写 `~/...` 或仓内相对）、
  证据注册 `evidence-registry.json`、`alignment-contract.json` 机器对齐
- 不引脚本运行时(rh/qjs/tinyvm)——稳定最小壳，插件以后是可选 feature 且默认关
- 与 agenterm 的关系：阶段 1 单向 git 依赖；阶段 2 后 agenterm 依赖本仓 core，
  本仓不再反向依赖 agenterm

## 状态

- [x] 阶段 1 骨架（本 README / Cargo.toml / 迁移说明）
- [ ] 阶段 1 代码迁入（src/ 17.7k 行 + tests/ 176K）+ agenterm 留指路
- [ ] 阶段 1 独立编译/测试绿
- [ ] 阶段 2 terminal-core 拆分（pty/vt100/raster/font/ime）+ agenterm 反向依赖