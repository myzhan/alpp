//! `.alpp` 配置文件：项目级指令语法管控与默认宏，优先级低于命令行（SPEC §10）。
//!
//! 发现规则：从第一个输入路径（目录本身；文件则取其父目录）向上逐级查找
//! `.alpp`，到文件系统根为止，找到即停；未找到视为空配置（DESIGN §4.8）。
//! 多输入路径时以第一个为锚，全进程一份配置。
//!
//! 配置只做**校验与宏表初值**，不参与条件求值状态机——与文件级无锁并行模型兼容。
//! `[directives]` deny/allow 是硬约束：无论指令位于活跃还是被抑制分支，出现即 E8。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// `.alpp` 配置内容（TOML）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub macros: MacrosSection,
    #[serde(default)]
    pub directives: DirectivesSection,
}

/// 合法指令关键字（SPEC §2）；`[directives]` 节只接受这些名字。
pub const DIRECTIVE_NAMES: [&str; 9] = [
    "if", "ifdef", "ifndef", "elif", "else", "endif", "define", "undef", "error",
];

/// `[macros]` 节：默认宏初值（优先级低于命令行）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacrosSection {
    /// 默认宏初值，`NAME` 或 `NAME=VALUE`；先于命令行 -D/-U 应用（优先级最低）。
    #[serde(default)]
    pub define: Vec<String>,
    /// 默认取消定义；先于命令行 -U 应用。
    #[serde(default)]
    pub undefine: Vec<String>,
}

/// `[directives]` 节：指令级语法管控（限制项目允许使用哪些宏语句）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectivesSection {
    /// 指令黑名单：文件内出现该指令即 E9，无论分支是否活跃（SPEC §7）。
    #[serde(default)]
    pub deny: Vec<String>,
    /// 指令白名单（可选）：设置后，不在名单内的指令一律 E9；deny 命中优先。
    #[serde(default)]
    pub allow: Option<Vec<String>>,
}

/// 已加载的配置：内容 + 来源路径（诊断信息用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub path: PathBuf,
    pub cfg: Config,
}

/// 解析 TOML 文本；宏名与指令名非法即错误（配置自身不许写坏名字）。
pub fn parse(text: &str) -> Result<Config, String> {
    let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    let m = &cfg.macros;
    let names = m
        .define
        .iter()
        .map(|d| name_part(d))
        .chain(m.undefine.iter().map(String::as_str));
    for name in names {
        if !crate::eval::is_valid_name(name.as_bytes()) {
            return Err(format!("invalid macro name `{name}`"));
        }
    }
    for name in cfg
        .directives
        .deny
        .iter()
        .chain(cfg.directives.allow.iter().flatten())
    {
        if !DIRECTIVE_NAMES.contains(&name.as_str()) {
            return Err(format!("invalid directive name `{name}`"));
        }
    }
    Ok(cfg)
}

/// `NAME=VALUE` 中的 NAME 部分。
fn name_part(define: &str) -> &str {
    define.split_once('=').map_or(define, |(n, _)| n)
}

/// 从 `start`（目录；文件则取其父目录）向上查找 `.alpp`，找到即停。
pub fn discover(start: &Path) -> Result<Option<Loaded>, String> {
    // 先转为绝对路径，避免相对路径的 parent() 在当前目录提前终止。
    let start = std::path::absolute(start).map_err(|e| e.to_string())?;
    let mut dir = if start.is_file() {
        start
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        let candidate = dir.join(".alpp");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate)
                .map_err(|e| format!("read {}: {e}", candidate.display()))?;
            let cfg = parse(&text).map_err(|e| format!("{}: {e}", candidate.display()))?;
            return Ok(Some(Loaded {
                path: candidate,
                cfg,
            }));
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return Ok(None),
        }
    }
}

impl Config {
    /// 单个指令关键字的管控校验；deny 命中优先于 allow 名单（SPEC §7 E8）。
    pub fn check_directive(&self, name: &str) -> Result<(), String> {
        if self.directives.deny.iter().any(|d| d == name) {
            return Err(format!("directive `{name}` is denied by .alpp config"));
        }
        if let Some(allow) = &self.directives.allow {
            if !allow.iter().any(|a| a == name) {
                return Err(format!("directive `{name}` is not in the allow list"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_and_defaults() {
        let c = parse("[macros]\ndefine = [\"C=2\"]\nundefine = [\"D\"]\n").unwrap();
        assert_eq!(c.macros.define, vec!["C=2"]);
        assert_eq!(c.macros.undefine, vec!["D"]);

        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn parse_rejects_unknown_fields_and_bad_names() {
        assert!(parse("[macros]\ndenyd = []\n").is_err(), "未知字段拒绝");
        assert!(parse("[unknown]\nx = 1\n").is_err());
        assert!(
            parse("[macros]\ndefine = [\"9bad\"]\n").is_err(),
            "非法宏名"
        );
        assert!(parse("[macros]\ndefine = [\"=5\"]\n").is_err());
        assert!(parse("not toml [").is_err());
    }

    #[test]
    fn parse_directives_and_reject_bad_names() {
        let c =
            parse("[directives]\ndeny = [\"define\", \"undef\"]\nallow = [\"ifdef\", \"endif\"]\n")
                .unwrap();
        assert_eq!(c.directives.deny, vec!["define", "undef"]);
        assert_eq!(
            c.directives.allow,
            Some(vec!["ifdef".into(), "endif".into()])
        );
        // 默认空配置
        assert_eq!(
            parse("[macros]\n").unwrap().directives,
            DirectivesSection::default()
        );
        // 非法指令名（不在关键字表）拒绝
        assert!(parse("[directives]\ndeny = [\"ifeq\"]\n").is_err());
        assert!(
            parse("[directives]\nallow = [\"IFDEF\"]\n").is_err(),
            "大小写敏感"
        );
    }

    #[test]
    fn check_directive_deny_over_allow() {
        let c = parse("[directives]\ndeny = [\"if\"]\nallow = [\"if\", \"ifdef\", \"endif\"]\n")
            .unwrap();
        assert!(c.check_directive("if").is_err(), "deny 优先于 allow");
        assert!(c.check_directive("ifdef").is_ok());
        assert!(c.check_directive("define").is_err(), "allow 外拒绝");
        // 无限制时全放行
        assert!(Config::default().check_directive("define").is_ok());
    }

    #[test]
    fn discover_walks_up_and_stops_at_first() {
        let root = std::env::temp_dir().join(format!("alpp-cfg-{}", std::process::id()));
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(root.join(".alpp"), "[directives]\ndeny = [\"error\"]\n").unwrap();
        std::fs::write(
            sub.parent().unwrap().join(".alpp"),
            "[directives]\ndeny = [\"define\"]\n",
        )
        .unwrap();

        // 从叶子向上：取最近一级
        let found = discover(&sub).unwrap().unwrap();
        assert_eq!(found.cfg.directives.deny, vec!["define"]);
        // 文件路径取父目录
        let f = sub.join("x.js");
        std::fs::write(&f, "").unwrap();
        assert_eq!(
            discover(&f).unwrap().unwrap().cfg.directives.deny,
            vec!["define"]
        );
        // 无配置目录 → None
        let empty = std::env::temp_dir().join(format!("alpp-nocfg-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(discover(&empty).unwrap().is_none());

        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&empty).unwrap();
    }
}
