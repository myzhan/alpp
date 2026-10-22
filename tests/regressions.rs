//! 已复现问题的 CLI 回归；源码与期望输出均来自 fixture。
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_alpp");

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn put(root: &Path, path: &str, name: &str) -> PathBuf {
    let dest = root.join(path);
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(&dest, fixture(name)).unwrap();
    dest
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn success(o: &Output) {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn mirror_absolute_and_parent_inputs_never_modify_sources() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "src/input.js", "ts/regressions/input.js");
    let cwd = t.path().join("work");
    fs::create_dir(&cwd).unwrap();
    for input in [src.to_str().unwrap(), "../src/input.js", "../src"] {
        success(&run(&cwd, &["--no-config", "-w", "-o", "out", input]));
        assert_eq!(fs::read(&src).unwrap(), fixture("ts/regressions/input.js"));
        assert_eq!(
            fs::read(cwd.join("out/input.js")).unwrap(),
            fixture("ts/regressions/write.js")
        );
    }
}

#[test]
fn mirror_absolute_input_within_cwd_preserves_relative_layout() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "sub/input.js", "ts/regressions/input.js");
    success(&run(
        t.path(),
        &["--no-config", "-w", "-o", "out", src.to_str().unwrap()],
    ));
    assert_eq!(
        fs::read(t.path().join("out/sub/input.js")).unwrap(),
        fixture("ts/regressions/write.js")
    );
    assert_eq!(fs::read(src).unwrap(), fixture("ts/regressions/input.js"));
}

#[test]
fn mirror_repeated_runs_exclude_output_for_processed_and_passthrough_files() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), "input.js", "ts/regressions/input.js");
    put(t.path(), "notes.txt", "ts/regressions/input.js");
    put(t.path(), "binary.js", "ts/regressions/binary.js");
    for _ in 0..3 {
        success(&run(t.path(), &["--no-config", "-w", "-o", "out", "."]));
        assert!(!t.path().join("out/out").exists());
        assert_eq!(
            fs::read(t.path().join("out/input.js")).unwrap(),
            fixture("ts/regressions/write.js")
        );
        assert_eq!(
            fs::read(t.path().join("out/notes.txt")).unwrap(),
            fixture("ts/regressions/input.js")
        );
        assert_eq!(
            fs::read(t.path().join("out/binary.js")).unwrap(),
            fixture("ts/regressions/binary.js")
        );
    }
}

#[test]
fn mirror_rejects_source_root_before_writing() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    for input in [".", "input.js"] {
        let o = run(t.path(), &["--no-config", "-w", "-o", ".", input]);
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(fs::read(&src).unwrap(), fixture("ts/regressions/input.js"));
    }
}

#[test]
fn mirror_rejects_conflicting_external_inputs_before_writing() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), "one/input.js", "ts/regressions/input.js");
    put(t.path(), "two/input.js", "ts/regressions/write.js");
    let cwd = t.path().join("work");
    fs::create_dir(&cwd).unwrap();
    let o = run(
        &cwd,
        &["--no-config", "-w", "-o", "out", "../one", "../two"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(!cwd.join("out").exists());
}

#[test]
fn mirror_explicit_unknown_file_is_not_overwritten_by_passthrough() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), "input.txt", "ts/regressions/input.js");
    success(&run(
        t.path(),
        &["--no-config", "-w", "-o", "out", "input.txt", "."],
    ));
    assert_eq!(
        fs::read(t.path().join("out/input.txt")).unwrap(),
        fixture("ts/regressions/write.js")
    );
}

