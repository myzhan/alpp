# ALPP — Any Language PreProcessor

用注释内嵌的 `#ifdef` 风格指令，为没有原生条件编译的语言（Java/Python/Lua/Shell 等）提供 C 宏式的条件编译。指令本身是普通注释，作者需保证开发树及各宏组合裁剪结果的语法合法；发布构建时用 alpp 按宏定义裁剪代码块。

示例（Lua，剥离调试日志）：

```lua
--#ifdef TRACE_NET
local log = require("log.trace")
--#endif

local function on_packet(buf)
    --#if TRACE_NET && SLOW_MS > 0
    local t0 = os.clock()
    --#endif
    dispatch(buf)
    --#ifdef TRACE_NET
    log.info("packet %d bytes", #buf)
    --#endif
    --#if TRACE_NET && SLOW_MS > 0
    log.info("handled in %.3f ms", (os.clock() - t0) * 1000)
    --#endif
end
```

发布构建：

```bash
alpp -w -DTRACE_NET ./src
```

输出（`SLOW_MS` 未定义按 0，`SLOW_MS > 0` 为假，计时块被裁）：

```lua
local log = require("log.trace")

local function on_packet(buf)
    dispatch(buf)
    log.info("packet %d bytes", #buf)
end
```

## 安装

```bash
cargo install --git https://github.com/myzhan/alpp
```

或从 [Releases](https://github.com/myzhan/alpp/releases) 下载对应平台的二进制（Linux/macOS：x86_64、aarch64；Windows：x86_64）。

## 用法

```bash
alpp [OPTIONS] [PATH]...          # 默认 dry-run：输出 unified diff，不修改文件
alpp -w -DRELEASE ./src           # 按宏裁剪并写回
alpp -w -o out -DRELEASE ./src    # 写到镜像目录（含未处理的二进制等，原样复制）
alpp --check ./src                # 只校验指令合法性（CI 用）
alpp --gate ./dist                # 发布门禁：扫描文件中残留指令则退出 3
```

常用选项：`-D NAME[=VAL]` / `-U NAME` 定义与取消宏；`-P c_family|hash|lua` 强制 Profile；`--prefix` 自定义指令前缀；`-j N` 并行度；`--strict` 把可疑指令升级为错误。

退出码：`0` 成功；`1` IO/用法错误；`2` 指令校验失败（E1–E8）；`3` `--gate` 发现残留。

## 语法

九条指令，与 C 预处理器同形：`#ifdef` `#ifndef` `#if` `#elif` `#else` `#endif` `#define` `#undef` `#error`。前缀按语言注释形态确定（见下表），允许任意缩进。

表达式支持 `defined(X)`、数值比较（`> >= < <= == !=`）、逻辑运算（`&& || !`）与括号；未定义标识符按 `0` 求值。注意**没有** `and`/`or`/`not` 文字运算符。字符串字面量内的指令样文本自动豁免，不会被误删。

文件起始的 UTF-8 BOM 不影响首行指令识别；若首行为指令，BOM 随整行删除。`#else` 后不能再出现同层 `#elif`（E2）；括号与 `!` 的累计嵌套上限为 128 层（超限 E7）。

## 支持的语言

| Profile | 前缀 | 扩展名 |
|---|---|---|
| `c_family` | `//#` | java js jsx mjs cjs ts tsx kt kts dart php scala sol |
| `hash` | `##` | py pyi sh bash zsh rb toml |
| `lua` | `--#` | lua |

只收录没有原生条件编译的语言；C/C++/Rust/Go/Swift/C# 请用各自的预处理机制。

## 项目配置 `.alpp`

仓库根目录可放置 `.alpp`（TOML）约束宏用法：

```toml
[macros]
define = ["DEBUG", "LEVEL=1"]   # 默认宏，优先级低于命令行 -D/-U

[directives]
deny = ["if", "elif", "define", "undef", "error"]   # 限制可用的指令；deny 命中或 allow 名单外报 E8
# allow = ["ifdef", "ifndef", "else", "endif"]
```

被禁指令无论所在分支是否活跃都会报错退出。

## 实现要点

- Rust 实现，单二进制；除被裁剪区间外任何字节原样保留（`\r\n`/tab/BOM/多字节字符不归一化），写回走临时文件 + 原子 rename
- SIMD 行扫描（memchr，SSE2/AVX2/NEON）+ rayon 文件级并行；单线程吞吐 2.2–2.6 GiB/s（`cargo bench` 可复现）
- 在项目内使用相对输入路径时，dry-run 的 diff 保留原始正文并转义路径，可管道给 `git -c core.autocrlf=false -c core.eol=lf apply`；输出不随并行度变化

## 镜像与批量处理

`alpp -w -o out ./src` 将已处理文件和未处理资产写入镜像。CWD 内输入保留相对目录结构；外部目录映射内部结构，外部单文件映射文件名。输出目录自动排除，路径冲突或越界在写盘前失败；预检包含输出文件系统上的大小写等价和文件/目录冲突。写回保留源权限，使用唯一临时文件。

`--check`/`--gate` 不生成改写内容；批量处理按线程数分批释放结果。显式文件同样受二进制检测和大小上限约束，`--prefix` 支持目录内未知扩展名且不能为空。

`--gate` 共用文件过滤规则，只保证实际扫描文件无残留；隐藏/忽略文件、未知扩展名及二进制/超限文件仍可能未检查。可按需调整 Profile、前缀和大小上限，或显式指定文件；`--verbose` 显示二进制/超限跳过原因。

## 文档

- [宏规范](docs/MACRO_SPEC.md)：指令语法、表达式文法、错误码、与 cpp 的差异对照
- [设计文档](docs/DESIGN.md)：架构、并行模型、性能基准、里程碑
- [AI Agent 技能](skills/alpp-macros/SKILL.md)：宏语法速查与批量改代码工作流
- [AGENTS.md](AGENTS.md)：本仓库的维护规范

## License

[MIT](LICENSE)
