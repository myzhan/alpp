//! 行扫描与指令识别（MACRO_SPEC §2 词法，DESIGN §4.4）。
//!
//! 生产入口 iter_lines 使用 memchr 定位换行，classify 校验行首前缀和精确关键字。
//! lines_with_anchor 保留为锚字节预筛的基准入口，尚未接入处理流水线。

use crate::profile::Profile;
use memchr::memchr_iter;

/// 一行的切片视图：`content` 不含行尾，`total_end` 含行尾（`\n` 或 `\r\n`）。
#[derive(Debug, Clone, Copy)]
pub struct Line<'a> {
    pub no: usize,    // 1-based
    pub start: usize, // 行起点绝对偏移
    pub content: &'a [u8],
    pub total_end: usize, // 绝对偏移，指向行尾符之后
}

/// 逐行迭代；`\r\n` 与 `\n` 都识别，末行无行尾符也产出。
///
/// 实现为结构体而非闭包链：行号/游标状态必须唯一（闭包链会因 `usize: Copy`
/// 被 `move` 复制成两份独立状态，导致尾部重复迭代）。
pub struct LineIter<'a> {
    bytes: &'a [u8],
    pos: usize,
    no: usize,
}

impl<'a> Iterator for LineIter<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Line<'a>> {
        if self.pos >= self.bytes.len() {
            return None;
        }
        let start = self.pos;
        self.no += 1;
        match memchr::memchr(b'\n', &self.bytes[start..]) {
            Some(off) => {
                let nl = start + off;
                self.pos = nl + 1;
                let mut content_end = nl;
                if content_end > start && self.bytes[content_end - 1] == b'\r' {
                    content_end -= 1;
                }
                Some(Line {
                    no: self.no,
                    start,
                    content: &self.bytes[start..content_end],
                    total_end: nl + 1,
                })
            }
            None => {
                self.pos = self.bytes.len();
                let mut content_end = self.bytes.len();
                if content_end > start && self.bytes[content_end - 1] == b'\r' {
                    content_end -= 1;
                }
                Some(Line {
                    no: self.no,
                    start,
                    content: &self.bytes[start..content_end],
                    total_end: self.bytes.len(),
                })
            }
        }
    }
}

pub fn iter_lines(bytes: &[u8]) -> LineIter<'_> {
    LineIter {
        bytes,
        pos: 0,
        no: 0,
    }
}

/// 块级预筛：返回所有含锚字节的行号（升序、去重）。
/// 仅用于把"绝大多数不含锚字节的行"排除出精确匹配路径。
///
/// 复杂度 O(N)：相邻锚点之间的区间互不重叠，换行只在增量区间内统计，
/// 绝不能对每个锚点重扫前缀（那是 O(A×N)，锚点密集时是平方级灾难）。
pub fn lines_with_anchor(bytes: &[u8], anchor: u8) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut line_no = 1usize;
    let mut scan_from = 0usize; // 上一锚点之后：换行统计的增量区间起点
    for pos in memchr_iter(anchor, bytes) {
        line_no += memchr_iter(b'\n', &bytes[scan_from..pos]).count();
        lines.push(line_no);
        scan_from = pos + 1;
    }
    lines.dedup(); // 同一行多个锚字节只记一次
    lines
}

