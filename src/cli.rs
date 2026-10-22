use clap::Parser;
use std::path::PathBuf;

/// ALPP - Any Language PreProcessor.
///
/// Strips `#ifdef`-style conditional blocks embedded in language comments
/// from a source tree, e.g. logging code in release builds.
#[derive(Parser, Debug)]
#[command(name = "alpp", version, about)]
pub struct Args {
    /// Files or directories to process. Defaults to the current directory.
    pub paths: Vec<PathBuf>,

    /// Define a macro (-DRELEASE, -DVERBOSE_LOG=1). Repeatable.
    #[arg(short = 'D', value_name = "NAME[=VAL]")]
    pub defines: Vec<String>,

    /// Undefine a macro. Repeatable.
    #[arg(short = 'U', value_name = "NAME")]
    pub undefines: Vec<String>,

    /// Force a language profile (c_family, hash, lua) instead of per-file detection.
    #[arg(short = 'P', long)]
    pub profile: Option<String>,

    /// Skip `.alpp` config discovery (macro deny/allow rules and default defines).
    #[arg(long)]
    pub no_config: bool,

    /// Override the directive prefix for all files (e.g. "//#").
    #[arg(long)]
    pub prefix: Option<String>,

    /// Write results back to files. Without this flag, alpp runs as a
    /// dry-run and prints a unified diff to stdout.
    #[arg(short = 'w', long)]
    pub write: bool,

    /// With --write: mirror the rewritten tree to <DIR> instead of in-place.
    #[arg(short = 'o', long, value_name = "DIR", requires = "write")]
    pub output: Option<PathBuf>,

    /// Only validate directives (balanced blocks, strict mode); never rewrite.
    /// Exit non-zero if any file is invalid. Intended for CI gates.
    #[arg(long, conflicts_with_all = ["gate", "write", "output"])]
    pub check: bool,

    /// 发布门禁：扫描文件中残留条件编译指令时退出 3。
    /// 同时校验指令；仍遵循文件过滤规则，不改写文件。
    #[arg(long, conflicts_with_all = ["write", "output", "check"])]
    pub gate: bool,

    /// Treat unknown `#`-prefixed comment lines that look like directives as errors.
    #[arg(long)]
    pub strict: bool,

    /// Worker threads for file-level parallelism. Defaults to available cores.
    #[arg(short = 'j', long)]
    pub threads: Option<usize>,

    /// Walk files ignored by .gitignore as well.
    #[arg(long)]
    pub no_ignore: bool,

    /// Skip files larger than this size in bytes.
    #[arg(long, default_value_t = 64 * 1024 * 1024, value_name = "BYTES")]
    pub max_size: u64,

    /// Print the full configuration before running.
    #[arg(long)]
    pub verbose: bool,
}
