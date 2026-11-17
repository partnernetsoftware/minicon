# bx changelog

## 0.2.0 — 2026-11-16

**语言**

- 条件表达式 `c ? a : b`（右结合，优先级低于 `||`、高于赋值）。
- 字符串插值 `"n=${x}"`，`\$` 转义字面美元。
- 闭包：任意嵌套深度、共享可变；只有被闭包引用的局部装箱成 cell，循环变量每轮独立。
- 键值迭代 `for k, v in xs`：对象给 key/value，数组与字符串给 index/value。
- 首行 `#!/usr/bin/env bx` 可执行脚本，行号不偏移。

**诊断**

- 运行时错误带函数名与定义文件，跨 `use` 与 `.bxb` 往返保留。
- include 内的编译错误指向被 include 的文件；缺文件报引用者；嵌套不叠前缀。
- `bx check [--json]`：只编译不执行，成功/失败都输出一行机器可读 JSON。

**运行时与 CLI**

- `--max-steps` / `--timeout` 执行预算；脚本之后的参数仍进 `argv`。
- `read("-")` 读 stdin 到 EOF，脚本可当管道过滤器。
- `exec_all`：最多 8 个并发，结果按输入顺序，单个失败隔离。
- `exec(..., {stdout, stderr})`：大输出直接落文件，绕开 8 MB 捕获上限。
- `spawn` 家族：后台进程 + `spawn_poll`/`spawn_wait`/`spawn_write`/`spawn_close`/`spawn_kill`。
- `test(name, fn)` + `bx test <file>`：语言原生测试运行器（汇总与退出码）。
- `patch(path, diff)`：原子应用 unified diff，上下文不匹配时不写文件。

**实验性 wasm 目标**

