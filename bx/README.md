# bx — 面向 agent 的二进制脚本运行时
当前版本 **0.2.0** — 变更见 [CHANGELOG.md](CHANGELOG.md)。

单文件原生可执行程序，**1011 KB，零依赖**。它执行的是**编译成字节码的脚本**，而脚本的原语是**原生二进制**。

```
$ bx eval "exec(['git','--version']).out.trim()"
git version 2.42.0.windows.2
```

## 为什么不是 node

agent 每调用一次工具，就要为一个进程付一次启动税。这台机器上的实测（node 侧 `spawnSync`，30 次取中位数）：

| 被测目标 | min | median |
|---|---:|---:|
| 进程创建地板（`cmd /c exit`） | 21.6 ms | 23.4 ms |
| **bx version** | **16.2 ms** | **18.4 ms** |
| **bx eval 1+1** | **17.9 ms** | **20.3 ms** |
| node -e 1+1 | 137.4 ms | 148.4 ms |

bx 的自身启动已经低到**低于这台机器的进程创建地板**（16 ms 的测量值就是 spawn 成本本身，解释器自身 < 2 ms）；node 在同样条件下还要再加 ~120 ms。

递归 `fib(24)` 的端到端总时长，bx 134 ms，node 155 ms——**V8 的 JIT 再快，也追不回启动差距**。

## 构建

```powershell
cd bx
cargo build --release
# 产物：target\release\bx.exe
```

Windows 下可以用 `bx\bx.cmd` 转发调用，或把 `target\release` 加进 PATH。

## 快速开始

```powershell
bx run examples\hello.bxs
bx eval "1 + 2 * 3"
bx build examples\hello.bxs -o hello.bxb   # 编译成字节码
bx run hello.bxb                            # 直接执行二进制
bx dis hello.bxb                            # 反汇编
bx check script.bxs                        # 只编译不执行，给 agent 当闸门
bx check --json script.bxs                 # 同上，输出一行 {"ok","file","chunks"|"error"}
bx run tools/verify.bxs <bx 或 .com>       # bx 原生端到端自检（跨平台）
git log | bx run filter.bxs               # read("-") 读 stdin，脚本可当管道过滤器
bx run --timeout 5000 script.bxs           # 墙钟超时（毫秒），超时退出 1
bx run --max-steps 1000000 script.bxs      # 指令步数上限，防止死循环
bx help exec                                # 单个内建的契约
bx --version                                # 等价 bx version（-v/-V 亦可）
bx --help                                   # 等价 bx help
```

```js
// 把原生二进制当作原语：返回结构化结果，而不是一堆字符串
let r = exec(["git", "--version"])
if (!r.ok) { error("git failed: " + r.err) }
print(r.out.trim(), "in", r.ms, "ms")

// 数据在脚本里流动
let data = parse(read(__dir__ + "/sample.json"))
let names = map(filter(data.items, fn(it) { return it.score >= 5 }), fn(it) { return it.name })
print(join(names, ","))
```

## 语言

JS 风格，语句以换行或 `;` 结束。

| 能力 | 写法 |
|---|---|
| 变量 | `let x = 1` / `const y = 2`（顶层即全局） |
| 函数 | `fn add(a, b) { return a + b }`，匿名 `fn(x) { return x * 2 }` |
| 条件 | `if c { } else if c2 { } else { }`（括号可选） |
| 循环 | `while c { }`、`for (let i = 0; i < n; i++) { }`、`for x in xs { }`、`for (let x in xs) { }` |
| 迭代 | `for x in xs` 对数组给元素、对字符串给字符、对对象给键；`for k, v in xs` 给键值对（对象 key/value、数组与字符串 index/value） |
| 跳转 | `break` / `continue` / `return` |
| 数组 | `[1, 2]`、`a[0]`、`a[-1]`、`a.push(3)` |
| 对象 | `{name: "bx"}`、`o.name`、`o["k"]`、`o.n += 1` |
| 运算 | `+ - * / %`、`== != < <= > >=`、`&& || !`、`+= -= *= /= %=`、`++ --` |
| 条件表达式 | `c ? a : b`（右结合） |
| 字符串插值 | `"n=${x}"`、`"a${1 + 2}b"`；插值内写字符串用单引号或转义双引号 |
| 包含 | `use "lib.bxs"`（相对当前文件内联） |
| 内建变量 | `argv` 脚本参数、`script` 文件名、`__dir__` 脚本目录 |
| 注释 | `//` 与 `/* */` |

语义要点（有意与 JS 不同/收紧的地方）：

