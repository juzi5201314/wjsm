use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use tokio::runtime::{Builder, Runtime};
use wjsm_runtime::{
    GcAlgorithmKind, GcExecutionStats, RuntimeOptions, compile_source,
    execute_with_writer_with_options_and_stats,
};

const WARMUP_RUNS: usize = 2;
const MEASURED_RUNS: usize = 9;
const MAX_HEAP_SIZE: usize = 12 * 1024 * 1024;

const EPOCHS: usize = 7;
const ALLOCATIONS_PER_EPOCH: usize = 900;
const SURVIVOR_SLOTS: usize = 96;
const BURST_SLOTS: usize = 18;
const PROBE_ROUNDS: usize = 6;

#[derive(Debug)]
struct RunSample {
    wall: Duration,
    stats: GcExecutionStats,
    stdout: String,
}

#[derive(Debug)]
struct Summary {
    wall_mean_ms: f64,
    wall_median_ms: f64,
    wall_p95_ms: f64,
    wall_cv_percent: f64,
    pause_max_ms: f64,
    pause_mean_ms: f64,
    relocated_mb: f64,
    relocated_objects: usize,
    external_fragmentation: f64,
    heap_used_mb: f64,
    reusable_mb: f64,
    committed_pages_max: usize,
    barrier_events: usize,
    load_barrier_hits: usize,
    pause_samples: usize,
}

fn main() -> Result<()> {
    let source = workload_source();
    let wasm = compile_source(&source).context("compile ZGC autoresearch workload")?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create tokio runtime for ZGC autoresearch workload")?;

    for _ in 0..WARMUP_RUNS {
        let sample = execute_once(&runtime, &wasm)?;
        assert_sample(&sample)?;
    }

    let mut samples = Vec::with_capacity(MEASURED_RUNS);
    for _ in 0..MEASURED_RUNS {
        let sample = execute_once(&runtime, &wasm)?;
        assert_sample(&sample)?;
        samples.push(sample);
    }

    let summary = summarize(&samples);
    print_metrics(&summary);
    Ok(())
}

fn execute_once(runtime: &Runtime, wasm: &[u8]) -> Result<RunSample> {
    let options = RuntimeOptions {
        max_heap_size: Some(MAX_HEAP_SIZE),
        gc_algorithm: GcAlgorithmKind::Zgc,
        ..RuntimeOptions::default()
    };

    let started = Instant::now();
    let (stdout, diagnostics, stats) = runtime.block_on(async {
        execute_with_writer_with_options_and_stats(wasm, Vec::new(), options).await
    })?;
    ensure!(
        diagnostics.is_empty(),
        "ZGC autoresearch workload emitted diagnostics: {}",
        String::from_utf8_lossy(&diagnostics)
    );

    Ok(RunSample {
        wall: started.elapsed(),
        stats,
        stdout: String::from_utf8(stdout).context("ZGC workload stdout must be utf8")?,
    })
}

fn assert_sample(sample: &RunSample) -> Result<()> {
    ensure!(
        sample.stdout.starts_with("zgc-autoresearch-ok survivors="),
        "ZGC workload did not complete expected path: {}",
        sample.stdout.trim()
    );
    ensure!(
        !sample.stats.pause_hist.is_empty() || sample.stats.last.has_pause_observation(),
        "ZGC workload did not observe a GC pause"
    );
    ensure!(
        sample.stats.last.relocated_objects != 0,
        "ZGC workload did not relocate any objects"
    );
    ensure!(
        sample.stats.last.committed_pages != 0,
        "ZGC workload did not report committed pages"
    );
    Ok(())
}