/// 已识别的指令（参数保留原始字节切片，校验在 parser/eval 完成）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive<'a> {
    If(&'a [u8]),     // 表达式
    Ifdef(&'a [u8]),  // 宏名
    Ifndef(&'a [u8]), // 宏名
    Elif(&'a [u8]),   // 表达式
    Else,
    Endif,
    Define {
        name: &'a [u8],
        value: Option<&'a [u8]>,
    },
    Undef(&'a [u8]),
    Error(&'a [u8]), // 消息，可为空
}

/// 指令识别错误：strict 模式下的未知指令（E4）与结构非法的指令（E7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    pub code: u8, // 4 或 7
    pub col: usize,
    pub message: String,
}

/// 识别一行是否为指令（SPEC §2）。
///
/// 返回 `Ok(None)` 表示"不是指令"（含字符串字面量豁免与宽松模式下的未知指令）。
pub fn classify<'a>(
    line: &'a [u8],
    profile: &Profile<'_>,
    strict: bool,
) -> Result<Option<Directive<'a>>, ScanError> {
    let indent = line
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let rest = &line[indent..];
    if !rest.starts_with(profile.prefix.as_bytes()) {
        return Ok(None);
    }
    let prefix_col = indent; // 列号（0-based）用于诊断
    let body = &rest[profile.prefix.len()..];

    // 按水平空白分词，禁止把 define_X、if(1) 等拆成合法指令和参数。
    let kw_len = body
        .iter()
        .take_while(|&&b| b != b' ' && b != b'\t')
        .count();
    let keyword = &body[..kw_len];
    let args_raw = &body[kw_len..];

    // 关键字合法性预检（真正的构造在下方）
    match keyword {
        b"ifdef" | b"ifndef" | b"define" | b"undef" | b"if" | b"elif" | b"else" | b"endif"
        | b"error" => {}
        _ => {
            // 未知指令：宽松模式当作普通注释（SPEC §1.4 保守解析）
            if strict {
                let text = String::from_utf8_lossy(keyword).into_owned();
                return Err(ScanError {
                    code: 4,
                    col: prefix_col,
                    message: format!("unknown directive `#{text}`"),
                });
            }
            return Ok(None);
        }
    };

    // 字符串字面量豁免（SPEC §2.3）：prefix 前存在未闭合引号 → 非指令行。
    // 能走到这里说明行首(缩进后)就是 prefix，即引号若存在必然始于本行且未闭合。
    if has_unclosed_quote(&line[..prefix_col]) {
        return Ok(None);
    }

    let args = trim_ascii(args_raw);

    let dir = match keyword {
        b"ifdef" => Directive::Ifdef(need_name(args, prefix_col)?),
        b"ifndef" => Directive::Ifndef(need_name(args, prefix_col)?),
        b"define" => {
            // #define NAME 或 #define NAME VALUE（VALUE 为剩余内容 trim）
            let (name, value) = match args.iter().position(|&b| b == b' ' || b == b'\t') {
                Some(sp) => {
                    let name = &args[..sp];
                    let value = trim_ascii(&args[sp + 1..]);
                    (need_name(name, prefix_col)?, Some(value))
                }
                None => (need_name(args, prefix_col)?, None),
            };
            Directive::Define { name, value }
        }
        b"undef" => Directive::Undef(need_name(args, prefix_col)?),
        b"if" => Directive::If(require_args(args, prefix_col)?),
        b"elif" => Directive::Elif(require_args(args, prefix_col)?),
        b"else" => {
            require_empty(args, prefix_col)?;
            Directive::Else
        }
        b"endif" => {
            require_empty(args, prefix_col)?;
            Directive::Endif
        }
        b"error" => Directive::Error(args), // 允许空消息
        _ => unreachable!("keyword table checked above"),
    };
    Ok(Some(dir))
}

fn require_args(args: &[u8], col: usize) -> Result<&[u8], ScanError> {
    if args.is_empty() {
        Err(ScanError {
            code: 7,
            col,
            message: "missing expression argument".into(),
        })
    } else {
        Ok(args)
    }
}

/// 校验参数是合法宏名（SPEC §3 文法），非法时返回 E7。
fn need_name(a: &[u8], col: usize) -> Result<&[u8], ScanError> {
    if crate::eval::is_valid_name(a) {
        Ok(a)
    } else {
        Err(ScanError {
            code: 7,
            col,
            message: format!("expected macro name, got `{}`", String::from_utf8_lossy(a)),
        })
    }
}

fn require_empty(args: &[u8], col: usize) -> Result<(), ScanError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(ScanError {
            code: 7,
            col,
            message: format!("unexpected trailing `{}`", String::from_utf8_lossy(args)),
        })
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|&b| b != b' ' && b != b'\t');
    match start {
        None => &[],
        Some(s) => {
            let end = bytes
                .iter()
                .rposition(|&b| b != b' ' && b != b'\t')
                .unwrap()
                + 1;
            &bytes[s..end]
        }
    }
}

