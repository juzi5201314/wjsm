use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::runtime::{Builder, Runtime};
use wjsm_runtime::{
    GcAlgorithmKind, GcExecutionStats, GcStats, RuntimeOptions, compile_source,
    execute_with_writer_with_options_and_stats,
};

const WORKLOAD_ITERATIONS: usize = 180;
const WARMUP_RUNS: usize = 1;
const MEASURED_RUNS: usize = 5;
const SURVIVOR_STRIDE: usize = 9;
const LIVE_SLOTS: usize = 28;
const BURST_PERIOD: usize = 17;
const BURST_SLOTS: usize = 10;
const BURST_LEN: usize = 18;
const POINTER_REWRITE_ROUNDS: usize = 4;
const MAX_HEAP_SIZE: usize = 8 * 1024 * 1024;

const ALGORITHMS: [GcAlgorithmKind; 3] = [
    GcAlgorithmKind::MarkSweep,
    GcAlgorithmKind::G1,
    GcAlgorithmKind::Zgc,
];

fn main() -> Result<()> {
    let source = workload_source();
    let wasm = compile_source(&source).context("failed to compile autoresearch GC workload")?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;

    let mut summaries = Vec::with_capacity(ALGORITHMS.len());
    for algorithm in ALGORITHMS {
        for _ in 0..WARMUP_RUNS {
            let _ = execute_once(&runtime, &wasm, algorithm)?;
        }

        let mut runs = Vec::with_capacity(MEASURED_RUNS);
        for _ in 0..MEASURED_RUNS {
            runs.push(execute_once(&runtime, &wasm, algorithm)?);
        }
        summaries.push(AlgorithmSummary::from_runs(algorithm, &runs)?);
    }

    let zgc = summaries
        .iter()
        .find(|summary| summary.algorithm == GcAlgorithmKind::Zgc)
        .context("ZGC summary missing")?;
    let primary = zgc.pause_max_ns as f64 + zgc.wall_ns_per_iteration;

    println!("METRIC zgc_latency_overhead_ns={primary:.3}");
    for summary in &summaries {
        print_summary_metrics(summary);
    }

    Ok(())
}

struct RunObservation {
    wall: Duration,
    stats: GcExecutionStats,
    stdout: String,
}

struct AlgorithmSummary {
    algorithm: GcAlgorithmKind,
    wall_ns_total: u128,
    wall_ns_per_iteration: f64,
    throughput_ops_per_sec: f64,
    pause_samples: usize,
    pause_max_ns: u64,
    pause_avg_ns: f64,
    freed_bytes_total: usize,
    heap_used_bytes_max: usize,
    reusable_bytes_max: usize,
    external_fragmentation_max: f64,
    committed_pages_max: usize,
    relocated_objects_total: usize,
    relocated_bytes_total: usize,
    barrier_events_total: usize,
    barrier_events_per_iteration: f64,
    satb_flushes_total: usize,
    rset_cards_max: usize,
    load_barrier_hits_total: usize,
    load_barrier_hits_per_iteration: f64,
}

impl AlgorithmSummary {
    fn from_runs(algorithm: GcAlgorithmKind, runs: &[RunObservation]) -> Result<Self> {
        if runs.is_empty() {
            bail!("{} produced no measured runs", algorithm.as_str());
        }

        let expected_stdout = runs[0].stdout.as_str();
        for run in runs {
            if run.stdout != expected_stdout {
                bail!(
                    "{} workload stdout changed between runs: `{}` vs `{}`",
                    algorithm.as_str(),
                    expected_stdout.trim(),
                    run.stdout.trim()
                );
            }
        }

        let mut pause_samples = 0usize;
        let mut pause_total_ns = 0u128;
        let mut pause_max_ns = 0u64;
        let mut freed_bytes_total = 0usize;
        let mut heap_used_bytes_max = 0usize;
        let mut reusable_bytes_max = 0usize;
        let mut external_fragmentation_max = 0.0f64;
        let mut committed_pages_max = 0usize;
        let mut relocated_objects_total = 0usize;
        let mut relocated_bytes_total = 0usize;
        let mut barrier_events_total = 0usize;
        let mut satb_flushes_total = 0usize;
        let mut rset_cards_max = 0usize;
        let mut load_barrier_hits_total = 0usize;

        for run in runs {
            let stats = &run.stats.last;
            let pause = pause_observation(&run.stats);
            pause_samples = pause_samples.saturating_add(pause.count);
            pause_total_ns = pause_total_ns.saturating_add(pause.total_ns);
            pause_max_ns = pause_max_ns.max(pause.max_ns);
            freed_bytes_total = freed_bytes_total.saturating_add(stats.freed_bytes);
            heap_used_bytes_max = heap_used_bytes_max.max(stats.heap_used_bytes);
            reusable_bytes_max = reusable_bytes_max.max(stats.free_bytes_reusable);
            external_fragmentation_max =
                external_fragmentation_max.max(stats.external_fragmentation);
            committed_pages_max = committed_pages_max.max(max_committed_pages(&run.stats));
            relocated_objects_total =
                relocated_objects_total.saturating_add(stats.relocated_objects);
            relocated_bytes_total = relocated_bytes_total.saturating_add(stats.relocated_bytes);
            barrier_events_total = barrier_events_total.saturating_add(stats.barrier_events);
            satb_flushes_total = satb_flushes_total.saturating_add(stats.satb_flushes);
            rset_cards_max = rset_cards_max.max(stats.rset_cards);
            load_barrier_hits_total =
                load_barrier_hits_total.saturating_add(load_barrier_hits(stats));
        }

        if pause_samples == 0 {
            bail!("{} did not observe any GC pause", algorithm.as_str());
        }

        let wall_ns_total: u128 = runs.iter().map(|run| run.wall.as_nanos()).sum();
        let measured_iterations = (runs.len() * WORKLOAD_ITERATIONS) as f64;
        let wall_ns_per_iteration = wall_ns_total as f64 / measured_iterations;
        let throughput_ops_per_sec = measured_iterations / (wall_ns_total as f64 / 1_000_000_000.0);
        let pause_avg_ns = pause_total_ns as f64 / pause_samples as f64;

        Ok(Self {
            algorithm,
            wall_ns_total,
            wall_ns_per_iteration,
            throughput_ops_per_sec,
            pause_samples,
            pause_max_ns,
            pause_avg_ns,
            freed_bytes_total,
            heap_used_bytes_max,
            reusable_bytes_max,
            external_fragmentation_max,
            committed_pages_max,
            relocated_objects_total,
            relocated_bytes_total,
            barrier_events_total,
            barrier_events_per_iteration: barrier_events_total as f64 / measured_iterations,
            satb_flushes_total,
            rset_cards_max,
            load_barrier_hits_total,
            load_barrier_hits_per_iteration: load_barrier_hits_total as f64 / measured_iterations,
        })
    }
}

