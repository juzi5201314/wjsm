//! 三种 GC 算法的 Criterion 基准。
//!
//! 负载编译只发生一次；Criterion 度量的是同一 WASM 在 mark-sweep、G1、ZGC
//! 下的执行时间。每个算法在进入 Criterion 前额外跑一次观测样本，打印 GC 常见指标，
//! 方便和 Criterion 的 wall-time 结果并排比较。
//!
//! 运行：cargo bench -p wjsm-runtime --bench gc_stress

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use tokio::runtime::{Builder, Runtime};
use wjsm_runtime::{
    GcAlgorithmKind, GcExecutionStats, RuntimeOptions, compile_source,
    execute_with_writer_with_options_and_stats,
};

const ITERATIONS: usize = 100;
const SURVIVOR_STRIDE: usize = 20;
const LIVE_SLOTS: usize = 12;
const BURST_PERIOD: usize = 48;
const BURST_SLOTS: usize = 4;
const BURST_LEN: usize = 16;
const MAX_HEAP_SIZE: usize = 8 * 1024 * 1024;

const ALGORITHMS: [GcAlgorithmKind; 3] = [
    GcAlgorithmKind::MarkSweep,
    GcAlgorithmKind::G1,
    GcAlgorithmKind::Zgc,
];

struct Observation {
    algorithm: GcAlgorithmKind,
    wall: Duration,
    stats: GcExecutionStats,
    stdout: String,
}

impl Observation {
    fn pause_max_ms(&self) -> f64 {
        let hist_max = self.stats.pause_hist.iter().copied().max().unwrap_or(0);
        nanos_to_ms(hist_max.max(self.stats.last.pause_ns_max))
    }

    fn pause_avg_ms(&self) -> f64 {
        if self.stats.pause_hist.is_empty() {
            return nanos_to_ms(self.stats.last.pause_ns_total)
                / self.stats.last.pause_count.max(1) as f64;
        }

        let total: u128 = self
            .stats
            .pause_hist
            .iter()
            .map(|&sample| sample as u128)
            .sum();
        nanos_to_ms((total / self.stats.pause_hist.len() as u128) as u64)
    }

    fn max_committed_pages(&self) -> usize {
        self.stats
            .memory_footprint_hist
            .iter()
            .map(|sample| sample.committed_pages)
            .max()
            .unwrap_or(self.stats.last.committed_pages)
    }
}

fn gc_algorithm_comparison(c: &mut Criterion) {
    let source = churn_source();
    let wasm = compile_source(&source).expect("compile GC benchmark workload");
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime for GC benchmark");

    let observations = observe_algorithms(&runtime, &wasm);
    print_observations(&observations);

    let mut group = c.benchmark_group("gc/mixed_churn");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));
    group.warm_up_time(Duration::from_millis(500));
    group.throughput(Throughput::Elements(ITERATIONS as u64));
    for algorithm in ALGORITHMS {
        group.bench_with_input(
            BenchmarkId::from_parameter(algorithm.as_str()),
            &algorithm,
            |b, &algorithm| {
                b.iter(|| {
                    let stats = execute_once(&runtime, &wasm, algorithm).stats;
                    black_box(stats.last.freed_bytes);
                    black_box(stats.last.pause_ns_max);
                    black_box(stats.last.relocated_bytes);
                });
            },
        );
    }
    group.finish();
}

fn observe_algorithms(runtime: &Runtime, wasm: &[u8]) -> Vec<Observation> {
    ALGORITHMS
        .iter()
        .map(|&algorithm| execute_once(runtime, wasm, algorithm))
        .collect()
}

fn execute_once(runtime: &Runtime, wasm: &[u8], algorithm: GcAlgorithmKind) -> Observation {
    let options = RuntimeOptions {
        max_heap_size: Some(MAX_HEAP_SIZE),
        gc_algorithm: algorithm,
        ..RuntimeOptions::default()
    };

    let started = std::time::Instant::now();
    let (stdout, diagnostics, stats) = runtime
        .block_on(async {
            execute_with_writer_with_options_and_stats(wasm, Vec::new(), options).await
        })
        .expect("execute GC benchmark workload");
    assert!(
        diagnostics.is_empty(),
        "{} emitted diagnostics: {}",
        algorithm.as_str(),
        String::from_utf8_lossy(&diagnostics)
    );
    assert!(
        !stats.pause_hist.is_empty() || stats.last.has_pause_observation(),
        "{} did not observe GC; benchmark workload is too small",
        algorithm.as_str()
    );

    Observation {
        algorithm,
        wall: started.elapsed(),
        stats,
        stdout: String::from_utf8(stdout).expect("GC benchmark stdout is utf8"),
    }
}