/// 行内引号奇偶扫描（处理 `\"` `\'` 转义）。true = 存在未闭合引号。
fn has_unclosed_quote(bytes: &[u8]) -> bool {
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1, // 跳过转义字符
            b'"' | b'\'' => match quote {
                None => quote = Some(bytes[i]),
                Some(q) if q == bytes[i] => quote = None,
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
    quote.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{C_FAMILY, HASH};

    fn class<'a>(line: &'a str, p: &Profile<'_>) -> Option<Directive<'a>> {
        classify(line.as_bytes(), p, false).unwrap()
    }

    #[test]
    fn recognizes_all_directives() {
        assert_eq!(
            class("//#ifdef RELEASE", &C_FAMILY),
            Some(Directive::Ifdef(b"RELEASE"))
        );
        assert_eq!(
            class("//#ifndef A", &C_FAMILY),
            Some(Directive::Ifndef(b"A"))
        );
        assert_eq!(
            class("//#if DEBUG && !R", &C_FAMILY),
            Some(Directive::If(b"DEBUG && !R"))
        );
        assert_eq!(
            class("//#elif X > 2", &C_FAMILY),
            Some(Directive::Elif(b"X > 2"))
        );
        assert_eq!(class("//#else", &C_FAMILY), Some(Directive::Else));
        assert_eq!(class("//#endif", &C_FAMILY), Some(Directive::Endif));
        assert_eq!(
            class("//#define FOO", &C_FAMILY),
            Some(Directive::Define {
                name: b"FOO",
                value: None
            })
        );
        assert_eq!(
            class("//#define LEVEL 3", &C_FAMILY),
            Some(Directive::Define {
                name: b"LEVEL",
                value: Some(b"3")
            })
        );
        assert_eq!(
            class("//#undef FOO", &C_FAMILY),
            Some(Directive::Undef(b"FOO"))
        );
        assert_eq!(
            class("//#error no way", &C_FAMILY),
            Some(Directive::Error(b"no way"))
        );
        assert_eq!(class("//#error", &C_FAMILY), Some(Directive::Error(b"")));
    }

    #[test]
    fn indentation_and_prefix_rules() {
        assert_eq!(
            class("\t//#ifdef A", &C_FAMILY),
            Some(Directive::Ifdef(b"A"))
        );
        assert_eq!(
            class("    ##ifndef R", &HASH),
            Some(Directive::Ifndef(b"R"))
        );
        // 关键字与前缀之间不允许空白
        assert_eq!(class("//# ifdef A", &C_FAMILY), None);
        // 前缀前有非空白内容 → 非指令
        assert_eq!(class("x := 1 //#ifdef A", &C_FAMILY), None);
    }

    #[test]
    fn unknown_directives_loose_vs_strict() {
        assert_eq!(class("//#include <x.h>", &C_FAMILY), None); // 宽松：普通注释
        let err = classify(b"//#include <x.h>", &C_FAMILY, true).unwrap_err();
        assert_eq!(err.code, 4);
        assert!(err.message.contains("include"));
    }

    #[test]
    fn malformed_known_directives_are_e7() {
        assert_eq!(classify(b"//#ifdef", &C_FAMILY, false).unwrap_err().code, 7);
        assert_eq!(
            classify(b"//#ifdef 1BAD", &C_FAMILY, false)
                .unwrap_err()
                .code,
            7
        );
        assert_eq!(classify(b"//#if", &C_FAMILY, false).unwrap_err().code, 7);
        assert_eq!(
            classify(b"//#else x", &C_FAMILY, false).unwrap_err().code,
            7
        );
        assert_eq!(
            classify(b"//#endif now", &C_FAMILY, false)
                .unwrap_err()
                .code,
            7
        );
    }

    #[test]
    fn string_literal_exemption() {
        // 行首即未闭合字符串，前缀在字符串内 → 豁免
        assert_eq!(
            class("\"usage: //#ifdef RELEASE to strip\"", &C_FAMILY),
            None
        );
        assert_eq!(class("'//#ifndef X'", &C_FAMILY), None);
        assert_eq!(class("\"a\\\"//#ifdef X\"", &C_FAMILY), None); // 转义引号
    }

    #[test]
    fn hash_profile_python() {
        assert_eq!(
            class("##ifndef RELEASE", &HASH),
            Some(Directive::Ifndef(b"RELEASE"))
        );
        // 单 # 是普通注释（filepp 的误伤点，必须不识别）
        assert_eq!(class("# ifndef RELEASE", &HASH), None);
        assert_eq!(class("#include <x>", &HASH), None);
    }

    #[test]
    fn iter_lines_handles_crlf_and_last_line() {
        let bytes = b"a\r\nbb\nccc\nd";
        let lines: Vec<Line<'_>> = iter_lines(bytes).collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].content, b"a");
        assert_eq!(lines[1].content, b"bb");
        assert_eq!(lines[3].content, b"d"); // 末行无换行
        assert_eq!(lines[3].total_end, bytes.len());
        assert_eq!(lines[2].no, 3);
    }

    #[test]
    fn anchor_prescan_finds_lines() {
        let bytes = b"abc\ndef //#ifdef X\nghi\n//#endif\n";
        assert_eq!(lines_with_anchor(bytes, b'/'), vec![2, 4]);
    }

    #[test]
    fn anchor_prescan_dense_hits_and_multibyte_lines() {
        // 同一行多个锚字节、连续行命中、空行：验证增量换行统计的正确性
        let bytes = b"//a\nb//c//\n\n//d\n";
        assert_eq!(lines_with_anchor(bytes, b'/'), vec![1, 2, 4]);
        // 行尾锚点与末行无换行的锚点
        assert_eq!(lines_with_anchor(b"x/\ny/\n", b'/'), vec![1, 2]);
        assert_eq!(lines_with_anchor(b"ab\nz/", b'/'), vec![2]);
    }
}
