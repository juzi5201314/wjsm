#!/usr/bin/env bash
set -euo pipefail

export WJSM_CACHE_DIR="${WJSM_CACHE_DIR:-/tmp/wjsm-autoresearch-cache}"
export WJSM_STARTUP_SNAPSHOT="${WJSM_STARTUP_SNAPSHOT:-1}"

cargo run --quiet -p wjsm-runtime --release --example zgc_autoresearch