fn print_observations(observations: &[Observation]) {
    eprintln!(
        "\nGC observation snapshot: workload=mixed_churn iterations={ITERATIONS} max_heap_mb={} columns=algorithm, wall_ms, pause_samples, pause_max_ms, pause_avg_ms, freed_mb, heap_used_mb, reusable_mb, external_fragmentation, committed_pages_max, relocated_objects, relocated_mb, barrier_events, satb_flushes, rset_cards, load_barrier_hits, stdout",
        MAX_HEAP_SIZE / 1024 / 1024
    );
    for obs in observations {
        let stats = &obs.stats.last;
        eprintln!(
            "gc_observation algorithm={} wall_ms={:.3} pause_samples={} pause_max_ms={:.3} pause_avg_ms={:.3} freed_mb={:.3} heap_used_mb={:.3} reusable_mb={:.3} external_fragmentation={:.6} committed_pages_max={} relocated_objects={} relocated_mb={:.3} barrier_events={} satb_flushes={} rset_cards={} load_barrier_hits={} stdout={}",
            obs.algorithm.as_str(),
            obs.wall.as_secs_f64() * 1000.0,
            obs.stats.pause_hist.len().max(stats.pause_count),
            obs.pause_max_ms(),
            obs.pause_avg_ms(),
            bytes_to_mib(stats.freed_bytes),
            bytes_to_mib(stats.heap_used_bytes),
            bytes_to_mib(stats.free_bytes_reusable),
            stats.external_fragmentation,
            obs.max_committed_pages(),
            stats.relocated_objects,
            bytes_to_mib(stats.relocated_bytes),
            stats.barrier_events,
            stats.satb_flushes,
            stats.rset_cards,
            stats.load_barrier_mark_hits + stats.load_barrier_relocate_hits,
            obs.stdout.trim(),
        );
    }
    eprintln!();
}

fn churn_source() -> String {
    format!(
        r#"
        const ITER = {ITERATIONS};
        const SURVIVOR_STRIDE = {SURVIVOR_STRIDE};
        const LIVE_SLOTS = {LIVE_SLOTS};
        const BURST_PERIOD = {BURST_PERIOD};
        const BURST_SLOTS = {BURST_SLOTS};
        const BURST_LEN = {BURST_LEN};
        const survivors = [];
        const bursts = [];
        let survivorCursor = 0;
        let burstCursor = 0;
        let checksum = 0;

        for (let s = 0; s < LIVE_SLOTS; s = s + 1) {{
            survivors.push(null);
        }}
        for (let b = 0; b < BURST_SLOTS; b = b + 1) {{
            bursts.push(null);
        }}

        for (let i = 0; i < ITER; i = i + 1) {{
            const obj = {{ a: i, b: i + 1, c: i + 2, d: i + 3, e: i + 4, f: i + 5 }};
            const pair = {{ left: obj, right: {{ v: i + 6, w: i + 7, x: i + 8, y: i + 9 }} }};

            if (i % SURVIVOR_STRIDE === 0) {{
                survivors[survivorCursor] = pair;
                survivorCursor = (survivorCursor + 1) % LIVE_SLOTS;
            }}

            if (i % BURST_PERIOD === 0) {{
                const burst = [];
                for (let j = 0; j < BURST_LEN; j = j + 1) {{
                    burst.push({{ idx: j, value: i + j, ref: obj, pad0: j + 1, pad1: j + 2, pad2: j + 3 }});
                }}
                bursts[burstCursor] = burst;
                burstCursor = (burstCursor + 1) % BURST_SLOTS;
            }}

            checksum = (checksum + obj.a + pair.right.v + survivorCursor + burstCursor) % 1000003;
        }}

        gc();
        console.log('survivorSlots=' + survivors.length + ' burstSlots=' + bursts.length + ' checksum=' + checksum);
        "#,
    )
}

fn nanos_to_ms(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000.0
}

fn bytes_to_mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

criterion_group!(benches, gc_algorithm_comparison);
criterion_main!(benches);
