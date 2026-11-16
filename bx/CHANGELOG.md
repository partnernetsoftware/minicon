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

- `bx build --wasm` 输出标准 WebAssembly 模块，`bx run *.wasm` 用自研解释器执行；零第三方 wasm 依赖。
- 覆盖数值子集、字符串（字面量、编译期 `+`）、整型数组（`[..]`、`arr[i]`、`len`）、运行期字符串拼接（`memory.copy`）、定长记录对象（`{a:1}`、`o.a`）、控制流（`if/else`、`while`、`for`、`break`/`continue`）与用户函数（参数、int 局部、返回、递归）；分配改用模块内 bump 堆——字符串在线性内存数据段里，数组走原型 `env.alloc` 堆；host ABI 见 PRD 42。

**打包与测试**

- 容器升级到 `BXB\x02`（每个 chunk 记录来源文件，v1 仍可读）。
- `bx/tools/verify.bxs`：16 项跨平台端到端验证，CI 每个 runner 用自己的产物跑。
- 自托管套件 119 项；15 项 Rust 单元测试（容器编解码、随机容器往返/截断、patch、JSON、VM 比较）；`.com` 产物级 selftest 31 项。

**性能**

- 调用帧与全局访问去分配：`fib(24)` ~137 → ~36 ms，1e6 循环 ~1.14 → ~0.22 s。

## 0.1.0

字节码 VM、`exec`/`sh` 原生进程原语、自托管测试套件、`minicon-bx-<cell>.com` 打包。