# AGENTS.md — alpp 仓库维护规范

面向 AI agent 的维护指南。alpp 是注释内嵌条件编译预处理器（Rust）。
语法规范看 `docs/MACRO_SPEC.md`，架构看 `docs/DESIGN.md`，本文件只记录**必须遵守的约定**与**实测踩过的坑**。

## 常用命令

```bash
cargo test                          # 全量测试（必须全绿才能交付）
cargo fmt -- --check                # CI 强制；不干净先 cargo fmt
cargo clippy --all-targets -- -D warnings   # CI 强制，零警告
cargo bench --no-run                # 基准编译检查（防腐烂）；跑基准用 cargo bench
```

CI（`.github/workflows/ci.yml`）：lint job（fmt + clippy）+ test job（ubuntu/macos/windows 矩阵）。推送前本地把上述四条全部跑一遍。

实测 CLI 用临时目录，**不要对 alpp 仓库自身跑 `alpp -w`**（walker 会捡到 `tests/fixtures/` 里的源码；确要对本仓库跑必须显式排除 tests/）。

## 架构速览

数据流：walker（ignore crate 收集文件）→ scanner（行分类/指令识别）→ pipeline（Frame 栈状态机，外层抑制）→ eval（递归下降表达式求值）→ rewriter（unified diff）→ main（rayon 按线程数分批处理 + 确定性输出）。

- Profile（`src/profile.rs`）：c_family `//#`、hash `##`、lua `--#`。
- 配置（`src/config.rs`）：`.alpp` 发现/解析/宏管控校验，全进程一份只读。
- 退出码：0 成功；1 IO/用法错误（含配置解析失败）；2 指令校验失败（E1–E8，E6 预留；含活跃 `#error`）；3 `--gate` 发现未剥离残留（G1，发布门禁）。
- 解析边界：文件首行识别 UTF-8 BOM；同层 else 后的 elif 报 E2；括号与一元 ! 累计嵌套最多 128 层，超限 E7。diff 返回原始字节 `Vec<u8>`。
- 错误输出：`<文件>:<行>: E<码>: <详情>`；`--gate` 残留为 `<文件>:<行>: G1: ...`；错误超过 50 条折叠为 `alpp: ... and N more errors`。

## 硬性规范（历史定稿，不要推翻）

### 语言选型

**只收录没有原生条件编译的语言**。有原生机制的（C/C++ 的 cpp 宏、Rust 的 cfg、Go 的 build tag、Swift/C# 的 `#if`）与标记性语言（HTML/XML）、YAML 一律不支持——已明确移除，不要加回。Java/Lua/Python 这类无原生机制的语言是核心支持对象。

### 测试资产形态（用户定稿）

1. **测试代码禁止内联 Rust 字符串**：语言源码、期望 write 输出、期望 diff 全部放 `tests/fixtures/<lang>/`（golden file）；Rust 侧只留读 fixture + 断言的薄逻辑。
2. **断言必须完整字符串逐字节对比**，不允许 contains/部分匹配（integration 里对 CLI 行为的粗检除外）。
3. 每语言**只保留一个**全语法参考 `all.<ext>`（java 为 `All.java`）+ 三份期望（`diff-all-default.diff`、`write-all-default.<ext>`、`write-all-macro.<ext>`）；命名统一 **all**（不用 full）。三场景：无宏、`-DFULL -DLEVEL=2`、`-DNEED_DB`（触发 E5 退出 2）。
4. fixture 须覆盖全部指令/表达式/块语义，头部注释即使用说明——它同时是"写代码参考"，改语法必须同步。
5. 测试与基准中的文件名用 `.js`（c_family 代表；不要用已移除语言的扩展名）。

### golden file 生成方法（违反即出错）

- 期望值**只能用真实 CLI 跑出 + 人工核对语义后固化**，禁止手算（diff hunk 头行数、保留行间空行极易算错）。
- `.diff` golden 必须用**真实文件名**生成（header `--- a/<file>`）；改名后要同步 sed header。
- GNU diff 细节：上下文行一律带前缀空格——**空行上下文是单空格行 `" "`**、**tab 行上下文是 `" \t"`**；转录时最容易丢。

### agent skill（`skills/alpp-macros/SKILL.md`）

- 放**仓库顶层 `skills/`**（通用 Agent Skills 约定），不放 `.qoder/` 等工具私有目录。
- **必须自包含**：不引用 alpp 仓库内其他文件（MACRO_SPEC/fixtures）——消费方是改业务代码的 agent，不会看本仓库。
- 宏语法、CLI、错误码、Profile 任何变更都须同步该 skill。