#[cfg(unix)]
#[test]
fn mirror_rejects_destination_symlink_escape_before_writing() {
    let t = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    put(t.path(), "sub/input.js", "ts/regressions/input.js");
    let protected = put(external.path(), "input.js", "ts/regressions/input.js");
    fs::create_dir(t.path().join("out")).unwrap();
    std::os::unix::fs::symlink(external.path(), t.path().join("out/sub")).unwrap();
    let o = run(t.path(), &["--no-config", "-w", "-o", "out", "sub"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        fs::read(protected).unwrap(),
        fixture("ts/regressions/input.js")
    );
}

#[test]
fn config_discovery_from_subdirectory_matches_absolute_anchor() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".alpp", "ts/regressions/deny.alpp");
    let src = put(t.path(), "src/input.js", "ts/regressions/input.js");
    let cwd = src.parent().unwrap();
    for input in [
        ".",
        "input.js",
        cwd.to_str().unwrap(),
        src.to_str().unwrap(),
    ] {
        let o = run(cwd, &["--check", input]);
        assert_eq!(o.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&o.stderr).contains("E8"));
        assert!(o.stdout.is_empty());
    }
}

#[test]
fn config_defaults_are_loaded_from_parent_of_cwd() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".alpp", "ts/regressions/defaults.alpp");
    let src = put(t.path(), "src/input.js", "ts/regressions/input.js");
    success(&run(src.parent().unwrap(), &["-w", "input.js"]));
    assert_eq!(
        fs::read(src).unwrap(),
        fixture("ts/regressions/write-flag.js")
    );
}

#[test]
fn write_does_not_clobber_existing_temp_filename() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    let other = put(t.path(), "input.alpp-tmp", "ts/regressions/keywords.js");
    success(&run(t.path(), &["--no-config", "-w", "input.js"]));
    assert_eq!(fs::read(src).unwrap(), fixture("ts/regressions/write.js"));
    assert_eq!(
        fs::read(other).unwrap(),
        fixture("ts/regressions/keywords.js")
    );
    assert_eq!(fs::read_dir(t.path()).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn write_and_mirror_preserve_executable_mode() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.sh", "shell/regressions/input.sh");
    fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();
    success(&run(
        t.path(),
        &["--no-config", "-w", "-o", "out", "input.sh"],
    ));
    success(&run(t.path(), &["--no-config", "-w", "input.sh"]));
    for path in [src, t.path().join("out/input.sh")] {
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read(path).unwrap(),
            fixture("shell/regressions/write.sh")
        );
    }
}

#[test]
fn scanner_requires_exact_keyword_boundary() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "keywords.js", "ts/regressions/keywords.js");
    let strict = run(
        t.path(),
        &["--no-config", "--strict", "--check", "keywords.js"],
    );
    assert_eq!(strict.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&strict.stderr).contains("E4"));
    success(&run(t.path(), &["--no-config", "-w", "keywords.js"]));
    assert_eq!(
        fs::read(src).unwrap(),
        fixture("ts/regressions/write-keywords.js")
    );
}

#[test]
fn empty_prefix_fails_without_rewriting() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    let o = run(t.path(), &["--no-config", "--prefix", "", "-w", "input.js"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(fs::read(src).unwrap(), fixture("ts/regressions/input.js"));
}

#[test]
fn explicit_and_discovered_files_share_binary_and_size_limits() {
    let t = tempfile::tempdir().unwrap();
    let binary = put(t.path(), "binary.js", "ts/regressions/binary.js");
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    for input in [".", "binary.js"] {
        success(&run(t.path(), &["--no-config", "-w", input]));
        assert_eq!(
            fs::read(&binary).unwrap(),
            fixture("ts/regressions/binary.js")
        );
    }
    // 重新放回源码，验证大小过滤在两种入口一致。
    for input in [".", "input.js"] {
        put(t.path(), "input.js", "ts/regressions/input.js");
        success(&run(
            t.path(),
            &["--no-config", "--max-size", "1", "-w", input],
        ));
        assert_eq!(fs::read(&src).unwrap(), fixture("ts/regressions/input.js"));
    }
}

#[test]
fn custom_prefix_processes_unknown_extensions_in_directory_walk() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.txt", "ts/regressions/custom.txt");
    success(&run(
        t.path(),
        &["--no-config", "--prefix", ";#", "-w", "."],
    ));
    assert_eq!(
        fs::read(src).unwrap(),
        fixture("ts/regressions/write-custom.txt")
    );
}