- 记录合并：`merge(a,b)` 在编译期合成 union shape（a 键序 + b 新键，重叠键取 b 值），支持链式；空对象字面量仍不支持。`n- 记录内建：`keys`（字符串数组）/`values`（整型数组）/`has`/`get(record,key[,default])`，运行时按 shape 键表比较；缺省键无 default 时中止（VM 返回 null）。`n- 捕获闭包补齐：`map/filter/reduce/each` 回调可捕获外层局部（回调 env 求值一次后传入）；全部可迭代对象（整型数组、字符串数组、记录、map）的循环变量均可被捕获（循环前分配一个 cell、每轮刷新，符合 VM 单共享绑定语义）。仍限制：同一变量持有不同字面量站点的函数值、记录数组。`n- 捕获闭包：函数字面量新增 env 指针参数，被捕获的外层局部/参数装箱为堆 cell（共享可变状态，计数器语义与 VM 一致）；捕获分析在字面量体检查前运行、支持嵌套传递，并驱动推断 pass B。限制：循环变量捕获、内建高阶回调捕获暂不支持（明确报错）。`n- 高阶内建扩展到字符串数组：`map` 回调返回 Str 时结果为字符串数组（Int 时整型数组），`filter` 保持元素类型；软推断能区分字符串数组字面量、遍历 `let` 初始化与 for-in 迭代器、推断回调内 `.len()`/混合 `+`。`n- 高阶数组内建：`map/filter/reduce` 与语句级 `each`，接受非捕获函数字面量，emitter 内联循环并直接调用提升后的字面量函数。`n- 整型数组内建：`range/sum/reverse/sort/slice`（reverse/sort 与 VM 一样原地修改并返回同一数组）；差分生成器加入数组内建覆盖。`n- wasm 模块缓存：`bx run --wasm source.bxs` 把发射出的模块缓存为脚本旁 `.bxcache/<hash>.wasm`；键含版本、发射器 tag、源码与递归 `use` 依赖，损坏/过期条目自动重编，`BX_CACHE_DIR` 可改目录。301 行脚本实测冷 52 ms → 热 33 ms（中位数，含进程启动）。- CLI：`bx run --wasm script.bxs` 可强制 wasm 目标（优先于 `BX_TARGET`），不必再设环境变量；单测覆盖。同时记录 wasm 目标 vs VM 的基准（fib 134→223 ms、loop 332→243 ms、calls 166→260 ms，含进程启动），并在 PRD_02_42 的 Engine performance 一节说明取舍。- 归并排序差分覆盖：集合差分生成器新增随机大小/随机值的比较器排序用例（int 正反序、重复键、字符串按长度、浮点、记录数组、稳定并列），10 个种子 × 50 例全部通过；PRD 记录稳定性（`<= 0` 取左段）与"`sort(records)` 无比较器"的显式拒绝/VM 静默 no-op 差异。- wasm 比较器排序改为**稳定自底向上归并排序**（客机实现，共享一个发射器覆盖 int/str/float/记录/嵌套数组：8 字节槽原始拷贝 + 按类型转换回调参数），把比较器排序从 O(n²) 回调降到 O(n log n)；500 记录基准 VM 95 ms / wasm 103 ms（原 94/178）。同时修复"三元表达式在函数体内引用参数"时发射器仍调用 checker 静态推断导致的 `undefined variable` 错误（改用发射器自身类型推导）。单测 106、suite 208、verifier 86。- 文档准确性（PRD 42）：核对并更正多处过期说法——字符串分支三元已支持、`exec`/`sh` 已实现、`pop` 表达式返回元素、`contains`/`index_of` 覆盖记录/行数组、空记录可打印、`merge({})` 可用、记录数组已支持、浮点数组的 `map`/`filter`/`reduce` 与比较器 `sort` 可用；并把"旧解释器"相关措辞改为"内置 tinyvm 核"（历史性能段保留并标注）。本轮仅文档修正。- 推断修复（真修）：两轮不动点推理现在让顶层语句共享同一份 locals（原先每条语句重新 clone，`let` 推出的类型对后续语句不可见），且临时 int 猜测不再覆盖后续更具体类型；`let ordered = sort(rows, cmp)` 之后再 `map`/`reduce` 不再报"can read fields of objects only, got int"。新增单测/套件用例（105/206）。PRD 中原"已知限制"改为已修复说明。- 基准扩容与文档更新：`bench.sh --wasm` 新增集合重负载（2000 元素 map/filter/reduce/sort，VM 109 / wasm 104 ms）与记录重负载（500 记录 map/比较器 sort/reduce，VM 94 / wasm 178 ms）两个用例；PRD 42 的 Engine performance 表更新为统一一次运行的 5 行数据并说明机器噪声；同时修复集合比较器/映射推断中"临时 int 猜测覆盖后续具体类型"的缺陷（部分链式 map/sort 场景），剩余推断边角已在 PRD 记为已知限制。- 文档准确性：把 README 与 PRD 42 中"wasm 解释器全面快于 VM"的旧说法更新为当前 tinyvm 核实测（BX_BENCH_RUNS=9：fib 130/225 ms、loop 337/244 ms、calls 167/258 ms，含进程启动），旧的解释器性能段落标注为历史记录并说明两处修复的缘由。- 集合差分工具体扩容：随机生成字符串表达式（trim/upper/lower/replace/repeat/slice/拼接/split-join/比较/`"${var}"` 插值）并与数组/记录字面量、map 回调组合；8 个种子 × 40 例全 PASS。同时修复软推断缺口（`trim/upper/lower/replace/repeat` 未按字符串类型推断，导致 `map([lower(s)], fn(x){ return len(x) })` 报错）。单测 104、suite 204、verifier 86。- wasm 字符串拼接修复：运行时 `+` 的每个操作数改用独立局部对，修复"字符串操作数是记录字段时互相覆盖临时指针/长度"导致插值 `"v=${o.a}-${o.b}"` 输出 `xx` 的缺陷；记录字段仍仅支持标量（数组/记录值字段显式拒绝，VM 允许——已记录）。单测 104、suite 204、verifier 86。- wasm 记录动态键：`get(record, key)` 在记录的**所有字段同类型**时接受非字面量键（客机内查找，返回该字段类型；缺失时用默认值或中止），异构记录仍需字面量键；`has(record, key)` 一直支持动态键。单测 103、suite 202、verifier 86。- wasm 结构化相等：`==`/`!=` 对记录与数组按值比较（记录逐字段、数组先比长度再逐元素，支持嵌套数组）；`contains`/`index_of` 扩展到记录数组、整型行与文本行；记录 `get(record, "key")` 改为字面量键（字段静态形状，动态键显式拒绝）并返回字段真实类型，`has(record, key)` 支持动态键。集合差分与 suite 均加入对照用例；单测 102、suite 200、verifier 86。- wasm `exec`/`sh`：返回 VM 形状记录 `{code, err, ms, ok, out, timeout}`（键序一致，打印可对比），`exec` 默认不经 shell（字符串命令按空白切分，未复刻 VM 引号规则）、`sh` 走平台 shell；支持 `cwd`/`stdin`/`timeout`/`shell` 选项，其余选项显式拒绝；程序缺失按 VM 消息中止（wasm 无 `file(line):` 前缀，已记录）；`ms` 为墙钟时间不可逐字节比较。新增 `bx.exec_record` 宿主。同时补齐字符串方法形式 `trim/upper/lower/split/slice/replace/repeat/index_of`。单测 101、suite 198、verifier 86。- wasm 记录字段类型化：记录字段支持 int/str/float/bool（每字段仍占 8 字节槽，字符串为 ptr+len），新增 `print_record_typed`/`print_arr_obj_typed` 宿主（kinds 每字段 1 字节），字段读写、记录数组、map/filter/each/比较器 sort、闭包捕获均按真实类型工作；`for k,v in record` 仍限整型字段，嵌套记录显式拒绝。集合差分开出 typed-record 模板，6 种子 ×40 全 PASS；单测 100、suite 196、verifier 86。- wasm 文件/查找别名：`read(path)`/`write(path, data)` 与 VM 同名（`read("-")` 读 stdin 到 EOF），新增 `which(name)`（缺点时返回空串，VM 返回 null；`write` 成功返回 0，VM 返回 null——均为已记录差异）。`exec`/`sh` 仍缺（其结果记录含 int/str/bool 混合字段，固定形状记录暂不支持），已在 PRD 记明。单测 99、suite 192、verifier 86。- wasm 集合内建与转换补齐：`push` 可用于表达式（返回数组）、`pop(arr)` 返回被移除元素（整型/浮点/字符串/文本行/整型行/记录数组；空数组中止，VM 返回 null 已记录）、`contains(arr, v)` 支持整型/字符串/浮点数组；`float()` 接受整数与字符串、`int()` 接受布尔，无法解析的数字字符串按 VM 措辞中止。集合差分新增这些用例（6 种子 ×40 全 PASS）；单测 98、suite 192、verifier 85。- wasm 退出码与断言：新增 `exit(code)`（CLI 以该码退出，保留之前 stdout）、`error(msg)` 与 `assert(cond, msg?)`（stderr 输出消息、退出码 1；VM 会带 `file(line):` 前缀，wasm 宿主无源码位置，故文本仅为消息本身——已记录的差异）；语句位与表达式位都支持，浏览器演示改为抛 JS 错误。审计确认其余错误路径（越界读 null、`% 0`、预算文案）为既有且已记录的差异。单测 97、suite 191、verifier 84。- wasm 控制流修复：`break` 在 `for` 各形态（整型/浮点/字符串/行/记录/字符、双变量、嵌套 if）里此前被当成 `continue`（分支到 continue 块），现改为跳出循环自身的退出块；字符串分支三元（`cond ? "a" : "b"`、嵌套）改为双臂产出 `(ptr,len)`。集合差分新增 break/continue/三元用例，6 种子 ×50 全 PASS；单测 96、suite 187、verifier 83。- 新增集合差分工具 `bx/tools/wasm-diff-coll.bxs`：随机生成整型/浮点/字符串数组、文本行、整型行、记录数组及 map/filter/each/sort/reverse/slice/reduce/push、嵌套/负索引、元素写、双变量遍历、闭包捕获等程序，VM 与 wasm 逐字节对比；6 个种子 ×50 例全 PASS，suite 加入 12 例、计数 186。差分工具同时确认上轮 Unicode 修复无回归。- wasm 字符串 Unicode 语义：`len(str)` 按字符计数（新增 `str_len` 宿主），`str_slice`/`str_index_of` 由字节偏移改为字符语义（与 JS 胶水一致），并新增 `for ch in "text"` / `for i, ch in "text"` 字符遍历；同时修复 `[["a"], []]` 这类“空行 + 文本行”被误判为整型行数组。差分工具新增 seed 参数与 unicode/空容器/负索引边界用例，5 个种子各 100 例全部通过。单测 95、suite 185、verifier 82。- wasm 文本行：字符串数组的数组（`[[\"a\",\"b\"],[\"c\"]]`）支持类型、索引（含 `rows[1][0]`）、遍历、`len`、`push`、元素写、`map`/`filter`/`each`、比较器 `sort`、`reverse`/`slice`、闭包捕获与 VM 风格打印（新宿主 `print_arr_strarr` 与演示胶水）。单测 94、suite 184、verifier 81、差分 300。- wasm 嵌套数组与通用 `reduce`：整型数组的数组可参与 `map`/`filter`/`each`/比较器 `sort`/`reverse`/`slice`（行是值槽；回调返回数组则得到新的行数组）；`reduce` 支持任意数组元素种类与数字/字符串累加器（`(acc, elem) -> acc`，可带种子或首元素起步，空数组带种子返回种子）。单测 91、suite 183、verifier 79、差分 300。- wasm 记录数组集合：`map` 可返回整型/浮点/字符串/记录数组，`filter` 保持记录形状，`each` 遍历记录，比较器 `sort` 支持整型/浮点/字符串/记录数组，`reverse`/`slice` 接受记录数组。同时修复函数字面量临时局部变量编号未计入捕获环境参数的真实 bug（闭包内构建数组/记录字面量会覆盖自身参数，`map(xs, fn(r){ return {b: r.a + 1} })` 曾读错字段）。单测 90、suite 182、verifier 78、差分 300。- wasm 数组内建按元素种类分发：默认 `sort`（整型/浮点/字符串数组）、原地 `reverse`（各类数组）、`slice`（整型/浮点/字符串数组）、`join`（1 或 2 参，整型/浮点数组自动转字符串）；并允许丢弃返回值的语句式调用（`reverse(a)`、`sort(a)`）与 VM 对齐。新增宿主 `arr_sort_f/arr_sort_str/arr_join_int/arr_join_f` 及演示胶水。单测 89、suite 181、verifier 77、差分 300。- 单引擎：删除自研 wasm 解码器/解释器与其单元测试，`bx/src/wasm.rs` 只保留编码器与原生宿主；`BX_WASM_OWN` 与遗留 `allow(dead_code)` 一并移除。`bx check --json` 改由轻量 section 扫描器给出 imports/exports/functions。单测 88（原 91 中的解释器测试随引擎删除），suite 180、verifier 76、差分 300 全绿。- 引擎：`.wasm` 现在由内置自有 tinyvm 核解码/校验/执行（`bx run`、`bx check` 默认走核；`BX_WASM_OWN=1` 保留一版旧解释器供差分）。非法模块带函数位置报出（`validation: type mismatch in function #N`），`--max-steps`/`--timeout` 映射到核的步数/中断上限；宿主实现仍单一（适配器复用既有 PrintHost 臂）。`.com` 约 +243 KB。- 修复：整型数组 push 的发射代码把值放到 i64 局部后又经 i32 局部写入（i32/i64 存储不匹配）；bx 自带解释器容忍该非法模块，tinyvm 校验器（P0 实验）当场报 `validation: type mismatch`，现已改为 i64 直传。`n- 浮点数组：`Ty::ArrF`（混合数字字面量提升），支持打印、读写、len、单/双变量遍历、push、sum/min/max、index_of、contains；map/filter/reduce 回调与比较器 sort 仍拒绝。`n- 数组搜索与极值：`index_of`（整型/字符串数组，函数式与方法式，缺失 -1）、数组 `.contains`、`min`/`max`（整型数组→int，字符串数组→字典序字符串）；通过宿主 import 实现，无 guest 循环。空数组 min/max 中止（VM 返回 null）。`n- map 打印与内建：`print(map)` 输出 `{"k":v,...}`（插入序）；`keys(map)`/`values(map)`/`has(map,key)`/`get(map,key[,default])`/`len(map)` 可用（wasm 扩展，VM 无 map）。浏览器胶水按 JS map 表格式化。`n- 数组套数组：`[[1,2],[3,4]]` 类型 `ArrArr`，条目存内层数组指针；支持 len、多层下标读、元素写、单/双变量遍历、push、负索引、行作为函数参数与打印。更深的嵌套与混合元素暂不支持。`n- 容器打印：`print` 支持整型数组/字符串数组/记录/记录数组，输出与 VM 完全一致的紧凑 JSON 风格；通过 4 个宿主 import 读取内存格式化（记录额外传键表）。map/函数仍不可打印，`console_log` 仅标量。`n- `push/pop` 覆盖全部数组类型：方法式与函数式均可，整型/字符串/记录数组按元素类型写入；空记录 `{}` 合法（`merge({}, {...})`、`keys({})`、`type({})` 可用，print 记录仍拒绝）。`n- `sort` 字符串数组比较器：比较器 `(str, str) -> int`，插入排序交换 ptr/len 对；推断新增 sort 比较器统一、Return 表达式遍历、soft(sort) 返回输入类型。`n- `sort` 自定义比较器：`sort(arr, fn(a,b){ return a-b })` 在 wasm 内做原地插入排序并调用字面量比较器（含捕获环境）；比较器返回整数、符号决定顺序；数组原地修改并返回。字符串数组比较器暂不支持。`n- 记录数组：`[{a:1},{a:2}]`（同 shape）类型为 `ArrObj`，条目存记录指针；支持 len、读写（含负索引）、`for r in xs`/`for i, r in xs`、作为函数参数；嵌套在数组字面量中的记录对象用独立局部保存数组指针。`n- 字符串数组：全字符串字面量类型推断、元素读写（负索引）、len、for-in、`split/join` 与字符串/整型数组 `slice`（负边界按 VM 规则 clamp）；浏览器胶水同一内存布局。`n- 内建对齐：`float/abs/floor/ceil/round/sqrt/trim/upper/lower/replace/index_of/repeat/type`（round 采用 VM 的 half-away 语义；字符串经 host `env.str_*`）。`n- 字符串插值与混合拼接（`"x=${e} end"`、`1 + "x"`）对齐 VM；浮点取负（f64.neg 0x9a，解释器支持 0x99-0x9f）；记录字段/数组元素写入与复合赋值（负索引同样生效）。`n- 性能：while/for 退出条件直接用反向比较指令（省去每轮 eqz），解释器无 deadline 时跳过超时位测试；bench 更新为 fib ~0.83×、loop ~0.58×、calls ~0.78× VM 耗时。
- argv 支持空格/Unicode、越界返回空串；capture 返回未 trim 的原始 stdout，结果区不足时报错而非静默空串。
- IO/映射失败显式化：read_file 缺失文件报 cannot read，write_file 返回 0/-1（签名扩展），映射满/未初始化/分配失败均报错而非静默。
- 索引对齐 VM：负索引（从末尾计）、浮点索引（截断）、字符串按 Unicode 字符取字符（host `env.str_char_at`）。`
- 修复 bool 值比较（`true == false`、`false < true`）在条件里缺失的类型错误；修复空 needle 字符串方法（`"".contains("")`）导致宿主 panic 的问题。`
- 运行时边界：数组索引越界、整数/浮点除零、取模零会以明确消息中止（`index out of bounds` / `division by zero` / `modulo by zero`），避免此前内存越界读/NaN 静默错误；VM 的 null 语义按已知分歧记录。`
- 修复作用域循环变量出现在调用实参里（如 `k + ":" + str(v)`）时的「undefined variable」构建错误：emitter 的 type_of 自行解析内建调用返回类型。`
- 修复 `continue` 在 for-in/for 中跳过步进的问题：循环体包一层 continue 目标块，索引自增与 for-step 始终执行。`
- 记录遍历 `for k, v in o`（声明序键表）与字符串方法 `.contains/.starts_with/.ends_with`（host `env.str_test`）；差分生成器限制乘法量级以避开 VM 的 9e15 浮点提升区间。`
- G1 目标选择：`bx build --target wasm|bxb` 与 `BX_TARGET=wasm|bxb`（默认仍为 bxb）；非法值退出 2，`BX_TARGET=wasm bx run 脚本` 走 wasm 且失败时提示回退。`
- `exists(path)` 返回 bool（与 VM 一致）；`file_exists(path)` 保留为 int 扩展。`
- bool 类型：`true`/`false`、比较作为表达式、`&&`/`||`/`!`（短路）、三元、`str(bool)`/`print(bool)`；条件接受 bool 变量。`
- 非捕获函数字面量：`let f = fn(x) { .. }`、直接调用、作为实参传给函数；捕获外层局部变量明确报错。`
- 动态键映射（wasm 侧扩展，VM 暂无）：`map_new`/`map_set`/`map_get`/`map_has`/`map_len`，字符串键 + int 值，固定 64 槽，支持 `for k, v in m` 按插入序遍历；浏览器胶水用 JS Map。`
- 原生工具：`now()`/`sleep(ms)`/`cwd()`/`env(name)`/`mkdir(path)`/`file_exists(path)`（浏览器胶水提供 stub）。`
- `str(x)`/`int(x)` 数值字符串互转（格式与 VM 一致）、`.len()` 与语句级 `.pop()`；`for v in` / `for k, v in` 数组遍历（索引/值局部变量按块作用域）；差分测试加入 for-in。`
- 数组可变：`arr.push(x)`（分配-复制-写回，容量按需增长）；差分测试加入 push。`
- 字符串比较（`== != < <= > >=`）经 host `str_compare` + i32 比较实现，与 VM 一致；差分测试加入字符串比较。`
- 函数参数/返回支持 int/float/string/array/record（调用点推断；字符串按 ptr/len 成对传递，字符串返回用 wasm 多值结果；int 实参可提升为 float）。`
- wasm 执行预算：`bx run --max-steps/--timeout *.wasm` 与原生 VM 同款保护（指令计数 + 周期截止检查）。`
- `webgl.*` 最小切片：`gl_create`/`gl_clear_color`/`gl_clear`/`gl_pixel`/`gl_png`（原生确定性软件帧缓冲；内建无依赖 PNG 编码，demo 胶水提供同语义 JS 实现）。`
- `web.*` 命名空间：`console_log`（int/float/string）与 `performance_now()`，原生 host、demo 胶水与 node 一致。`
- `bx build --wasm --html` 生成自包含浏览器 demo（base64 模块 + 浏览器/node 双端胶水，node 可直接运行）。`
- wasm 解释器性能：函数帧独立标签基址（修复“块内调用后跳过块外代码”的 bug）、调用走 locals 池且结果留在操作数栈；fib/loop/calls 均快于原生 VM（`bench.sh --wasm`）。`
- `bx check <module> --json` 输出 wasm 模块 imports/exports/函数数；差分测试加入用户函数与递归；函数内可用字符串/数组/记录/浮点局部变量（每个函数独立临时槽）。`
- 浮点类型：浮点字面量/全局、`/` 浮点除法、混合提升、`env.print_f64` 与 VM 一致的格式化、`env.fmod`；修正解释器比较/位运算 opcode 映射（`rem`、`le`、`ge` 等与 wasm 规范、node 一致）；函数可读写字符串/数组/记录全局。

- `bx build --wasm` 输出标准 WebAssembly 模块，`bx run *.wasm` 用自研解释器执行；零第三方 wasm 依赖。
- 覆盖数值子集、字符串（字面量、编译期 `+`）、整型数组（`[..]`、`arr[i]`、`len`）、运行期字符串拼接（`memory.copy`）、定长记录对象（`{a:1}`、`o.a`）、控制流（`if/else`、`while`、`for`、`break`/`continue`）、用户函数（参数、int 局部、返回、递归）、argv 访问（`bx.argc`/`bx.arg`）、文件读写（`bx.read_file`/`bx.write_file`）与原生进程（`bx.run`/`bx.capture`）；分配改用模块内 bump 堆——字符串在线性内存数据段里，数组走原型 `env.alloc` 堆；host ABI 见 PRD 42。

**打包与测试**

- 容器升级到 `BXB\x02`（每个 chunk 记录来源文件，v1 仍可读）。
- `bx/tools/verify.bxs`：16 项跨平台端到端验证，CI 每个 runner 用自己的产物跑。
- 自托管套件 119 项；15 项 Rust 单元测试（容器编解码、随机容器往返/截断、patch、JSON、VM 比较）；`.com` 产物级 selftest 31 项。

**性能**

- 调用帧与全局访问去分配：`fib(24)` ~137 → ~36 ms，1e6 循环 ~1.14 → ~0.22 s。

## 0.1.0

字节码 VM、`exec`/`sh` 原生进程原语、自托管测试套件、`minicon-bx-<cell>.com` 打包。