fn summarize(samples: &[RunSample]) -> Summary {
    let mut wall_ms: Vec<f64> = samples.iter().map(|sample| ms(sample.wall)).collect();
    wall_ms.sort_by(f64::total_cmp);

    let wall_mean_ms = mean(&wall_ms);
    let wall_median_ms = percentile(&wall_ms, 50);
    let wall_p95_ms = percentile(&wall_ms, 95);
    let wall_cv_percent = coefficient_of_variation_percent(&wall_ms, wall_mean_ms);

    let pause_max_ms = samples.iter().map(sample_pause_max_ms).fold(0.0, f64::max);
    let pause_mean_ms = mean(&samples.iter().map(sample_pause_mean_ms).collect::<Vec<_>>());
    let relocated_mb = mean(
        &samples
            .iter()
            .map(|sample| bytes_to_mib(sample.stats.last.relocated_bytes))
            .collect::<Vec<_>>(),
    );
    let relocated_objects = samples
        .iter()
        .map(|sample| sample.stats.last.relocated_objects)
        .max()
        .unwrap_or_default();
    let external_fragmentation = mean(
        &samples
            .iter()
            .map(|sample| sample.stats.last.external_fragmentation)
            .collect::<Vec<_>>(),
    );
    let heap_used_mb = mean(
        &samples
            .iter()
            .map(|sample| bytes_to_mib(sample.stats.last.heap_used_bytes))
            .collect::<Vec<_>>(),
    );
    let reusable_mb = mean(
        &samples
            .iter()
            .map(|sample| bytes_to_mib(sample.stats.last.free_bytes_reusable))
            .collect::<Vec<_>>(),
    );
    let committed_pages_max = samples
        .iter()
        .flat_map(|sample| {
            sample
                .stats
                .memory_footprint_hist
                .iter()
                .map(|footprint| footprint.committed_pages)
        })
        .max()
        .unwrap_or_else(|| {
            samples
                .iter()
                .map(|sample| sample.stats.last.committed_pages)
                .max()
                .unwrap_or_default()
        });
    let barrier_events = samples
        .iter()
        .map(|sample| sample.stats.last.barrier_events)
        .max()
        .unwrap_or_default();
    let load_barrier_hits = samples
        .iter()
        .map(|sample| {
            sample
                .stats
                .last
                .load_barrier_mark_hits
                .saturating_add(sample.stats.last.load_barrier_relocate_hits)
        })
        .max()
        .unwrap_or_default();
    let pause_samples = samples
        .iter()
        .map(|sample| {
            sample
                .stats
                .pause_hist
                .len()
                .max(sample.stats.last.pause_count)
        })
        .max()
        .unwrap_or_default();

    Summary {
        wall_mean_ms,
        wall_median_ms,
        wall_p95_ms,
        wall_cv_percent,
        pause_max_ms,
        pause_mean_ms,
        relocated_mb,
        relocated_objects,
        external_fragmentation,
        heap_used_mb,
        reusable_mb,
        committed_pages_max,
        barrier_events,
        load_barrier_hits,
        pause_samples,
    }
}

fn print_metrics(summary: &Summary) {
    println!("METRIC zgc_wall_p95_ms={:.6}", summary.wall_p95_ms);
    println!("METRIC zgc_wall_mean_ms={:.6}", summary.wall_mean_ms);
    println!("METRIC zgc_wall_median_ms={:.6}", summary.wall_median_ms);
    println!("METRIC zgc_wall_cv_percent={:.6}", summary.wall_cv_percent);
    println!("METRIC zgc_pause_max_ms={:.6}", summary.pause_max_ms);
    println!("METRIC zgc_pause_mean_ms={:.6}", summary.pause_mean_ms);
    println!("METRIC zgc_relocated_mb={:.6}", summary.relocated_mb);
    println!("METRIC zgc_relocated_objects={}", summary.relocated_objects);
    println!(
        "METRIC zgc_external_fragmentation={:.6}",
        summary.external_fragmentation
    );
    println!("METRIC zgc_heap_used_mb={:.6}", summary.heap_used_mb);
    println!("METRIC zgc_reusable_mb={:.6}", summary.reusable_mb);
    println!(
        "METRIC zgc_committed_pages_max={}",
        summary.committed_pages_max
    );
    println!("METRIC zgc_barrier_events={}", summary.barrier_events);
    println!("METRIC zgc_load_barrier_hits={}", summary.load_barrier_hits);
    println!("METRIC zgc_pause_samples={}", summary.pause_samples);
}

fn percentile(sorted_values: &[f64], percentile: usize) -> f64 {
    if sorted_values.is_empty() {
        return 0.0;
    }
    let rank = (sorted_values.len() * percentile).div_ceil(100);
    sorted_values[rank.saturating_sub(1).min(sorted_values.len() - 1)]
}

