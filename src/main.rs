//! CLI 入口：参数 → 配置发现 → 文件收集 → 并行处理 → 聚合输出（diff/write/check）→ 退出码。

use alpp::cli::Args;
use alpp::config::{self, Config, Loaded};
use alpp::error::FileError;
use alpp::eval::{MacroTable, MacroValue};
use alpp::pipeline;
use alpp::profile::{self, Profile};
use alpp::rewriter;
use alpp::walker::{self, FileJob};
use clap::Parser;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("alpp: {msg}");
            std::process::ExitCode::from(1)
        }
    }
}

/// 诊断输出上限：超出部分折叠为一行汇总（M2 诊断聚合增强，DESIGN §4.7）。
const MAX_PRINTED_ERRORS: usize = 50;

/// 宏表构建失败：用法错误（退出 1）。
enum TableError {
    Usage(String),
}

fn run(args: &Args) -> Result<std::process::ExitCode, String> {
    let paths = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };

    // `.alpp` 配置发现：以第一个输入路径为锚向上查找；--no-config 跳过（SPEC §10）
    let discovered: Option<Loaded> = if args.no_config {
        None
    } else {
        config::discover(&paths[0]).map_err(|e| format!("config: {e}"))?
    };

    let table = match build_macro_table(args, discovered.as_ref()) {
        Ok(t) => t,
        Err(TableError::Usage(m)) => return Err(m),
    };

    // --profile 提前校验，并强制应用到目录遍历的全部文件（含未知扩展名）
    let force = args
        .profile
        .as_ref()
        .map(|name| {
            profile::by_name(name)
                .ok_or_else(|| format!("unknown profile `{name}` (expected c_family|hash|lua)"))
        })
        .transpose()?;

    if args.prefix.as_deref() == Some("") {
        return Err("--prefix must not be empty".into());
    }
    let mirror_root = args
        .output
        .as_deref()
        .map(resolve_path)
        .transpose()
        .map_err(|e| format!("output: {e}"))?;
    if let Some(root) = &mirror_root {
        for path in &paths {
            let input = path
                .canonicalize()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if input.starts_with(root) {
                return Err("output directory must not contain an input path".into());
            }
        }
    }
    // 自定义前缀同样覆盖未知扩展名，具体前缀在 process_one 中绑定。
    let force = force.or_else(|| args.prefix.as_ref().map(|_| profile::C_FAMILY));
    let mut col = walker::collect(
        &paths,
        args.max_size,
        args.no_ignore,
        force,
        mirror_root.as_deref(),
    )
    .map_err(|e| format!("collect: {e}"))?;
    col.jobs.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = HashSet::new();
    col.jobs.retain(|job| seen.insert(file_key(&job.path)));
    // 重叠输入中已处理的文件不能再作为透传文件复制，覆盖其裁剪结果。
    col.passthrough.retain(|p| !seen.contains(&file_key(p)));
    col.skipped.retain(|(p, _)| !seen.contains(&file_key(p)));
    let destinations = mirror_root
        .as_deref()
        .map(|root| mirror_destinations(&paths, &col, root))
        .transpose()
        .map_err(|e| format!("output: {e}"))?
        .unwrap_or_default();

    // 线程池：默认核数，-j 覆盖（DESIGN §4.7）
    let mut pool_builder = rayon::ThreadPoolBuilder::new();
    if let Some(n) = args.threads {
        pool_builder = pool_builder.num_threads(n);
    }
    pool_builder
        .build_global()
        .map_err(|e| format!("thread pool: {e}"))?;

    let mut stdout = None;
    let mut errors = Vec::new();
    let mut residual = Vec::new();
    let mut had_io_error = false;
    let mut changed = 0;

    // 只保留一个线程批次的内容，消费后释放；排序先于并行，输出顺序不随调度改变。
    for jobs in col.jobs.chunks(rayon::current_num_threads()) {
        let results: Vec<_> = jobs
            .par_iter()
            .map(|job| process_one(job, args, &table, discovered.as_ref()))
            .collect();
        for (job, result) in jobs.iter().zip(results) {
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(e) => {
                    had_io_error |= e.code == 0;
                    errors.push(e);
                    continue;
                }
            };
            changed += usize::from(outcome.deleted_lines > 0);
            if args.gate {
                if let Some(line) = outcome.first_directive {
                    residual.push(format!(
                        "{}:{}: G1: residual conditional-compilation directive",
                        job.path.display(),
                        line
                    ));
                }
            } else if args.check {
                // 校验模式不生成文件内容或 diff。
            } else if args.write {
                if outcome.deleted_lines > 0 || mirror_root.is_some() {
                    let dest = destinations.get(&job.path).unwrap_or(&job.path);
                    if let Err(e) = write_back(&job.path, dest, Some(&outcome.output)) {
                        had_io_error = true;
                        eprintln!("alpp: write {}: {e}", job.path.display());
                    } else if args.verbose {
                        eprintln!(
                            "written: {} ({} lines removed)",
                            job.path.display(),
                            outcome.deleted_lines
                        );
                    }
                }
            } else if !outcome.output.is_empty() {
                if stdout.is_none() {
                    stdout = Some(stdout_writer().map_err(|e| format!("stdout: {e}"))?);
                }
                stdout
                    .as_mut()
                    .unwrap()
                    .write_all(&outcome.output)
                    .map_err(|e| format!("stdout: {e}"))?;
            }
        }
    }
    if let Some(mut stdout) = stdout {
        stdout.flush().map_err(|e| format!("stdout: {e}"))?;
    }

    for e in errors.iter().take(MAX_PRINTED_ERRORS) {
        eprintln!("{e}");
    }
    if errors.len() > MAX_PRINTED_ERRORS {
        eprintln!(
            "alpp: ... and {} more errors",
            errors.len() - MAX_PRINTED_ERRORS
        );
    }

    // 未处理文件与已处理文件使用同一套目标映射和安全写回。
    if mirror_root.is_some() {
        for p in col
            .passthrough
            .iter()
            .chain(col.skipped.iter().map(|(p, _)| p))
        {
            if let Err(e) = write_back(p, &destinations[p], None) {
                had_io_error = true;
                eprintln!("alpp: mirror {}: {e}", p.display());
            }
        }
    }

    if args.verbose || (args.write && changed > 0) {
        eprintln!(
            "alpp: processed={} changed={} errors={} skipped={}",
            col.jobs.len(),
            changed,
            errors.len(),
            col.skipped.len()
        );
    }
    if args.verbose {
        for (p, why) in &col.skipped {
            eprintln!("alpp: skipped {}: {why}", p.display());
        }
    }
    for r in &residual {
        eprintln!("{r}");
    }

    Ok(std::process::ExitCode::from(if had_io_error {
        1
    } else if !errors.is_empty() {
        2
    } else if !residual.is_empty() {
        3
    } else {
        0
    }))
}

