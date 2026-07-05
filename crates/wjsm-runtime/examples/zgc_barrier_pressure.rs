use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::runtime::{Builder, Runtime};
use wjsm_runtime::{
    GcAlgorithmKind, GcExecutionStats, RuntimeOptions, compile_source,
    execute_with_writer_with_options_and_stats,
};

const ANCHOR_COUNT: usize = 192;
const ALLOCATION_BURST: usize = 3300;
const READ_ROUNDS: usize = 512;
const CYCLES: usize = 2;
const WARMUP_RUNS: usize = 0;
const MEASURED_RUNS: usize = 3;
const MAX_HEAP_SIZE: usize = 16 * 1024 * 1024;

const ALGORITHMS: [GcAlgorithmKind; 3] = [
    GcAlgorithmKind::MarkSweep,
    GcAlgorithmKind::G1,
    GcAlgorithmKind::Zgc,
];

fn main() -> Result<()> {
    let source = workload_source();
    let wasm =
        compile_source(&source).context("failed to compile ZGC barrier-pressure workload")?;
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
    if zgc.load_barrier_hits_total == 0 {
        bail!("barrier-pressure workload produced zero ZGC load-barrier hits");
    }
    let overhead_ns_per_hit = zgc.wall_ns_total as f64 / zgc.load_barrier_hits_total as f64;

    println!("METRIC zgc_load_barrier_overhead_ns_per_hit={overhead_ns_per_hit:.6}");
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
    wall_ns_per_cycle: f64,
    throughput_reads_per_sec: f64,
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
    satb_flushes_total: usize,
    load_barrier_mark_hits_total: usize,
    load_barrier_relocate_hits_total: usize,
    load_barrier_hits_total: usize,
    load_barrier_hits_per_read: f64,
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
        let mut load_barrier_mark_hits_total = 0usize;
        let mut load_barrier_relocate_hits_total = 0usize;

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
            load_barrier_mark_hits_total =
                load_barrier_mark_hits_total.saturating_add(stats.load_barrier_mark_hits);
            load_barrier_relocate_hits_total =
                load_barrier_relocate_hits_total.saturating_add(stats.load_barrier_relocate_hits);
        }

        if pause_samples == 0 {
            bail!("{} did not observe any GC pause", algorithm.as_str());
        }

        let wall_ns_total: u128 = runs.iter().map(|run| run.wall.as_nanos()).sum();
        let measured_cycles = (runs.len() * CYCLES) as f64;
        let measured_reads = (runs.len() * CYCLES * READ_ROUNDS) as f64;
        let load_barrier_hits_total =
            load_barrier_mark_hits_total.saturating_add(load_barrier_relocate_hits_total);

        Ok(Self {
            algorithm,
            wall_ns_total,
            wall_ns_per_cycle: wall_ns_total as f64 / measured_cycles,
            throughput_reads_per_sec: measured_reads / (wall_ns_total as f64 / 1_000_000_000.0),
            pause_samples,
            pause_max_ns,
            pause_avg_ns: pause_total_ns as f64 / pause_samples as f64,
            freed_bytes_total,
            heap_used_bytes_max,
            reusable_bytes_max,
            external_fragmentation_max,
            committed_pages_max,
            relocated_objects_total,
            relocated_bytes_total,
            barrier_events_total,
            satb_flushes_total,
            load_barrier_mark_hits_total,
            load_barrier_relocate_hits_total,
            load_barrier_hits_total,
            load_barrier_hits_per_read: load_barrier_hits_total as f64 / measured_reads,
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

fn execute_once(
    runtime: &Runtime,
    wasm: &[u8],
    algorithm: GcAlgorithmKind,
) -> Result<RunObservation> {
    let options = RuntimeOptions {
        max_heap_size: Some(MAX_HEAP_SIZE),
        gc_algorithm: algorithm,
        ..RuntimeOptions::default()
    };

    let started = Instant::now();
    let (stdout, diagnostics, stats) = runtime
        .block_on(async {
            execute_with_writer_with_options_and_stats(wasm, Vec::new(), options).await
        })
        .with_context(|| {
            format!(
                "{} barrier-pressure workload execution failed",
                algorithm.as_str()
            )
        })?;

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
        "METRIC {prefix}_wall_ns_per_cycle={:.3}",
        summary.wall_ns_per_cycle
    );
    println!(
        "METRIC {prefix}_throughput_reads_per_sec={:.3}",
        summary.throughput_reads_per_sec
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
        "METRIC {prefix}_satb_flushes_total={}",
        summary.satb_flushes_total
    );
    println!(
        "METRIC {prefix}_load_barrier_mark_hits_total={}",
        summary.load_barrier_mark_hits_total
    );
    println!(
        "METRIC {prefix}_load_barrier_relocate_hits_total={}",
        summary.load_barrier_relocate_hits_total
    );
    println!(
        "METRIC {prefix}_load_barrier_hits_total={}",
        summary.load_barrier_hits_total
    );
    println!(
        "METRIC {prefix}_load_barrier_hits_per_read={:.9}",
        summary.load_barrier_hits_per_read
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
        const ANCHOR_COUNT = {ANCHOR_COUNT};
        const ALLOCATION_BURST = {ALLOCATION_BURST};
        const READ_ROUNDS = {READ_ROUNDS};
        const CYCLES = {CYCLES};
        const anchors = [];
        const retained = [];
        let checksum = 0;

        for (let anchorIndex = 0; anchorIndex < ANCHOR_COUNT; anchorIndex = anchorIndex + 1) {{
            anchors.push({{
                id: anchorIndex,
                a: anchorIndex + 1,
                b: anchorIndex + 2,
                c: anchorIndex + 3,
                next: null,
                link: null
            }});
        }}

        for (let cycleIndex = 0; cycleIndex < CYCLES; cycleIndex = cycleIndex + 1) {{
            for (let burstIndex = 0; burstIndex < ALLOCATION_BURST; burstIndex = burstIndex + 1) {{
                const anchor = anchors[(burstIndex + cycleIndex) % ANCHOR_COUNT];
                const transient = {{
                    id: burstIndex,
                    a: burstIndex + 1,
                    b: burstIndex + 2,
                    c: burstIndex + 3,
                    d: burstIndex + 4,
                    ref: anchor
                }};
                if (burstIndex % 257 === 0) {{
                    anchor.link = transient;
                    retained.push(transient);
                }}
            }}

            for (let readIndex = 0; readIndex < READ_ROUNDS; readIndex = readIndex + 1) {{
                const obj = anchors[(readIndex + cycleIndex) % ANCHOR_COUNT];
                checksum = (checksum + obj.a + obj.b + obj.c + obj.id) % 1000003;
                if (obj.link !== null) {{
                    checksum = (checksum + obj.link.a + obj.link.b) % 1000003;
                }}
            }}

            gc();
        }}

        console.log('anchors=' + anchors.length + ' retained=' + retained.length + ' checksum=' + checksum);
        "#,
    )
}