fn coefficient_of_variation_percent(values: &[f64], mean_value: f64) -> f64 {
    if values.is_empty() || mean_value == 0.0 {
        return 0.0;
    }
    let variance = values
        .iter()
        .map(|value| {
            let delta = *value - mean_value;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64;
    variance.sqrt() / mean_value * 100.0
}

fn sample_pause_max_ms(sample: &RunSample) -> f64 {
    let hist_max = sample.stats.pause_hist.iter().copied().max().unwrap_or(0);
    nanos_to_ms(hist_max.max(sample.stats.last.pause_ns_max))
}

fn sample_pause_mean_ms(sample: &RunSample) -> f64 {
    if !sample.stats.pause_hist.is_empty() {
        return nanos_to_ms(
            (sample
                .stats
                .pause_hist
                .iter()
                .map(|&pause| pause as u128)
                .sum::<u128>()
                / sample.stats.pause_hist.len() as u128) as u64,
        );
    }
    nanos_to_ms(sample.stats.last.pause_ns_total) / sample.stats.last.pause_count.max(1) as f64
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn nanos_to_ms(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000.0
}

fn bytes_to_mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn workload_source() -> String {
    format!(
        r#"
        const EPOCHS = {EPOCHS};
        const ALLOCATIONS_PER_EPOCH = {ALLOCATIONS_PER_EPOCH};
        const SURVIVOR_SLOTS = {SURVIVOR_SLOTS};
        const BURST_SLOTS = {BURST_SLOTS};
        const PROBE_ROUNDS = {PROBE_ROUNDS};
        const survivors = [];
        let cursor = 0;
        let checksum = 0;

        for (let slot = 0; slot < SURVIVOR_SLOTS; slot = slot + 1) {{
            survivors.push(null);
        }}

        for (let epoch = 0; epoch < EPOCHS; epoch = epoch + 1) {{
            for (let i = 0; i < ALLOCATIONS_PER_EPOCH; i = i + 1) {{
                const base = epoch * 100000 + i;
                const survivor = {{ a: base, b: base + 1, c: base + 2, d: base + 3, e: base + 4, f: base + 5 }};
                const transient = {{ a: base + 6, b: base + 7, c: base + 8, d: base + 9, e: base + 10, f: base + 11 }};

                if (i % 3 === 0) {{
                    survivors[cursor] = survivor;
                    cursor = (cursor + 1) % SURVIVOR_SLOTS;
                }}

                if (i % 37 === 0) {{
                    const burst = [];
                    for (let burstSlot = 0; burstSlot < BURST_SLOTS; burstSlot = burstSlot + 1) {{
                        burst.push({{ a: base + burstSlot, b: burstSlot + 1, c: burstSlot + 2, d: burstSlot + 3 }});
                    }}
                    checksum = (checksum + burst.length + burst[0].a) % 1000000007;
                }}

                checksum = (checksum + survivor.a + transient.e + cursor) % 1000000007;
            }}

            gc();

            for (let probeRound = 0; probeRound < PROBE_ROUNDS; probeRound = probeRound + 1) {{
                for (let probe = 0; probe < SURVIVOR_SLOTS; probe = probe + 1) {{
                    const item = survivors[probe];
                    if (item !== null) {{
                        checksum = (checksum + item.a + item.c + item.f) % 1000000007;
                    }}
                }}
            }}

            for (let drop = epoch % 4; drop < SURVIVOR_SLOTS; drop = drop + 4) {{
                survivors[drop] = null;
            }}
        }}

        gc();

        let live = 0;
        for (let finalProbe = 0; finalProbe < SURVIVOR_SLOTS; finalProbe = finalProbe + 1) {{
            const finalItem = survivors[finalProbe];
            if (finalItem !== null) {{
                live = live + 1;
                checksum = (checksum + finalItem.b + finalItem.e) % 1000000007;
            }}
        }}

        console.log('zgc-autoresearch-ok survivors=' + live + ' checksum=' + checksum);
        "#,
    )
}
