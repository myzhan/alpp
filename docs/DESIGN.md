# ALPP 设计文档

- 版本：0.1（对应 MACRO_SPEC v1.1）
- 状态：Draft
- 术语约定：本文中「指令」指 `MACRO_SPEC.md` 定义的注释内嵌条件编译指令；「宏」指指令操作的条件变量。

## 1. 目标与非目标

### 1.1 目标

1. **语言无关的条件编译**：开发者用注释形式在源码中显式写 `#ifdef` 风格指令，ALPP 在发布构建时按宏定义裁剪代码块。指令本身是注释；作者负责开发树及裁剪结果的宿主语法合法性。
2. **多平台**：纯 Rust 实现，当前发布矩阵为 Linux/macOS 的 x86_64 与 aarch64，以及 Windows x86_64；Linux 使用 GNU 目标。
3. **多线程**：文件级数据并行，吞吐随核数近线性扩展；文件间零共享状态。
4. **SIMD 加速**：行扫描的热路径（定位指令行）使用向量化字节搜索（SSE2/AVX2/NEON，运行时派发），目标是单线程扫描吞吐 ≥ 1 GB/s。
5. **安全默认**：默认 dry-run 输出 diff，显式 `--write` 才落盘；`--check` 供 CI 门禁。

### 1.2 非目标（V1）

- 不做完整词法/语法解析（不引入 tree-sitter；行级状态机 + 字符串字面量启发式即可满足需求）。
- 不支持跨文件的宏作用域（保证文件级并行与增量正确性）。
- 不做宏函数、字符串拼接等 C 预处理器全量特性。
- 不处理块注释内嵌指令（`/* #ifdef */`），V1 仅行注释（`//`、`#`、`--`）（见 §7 开放问题）。

### 1.3 与既有方案的差异

| 方案 | 缺陷 | ALPP 的改进 |
|---|---|---|
| filepp（Perl） | `#` 注释语言（Python）中普通注释易误判为指令；字符串字面量中的指令文本会被误解析；无 dry-run/diff | 关键字精确匹配 + 宽松默认模式（§4.5）；行首前缀限制（§4.4）；内置 diff/check |
| gpp | 默认模式条件语义与 cpp 不一致；cpp 模式要求指令顶格（非 C 语言非法）；tab 转空格 | 语义严格对齐 cpp；指令允许缩进；逐字节保留原文 |
| cpp 直接跑非 C 文件 | 撇号/引号导致解析错误、行标记污染 | 无 cpp 兼容负担，面向任意文本 |
| ast-grep 模式剥离 | 需维护每语言规则，无显式标记 | ALPP 显式标记，零规则维护；二者可互补使用 |

## 2. 总体架构

```text
CLI → 配置发现与宏表 → walker 收集/筛选文件 → 按路径排序
    → rayon 按线程数分批处理：读入 → pipeline 分析 → 按模式生成内容
    → 主线程依序输出 diff / 原子写回 / 汇总诊断
```

| 模块 | 职责 | 关键数据结构 |
|---|---|---|
| `cli.rs` | 参数解析与模式互斥 | `Args` |
| `profile.rs` | 扩展名与前缀映射 | `Profile` |
| `config.rs` | `.alpp` 发现、默认宏与指令管控 | `Config`, `Loaded` |
| `walker.rs` | gitignore 感知遍历、大小/二进制过滤、输出目录排除 | `FileJob`, `Collection` |
| `scanner.rs` | 行迭代、前缀及关键字精确识别 | `Line`, `Directive` |
| `pipeline.rs` | 条件栈、删除计划、按需拼装输出 | `Frame`, `Analysis`, `Outcome` |
| `eval.rs` | 表达式求值与文件内宏表 | `MacroTable`, `MacroValue` |
| `rewriter.rs` | 根据删除行号生成 unified diff | 删除行号与上下文区间 |
| `main.rs` | 批处理、目标路径校验、写回和退出码 | `Processed`, `FileError` |

## 3. 数据流与不变量

1. **输入**：`Vec<PathBuf>` → walker 收集 `FileJob`，文件路径元数据仍整体驻留。
2. **每文件处理是纯函数**：`(bytes, profile, macro_table, flags) → Result<Analysis, FileError>`。除诊断收集外无共享可变状态，这是并行的根基（不变量 I1）。
3. **重写是整行删除**：分析阶段产出删除行号与合并后的删除字节区间；需要写文件时批量复制保留段，整体 O(n)（不变量 I2）。check/gate 不生成文件输出，dry-run 使用同次读取的原始内容生成 diff。
4. **行尾与编码保真**：除被删除区间外，任何字节（含 `\r\n`、tab、BOM、UTF-8 多字节字符）原样保留（不变量 I3）。