- **顶层 `let` 是全局**，函数内 `let` 是局部。只有**被嵌套函数用到的**局部与参数才装箱成 cell（其余保持普通槽位），嵌套函数**按引用捕获**这些名字：闭包写回外层变量、两个闭包共享一个状态都成立；循环变量每轮是新 cell，所以每轮创建的闭包各自保留当轮的值；嵌套任意深度都能读到、写回更外层函数的局部（捕获逐层转发）。
- `print(obj)` 输出紧凑 JSON；`json(v)` 显式序列化；对象键排序保证输出稳定（便于 diff 与断言）。
- `==` 对数组/对象做**结构相等**，int 与 float 数值互比；真值判断只有 `false / null / 0 / ""` 为假。
- `+` 在任一侧是字符串时做拼接，两个数组相加以拼接，其余走数值。
- 点号调用是**方法糖**：`s.split(",")` 等价 `split(s, ",")`，映射到同名内建。
- 字符串插值对单/双引号字符串都生效；要写字面量 `${` 用 `\$`（`"cost: \$5"`、`"literal \${x}"`），插值内部的表达式里写字符串用单引号即可。

### 错误处理

bx 没有 `try/catch` 语法——运行时错误就是一次栈展开。要兜底时用 `attempt`，失败变成值而不是进程退出：
运行时错误的位置带上**出错函数名**与**定义文件**：`<script>(3) in inner() [lib.bxs]: undefined variable 'x'`，闭包是 `in <lambda>()`；顶层错误不带函数段。上下文只拼一次，不会出现重复的 `location: location:` 前缀。

```js
let r = attempt(fn() { return 1 / 0 })
if (!r.ok) { print("failed:", r.error) }   // failed: <eval>(1): division by zero
```

## 内建

`bx help` 打印全部签名，`bx help <name>` 看单条契约。共 63 个，按能力分组：

**值** `print eprint len type str int float json parse keys values has get merge`
**数组** `push pop slice join sort reverse range sum min max map filter each reduce`
**字符串** `split trim upper lower replace contains index_of starts_with ends_with repeat`
**数字** `abs floor ceil round sqrt`
**进程** `exec(cmd, opts?) sh(cmd, opts?) which(name)`
**文件** `read write append exists list mkdir remove`
**环境** `cwd env now sleep exit error attempt assert basename dirname`

`exec` 是核心原语：

```js
exec("git status")                                  // 字符串按引号规则切分，不经过 shell
exec(["git", "status"], {cwd: "/repo", timeout: 5000})
exec("dir", {shell: true, stdin: "y\n", max: 1048576})
// -> {code: 0, ok: true, out: "...", err: "", ms: 21, timeout: false}

exec(["cargo", "build"], {stdout: "build.log", stderr: "build.err"})   // 大输出直接落文件
exec_all([["git", "status"], ["git", "diff"]], {cwd: "/repo"})
// -> [result, result]：顺序与输入一致，最多 8 个并发，单个失败不影响其他

let h = spawn(["git", "log", "--oneline"])
let first = spawn_poll(h.id)        // {running, code, out, err}，增量读取
let rest = spawn_wait(h.id)         // 等它结束并收完输出
spawn_write(h.id, "y\n"); spawn_close(h.id)   // stdin 写/关（EOF）
spawn_kill(h.id)                    // 终止并收集剩余输出
```

默认 120 s 超时、8 MB 输出上限，超时返回 `code: -1, timeout: true`，stdout/stderr 用独立线程排空，不会因管道写满而死锁。

## 可执行脚本

首行写 `#!/usr/bin/env bx` 即可在 Unix 上直接 `./script.bxs`：解析器把这一行抹成空格，行号不偏移，`bx check`/`build` 一视同仁。

## 结构化编辑

`patch(path, diff)` 应用 unified diff，是 agent 自迭代时改代码的原子原语：任一 hunk 上下文不匹配就报错，**文件保持原样**：

```js
let r = patch("src/lib.rs", read(__dir__ + "/fix.diff"))   // -> {ok, path, hunks, bytes}
```

## 性能基线

`bash bx/scripts/bench.sh` 用脚本内部的 `now()` 计时（不含进程启动），`--com <minicon-bx-*.com>` 可测同一组用例的打包产物。本机参考值：

| 用例 | 当前（0.2.0） | 第 3 轮（无闭包） |
|---|---:|---:|
| `fib(24)` | ~36 ms | ~98 ms |
| 1e6 次 while 累加 | ~0.22 s | ~1.1 s |
| 20 万次函数调用 | ~66 ms | ~254 ms |
| 20 万次 `map` 回调 | ~30 ms | — |

