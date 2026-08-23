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

## 构建

```bash
cargo build --release        # 产物 target/release/minicon
cargo test                   # 全量(含黑盒/吞吐门)
```

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