#[test]
fn diff_for_retained_unterminated_tail_applies_to_write_output() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    let o = run(t.path(), &["--no-config", "input.js"]);
    success(&o);
    assert_eq!(o.stdout, fixture("ts/regressions/input.diff"));
    let patch = t.path().join("change.patch");
    fs::write(&patch, &o.stdout).unwrap();
    let applied = Command::new("git")
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.eol=lf",
            "apply",
            "--no-index",
        ])
        .arg(&patch)
        .current_dir(t.path())
        .output()
        .unwrap();
    success(&applied);
    assert_eq!(fs::read(src).unwrap(), fixture("ts/regressions/write.js"));
}

#[test]
fn stdout_write_failure_is_exit_1() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    let o = Command::new(BIN)
        .args(["--no-config", "input.js"])
        .current_dir(t.path())
        .stdout(Stdio::from(fs::File::open(&src).unwrap()))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("stdout"));
    assert_eq!(fs::read(src).unwrap(), fixture("ts/regressions/input.js"));
}

#[test]
fn check_rejects_write_and_output_without_side_effects() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "input.js", "ts/regressions/input.js");
    let o = run(
        t.path(),
        &["--no-config", "--check", "-w", "-o", "out", "input.js"],
    );
    assert!(!o.status.success());
    assert!(!t.path().join("out").exists());
    assert_eq!(fs::read(src).unwrap(), fixture("ts/regressions/input.js"));
}

#[test]
fn batches_preserve_diff_and_error_order_across_thread_counts() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..65 {
        put(t.path(), &format!("f{i:02}.js"), "ts/regressions/input.js");
    }
    for mode in [None, Some("--check"), Some("--gate")] {
        let mut args = vec!["--no-config", "-j", "1", "."];
        if let Some(mode) = mode {
            args.push(mode);
        }
        let one = run(t.path(), &args);
        args[2] = "4";
        let four = run(t.path(), &args);
        assert_eq!(one.status.code(), four.status.code());
        assert_eq!(one.stdout, four.stdout);
        assert_eq!(one.stderr, four.stderr);
    }
}

#[cfg(unix)]
#[test]
fn symlink_aliases_keep_their_profiles_and_write_targets() {
    use std::os::unix::fs::symlink;
    let t = tempfile::tempdir().unwrap();
    put(t.path(), "z.lua", "lua/regressions/path.lua");
    symlink("z.lua", t.path().join("a.txt")).unwrap();
    for jobs in ["1", "4"] {
        let gate = run(
            t.path(),
            &["--no-config", "--gate", "-j", jobs, "a.txt", "z.lua"],
        );
        assert_eq!(gate.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&gate.stderr).contains("z.lua:1: G1"));
    }
    put(t.path(), "z.js", "ts/regressions/input.js");
    symlink("z.js", t.path().join("a.js")).unwrap();
    success(&run(t.path(), &["--no-config", "-w", "a.js", "z.js"]));
    for path in ["a.js", "z.js"] {
        assert_eq!(
            fs::read(t.path().join(path)).unwrap(),
            fixture("ts/regressions/write.js")
        );
    }
}

