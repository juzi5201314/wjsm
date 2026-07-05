//! T5.4 linear memory footprint 长运行基准。
//!
//! 默认快速跳过；设置 `WJSM_GC_BENCH=1` 后用同一 inline workload
//! 依次验证 mark-sweep / G1 / ZGC 在存活量下降后复用可回收空间，
//! 不让 committed linear memory 在尾段持续增长。

use std::ffi::OsStr;

use anyhow::{Context, Result, ensure};
use tokio::runtime::Builder;
use wjsm_runtime::{
    GcAlgorithmKind, GcExecutionStats, RuntimeOptions, compile_source,
    execute_with_writer_with_options_and_stats,
};

const GROW_SLOTS: usize = 400;
const LOW_SLOTS: usize = 16;
const PAYLOAD_LEN: usize = 12;
const REGROW_ROUNDS: usize = 1;
const STEADY_GC_ROUNDS: usize = 4;
const MAX_HEAP_SIZE: usize = 8 * 1024 * 1024;
const MIN_REUSABLE_BYTES: usize = 1024;

#[derive(Debug)]
struct FootprintResult {
    algorithm: GcAlgorithmKind,
    stats: GcExecutionStats,
    stdout: String,
}

impl FootprintResult {
    fn first_committed_pages(&self) -> usize {
        self.stats
            .memory_footprint_hist
            .first()
            .map(|sample| sample.committed_pages)
            .unwrap_or_default()
    }

    fn final_committed_pages(&self) -> usize {
        self.stats.last.committed_pages
    }

    fn max_committed_pages(&self) -> usize {
        self.stats
            .memory_footprint_hist
            .iter()
            .map(|sample| sample.committed_pages)
            .max()
            .unwrap_or_default()
    }

    fn max_reusable_bytes(&self) -> usize {
        self.stats
            .memory_footprint_hist
            .iter()
            .map(|sample| sample.free_bytes_reusable)
            .max()
            .unwrap_or_default()
    }
}

#[test]
fn gc_footprint_long_run() -> Result<()> {
    if std::env::var_os("WJSM_GC_BENCH").as_deref() != Some(OsStr::new("1")) {
        eprintln!("skip gc_footprint_long_run: set WJSM_GC_BENCH=1 to run footprint bench");
        return Ok(());
    }

    let source = footprint_source();
    let wasm = compile_source(&source).context("compile inline GC footprint workload")?;

    let mark_sweep = run_footprint(&wasm, GcAlgorithmKind::MarkSweep)?;
    print_result(&mark_sweep);
    let g1 = run_footprint(&wasm, GcAlgorithmKind::G1)?;
    print_result(&g1);
    let zgc = run_footprint(&wasm, GcAlgorithmKind::Zgc)?;
    print_result(&zgc);

    assert_footprint(&mark_sweep, 0)?;
    assert_footprint(&g1, 1)?;
    assert_footprint(&zgc, 1)?;

    Ok(())
}

fn footprint_source() -> String {
    format!(
        r#"
        const GROW_SLOTS = {GROW_SLOTS};
        const LOW_SLOTS = {LOW_SLOTS};
        const PAYLOAD_LEN = {PAYLOAD_LEN};
        const REGROW_ROUNDS = {REGROW_ROUNDS};
        const STEADY_GC_ROUNDS = {STEADY_GC_ROUNDS};
        const live = [];
        let checksum = 0;

        gc();

        for (let initSlot = 0; initSlot < GROW_SLOTS; initSlot = initSlot + 1) {{
            const initValue = 100000 + initSlot;
            live.push({{ round: 1, slot: initSlot, a: initValue, b: initValue + 1, c: initValue + 2, d: initValue + 3, e: PAYLOAD_LEN }});
            checksum = (checksum + initValue) % 1000000007;
        }}
        gc();

        for (let firstDropSlot = LOW_SLOTS; firstDropSlot < live.length; firstDropSlot = firstDropSlot + 1) {{
            live[firstDropSlot] = null;
        }}
        live.length = LOW_SLOTS;
        gc();

        for (let regrowRound = 0; regrowRound < REGROW_ROUNDS; regrowRound = regrowRound + 1) {{
            while (live.length < GROW_SLOTS) {{
                const regrowSlot = live.length;
                const regrowValue = (regrowRound + 2) * 100000 + regrowSlot;
                live.push({{ round: regrowRound + 2, slot: regrowSlot, a: regrowValue, b: regrowValue + 1, c: regrowValue + 2, d: regrowValue + 3, e: PAYLOAD_LEN }});
                checksum = (checksum + regrowValue) % 1000000007;
            }}

            for (let churnSlot = 0; churnSlot < GROW_SLOTS; churnSlot = churnSlot + 1) {{
                const churnValue = (regrowRound + 50) * 100000 + churnSlot;
                const churnTmp = {{ round: regrowRound + 50, slot: churnSlot, a: churnValue, b: churnValue + 1, c: churnValue + 2, d: churnValue + 3, e: PAYLOAD_LEN }};
                checksum = (checksum + churnTmp.a + churnTmp.e) % 1000000007;
            }}
            gc();

            for (let dropSlot = LOW_SLOTS; dropSlot < live.length; dropSlot = dropSlot + 1) {{
                live[dropSlot] = null;
            }}
            live.length = LOW_SLOTS;
            gc();
        }}

        while (live.length < GROW_SLOTS) {{
            const finalSlot = live.length;
            const finalValue = 9000000 + finalSlot;
            live.push({{ round: 90, slot: finalSlot, a: finalValue, b: finalValue + 1, c: finalValue + 2, d: finalValue + 3, e: PAYLOAD_LEN }});
            checksum = (checksum + finalValue) % 1000000007;
        }}

        for (let steadyRound = 0; steadyRound < STEADY_GC_ROUNDS; steadyRound = steadyRound + 1) {{
            gc();
        }}

        let liveChecksum = live.length;
        console.log('footprint-ok live=' + live.length + ' checksum=' + ((checksum + liveChecksum) % 1000000007));
        "#,
    )
}