### `.alpp` 配置

优先级**低于命令行**（配置 define → 配置 undefine → `-D` → `-U`，后写者胜）。`[macros]` 节只保留 define/undefine（默认宏初值）——**宏名不做配置管控**；防滥用靠 `[directives]` 节的指令级语法管控（deny/allow，限九个关键字）。指令管控是硬约束：**被抑制分支也违规**（E8 ForbiddenDirective，与 E5“活跃才触发”不同）。格式与语义见 MACRO_SPEC §10。

## 变更同步链路

改宏语法 / CLI / 错误码 / Profile（新增或删除语言）时，以下文件必须一起改，缺一即测试失败或文档漂移（无论改动大小，交付前都要按"验证纪律"第一条例行 review 这些文档是否过期）：

```
src/profile.rs（常量 + ALL + 单测）
docs/MACRO_SPEC.md（§2.2 表格 + §2.1 词法 EBNF + §7 错误码 + 相关节）
skills/alpp-macros/SKILL.md（自包含全文）
tests/fixtures/<lang>/（golden 目录）+ tests/languages.rs
docs/DESIGN.md（§4.x + §5 测试计数 + §6 里程碑验收行）
README.md（若影响门面）
```

删除 Profile 的额外点：scanner 的对应分支、cli.rs `-P` 帮助文本、main.rs unknown profile 错误信息、integration 混合 profile 测试、benches/rewriter 中的文件扩展名。

## 已知陷阱（全部实测踩过）

1. **find 按名字前缀批量删除会误伤**：`! -name "full*"` 匹配的是文件名开头，曾把 `write-full-*`、`diff-full-*`、`Full.java` 全部误删。批量操作前先 `find ... | wc -l` 核对清单。
2. **alpp 表达式没有 `and`/`or`/`not` 文字运算符**，只有 `&&`/`||`/`!`；写了报 E7。
3. **fixture 源码在 IDE 必报重复声明**（所有条件分支并存），固有现象，忽略；`cargo test` 与 CLI 才是权威结论。
4. **IDE 的 Rust 诊断在文件编辑后常滞后**报幽灵错误（编辑器缓存），以 `cargo check` 实际输出为准。
5. **zsh 中 `echo ===X===` 会被当 glob**，需加引号。
6. **`cargo fmt -- --check` 失败会短路 `&&` 验证链**：后面命令根本没执行，此时打印的 `$?` 不是你以为的那条命令的退出码。验证链中每步失败都要单独确认。
7. `#endif`/`#else` 行尾不能有内容；关键字与前缀之间不能有空格（`//# if` 不是指令）——写测试样例时常错。
8. Python fixture 缩进块被裁剪后必须仍有语句（`##else` 配 `pass` 或反转条件），行级删除模型的固有限制。
9. **临时目录名禁止用 `{:?}` 格式化 `SystemTime`**：Debug 输出含冒号，Windows 文件名非法（CI 报 ERROR_INVALID_NAME 267），用 UNIX_EPOCH 纳秒数等纯数字。
10. **仓库必须保留 `.gitattributes`（`* text=auto eol=lf`）**：GitHub Windows runner 的 `core.autocrlf=true` 会把文本检出转 CRLF，fixtures/golden 被改写后逐字节对比全部失败（实录：lint/ubuntu/macos 全绿仅 windows 红）。新增二进制或行尾敏感资产时补 `**/<pattern> -text`。
11. **Git 对拍必须显式隔离换行配置**：临时目录不继承仓库 `.gitattributes`，调用 `git -c core.autocrlf=false -c core.eol=lf apply`，不能依赖 runner 的全局 Git 配置。

## 验证纪律

- **文档时效 review（每次改动必做）**：交付前通读 `docs/`（MACRO_SPEC.md、DESIGN.md）、`skills/alpp-macros/SKILL.md`、`README.md` 与本文件，逐项核对描述与代码/测试实际行为一致——尤其测试计数、fixture 文件数、语言列表、命令示例、错误码表、退出码、里程碑状态。发现过期立即修正，不允许留到下次。
- **实证优先**：任何"应该是这样"的结论（尤其错误码、退出码、diff 格式）必须先跑真实 CLI 复现确认，再写进测试或文档；不确定某行是否被识别为指令，先 dry-run。
- 改动后全量 `cargo test`；涉及输出格式/退出码的改动，另在临时目录手工端到端验证一遍各场景（dry-run、-w、--check、--gate、错误路径）。
- 交付前对照本文件"硬性规范"逐条自查。
