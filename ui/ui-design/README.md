# minicon UI 设计稿

`Terminal Styles.dc.html` — 用浏览器打开即可，画布可平移缩放。四轮内容自上而下：

- **Turn 4 工具条归位** — 头部 7 个工具减到 2 个（新建 + 设置），其余进设置面板；4a/4b/4c 三种放置方案与设置面板内容。
- **Turn 3 按钮清单** — 对照 `src/ui.rs` 的真实尺寸与命中区，标出状态样式与两处问题。
- **Turn 2 配色主题** — 2a 现状灰（`src/main.rs` 现值）/ 2b 官网蓝墨（`docs/index.html` 变量）/ 2c 浅色。
- **Turn 1 结构方案** — 1a 硬边框 / 1b 留白分区 / 1c 机箱框架，各含深浅两套。

## 实现时要注意的几点

1. 状态条：已确认要做。新增 24dip 的 status `Rect`，从 terminal 高度扣除；`Layout` 与 `TreeHit` 之外还需一个 `StatusHit`。
2. 头部工具 7 → 2：保留 `new_root`，新增 `settings`；`help` / `language_*` / `zoom_*` 六个 Rect 删除，功能移入设置面板（帮助并入面板底部快捷键区，`help_lines()` 直接复用）。窄侧栏溢出测试可随之删除。
3. 关闭键当前常驻每一行；设计稿改为悬停/选中才出现。
4. 空态文案是繁体（`準備開啟新終端` / `新建終端`），与其他中文串不一致。
5. 终端正文颜色仍由 `src/palette.rs` 的 xterm 256 表决定，主题只改外壳（tab 条、侧栏、输入区、边框）与状态色。

## 色值表

三套主题并存、运行时可切换，设置面板里用三个色块选择。终端正文不受主题影响。

### 2a Neutral Ink（= 现状）
`tree_bg #080808` · `active_bg #323232` · `rule #484848` · `text #F5F5F5` · `muted #C0C0C0 / #A8A8A8` · `canvas #000000` · `accent #FFFFFF` · `error #FF5C5C`

### 2b Docs Ink（= 官网同源）
`bg #070b10` · `chrome #0b1016` · `surface #111821` · `surface-2 #17212c` · `border #2b3948` · `text #eef4f8` · `muted #9baab8` · `green #7ee787` · `cyan #68d8f0` · `amber #f0c86b` · `error #f2776b`

### 2c Paper Ink（浅色）
`canvas #ffffff` · `chrome #f3f5f7` · `surface #e4e9ee` · `border #c7d0d9` · `text #0b1016` · `muted #5b6a77` · `green #2f7d43` · `cyan #1f6f86` · `amber #8a5c1f` · `error #b03a2e`