## 4. 关键设计

### 4.1 CLI 与运行模式

四种模式互斥：

| 模式 | 行为 |
|---|---|
| 默认（dry-run） | 对每个发生改动的文件输出 unified diff 到 stdout；无改动文件沉默 |
| `--write` | 原地写回（写临时文件 + rename，保证原子性）；`-o <dir>` 可选镜像输出到新目录树 |
| `--check` | 只验证指令合法性（块配平、strict 检查），不改写；任何错误 → 退出码 2。用于 CI 语法门禁 |
| `--gate` | 发布门禁（M4）：验证指令合法性之外，实际扫描文件中残留任何指令行即违规 → 退出码 3（诊断 `G1`）；不改写。与 `--write`/`-o`/`--check` 互斥 |

全局宏来源优先级：文件内 `#define` > `--undefine (-U)` > `--define (-D)`（后两者都作用于文件起始时刻的宏表初值，`-U` 用于压制 `-D` 或内置宏）。文件内指令的生命周期仅限该文件（§4.7）。

### 4.2 语言 Profile 与前缀

- 内置三个 profile：`c_family`（`//#`）、`hash`（`##`）、`lua`（`--#`），映射表见 `src/profile.rs`。
- 选型原则：只收录**没有原生条件编译**的语言；有原生机制的（C/C++/Rust/Go/Swift/C# 等）与标记性语言（HTML/XML）不支持。
- 覆盖手段（优先级从高到低）：`--prefix`（全局强制）> `--profile`（强制某 profile）> 按扩展名自动 > 跳过（未知扩展名不处理，除非显式传入该文件——显式文件使用 `c_family` 兜底并告警）。`--prefix` 必须非空，也会覆盖目录遍历中的未知扩展名。
- 选 `##` 作为 hash 语言前缀的依据：调研期实测，`#` 单字符前缀会把普通注释（如 `# 注释`）误判为指令；`##` 在注释正文中出现概率趋近于零。
- 前缀匹配允许行首缩进（任意水平空白），兼容代码内缩进的块。

### 4.3 文件发现（walker）

- 基于 `ignore` crate：尊重 `.gitignore`/`.ignore`，隐藏文件默认跳过，`--no-ignore` 关闭 gitignore/global gitignore 过滤，仍保留 `.ignore` 与隐藏文件过滤。
- 显式文件与目录发现的文件共用二进制和大小过滤；二进制剔除读头部 8KB，含 NUL 字节即跳过。
- 单文件上限：默认 64MiB，超出跳过（`--verbose` 展示原因，可通过 `--max-size` 调整）。
- gate 共用发现/过滤规则，不对未扫描文件提供无残留保证。输入去重只归一化父目录，不合并末级符号链接与目标，避免丢失不同 Profile 或写回路径。

### 4.4 行扫描（SIMD 热路径）

当前处理入口使用 `iter_lines`，通过 `memchr` 搜索换行，再逐行确认前缀和关键字。
文件首行识别时跳过 UTF-8 BOM；若该行为指令，BOM 随整行删除，普通首行的 BOM 保留。前缀之前仅允许空格或 tab；关键字按水平空白分词，`//#define_X`、`//#if(1)` 不会拆成合法指令。
`lines_with_anchor` 是基准中保留的锚字节预筛实现，尚未接入生产流水线。

`memchr` 自身按平台选择 SIMD 实现，不手写 intrinsics。当前仍按单文件完整读入；
批次内存随并发线程数和单文件大小变化，不再累积整棵目录的输出。分块读文件是后续选项。

字符串中的行内前缀因不在行首而自然跳过；跨行原始字符串不在 V1 覆盖范围。

### 4.5 指令解析与块状态机

- `classify` 返回指令或普通行，pipeline 用 `Frame { taken, keeping, else_seen, open_line }` 维护条件栈。
- 任一外层未保留时内层受抑制；每层按 `taken` 选择分支，`else_seen` 检查重复 else 和 else 后的 elif（E2）。`keeping` 已包含外层抑制，检查栈顶即可取得当前状态，不再每行遍历全栈。
- 指令行与未保留代码行登记到删除计划，保留段保持原始字节。
- **宽松默认模式**：以 `#` 开头但不匹配指令关键字表精确字段的行一律视为普通注释（filepp 误伤的根治）；`--strict` 下则报错（错误码 E4）。
- `#error MSG`：在活跃分支中出现即产生该文件的致命错误（E5）。