/// 标准 stdout 会吞掉 EBADF；复制标准句柄到 File，让写入/flush 失败真实返回。
fn stdout_writer() -> std::io::Result<impl std::io::Write> {
    let stdout = std::io::stdout();
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let handle = stdout.as_fd().try_clone_to_owned()?;
        Ok(std::io::BufWriter::new(std::fs::File::from(handle)))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        // 验证句柄有效性，同时保留标准库的 Windows 控制台 Unicode 输出。
        let _handle = stdout.as_handle().try_clone_to_owned()?;
        Ok(stdout.lock())
    }
}

/// 主线程消费的单文件结果；output 只包含当前模式实际需要的文件或 diff 字节。
struct Processed {
    deleted_lines: usize,
    first_directive: Option<usize>,
    output: Vec<u8>,
}

/// 处理单个文件：读入 → 选择生效 Profile → 流水线处理。IO 错误记为 code 0。
fn process_one(
    job: &FileJob,
    args: &Args,
    table: &MacroTable,
    loaded: Option<&Loaded>,
) -> Result<Processed, FileError> {
    let bytes = std::fs::read(&job.path).map_err(|e| FileError {
        path: job.path.display().to_string(),
        line: 0,
        code: 0,
        message: format!("io: {e}"),
    })?;

    // Profile 优先级：--prefix > --profile（已烘焙进 job）> 按扩展名（DESIGN §4.2）
    let owned_custom;
    let profile: &Profile = if let Some(px) = &args.prefix {
        owned_custom = Profile {
            name: "custom",
            prefix: px.as_str(),
            extensions: &[],
        };
        &owned_custom
    } else {
        &job.profile
    };

    let empty = Config::default();
    let cfg = loaded.map(|l| &l.cfg).unwrap_or(&empty);
    let analysis = pipeline::analyze_bytes(&bytes, &job.path, profile, table, args.strict, cfg)?;
    let changed = !analysis.deleted_lines.is_empty();
    let output = if args.check || args.gate {
        Vec::new()
    } else if args.write && (changed || args.output.is_some()) {
        if changed {
            analysis.rewrite(&bytes)
        } else {
            bytes
        }
    } else if !args.write && changed {
        // 使用本次解析的原始字节，避免重读文件后删除行号与内容不匹配。
        rewriter::unified_diff(&job.path, &bytes, &analysis.deleted_lines)
    } else {
        Vec::new()
    };
    Ok(Processed {
        deleted_lines: analysis.deleted_lines.len(),
        first_directive: analysis.directive_lines.first().copied(),
        output,
    })
}

