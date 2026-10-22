//! 输出生成：dry-run 的 unified diff（DESIGN §4.1）。
//!
//! ALPP 的编辑只有"整行删除"，因此 diff 不需要通用 LCS：删除行号区间已知，
//! 每个连续删除区间生成一个 hunk，前后带 3 行上下文。

use std::path::Path;

const CTX: usize = 3;

/// 生成保留原始内容字节的 unified diff。`deleted` 必须是升序的 1-based 行号。
///
/// 语义对齐 GNU diff -u（实测确认）：
/// - 相邻删除区间的间隔（未删行数）≤ 2×上下文 时合并为一个 hunk；
/// - 新侧起始行 = 旧侧起始行 − 位于其之前的已删行数。
pub fn unified_diff(path: &Path, old: &[u8], deleted: &[usize]) -> Vec<u8> {
    let lines: Vec<&[u8]> = split_lines(old);
    let total = lines.len();

    let mut out = Vec::new();
    out.extend_from_slice(format!("--- {}\n", patch_path(b'a', path)).as_bytes());
    out.extend_from_slice(format!("+++ {}\n", patch_path(b'b', path)).as_bytes());

    for (hunk_start, hunk_end) in hunk_ranges(deleted) {
        // 1-based 闭区间 [hunk_start, hunk_end]
        let ctx_before = hunk_start.saturating_sub(1).min(CTX);
        let ctx_start = hunk_start - ctx_before; // 1-based
        let ctx_after = (hunk_end + CTX).min(total);
        let n_old = ctx_before + (hunk_end - hunk_start + 1) + (ctx_after - hunk_end);
        let n_del = deleted.partition_point(|&n| n <= hunk_end)
            - deleted.partition_point(|&n| n < hunk_start);
        let n_new = n_old - n_del;
        let new_start = ctx_start - deleted.partition_point(|&n| n < ctx_start);

        out.extend_from_slice(
            format!("@@ -{},{} +{},{} @@\n", ctx_start, n_old, new_start, n_new).as_bytes(),
        );

        for no in ctx_start..=ctx_after {
            let body = lines[no - 1];
            if deleted.binary_search(&no).is_ok() {
                out.push(b'-');
            } else {
                out.push(b' ');
            }
            out.extend_from_slice(body);
            out.push(b'\n');
            if no == total && !old.ends_with(b"\n") {
                out.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
    }

    out
}

/// 把升序行号切成 hunk 区间（1-based 闭区间）。
/// 间隔 ≤ 2×上下文 的相邻删除区间合并（GNU diff 行为，避免 hunk 上下文交叠）。
fn hunk_ranges(deleted: &[usize]) -> Vec<(usize, usize)> {
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &n in deleted {
        match hunks.last_mut() {
            Some((_, e)) if n - *e <= 1 + 2 * CTX => *e = n,
            _ => hunks.push((n, n)),
        }
    }
    hunks
}

/// Git 的 C 风格路径引用；非 ASCII 字节用八进制，避免有损 UTF-8 转换。
fn patch_path(side: u8, path: &Path) -> String {
    use std::fmt::Write;
    // walker 的 ./file 路径含 CurDir，Git 会将其判为非法补丁路径。
    let path: std::path::PathBuf = path
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    let mut bytes = vec![side, b'/'];
    bytes.extend_from_slice(path.as_os_str().as_encoded_bytes());
    #[cfg(windows)]
    for byte in &mut bytes {
        if *byte == b'\\' {
            *byte = b'/';
        }
    }
    if bytes
        .iter()
        .all(|b| (b'!'..=b'~').contains(b) && !matches!(b, b'"' | b'\\'))
    {
        return String::from_utf8(bytes).unwrap();
    }
    let mut quoted = String::from("\"");
    for byte in bytes {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b' '..=b'~' => quoted.push(char::from(byte)),
            _ => write!(quoted, "\\{byte:03o}").unwrap(),
        }
    }
    quoted.push('"');
    quoted
}

fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    // 仅拆 LF；CR（包括末尾孤立的 CR）属于补丁正文，必须保留。
    let mut lines = Vec::new();
    let mut start = 0;
    for nl in memchr::memchr_iter(b'\n', bytes) {
        lines.push(&bytes[start..nl]);
        start = nl + 1;
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn diff(path: &Path, old: &[u8], deleted: &[usize]) -> String {
        String::from_utf8(unified_diff(path, old, deleted)).unwrap()
    }

    fn p() -> PathBuf {
        PathBuf::from("demo.js")
    }

    #[test]
    fn single_run_hunk() {
        let old = b"l1\nl2\nl3\nl4\nl5\n";
        let d = diff(p().as_path(), old, &[3]);
        // ctx_before=2, del=1, ctx_after=2 → old 5 行, new 4 行
        let expect = "--- a/demo.js\n+++ b/demo.js\n@@ -1,5 +1,4 @@\n l1\n l2\n-l3\n l4\n l5\n";
        assert_eq!(d, expect);
    }

    #[test]
    fn context_shrinks_near_edges() {
        let old = b"l1\nl2\n";
        let d = diff(p().as_path(), old, &[1, 2]);
        // 删除 1..2：前面无上下文，后面只有 0 行
        let expect = "--- a/demo.js\n+++ b/demo.js\n@@ -1,2 +1,0 @@\n-l1\n-l2\n";
        assert_eq!(d, expect);
    }

    #[test]
    fn nearby_runs_merge_into_one_hunk() {
        // deleted=[2,3,4,8]：间隔 3 行 ≤ 2×上下文 → GNU diff 合并为单 hunk
        let old: Vec<u8> = (1..=10)
            .flat_map(|i| format!("l{i}\n").into_bytes())
            .collect();
        let d = diff(p().as_path(), &old, &[2, 3, 4, 8]);
        // ctx [1..10]，old 10 行，删 4 行 → new 6 行，起始不变
        let expect = "@@ -1,10 +1,6 @@";
        assert!(d.contains(expect), "missing {expect} in:\n{d}");
        assert!(d.contains("-l2\n-l3\n-l4\n"));
        assert!(d.contains("-l8\n"));
    }

    #[test]
    fn far_runs_two_hunks_new_side_renumbers() {
        // 删 2,3,4 与 12：间隔 7 行 > 2×上下文 → 两个 hunk（对齐 GNU diff 实测输出）
        let old: Vec<u8> = (1..=16)
            .flat_map(|i| format!("l{i}\n").into_bytes())
            .collect();
        let d = diff(p().as_path(), &old, &[2, 3, 4, 12]);
        // hunk1: ctx [1..7] → @@ -1,7 +1,4 @@
        assert!(d.contains("@@ -1,7 +1,4 @@"), "missing in:\n{d}");
        // hunk2: del 12, ctx [9..15]，前面已删 3 行 → 新侧 9-3=6 起，7 行删 1
        assert!(d.contains("@@ -9,7 +6,6 @@"), "missing in:\n{d}");
        assert!(d.contains("-l12\n"));
    }

    #[test]
    fn crlf_content_line_bodies_preserve_cr() {
        let old = b"a\r\nb\r\nc\r\n";
        let d = diff(p().as_path(), old, &[2]);
        assert_eq!(
            d,
            "--- a/demo.js\n+++ b/demo.js\n@@ -1,3 +1,2 @@\n a\r\n-b\r\n c\r\n"
        );
    }

    #[test]
    fn no_newline_marker() {
        let old = b"a\nb"; // 末行无换行且被删
        let d = diff(p().as_path(), old, &[2]);
        assert!(d.contains("\\ No newline at end of file\n"));
    }
}
