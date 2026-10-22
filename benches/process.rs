//! 性能基准（DESIGN §4.9）。
//!
//! 运行：`cargo bench`。
//! 目标：指令行占比 <1% 的真实形态下，单线程完整处理吞吐 ≥ 1 GB/s。
//!
//! 语料为确定性合成代码（LCG 种子固定，跨机器可比），按注入的指令密度分档：
//! 0%（纯代码，扫描应几乎零成本）/ 1%（典型发布树）/ 10%（重标注）。

use alpp::eval::MacroTable;
use alpp::pipeline::process_bytes;
use alpp::profile::C_FAMILY;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::path::Path;

/// 确定性伪随机（xorshift64），跨平台可复现。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// 生成 n_lines 行 Go 风格代码，按 density 概率注入条件编译块。
/// 每行约 30 字节，50 万行 ≈ 15 MB。
fn gen_corpus(n_lines: usize, density: f64, seed: u64) -> String {
    let mut rng = Rng(seed | 1);
    let mut out = String::with_capacity(n_lines * 32);
    let mut in_block = false;
    let mut fn_id = 0usize;
    let mut i = 0;
    while i < n_lines {
        let r = (rng.next() % 10_000) as f64 / 10_000.0;
        if !in_block && r < density {
            in_block = true;
            out.push_str("\t//#ifndef RELEASE\n");
            continue;
        }
        if in_block && r < density {
            in_block = false;
            out.push_str("\t//#endif\n");
            continue;
        }
        match i % 40 {
            0 => {
                fn_id += 1;
                out.push_str(&format!("func handler{fn_id}(req *Request) Response {{\n"));
            }
            39 => out.push_str("}\n"),
            _ => {
                let v = rng.next() % 1000;
                out.push_str(&format!("\tres.{v} = compute(req, {v}) // work\n"));
            }
        }
        i += 1;
    }
    if in_block {
        out.push_str("\t//#endif\n");
    }
    out
}

fn bench_full_process(c: &mut Criterion) {
    let mut group = c.benchmark_group("process/full");
    for (label, density) in [("d0", 0.0), ("d1", 0.01), ("d10", 0.10)] {
        let corpus = gen_corpus(100_000, density, 42);
        let bytes = corpus.len() as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(BenchmarkId::new("c_family", label), &corpus, |b, src| {
            let table = MacroTable::new();
            let mut t = table;
            t.define("RELEASE", alpp::eval::MacroValue::Defined);
            b.iter(|| {
                let o = process_bytes(
                    black_box(src.as_bytes()),
                    Path::new("bench.js"),
                    &C_FAMILY,
                    &t,
                    false,
                    &alpp::config::Config::default(),
                )
                .unwrap();
                black_box(o.deleted_lines.len())
            });
        });
    }
    group.finish();
}

fn bench_scan_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("process/scan_only");
    let corpus = gen_corpus(100_000, 0.01, 7);
    group.throughput(Throughput::Bytes(corpus.len() as u64));
    group.bench_function("anchor_prefilter", |b| {
        b.iter(|| {
            // 第一级 SIMD 预筛成本：块级锚字节搜索
            black_box(alpp::scanner::lines_with_anchor(
                black_box(corpus.as_bytes()),
                b'/',
            ))
        })
    });
    group.bench_function("line_iter", |b| {
        b.iter(|| {
            let n = alpp::scanner::iter_lines(black_box(corpus.as_bytes())).count();
            black_box(n)
        })
    });
    group.finish();
}

/// 多线程扩展性：N 线程处理 N 份独立语料（文件级并行模型）。
fn bench_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("process/parallel");
    let corpus = gen_corpus(100_000, 0.01, 99);
    let n_files = 8usize;
    group.throughput(Throughput::Bytes(corpus.len() as u64 * n_files as u64));

    for threads in [1usize, 4, 8] {
        group.bench_with_input(BenchmarkId::new("files8_t", threads), &threads, |b, &t| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(t)
                .build()
                .unwrap();
            let table = {
                let mut tt = MacroTable::new();
                tt.define("RELEASE", alpp::eval::MacroValue::Defined);
                tt
            };
            b.iter(|| {
                pool.install(|| {
                    (0..n_files).into_par_iter().for_each(|_| {
                        let _ = process_bytes(
                            black_box(corpus.as_bytes()),
                            Path::new("bench.js"),
                            &C_FAMILY,
                            &table,
                            false,
                            &alpp::config::Config::default(),
                        );
                    });
                })
            });
        });
    }
    group.finish();
}

/// 包含遍历、读盘、状态机和 stdout 的真实 CLI 基准；文件准备不计入耗时。
fn bench_cli(c: &mut Criterion) {
    use std::process::{Command, Stdio};
    let root = tempfile::tempdir().unwrap();
    let mut input = std::fs::read("tests/fixtures/ts/regressions/input.js").unwrap();
    input.push(b'\n');
    let input = input.repeat(4096);
    for i in 0..64 {
        std::fs::write(root.path().join(format!("f{i:02}.js")), &input).unwrap();
    }
    let mut group = c.benchmark_group("cli/tree");
    group.throughput(Throughput::Bytes(64 * input.len() as u64));
    group.sample_size(10);
    for mode in ["check", "diff"] {
        group.bench_function(mode, |b| {
            b.iter(|| {
                let mut command = Command::new(env!("CARGO_BIN_EXE_alpp"));
                command
                    .args(["--no-config", "-j", "1", "."])
                    .current_dir(root.path())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if mode == "check" {
                    command.arg("--check");
                }
                assert!(command.status().unwrap().success());
            })
        });
    }
    group.finish();
}

use rayon::prelude::*;

criterion_group!(
    benches,
    bench_full_process,
    bench_scan_only,
    bench_parallel,
    bench_cli
);
criterion_main!(benches);
