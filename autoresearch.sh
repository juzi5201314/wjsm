#!/usr/bin/env bash
set -euo pipefail

export CARGO_TERM_COLOR=never
export RUST_BACKTRACE=0

cargo run --quiet --release -p wjsm-runtime --example zgc_autoresearch