### 4.6 表达式求值（eval）

- 文法与求值顺序见 MACRO_SPEC §4。实现为递归下降，输入是前缀后的字节切片，输出 `i64`；括号与一元 `!` 累计嵌套超过 128 层报 E7，避免栈溢出。
- `defined(NAME)` 查询宏表；`#if` 中未定义标识符按 `0` 处理（与 cpp 一致），`--strict` 不改变未定义标识符的求值规则。
- 宏表：`HashMap<String, MacroValue>`；`MacroValue = Defined | Num(i64) | Str`（`-DX=1`、`-DX=prod`）。

### 4.7 并行模型

- **文件级并行**：walker 先收集路径并排序，rayon 每批处理至多线程数个文件；主线程按排序消费并释放该批内容。宏表按文件克隆，无跨文件宏传播。
- 线程数：默认 = 可用核数；`-j N` 覆盖。
- 内存：check/gate 只生成分析结果；dry-run 只为变更文件生成 diff，使用同次读取的原始字节；原地写回只为变更文件生成输出。路径和诊断仍按文件数增长，不承诺常量总内存。
- diff API：`rewriter::unified_diff` 返回 `Vec<u8>`，保留原始正文；路径移除冗余 `./` 并按 Git C 风格引用，避免 CRLF、非 UTF-8 内容及特殊文件名产生不可应用的补丁。
- 输出契约：stdout 只承载 dry-run diff，写入或 flush 失败退出 1；错误与摘要走 stderr。`--write` 有变更时输出 `alpp: processed=N changed=N errors=N skipped=N`，`--verbose` 另加 written/skipped 明细。错误按路径输出，最多显示 50 条，退出码依据全量错误。
- 镜像树完整性：二进制/超限/未知扩展名文件原样复制；重叠输入中的已处理文件不会再次被透传副本覆盖。继续尊重 ignore/隐藏文件规则。
- 镜像路径：CWD 内输入保留相对 CWD 的目录结构；CWD 外的目录按其内部结构映射，外部单文件按文件名映射。绝对路径和 `..` 均先解析后生成目标；多输入目标冲突、输出包含输入、已存在的输出符号链接越界均在写盘前拒绝。输出目录从输入遍历中排除，重复运行不会产生 `out/out`。
- 镜像预检：在输出目录或其最近已有祖先中创建临时目录，以空文件路径验证名称等价和文件/目录冲突，结束时自动清理；不同输入别名映射同一目标也拒绝。仅复制有明确来源的文件，不提供整个目录的事务回滚。
- 写回安全：使用同目录唯一临时文件，写入成功后保留源权限并原子替换目标；失败清理临时文件，原地无变化不写盘。镜像透传使用流式复制，不将大型二进制完整读入内存。

### 4.8 配置文件 `.alpp`（M2 提前落地 ✅）

- 业务仓库可放置 `.alpp`（TOML）约束宏用法：`[macros] define/undefine` + `[directives] deny/allow`（格式与语义见 MACRO_SPEC §10）；发现规则：先将第一个输入路径转为绝对路径，再向上逐级查找，找到即停。
- **与并行模型的关系**：配置只做校验（`[directives]` deny/allow → E8）与宏表初值，不参与条件求值状态机，发现一次后全进程共享只读，不破坏 4.7 的无锁模型。
- 优先级（低→高）：配置 define → 配置 undefine → `-D` → `-U`；`--no-config` 逃生门。
- E8 管控对抑制分支同样生效（出现即违规），与 E5 的“活跃才触发”不同。
- `[directives]` 节做指令级语法管控（E8 ForbiddenDirective）：限制项目可用的宏语句（如只许 ifdef/endif）；名单值校验限定九个关键字，非法名 → 配置解析失败退出 1。`[macros]` 节只保留 define/undefine（默认宏，优先级低于 CLI）；宏名不做配置管控。
- 新增依赖：`toml` + `serde`（配置解析）。

### 4.9 跨平台策略

