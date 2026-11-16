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