struct PauseObservation {
    count: usize,
    total_ns: u128,
    max_ns: u64,
}

fn pause_observation(stats: &GcExecutionStats) -> PauseObservation {
    if !stats.pause_hist.is_empty() {
        let total_ns = stats.pause_hist.iter().map(|&sample| sample as u128).sum();
        let max_ns = stats.pause_hist.iter().copied().max().unwrap_or(0);
        return PauseObservation {
            count: stats.pause_hist.len(),
            total_ns,
            max_ns,
        };
    }

    PauseObservation {
        count: stats.last.pause_count,
        total_ns: stats.last.pause_ns_total as u128,
        max_ns: stats.last.pause_ns_max,
    }
}

fn max_committed_pages(stats: &GcExecutionStats) -> usize {
    stats
        .memory_footprint_hist
        .iter()
        .map(|sample| sample.committed_pages)
        .max()
        .unwrap_or(stats.last.committed_pages)
}

fn load_barrier_hits(stats: &GcStats) -> usize {
    stats
        .load_barrier_mark_hits
        .saturating_add(stats.load_barrier_relocate_hits)
}

fn execute_once(
    runtime: &Runtime,
    wasm: &[u8],
    algorithm: GcAlgorithmKind,
) -> Result<RunObservation> {
    let options = RuntimeOptions {
        max_heap_size: Some(MAX_HEAP_SIZE),
        shadow_stack_max: 16 * 1024 * 1024,
        gc_algorithm: algorithm,
        ..RuntimeOptions::default()
    };

    let started = Instant::now();
    let (stdout, diagnostics, stats) = runtime
        .block_on(async {
            execute_with_writer_with_options_and_stats(wasm, Vec::new(), options).await
        })
        .with_context(|| format!("{} workload execution failed", algorithm.as_str()))?;

    if !diagnostics.is_empty() {
        bail!(
            "{} emitted diagnostics: {}",
            algorithm.as_str(),
            String::from_utf8_lossy(&diagnostics)
        );
    }

    let pause = pause_observation(&stats);
    if pause.count == 0 {
        bail!("{} workload did not trigger GC", algorithm.as_str());
    }

    Ok(RunObservation {
        wall: started.elapsed(),
        stats,
        stdout: String::from_utf8(stdout).context("workload stdout was not UTF-8")?,
    })
}

