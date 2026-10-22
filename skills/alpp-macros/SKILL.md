---
name: alpp-macros
description: Write conditional-compilation directives embedded in source comments per the alpp spec (any language preprocessor) so debug/logging/variant code can be stripped at release builds. Self-contained reference covering the directive set, expression grammar, per-language examples for Java/TS/Python/Shell/Lua, error codes, and CLI usage for batch code edits. Use when writing or modifying code containing ALPP directives (//#, ##, --#), when adding strippable debug/logging blocks, preparing multi-mode build variants, or when the user mentions alpp, 条件编译, 宏指令, or trimming debug code.
---

# ALPP 宏语法与批量改代码指南

alpp（any language preprocessor）读取**宿主语言行注释内**的 cpp 风格指令，在发布构建时按宏开关整行裁剪代码。指令本身是普通注释；作者仍需验证开发树和各宏组合裁剪结果的宿主语法合法性。

对应宏规范 v1.1。本文件自包含——写或改含 alpp 指令的代码时，只看本文件即可。

## 1. 按语言选前缀（必须正确）

| Profile | 前缀 | 语言 |
|---|---|---|
| `c_family` | `//#` | Java/JS/TS/Kotlin/Dart/PHP/Scala/Solidity |
| `hash` | `##` | Python/Ruby/Shell/TOML |
| `lua` | `--#` | Lua |

- 按文件扩展名自动选择；CLI 可用 `-P <profile>` 或 `--prefix <p>` 覆盖。
- **只收录没有原生条件编译的语言**：C/C++（cpp 宏）、Rust（cfg/feature）、Go（build tag）、Swift/C#（`#if`）等有原生机制的语言不支持；HTML/XML 等标记性语言不支持。
- hash 语言**必须双井号**：单 `#` 是普通注释，不会生效。

## 2. 指令集

| 指令 | 参数 | 语义 |
|---|---|---|
| `#ifdef NAME` | 宏名 | NAME 已定义 → 保留本块 |
| `#ifndef NAME` | 宏名 | NAME 未定义 → 保留本块 |
| `#if EXPR` | 表达式 | EXPR 非 0 → 保留本块 |
| `#elif EXPR` | 表达式 | 前面分支都未保留且 EXPR 为真 → 保留（每层至多生效一次） |
| `#else` | 无 | 前面都未保留 → 保留 |
| `#endif` | 无 | 关闭最近一层（必须配平） |
| `#define NAME [VALUE]` | 宏名 [值] | 文件内定义宏；无值 = 1；整数 = 数值；其他 = 字符串（按 0） |
| `#undef NAME` | 宏名 | 取消定义 |
| `#error MSG` | 任意文本 | 处于**活跃**分支 → 文件级致命错误（退出码 2） |

## 3. 表达式文法

```ebnf
or      ::= and { "||" and }
and     ::= eq  { "&&" eq }
eq      ::= rel { ("==" | "!=") rel }
rel     ::= unary { ("<" | ">" | "<=" | ">=") unary }
unary   ::= "!" unary | "(" expr ")" | "defined" "(" NAME ")" | NAME | 整数
```

- 优先级从低到高：`||` < `&&` < `==`/`!=` < `< > <= >=` < `!` < 括号；短路求值。
- **没有** `and`/`or`/`not` 文字运算符（Python 风格写法会报表达式错误 E7）。
- 未定义标识符按 `0`；`defined(X)` 已定义时为真；无值宏 = `1`；整数字面量十进制；比较为 i64 语义。
- 括号与一元 `!` 累计嵌套最多 128 层，求值时超限报 E7。
- 宏不展开：`-DA=5` 只用于条件求值，**不会**把保留行里的 `A` 替换成 5。

## 4. 语法硬规则（违规即错误）

1. 指令**独占一行**：前缀前只允许空白（缩进 OK）；关键字与前缀之间**不能有空格**（`//# if` 不是指令）。
2. 关键字与参数之间需要 ≥1 个空白；`//#define_X`、`//#if(1)` 为未知关键字，宽松模式保留、strict 模式报 E4；`#endif`/`#else` 行尾不能有内容。
3. `if*`/`endif` 必须配平；重复 `#else`、`#else` 后的 `#elif`、孤儿 `#else/#elif/#endif` 均为错误（E1/E2/E3）。
4. 字符串字面量内的指令样文本不是指令（`log("see //#ifdef docs")` 整行保留）。
5. 保留行字节级保真（缩进、tab、CRLF 原样）；被裁剪分支与指令行**整行删除**。
6. 外层分支未保留时，内层指令只做配平登记：不求值、`#error` 不触发——可以放心在未启用的变体块里放 `#error`。
7. 文件起始 UTF-8 BOM 不影响首行指令识别；指令首行的 BOM 随整行删除，普通首行的 BOM 原样保留。
8. 宏作用域：CLI `-D` 全局生效；文件内 `#define/#undef` **只影响本文件、只影响其后**的表达式，不跨文件。

## 5. 各语言即抄即用示例

**Java**（JS/TS/Kotlin/Dart/PHP/Scala 同理，只换宿主代码）：

```java
//#if MODE == 1
private static final String TAG = "dev";
//#elif MODE == 2
private static final String TAG = "staging";
//#else
private static final String TAG = "prod";
//#endif

void run() {
    //#ifdef DEBUG
    System.out.println("enter run");
    //#endif
}
```

**TypeScript / JavaScript**：

```ts
export function send(msg: string) {
    //#ifdef DEBUG
    console.log("[debug]", msg);
    //#endif
    transport.push(msg);
}
```

**Python**（缩进内的指令同样合法；务必双井号）：

```python
##ifdef DEBUG
LOG_LEVEL = "debug"
##else
LOG_LEVEL = "info"
##endif

def handle(req):
    ##if TRACE
    logging.debug("handle %s", req)
    ##endif
    return req
```

**Shell**：

```bash
##ifdef DRY_RUN
echo "[dry-run] would deploy to $TARGET"
##else
sh steps.sh "$TARGET"
##endif
```

**Lua**（行注释是 `--`，前缀为 `--#`）：

```lua
--#ifdef VERBOSE
local function debug_log(msg)
    log.info("[debug]", msg)
end
--#else
local function debug_log(msg) end
--#endif

--#ifndef RELEASE
debug_log("boot")
--#endif
```

## 6. 批量修改代码工作流

给现有代码加可裁剪块时：

1. **选定宏名**：全大写下划线，语义清晰（`DEBUG_NET`、`TRACE`、`LEGACY_API`）。约定一套项目级宏表（如 `DEBUG`/`TRACE`/`RELEASE`），避免散落即兴宏名。
2. **包裹而非改写**：把要裁剪的语句原样包进 `#ifdef` 块，不改动语句本身（保证关闭宏时语义不变）。
3. **改完必须验证**（装好 alpp 后在仓库根执行）：
   - `alpp <files-or-dirs>` — dry-run 看裁剪 diff，退出码 0 = 指令全部合法；
   - 模拟发布构建：`alpp -DRELEASE <dir>` 核对要删的内容确实消失；
   - 需要落盘：`alpp -w -DRELEASE <dir>`；校验存量代码：`alpp --check <dir>`；发布分支门禁：`alpp --gate <dir>`（残留指令 → 退出码 3）。

### 批量操作注意

- 一次改多个文件时逐文件保持配平；不要跨文件共享 `#define`（作用域限单文件）。
- 缩进敏感语言（Python）把块整体包裹即可，alpp 不动保留行的缩进；不要让 `#endif` 挤掉必要的空行。
- 单行内的条件逻辑（如 `x = debug ? 1 : 0`）不归 alpp 管；只有整行粒度的块裁剪。
- 不确定某行是否会被识别为指令时，先对该文件跑一次 dry-run 确认。

## 7. 项目配置文件 `.alpp`

业务仓库根目录可放置 `.alpp`（TOML）约束宏用法、提供默认宏：

```toml
[macros]
# 默认宏初值，优先级最低，被命令行 -D/-U 覆盖
define = ["DEBUG", "LEVEL=1"]
undefine = []

[directives]
# 指令级语法管控：限制项目允许写哪些宏语句（如只许 ifdef/ifndef/else/endif）
# 名单只能取九个关键字：if/ifdef/ifndef/elif/else/endif/define/undef/error
deny = ["if", "elif", "define", "undef", "error"]
# 或用白名单（可选）：名单外指令一律 E8；deny 命中优先
# allow = ["ifdef", "ifndef", "else", "endif"]
```

- 发现规则：从第一个输入路径转换后的绝对路径向上逐级查找 `.alpp`，找到即停；找不到视为无配置。
- **改业务代码前先看仓库有没有 `.alpp`**：优先用 `define` 里列出的项目级宏表；只允许使用名单内的指令。
- E8 对**被抑制分支同样生效**——不要以为注释掉的指令不算数。
- 指令被禁（如 `#define` 在 deny 名单）：不要在文件内写该指令，等价值用 CLI 传入（如 `-DLEVEL=2`）或与维护者确认修改 `.alpp`。
- 宏名不受配置管控；`alpp --no-config` 可跳过配置（应急用）。

## 8. CLI 速查

```text
alpp [OPTIONS] [PATH]...

  （默认）                    dry-run：输出 unified diff，不改文件
  -w, --write                 写回（原地）
  -o DIR                      镜像输出到目录树（未变更文件原样复制）
  -D NAME[=VALUE]             定义宏；-U NAME 取消（后写者胜）
  -P PROFILE                  强制 profile（c_family|hash|lua）
  --prefix P                  非空自定义前缀（目录内未知扩展名同样生效）
  --strict                    未知关键字报 E4
  --check                     只校验指令合法性（CI 门禁）
  --gate                      发布门禁：扫描文件中残留任何指令行 → 退出码 3（不改写）
  --no-config                 跳过 `.alpp` 配置发现
  -j N                        并行度（默认 CPU 核数）

退出码：0 成功；1 IO/用法错误（含配置解析失败）；2 指令校验失败（含 #error 与 E8 宏管控）；3 --gate 发现未剥离残留（G1）
错误输出格式：<文件>:<行>: E<码>: <详情>
`-w` 有变更时 stderr 输出一行摘要 `alpp: processed=N changed=N errors=N skipped=N`；stdout 仅承载 dry-run 的 diff，可安全管道
```

镜像规则：CWD 内输入保留相对 CWD 的目录结构；外部目录映射内部结构，外部单文件映射文件名。输出目录从遍历排除；目标冲突、输出包含输入、输出符号链接越界会在写盘前拒绝。二进制/超限文件（含显式文件）不处理，镜像时原样复制。写回使用唯一临时文件并保留源权限。

显式符号链接与目标文件独立选择 Profile、独立处理；原地写回会替换传入路径，镜像映射同一目标则报冲突。大小写等价及文件/目录冲突按输出文件系统预检。

`--gate` 只保证实际扫描文件无残留：未知扩展名、隐藏/忽略文件及二进制/超限文件仍会跳过。按需要调整 Profile/前缀/大小上限或显式指定文件，`--verbose` 可显示二进制/超限跳过原因。

`--check` 与 `--write`/`-o` 互斥。dry-run 输出失败（含 flush）退出 1；无末尾换行的上下文和删除行均有标准 diff 标记；正文保留 CRLF 和非 UTF-8 字节，特殊路径按 Git 规则转义。

## 9. 错误码

| 码 | 名称 | 触发条件 |
|---|---|---|
| E1 | Unterminated | 文件结束仍有未配平的条件层 |
| E2 | InvalidBranchOrder | 同层重复 `#else`，或 `#else` 后出现 `#elif` |
| E3 | OrphanDirective | `#else/#elif/#endif` 无对应开层 |
| E4 | UnknownDirective | `--strict` 下前缀后跟非关键字表的指令样文本 |
| E5 | UserError | 活跃分支中的 `#error MSG` |
| E6 | StringEmbedded | 预留；当前不产生此诊断，行内字符串前缀原样保留 |
| E7 | BadExpression | 表达式语法/类型错误或嵌套超限（如用了 `and`/`or` 文字运算符，或括号与 `!` 累计超过 128 层） |
| E8 | ForbiddenDirective | 指令关键字被 `.alpp` 配置 `[directives]` 节管控（deny 名单或 allow 名单外），无论分支是否活跃 |

非指令诊断：`--gate` 模式残留指令行报 `<文件>:<行>: G1: residual conditional-compilation directive`（退出码 3，指令错误优先退出 2）。