全局名解析成槽位下标并按 chunk 缓存，热路径不再查哈希表；全局读写也不 clone 名字。调用路径做过四次去分配：chunk 名/来源文件走 `Rc<String>`、帧槽用复用池、`CALL` 参数直接 `drain` 进帧槽、内建回调（`map`/`filter`/`reduce`/`sort`）不再为每个元素构造 `Vec`。`fib(24)` 从 ~137 ms 降到 ~56 ms，20 万次 `map` 从 ~46 ms 降到 ~29 ms；两边 `dis` 字节码逐条一致，继续优化需要 profiler。
## 二进制

`.bxb` 是带魔数 `BXB\x02` 的版本化容器：常量池 + 字节码 + 行号表（行程编码）+ 每个 chunk 的来源文件（v1 容器仍可读，来源文件未知）。

```powershell
bx build script.bxs          # -> script.bxb，可分发、跨机器执行
bx run script.bxb one two    # 参数进 argv
bx dis script.bxb
```

`bx run *.bxs` 也会把编译结果缓存到脚本旁边的 `.bxcache/`，下次直接加载字节码。键 = 主脚本 + **递归 `use` 依赖的文件内容** + 版本：改被 include 的文件会让缓存失效（这正是 v0.1 的一个真 bug，已修）。设 `BX_CACHE_DIR` 可改位置，写不进去就静默跳过缓存。

30 000 行生成脚本的实测：

| 路径 | 耗时 |
|---|---:|
| `bx build`（parse + compile + 写文件） | 196 ms |
| `bx run big.bxb`（解码 + 执行） | 70 ms |
| `bx run big.bxs`（无缓存） | 239 ms |

**字节码路径比源码路径快 3.4×**，脚本越大差距越明显——这正是“执行二进制”而不是“每次重新解析”的意义。

## 扩展点（agent 自我进化的入口）

1. **加内建**：`src/builtins.rs` 写一个 `fn b_xxx(vm: &mut Vm, args: &[Value]) -> Result<Value, String>`，再往 `BUILTINS` 表加一行（`sig` 会自动出现在 `bx help`）。不需要改解析器、编译器或 VM。
2. **加语法**：`src/parser.rs` 出 AST，`src/chunk.rs` 的 `compile_expr/compile_stmt` 出指令；新语义优先用已有指令组合（如 `++` 被编译成 `Compound`）。
3. **加指令**：`chunk::op` 加常量 + `op_name`，`vm.rs` 加一个 `match` 分支。指令带 4 字节操作数，`has_operand` 决定长度。

改完跑 `pwsh tests\run.ps1`——它只是启动器，**51 项用例都写在 [tests/suite.bxs](tests/suite.bxs) 里、由 bx 自己执行**。这是自举的入口：bx 跑不动自己的测试，就不该被当成工具。基准用 `pwsh tests\bench.ps1`。字节码格式变更必须同步升 `MAGIC` 的版本号，否则旧 `.bxb` 会按新布局误读。

## 限制（现在明确不做）

- 闭包按可见名字捕获（含逐层转发的更外层局部），所以闭包会带上创建点可见的变量数；没有 `try/catch`（用 `attempt`）、正则、模块导出（只有 `use` 内联）。
- 单线程；没有异步。阻塞内建就是阻塞。
- `.bxb` 是**可执行内容**，只运行可信产物（解码会校验魔数、操作码与行号表，但不校验跳转目标）。
- 资源上限：调用深度 220，步数 `BX_MAX_STEPS`（默认 10 亿），`exec` 输出 8 MB。

详细的架构、字节码布局、指令表与取舍记录见 [DESIGN.md](DESIGN.md)。

## minicon 集成

bx 以独立 workspace 住在 `bx/`（`Cargo.toml` 里的空 `[workspace]`），不进入
MiniCon 的 workspace members，终端二进制里也没有 bx 代码。

```bash
bash bx/scripts/test.sh      # 宿主构建 + 51 项自测
bash bx/scripts/build-cells.sh   # 出每平台产物（宿主感知）
```

`build-cells.sh` 能做什么取决于宿主：

| 宿主 | 可出 |
|---|---|
| Windows | win-x86_64（原生）、win-aarch64（cargo-xwin + lld-link）、lnx-x86_64 / lnx-aarch64（rust-lld 自包含 musl） |
| macOS | osx-*（原生）、lnx-*（cargo-zigbuild）、win-*（cargo-xwin） |
| Linux | lnx-*（原生）、win-*（cargo-xwin） |

产物落在 `loader/dist/rust/minicon-bx-<cell>.com`；壳与容器协议见
[`loader/bx/README.md`](../../loader/bx/README.md)。

CI：`minicon-com.yml` 的 build job 跑同一脚本并写 `manifest.json`，六个原生
runner 各执行自己那份（`version`、`eval "6*7"`=42、未知命令非零），receipt 记
`bx_lane`，aggregate 要求六格齐全。