fn run_footprint(wasm: &[u8], algorithm: GcAlgorithmKind) -> Result<FootprintResult> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create tokio runtime for GC footprint bench")?;
    let options = RuntimeOptions {
        max_heap_size: Some(MAX_HEAP_SIZE),
        gc_algorithm: algorithm,
        ..RuntimeOptions::default()
    };

    let (out, diagnostics, stats) = runtime.block_on(async {
        execute_with_writer_with_options_and_stats(wasm, Vec::new(), options).await
    })?;
    ensure!(
        diagnostics.is_empty(),
        "{algorithm:?} emitted diagnostics: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    Ok(FootprintResult {
        algorithm,
        stats,
        stdout: String::from_utf8(out).context("GC footprint stdout must be utf8")?,
    })
}

fn print_result(result: &FootprintResult) {
    eprintln!(
        "gc_footprint_long_run algorithm={:?} samples={} committed_first={} committed_max={} committed_final={} reusable_max={} reusable_final={} regions_free={} stdout={}",
        result.algorithm,
        result.stats.memory_footprint_hist.len(),
        result.first_committed_pages(),
        result.max_committed_pages(),
        result.final_committed_pages(),
        result.max_reusable_bytes(),
        result.stats.last.free_bytes_reusable,
        result.stats.last.regions_free,
        result.stdout.trim(),
    );
}

fn assert_footprint(result: &FootprintResult, tail_growth_allowance_pages: usize) -> Result<()> {
    let expected_prefix = format!("footprint-ok live={GROW_SLOTS} checksum=");
    ensure!(
        result.stdout.starts_with(&expected_prefix),
        "{:?} workload did not complete expected growth/drop/regrowth path: {}",
        result.algorithm,
        result.stdout.trim()
    );
    ensure!(
        result.stats.last.committed_pages > 0,
        "{:?} did not report committed_pages",
        result.algorithm
    );
    ensure!(
        result.stats.memory_footprint_hist.len() >= 6,
        "{:?} expected multiple footprint samples, got {}",
        result.algorithm,
        result.stats.memory_footprint_hist.len()
    );
    let last_sample = result
        .stats
        .memory_footprint_hist
        .last()
        .context("memory footprint history must have a final sample")?;
    ensure!(
        last_sample.committed_pages == result.stats.last.committed_pages,
        "{:?} final footprint sample committed_pages {} != last stats {}",
        result.algorithm,
        last_sample.committed_pages,
        result.stats.last.committed_pages
    );
    ensure!(
        last_sample.free_bytes_reusable == result.stats.last.free_bytes_reusable,
        "{:?} final footprint sample free_bytes_reusable {} != last stats {}",
        result.algorithm,
        last_sample.free_bytes_reusable,
        result.stats.last.free_bytes_reusable
    );
    ensure!(
        result.max_reusable_bytes() >= MIN_REUSABLE_BYTES,
        "{:?} never reported reusable free bytes >= {MIN_REUSABLE_BYTES}; max={}",
        result.algorithm,
        result.max_reusable_bytes()
    );

    let tail_len = STEADY_GC_ROUNDS.min(result.stats.memory_footprint_hist.len());
    let tail =
        &result.stats.memory_footprint_hist[result.stats.memory_footprint_hist.len() - tail_len..];
    let tail_min = tail
        .iter()
        .map(|sample| sample.committed_pages)
        .min()
        .unwrap_or_default();
    let tail_max = tail
        .iter()
        .map(|sample| sample.committed_pages)
        .max()
        .unwrap_or_default();
    ensure!(
        tail_max.saturating_sub(tail_min) <= tail_growth_allowance_pages,
        "{:?} committed_pages kept growing in steady tail: min={} max={} allowance={}",
        result.algorithm,
        tail_min,
        tail_max,
        tail_growth_allowance_pages
    );
    ensure!(
        result.final_committed_pages() <= result.max_committed_pages(),
        "{:?} final committed pages exceeded observed peak",
        result.algorithm
    );
    Ok(())
}