#[cfg(unix)]
#[test]
fn mirror_rejects_aliases_sharing_a_destination() {
    let t = tempfile::tempdir().unwrap();
    let src = put(t.path(), "z.lua", "lua/regressions/path.lua");
    std::os::unix::fs::symlink("z.lua", t.path().join("a.txt")).unwrap();
    let o = run(
        t.path(),
        &["--no-config", "-w", "-o", "out", "a.txt", "z.lua"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(!t.path().join("out").exists());
    assert_eq!(fs::read(src).unwrap(), fixture("lua/regressions/path.lua"));
}

#[test]
fn mirror_checks_case_collisions_on_the_output_filesystem() {
    let t = tempfile::tempdir().unwrap();
    let left = put(t.path(), "one/Foo.js", "ts/regressions/input.js");
    let right = put(t.path(), "two/foo.js", "ts/regressions/keywords.js");
    let cwd = t.path().join("work");
    fs::create_dir(&cwd).unwrap();
    let probe = cwd.join("CaseProbe");
    fs::write(&probe, []).unwrap();
    let insensitive = cwd.join("caseprobe").exists();
    fs::remove_file(probe).unwrap();
    let o = run(
        &cwd,
        &["--no-config", "-w", "-o", "out", "../one", "../two"],
    );
    if insensitive {
        assert_eq!(o.status.code(), Some(1));
        assert!(!cwd.join("out").exists());
    } else {
        success(&o);
        assert_eq!(
            fs::read(cwd.join("out/Foo.js")).unwrap(),
            fixture("ts/regressions/write.js")
        );
        assert_eq!(
            fs::read(cwd.join("out/foo.js")).unwrap(),
            fixture("ts/regressions/write-keywords.js")
        );
    }
    assert_eq!(fs::read(left).unwrap(), fixture("ts/regressions/input.js"));
    assert_eq!(
        fs::read(right).unwrap(),
        fixture("ts/regressions/keywords.js")
    );
    // 包括失败路径在内，预检临时目录都已清理。
    assert_eq!(
        fs::read_dir(cwd).unwrap().count(),
        usize::from(!insensitive)
    );
}

#[test]
fn mirror_rejects_file_directory_collisions_before_any_write() {
    for (file, directory) in [("one", "two"), ("two", "one")] {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), &format!("{file}/a.js"), "ts/regressions/input.js");
        put(
            t.path(),
            &format!("{directory}/a.js/b.js"),
            "ts/regressions/input.js",
        );
        let cwd = t.path().join("work");
        fs::create_dir(&cwd).unwrap();
        let o = run(
            &cwd,
            &["--no-config", "-w", "-o", "out", "../one", "../two"],
        );
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(fs::read_dir(cwd).unwrap().count(), 0);
    }
}

/// 用真实 Git 应用完整补丁，再与 CLI 写回及 golden 做逐字节对拍。
fn assert_patch_roundtrip(root: &Path, name: &Path, source: &str, expected: &str) {
    let src = root.join(name);
    fs::write(&src, fixture(source)).unwrap();
    let diff = Command::new(BIN)
        .arg("--no-config")
        .arg(name)
        .current_dir(root)
        .output()
        .unwrap();
    success(&diff);
    success(
        &Command::new(BIN)
            .args(["--no-config", "-w"])
            .arg(name)
            .current_dir(root)
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(&src).unwrap(), fixture(expected));
    fs::write(&src, fixture(source)).unwrap();
    let patch = root.join("change.patch");
    fs::write(&patch, diff.stdout).unwrap();
    let applied = Command::new("git")
        // 模拟 Windows runner 的外部配置；命令自身须明确关闭换行转换。
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.autocrlf")
        .env("GIT_CONFIG_VALUE_0", "true")
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.eol=lf",
            "apply",
            "--no-index",
        ])
        .arg(patch)
        .current_dir(root)
        .output()
        .unwrap();
    success(&applied);
    assert_eq!(fs::read(src).unwrap(), fixture(expected));
}

#[test]
fn diff_roundtrip_preserves_original_content_bytes() {
    for name in ["crlf.js", "mixed.js", "non-utf8.js", "tail-cr.js"] {
        let t = tempfile::tempdir().unwrap();
        assert_patch_roundtrip(
            t.path(),
            Path::new(name),
            &format!("ts/regressions/{name}"),
            &format!("ts/regressions/write-{name}"),
        );
    }
}