fn print_summary_metrics(summary: &AlgorithmSummary) {
    let prefix = metric_prefix(summary.algorithm);
    println!("METRIC {prefix}_wall_ns_total={}", summary.wall_ns_total);
    println!(
        "METRIC {prefix}_wall_ns_per_iter={:.3}",
        summary.wall_ns_per_iteration
    );
    println!(
        "METRIC {prefix}_throughput_ops_per_sec={:.3}",
        summary.throughput_ops_per_sec
    );
    println!("METRIC {prefix}_pause_samples={}", summary.pause_samples);
    println!("METRIC {prefix}_pause_max_ns={}", summary.pause_max_ns);
    println!("METRIC {prefix}_pause_avg_ns={:.3}", summary.pause_avg_ns);
    println!(
        "METRIC {prefix}_freed_bytes_total={}",
        summary.freed_bytes_total
    );
    println!(
        "METRIC {prefix}_heap_used_bytes_max={}",
        summary.heap_used_bytes_max
    );
    println!(
        "METRIC {prefix}_reusable_bytes_max={}",
        summary.reusable_bytes_max
    );
    println!(
        "METRIC {prefix}_external_fragmentation_max={:.9}",
        summary.external_fragmentation_max
    );
    println!(
        "METRIC {prefix}_committed_pages_max={}",
        summary.committed_pages_max
    );
    println!(
        "METRIC {prefix}_relocated_objects_total={}",
        summary.relocated_objects_total
    );
    println!(
        "METRIC {prefix}_relocated_bytes_total={}",
        summary.relocated_bytes_total
    );
    println!(
        "METRIC {prefix}_barrier_events_total={}",
        summary.barrier_events_total
    );
    println!(
        "METRIC {prefix}_barrier_events_per_iter={:.6}",
        summary.barrier_events_per_iteration
    );
    println!(
        "METRIC {prefix}_satb_flushes_total={}",
        summary.satb_flushes_total
    );
    println!("METRIC {prefix}_rset_cards_max={}", summary.rset_cards_max);
    println!(
        "METRIC {prefix}_load_barrier_hits_total={}",
        summary.load_barrier_hits_total
    );
    println!(
        "METRIC {prefix}_load_barrier_hits_per_iter={:.6}",
        summary.load_barrier_hits_per_iteration
    );
}

fn metric_prefix(algorithm: GcAlgorithmKind) -> &'static str {
    match algorithm {
        GcAlgorithmKind::MarkSweep => "mark_sweep",
        GcAlgorithmKind::G1 => "g1",
        GcAlgorithmKind::Zgc => "zgc",
    }
}

fn workload_source() -> String {
    format!(
        r#"
        const ITER = {WORKLOAD_ITERATIONS};
        const SURVIVOR_STRIDE = {SURVIVOR_STRIDE};
        const LIVE_SLOTS = {LIVE_SLOTS};
        const BURST_PERIOD = {BURST_PERIOD};
        const BURST_SLOTS = {BURST_SLOTS};
        const BURST_LEN = {BURST_LEN};
        const POINTER_REWRITE_ROUNDS = {POINTER_REWRITE_ROUNDS};
        const survivors = [];
        const bursts = [];
        let checksum = 0;
        let survivorCursor = 0;
        let burstCursor = 0;

        for (let s = 0; s < LIVE_SLOTS; s = s + 1) {{
            survivors.push({{ id: s, value: s + 1, next: null, alt: null }});
        }}
        for (let initBurstIndex = 0; initBurstIndex < BURST_SLOTS; initBurstIndex = initBurstIndex + 1) {{
            bursts.push(null);
        }}

        for (let i = 0; i < ITER; i = i + 1) {{
            const obj = {{ a: i, b: i + 1, c: i + 2, d: i + 3, e: i + 4, f: i + 5 }};
            const pair = {{ left: obj, right: {{ v: i + 6, w: i + 7, x: i + 8, y: i + 9 }}, tag: i }};
            const previous = survivors[survivorCursor];
            pair.left.prev = previous;
            previous.next = pair;
            previous.alt = obj;

            for (let r = 0; r < POINTER_REWRITE_ROUNDS; r = r + 1) {{
                const slot = (survivorCursor + r) % LIVE_SLOTS;
                survivors[slot].alt = r % 2 === 0 ? pair : obj;
                survivors[slot].value = survivors[slot].value + r + i;
            }}

            if (i % SURVIVOR_STRIDE === 0) {{
                survivors[survivorCursor] = pair;
                survivorCursor = (survivorCursor + 1) % LIVE_SLOTS;
            }}

            if (i % BURST_PERIOD === 0) {{
                const burst = [];
                for (let j = 0; j < BURST_LEN; j = j + 1) {{
                    const item = {{ idx: j, value: i + j, ref: obj, pair: pair, pad0: j + 1, pad1: j + 2, pad2: j + 3 }};
                    burst.push(item);
                    checksum = (checksum + item.value + item.pad2) % 1000003;
                }}
                bursts[burstCursor] = burst;
                burstCursor = (burstCursor + 1) % BURST_SLOTS;
            }}

            checksum = (checksum + obj.a + pair.right.v + survivorCursor + burstCursor + previous.id) % 1000003;
        }}

        gc();

        for (let k = 0; k < LIVE_SLOTS; k = k + 1) {{
            const entry = survivors[k];
            if (entry !== null) {{
                checksum = (checksum + entry.tag + entry.value + k) % 1000003;
            }}
        }}
        for (let finalBurstIndex = 0; finalBurstIndex < BURST_SLOTS; finalBurstIndex = finalBurstIndex + 1) {{
            const burst = bursts[finalBurstIndex];
            if (burst !== null) {{
                checksum = (checksum + burst.length + finalBurstIndex) % 1000003;
            }}
        }}

        console.log('survivorSlots=' + survivors.length + ' burstSlots=' + bursts.length + ' checksum=' + checksum);
        "#,
    )
}
