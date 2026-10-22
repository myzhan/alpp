//! 文件发现：显式路径 + 目录递归（gitignore 感知），按扩展名绑定 Profile。
//! 二进制剔除与大小上限在收集阶段完成（DESIGN §4.3）。

use crate::profile::{self, Profile};
use std::path::{Path, PathBuf};

/// 一个待处理文件及其绑定的语言 Profile。
#[derive(Debug, Clone)]
pub struct FileJob {
    pub path: PathBuf,
    pub profile: Profile<'static>,
}

/// 收集结果：待处理文件 + 被跳过文件（原因）+ 未处理穿透文件。
/// skipped 项仅在 verbose 下展示；passthrough（未知扩展名）与 skipped（二进制/超限）
/// 在 `-o` 镜像模式下原样复制，保证镜像树完整（DESIGN §4.7）。
#[derive(Debug, Default)]
pub struct Collection {
    pub jobs: Vec<FileJob>,
    pub skipped: Vec<(PathBuf, String)>,
    pub passthrough: Vec<PathBuf>,
}

/// 收集待处理文件。`force` 非空时（--profile）对所有发现的文件强制使用该 Profile，
/// 包括目录遍历中的未知扩展名文件（二进制/超限仍跳过）。
pub fn collect(
    paths: &[PathBuf],
    max_size: u64,
    no_ignore: bool,
    force: Option<Profile<'static>>,
    excluded: Option<&Path>,
) -> Result<Collection, std::io::Error> {
    let mut col = Collection::default();
    for p in paths {
        if excluded.is_some_and(|root| p.canonicalize().is_ok_and(|p| p.starts_with(root))) {
            continue;
        }
        let meta = std::fs::metadata(p)
            .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", p.display())))?;
        if meta.is_file() {
            // 显式指定的文件：未知扩展名回退 c_family 并提示（DESIGN §4.2）
            let profile = match force {
                Some(p) => p,
                None => profile::by_extension(extension_of(p)).unwrap_or_else(|| {
                    eprintln!(
                        "alpp: {}: unknown extension -> forced c_family",
                        p.display()
                    );
                    profile::C_FAMILY
                }),
            };
            collect_file(p, &meta, profile, max_size, &mut col)?;
        } else if meta.is_dir() {
            collect_dir(p, max_size, no_ignore, force, excluded, &mut col)?;
        }
    }
    Ok(col)
}

fn collect_dir(
    dir: &Path,
    max_size: u64,
    no_ignore: bool,
    force: Option<Profile<'static>>,
    excluded: Option<&Path>,
    col: &mut Collection,
) -> Result<(), std::io::Error> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .hidden(true)
        .git_ignore(!no_ignore)
        .git_global(!no_ignore);
    if let Some(root) = excluded {
        let root = root.to_path_buf();
        builder.filter_entry(move |entry| {
            !entry
                .path()
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&root))
        });
    }
    for entry in builder.build() {
        let entry = entry.map_err(|e| std::io::Error::other(e.to_string()))?;
        let path = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let profile = match force {
            Some(p) => p,
            None => match profile::by_extension(extension_of(path)) {
                Some(p) => p,
                None => {
                    // 目录遍历中未知扩展名静默跳过；记录供镜像补齐
                    col.passthrough.push(path.to_path_buf());
                    continue;
                }
            },
        };
        let meta = std::fs::metadata(path)?;
        collect_file(path, &meta, profile, max_size, col)?;
    }
    Ok(())
}

/// 显式文件和目录发现的文件共用过滤规则。
fn collect_file(
    path: &Path,
    meta: &std::fs::Metadata,
    profile: Profile<'static>,
    max_size: u64,
    col: &mut Collection,
) -> std::io::Result<()> {
    if meta.len() > max_size {
        col.skipped.push((
            path.to_path_buf(),
            format!("file too large (> {max_size}B)"),
        ));
    } else if is_binary(path)? {
        col.skipped.push((path.to_path_buf(), "binary file".into()));
    } else {
        col.jobs.push(FileJob {
            path: path.to_path_buf(),
            profile,
        });
    }
    Ok(())
}

fn extension_of(p: &Path) -> &str {
    p.extension().and_then(|e| e.to_str()).unwrap_or("")
}

/// 头部 8KB 含 NUL 视为二进制（与 git 判定一致）。
fn is_binary(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 8192];
    let n = f.read(&mut head)?;
    Ok(head[..n].contains(&0))
}