- 依赖全部为纯 Rust crate（clap/ignore/memchr/rayon/anyhow/thiserror/serde/toml/tempfile），无 C 工具链依赖，`cross` 即可交叉编译。
- 产物矩阵：`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、`x86_64-apple-darwin`、`aarch64-apple-darwin`、`x86_64-pc-windows-msvc`。
- SIMD 派发：x86_64 运行时检测 SSE2/AVX2；aarch64 NEON 为基线无条件启用；其他平台回退标量 memchr（纯 Rust 实现，仍有不错的吞吐）。
- 行尾：`\n` 与 `\r\n` 都识别为行界，删除区间时连同行尾符一起删除，不引入行尾归一化。
- 路径：一律 `std::path`，不拼接字符串路径。

### 4.10 性能目标与基准

| 指标 | 目标 | M1 实测（M1 芯片，6P+6E，release+lto） |
|---|---|---|
| 单线程全处理吞吐（d0 纯代码） | ≥ 1 GB/s | **2.21 GiB/s** ✅ |
| 单线程全处理吞吐（d1 典型发布树） | ≥ 1 GB/s | **2.58 GiB/s** ✅ |
| 单线程全处理吞吐（d10 重标注） | ≥ 1 GB/s | 1.93 GiB/s ✅ |
| 仅扫描（line_iter） | — | 3.70 GiB/s |
| 仅扫描（锚字节预筛） | — | 2.44 GiB/s |
| 并行扩展 t4/t8（8 文件） | 8 核 ≥ 6× | t4 = **3.70×**，t8 = 2.67× |

并行说明：实测机为异构核（6 性能核 + 6 能效核）。t4 全部落在性能核上近线性扩展；
t8 的 2 个线程被调度到能效核成为长尾，加速比反而回落。同构 8 核平台（Linux CI）
预计可达 ≥6×；`-j` 参数已支持用户按核型自选线程数。10 万行（~3.6 MB）确定性语料，
criterion 100 样本；数据可由 `cargo bench` 复现。

基准用 criterion（`benches/process.rs`）：CLI `cli/tree` 覆盖 64 文件的遍历/读盘/分析/diff 与 check；另有确定性合成语料（xorshift64 种子固定，跨机器可比）× {0%, 1%, 10% 指令密度} × {全处理, 仅扫描, 多线程}。CI 编译基准；吞吐与 RSS 需在固定环境单独测量，尚无性能阈值门禁。

### 4.10 错误处理与退出码

| 码 | 含义 |
|---|---|
| 0 | 成功（含 dry-run 有改动） |
| 1 | 运行错误（IO、路径不存在、未知 profile/宏名） |
| 2 | 指令校验失败（未闭合 E1、分支顺序错误 E2、外层失配 E3、strict 未知指令 E4、#error E5、表达式错误 E7、配置指令管控 E8） |
| 3 | `--gate` 发现未剥离残留（M4 已实现：实际扫描文件中存在指令行即违规，诊断 `G1: residual conditional-compilation directive`；指令校验错误仍优先退出 2） |

诊断格式：`<path>:<line>: E<n>: <message>`，全部汇总在 stderr，多错误不中断其他文件的处理（最终以最高严重级退出码退出）；超过 50 条折叠为 `alpp: ... and N more errors`。

## 5. 测试策略

当前 Unix 共 112 个测试，Windows 跳过 5 个 Unix 专属用例。`tests/regressions.rs` 的 29 个用例覆盖路径越界、镜像重复运行/透传/冲突、相对配置发现、权限与临时文件、关键字边界、输入过滤、末行无换行 diff 的 `git apply` 对拍、stdout 错误及批次确定性；另覆盖符号链接别名、文件系统目标冲突、CRLF/非 UTF-8/特殊路径 diff 对拍、BOM、分支顺序与表达式深度。Git 对拍显式禁用 autocrlf，避免 runner 配置改写字节。新增回归 fixture 与全语法参考 fixture 分目录保存。

1. **单元测试**：parser 状态机表驱动用例（嵌套/else/未闭合/字符串内前缀/缩进/`\r\n`）；eval 文法用例（优先级、defined、未定义标识符）；config 解析与管控。— 44 个。
2. **集成测试**：`tests/integration.rs` 以真实二进制端到端验证（临时文件树 + `CARGO_BIN_EXE`），覆盖四模式（dry-run/`--write`/`--check`/`--gate`）、`-D/-U`、`--profile`、`--strict`、`.alpp` 配置（E8）、退出码 0/1/2/3、gitignore/二进制跳过、CRLF 保真、镜像树完整性（含未处理文件补齐）、`-j1`/`-jN` 输出逐字节一致（并发确定性）；`tests/languages.rs` 按语言专项验证（5 种语言 × 3 Profile，只收录无原生条件编译的语言），采用 golden file：每种语言一个全语法参考 `tests/fixtures/<lang>/all.<ext>`（java 为 All.java；全语法参考共 20 个文件 = 5 输入 + 15 期望，不含 regressions 子目录），Rust 侧只保留测试逻辑，全部断言为完整字符串逐字节对比。all fixture 覆盖 §3 指令全集（ifdef/ifndef/if/elif/else/endif/define/undef/error）+ §4 表达式全集（defined/数值比较/逻辑短路/括号优先级）+ §5 块语义（嵌套/外层抑制）+ §6 输出规则（字符串豁免、缩进指令），头部注释即使用说明，可直接作为写代码时的语法参考；三场景（default、`-DFULL -DLEVEL=2`、`-DNEED_DB` 触发 E5）。— M1 落地 26 + 5 个，M2/M4 补至 32 + 5 个。
3. **属性测试**：`tests/props.rs`（M4 已落地）。seed 扫描式随机指令树（xorshift64*，900 棵/三 Profile 轮换，构造性配平），断言三条不变量：平衡树不产生 E1/E2/E3、strip 完备（输出无指令行）、幂等（二次处理零变更）。失败时打印 seed 可精确复现——以此替代 proptest 框架（seed 收缩无意义，零新依赖）。
4. **模糊测试**（cargo-fuzz，后续）：fuzz parser 的行输入，要求无 panic、无不配平漏报。
5. **对拍**：filepp 直接对拍不可行（无 unified diff/错误码体系，语义差异由 MACRO_SPEC §8 与 cpp 的差异表覆盖，调研期已验证其行为基线）；维持文档化决策，不做运行时对拍。
6. **CI**（GitHub Actions）：`.github/workflows/ci.yml` lint job（`cargo fmt --check` + `cargo clippy --all-targets -- -D warnings`）+ test job（ubuntu/macos/windows 矩阵跑 `cargo test` 与 `cargo bench --no-run` 编译检查）；`.github/workflows/release.yml`（M3）推送 `v*` tag 时五平台（linux-x86_64/linux-aarch64/macos-x86_64/macos-aarch64/windows-x86_64）构建并上传 tar.gz/zip 到 GitHub Release；本地提交前四步均可本地预验证。

## 6. 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M1 ✅ | scanner + parser + eval + rewriter 闭环（含 rayon 文件级并行）；三模式 CLI | 单元 44 + 集成 26 + 语言专项 5 全绿（75）；多语言（java/ts/py/sh/lua）完整字符串对比通过，每语言一个全语法参考 fixture；基准数据见 §4.10 |
| M2 ✅ | 诊断聚合增强（错误输出上限 50 + 折叠行）、`-o` 镜像补齐未处理文件（§4.7）；~~`.alpp` 配置文件~~ ✅ 提前落地（§4.8）；并发确定性以 `-j1`/`-jN` 输出逐字节一致测试替代 loom（文件级无共享，无共享内存协议可验） | 吞吐 ≥1GB/s ✅（§4.10）；异构核机 t4=3.70×（同构 8 核预计 ≥6×，见 §4.10 说明） |
| M3 ✅ | 吞吐达标 ✅（§4.10）；release profile（lto=thin）已配置；跨平台产物：`.github/workflows/release.yml` 五平台出包（tag `v*` 触发） | `cargo bench` ≥1GB/s；五平台产物出包（首次打 tag 时验证 runner 矩阵） |
| M4 ✅ | `--gate` 残留门禁（退出码 3，诊断 G1）；属性测试 `tests/props.rs`（seed 扫描替代 proptest，§5.3）；filepp 对拍以文档化差异表替代（§5.5）；cargo-fuzz 留后续 | M4 首次验收 83 测试全绿；本地四步 + YAML 校验通过 |

## 7. 开放问题

1. 块注释内嵌指令（`/* #ifdef */`）是否值得支持？当前判定：不值得（行注释已覆盖主流语言，块注释引入跨行词法复杂度）。
2. `.alpp` 宏默认值与指令管控已落地；自定义扩展名映射留待后续。
3. `#include` 宏表共享：与无锁并行冲突，除非引入「两阶段：先收集全局 define 文件再并行」；暂不做成核心特性。
4. JavaScript 模板字符串、Lua 长字符串等跨行原始字符串内的指令前缀：V1 已知局限，由编码规范规避（指令不得出现在字符串内）。