#[test]
fn diff_roundtrip_quotes_paths() {
    for name in ["./input.js", "a b.js", "中文.js"] {
        let t = tempfile::tempdir().unwrap();
        assert_patch_roundtrip(
            t.path(),
            Path::new(name),
            "ts/regressions/input.js",
            "ts/regressions/write.js",
        );
    }
}

#[cfg(unix)]
#[test]
fn diff_roundtrip_quotes_control_and_non_utf8_paths() {
    use std::os::unix::ffi::OsStrExt;
    for name in [b"a\tb.js".as_slice(), b"a\nb.js", b"a\"b.js", b"a\\b.js"] {
        let t = tempfile::tempdir().unwrap();
        assert_patch_roundtrip(
            t.path(),
            Path::new(std::ffi::OsStr::from_bytes(name)),
            "ts/regressions/input.js",
            "ts/regressions/write.js",
        );
    }
    // macOS 的默认文件系统拒绝非法 UTF-8 文件名；Linux 验证原始路径字节。
    #[cfg(target_os = "linux")]
    {
        let t = tempfile::tempdir().unwrap();
        assert_patch_roundtrip(
            t.path(),
            Path::new(std::ffi::OsStr::from_bytes(b"a\xff.js")),
            "ts/regressions/input.js",
            "ts/regressions/write.js",
        );
    }
}

#[test]
fn bom_at_file_start_does_not_hide_directives() {
    for name in [
        "bom-define.lua",
        "bom-conditional.lua",
        "bom-plain.lua",
        "bom-define-only.lua",
    ] {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), name, &format!("lua/regressions/{name}"));
        success(&run(t.path(), &["--no-config", "--check", name]));
        let gate = run(t.path(), &["--no-config", "--gate", name]);
        assert_eq!(gate.status.code(), Some(3));
        assert_patch_roundtrip(
            t.path(),
            Path::new(name),
            &format!("lua/regressions/{name}"),
            &format!("lua/regressions/write-{name}"),
        );
        success(&run(t.path(), &["--no-config", "--gate", name]));
    }
}

#[test]
fn elif_after_else_fails_even_in_suppressed_blocks() {
    for name in ["elif-after-else.lua", "suppressed-elif-after-else.lua"] {
        let t = tempfile::tempdir().unwrap();
        let src = put(t.path(), name, &format!("lua/regressions/{name}"));
        for mode in ["--check", "--gate", "-w"] {
            let o = run(t.path(), &["--no-config", mode, name]);
            assert_eq!(o.status.code(), Some(2));
            assert!(String::from_utf8_lossy(&o.stderr).contains("E2: #elif after #else"));
            assert_eq!(
                fs::read(&src).unwrap(),
                fixture(&format!("lua/regressions/{name}"))
            );
        }
    }
}

#[test]
fn expression_depth_is_bounded_with_file_diagnostics() {
    let t = tempfile::tempdir().unwrap();
    let valid = put(t.path(), "valid.js", "ts/regressions/expr-limit.js");
    success(&run(t.path(), &["--no-config", "-w", "valid.js"]));
    assert_eq!(
        fs::read(valid).unwrap(),
        fixture("ts/regressions/write-expr-limit.js")
    );
    for name in [
        "expr-over-parentheses.js",
        "expr-over-unary.js",
        "expr-over-mixed.js",
    ] {
        let src = put(t.path(), name, &format!("ts/regressions/{name}"));
        for jobs in ["1", "4"] {
            let o = run(t.path(), &["--no-config", "--check", "-j", jobs, name]);
            assert_eq!(o.status.code(), Some(2));
            assert!(String::from_utf8_lossy(&o.stderr)
                .contains("E7: bad expression: expression nesting exceeds 128 levels"));
            assert_eq!(
                fs::read(&src).unwrap(),
                fixture(&format!("ts/regressions/{name}"))
            );
        }
    }
}
