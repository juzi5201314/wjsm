#!/usr/bin/env bash
# Legacy zgc_barrier_pressure / zgc_autoresearch examples were retired in Task 26.
# Reproducible GC performance measurement lives in wjsm-gc-bench.
set -euo pipefail

export CARGO_TERM_COLOR=never
export RUST_BACKTRACE=0

cargo build --quiet --release -p wjsm-gc-bench
target/release/wjsm-gc-bench preflight --heap 256m --profile pr --output /tmp/wjsm-gc-autoresearch-preflight.json
target/release/wjsm-gc-bench micro --component allocator --heap 256m --samples 5 --output /tmp/wjsm-gc-autoresearch-micro.json
cat /tmp/wjsm-gc-autoresearch-preflight.json
cat /tmp/wjsm-gc-autoresearch-micro.json
