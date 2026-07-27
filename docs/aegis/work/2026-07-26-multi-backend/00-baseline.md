# P0 性能基线（multi-backend 计划）

- 采集时间：2026-07-26
- commit：`2064e5446806bf3cee8c0d62a9dd91354bcdc767`
- 构建：`cargo build --release`（target/release/wjsm）

## gc-bench

命令：
```
./target/release/wjsm-gc-bench run --scenario churn --gc zgc --output /tmp/gcbench-zgc.json
./target/release/wjsm-gc-bench run --scenario churn --gc mark-sweep --output /tmp/gcbench-ms.json
```
（计划中的 `--scenario iteration` 不存在；实际可用场景为 churn/request/chain/cycle/wide/mutation/humongous/idle-uncommit/saturation，取默认 churn。终测须用同一命令。）

### zgc / churn
```json
{
  "steady_state_ns": {
    "count": 10,
    "mean": 16627631.4,
    "min": 14401287,
    "p50": 16978333,
    "p99": 18321669,
    "max": 18321669
  },
  "gc_cpu_ns": {
    "count": 10,
    "mean": 2726046.8,
    "min": 2087731,
    "p50": 2771593,
    "p99": 4018271,
    "max": 4018271
  },
  "pause_max_ns": {
    "count": 10,
    "mean": 1462.6,
    "min": 660,
    "p50": 1341,
    "p99": 2210,
    "max": 2210
  },
  "metrics": {
    "gc_cpu_per_allocated_byte": 8.288678212643818,
    "mark_cpu_per_live_byte": 0.8178274069644418,
    "relocation_cpu_per_relocated_byte": null,
    "allocation_rate_bytes_per_sec": 19779606.131995443,
    "gc_overhead_percent": 16.39467904009467,
    "barrier_load_events_per_sec": 123288.75656938125,
    "barrier_store_events_per_sec": 433194.5919850015,
    "gc_cycles_per_sec": 60.1408568631128
  }
}
```

### mark-sweep / churn
```json
{
  "steady_state_ns": {
    "count": 10,
    "mean": 15712369.0,
    "min": 13140498,
    "p50": 15390631,
    "p99": 20628032,
    "max": 20628032
  },
  "gc_cpu_ns": {
    "count": 10,
    "mean": 0.0,
    "min": 0,
    "p50": 0,
    "p99": 0,
    "max": 0
  },
  "pause_max_ns": {
    "count": 10,
    "mean": 724306.3,
    "min": 502048,
    "p50": 681692,
    "p99": 959467,
    "max": 959467
  },
  "metrics": {
    "gc_cpu_per_allocated_byte": 0.0,
    "mark_cpu_per_live_byte": null,
    "relocation_cpu_per_relocated_byte": null,
    "allocation_rate_bytes_per_sec": 20931789.471084848,
    "gc_overhead_percent": null,
    "barrier_load_events_per_sec": 130470.45929229385,
    "barrier_store_events_per_sec": 458428.6430645818,
    "gc_cycles_per_sec": 63.644126484045785
  }
}
```

## 热点微基准（min-of-5，`time ./target/release/wjsm run -e '<snippet>'`）

| 基准 | snippet | min (s) | 5 次采样 (s) | stdout |
|---|---|---|---|---|
| array_callback | `const a=Array(200000).fill(0).map((_,i)=>i*2); console.log(a.reduce((s,x)=>s+x,0));` | 2.3147 | [2.63, 2.5478, 2.3708, 2.3643, 2.3147] | `39999800000` |
| prop_access | `const o={x:1}; let s=0; for(let i=0;i<2000000;i++) s+=o.x; console.log(s);` | 3.4146 | [3.4883, 3.4146, 3.4984, 3.4454, 3.4606] | `2000000` |
| json | `const j=JSON.stringify(Array(20000).fill({a:1,b:"s"})); console.log(JSON.parse(j).length);` | 0.1924 | [0.2212, 0.1924, 0.1982, 0.2028, 0.196] | `20000` |
| eq_typeof | `let n=0; for(let i=0;i<2000000;i++){ if(typeof i==="number"&&i==i) n++; } console.log(n);` | 0.5496 | [0.5741, 0.5538, 0.5592, 0.5496, 0.5588] | `2000000` |

## 阈值

- gc-bench 指标 ±5% 内；4 个微基准 min-of-5 不高于基线 5%。
- 基线丢失重采法：`git stash && git checkout 2064e5446806bf3cee8c0d62a9dd91354bcdc767` 重跑后切回。