/// 写回在同目录创建唯一临时文件；失败自动清理，成功保留源权限后原子替换。
/// bytes=None 用于镜像透传，流式复制避免读入整个二进制文件。
fn write_back(src: &Path, dest: &Path, bytes: Option<&[u8]>) -> std::io::Result<()> {
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(bytes) = bytes {
        tmp.write_all(bytes)?;
    } else {
        std::io::copy(&mut std::fs::File::open(src)?, &mut tmp)?;
    }
    tmp.as_file()
        .set_permissions(std::fs::metadata(src)?.permissions())?;
    tmp.persist(dest).map_err(|e| e.error)?;
    Ok(())
}

/// 仅归一化父目录；末级符号链接是独立的处理/写回路径，不能与目标文件合并。
fn file_key(path: &Path) -> PathBuf {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    match (parent.canonicalize(), path.file_name()) {
        (Ok(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

/// 解析已存在的祖先，保证尚未创建的输出目录也按真实目录边界检查。
fn resolve_path(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => path.canonicalize(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = path
                .file_name()
                .ok_or_else(|| std::io::Error::other("invalid output path"))?;
            Ok(resolve_path(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

/// 写盘前验证整棵镜像的路径，绝对路径、.. 和已存在的符号链接均不能越界。
fn mirror_destinations(
    inputs: &[PathBuf],
    col: &walker::Collection,
    root: &Path,
) -> std::io::Result<HashMap<PathBuf, PathBuf>> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let bases: Vec<_> = inputs
        .iter()
        .map(|p| {
            let p = p.canonicalize()?;
            Ok(if p.is_dir() {
                p
            } else {
                p.parent().unwrap().to_path_buf()
            })
        })
        .collect::<std::io::Result<_>>()?;
    let sources: Vec<_> = col
        .jobs
        .iter()
        .map(|j| &j.path)
        .chain(col.passthrough.iter())
        .chain(col.skipped.iter().map(|(p, _)| p))
        .collect();
    let originals: HashSet<_> = sources
        .iter()
        .map(|p| p.canonicalize())
        .collect::<std::io::Result<_>>()?;
    let mut destinations = HashMap::new();
    let mut owners = HashMap::new();
    // 在输出文件系统上预建空路径，交由文件系统判断大小写/名称等价与文件目录冲突。
    // 不写入真实目标，错误时 TempDir 自动清理。
    let ancestor = root
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or_else(|| std::io::Error::other("output has no existing directory"))?;
    let probe = tempfile::tempdir_in(ancestor)?;
    for path in sources {
        let src = path.canonicalize()?;
        // CWD 内保持原有目录布局；外部输入按所属输入目录（单文件按父目录）映射。
        let base = if src.starts_with(&cwd) {
            &cwd
        } else {
            bases
                .iter()
                .find(|base| src.starts_with(base))
                .ok_or_else(|| std::io::Error::other("input has no mirror base"))?
        };
        let dest = root.join(src.strip_prefix(base).map_err(std::io::Error::other)?);
        let resolved = resolve_path(&dest)?;
        if !resolved.starts_with(root) || originals.contains(&resolved) || resolved.is_dir() {
            return Err(std::io::Error::other(format!(
                "unsafe mirror destination: {}",
                dest.display()
            )));
        }
        if let Some(previous) = owners.insert(resolved.clone(), file_key(path)) {
            if previous != file_key(path) {
                return Err(std::io::Error::other(format!(
                    "mirror destination collision: {}",
                    dest.display()
                )));
            }
        } else {
            let reserved = probe
                .path()
                .join(resolved.strip_prefix(root).map_err(std::io::Error::other)?);
            let reserve = || -> std::io::Result<()> {
                std::fs::create_dir_all(reserved.parent().unwrap())?;
                std::fs::File::create_new(&reserved)?;
                Ok(())
            };
            reserve().map_err(|e| {
                std::io::Error::other(format!(
                    "mirror destination conflict: {}: {e}",
                    dest.display()
                ))
            })?;
        }
        destinations.insert(path.clone(), dest);
    }
    Ok(destinations)
}

/// 应用一条 `NAME[=VALUE]` 宏定义（CLI -D 与配置 define 共用）。
fn apply_define(t: &mut MacroTable, spec: &str) -> Result<(), String> {
    let (name, value) = match spec.split_once('=') {
        Some((n, v)) => (n, v),
        None => (spec, ""),
    };
    if !alpp::eval::is_valid_name(name.as_bytes()) {
        return Err(format!("invalid macro name in define `{spec}`"));
    }
    let v = if value.is_empty() && !spec.contains('=') {
        MacroValue::Defined
    } else {
        value
            .parse::<i64>()
            .map_or_else(|_| MacroValue::Str(value.to_string()), MacroValue::Num)
    };
    t.define(name, v);
    Ok(())
}

/// 构建初始宏表。优先级从低到高（SPEC §10）：
/// 配置 define → 配置 undefine → 命令行 -D → 命令行 -U（后写者胜）。
fn build_macro_table(args: &Args, loaded: Option<&Loaded>) -> Result<MacroTable, TableError> {
    let empty = Config::default();
    let cfg = loaded.map(|l| &l.cfg).unwrap_or(&empty);

    let mut t = MacroTable::new();

    // 1. 配置层（最低优先级）
    for spec in &cfg.macros.define {
        apply_define(&mut t, spec).map_err(TableError::Usage)?;
    }
    for u in &cfg.macros.undefine {
        t.undef(u);
    }

    // 2. 命令行 -D
    for d in &args.defines {
        let name = d.split_once('=').map_or(d.as_str(), |(n, _)| n);
        if !alpp::eval::is_valid_name(name.as_bytes()) {
            return Err(TableError::Usage(format!("invalid macro name in -D{d}")));
        }
        apply_define(&mut t, d).map_err(TableError::Usage)?;
    }
    // 3. 命令行 -U（后写者胜）
    for u in &args.undefines {
        if !alpp::eval::is_valid_name(u.as_bytes()) {
            return Err(TableError::Usage(format!("invalid macro name in -U{u}")));
        }
        t.undef(u);
    }
    Ok(t)
}
