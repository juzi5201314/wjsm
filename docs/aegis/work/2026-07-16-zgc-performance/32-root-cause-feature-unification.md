# 32 - 根因纠正：feature 统一泄漏（推翻 30/31 的 Engine 池结论）

## 结论

30/31 号文档诊断的"全局 ENGINE_POOL 状态污染"**不成立**。103 个测试失败的真正根因是
**cargo feature 统一（feature unification）把开发中的 `managed-heap-v2` 泄漏进了
`--workspace` 构建面**，使全部测试静默运行在未完成的 V2 堆上。已修复并三次全量验证绿。

## 旧结论为何错误

1. **nextest 是严格的 process-per-test 模型**：每个测试独立进程。"同一包内的测试
   复用进程、早期测试污染全局单例"在物理上不可能——test #1 与 #1562 从不共享地址空间。
2. **决定性实验**（2026-07-21）：
   ```bash
   # workspace 构建面 + 单线程 + 仅 3 个测试 → 3/3 失败
   cargo nextest run --workspace --test-threads=1 \
     -E 'test(happy__regex_exec) + test(happy__typedarray_simple) + test(happy__regexp_lastindex)'
   ```
   无并发、无 Engine 复用、无"历史污染"，照样失败。失败与并发无关，与**构建面**有关。
3. "单独运行通过"的真实原因：`-p wjsm-runtime` 只构建该包及依赖，feature 集不含
   V2；`--workspace` 构建所有成员，feature 集被统一。对照组差异是 feature，不是并发。

## 泄漏机制

cargo 在同一次构建里对每个 crate 只编译一份，feature 取**所有被选中包需求的并集**。
工作区内存在两条无条件启用边：

| 泄漏边 | 位置 |
|---|---|
| `wjsm-gc-bench → wjsm-runtime/managed-heap-v2` | `crates/wjsm-gc-bench/Cargo.toml` 依赖声明 |
| `wjsm-backend-wasm-v2 → wjsm-backend-wasm/managed-heap-v2` | 垫片 crate（本身是工作区成员） |

任何 `cargo nextest run --workspace` / `cargo build --workspace` 都会把 V2 打开：
后端产物换成 V2 WASM 契约（额外 memory64 共享堆 import/export），runtime 走
`heap_access_v2` 路径。V2 尚在 Task 15 切换中，未覆盖的边缘即为失败集：

- `happy__regex_exec` 输出 `[world, 0, 0, 0]`：V2 数组最小容量 `.max(4)` 被当作长度渲染；
- fetch/streams "requested 63041187568 bytes"：V2 主存数据段/句柄路径读到字符串字节
  （0x756c6177 = ASCII "walu"）当作 capacity；
- typedarray / regexp lastIndex / vm realm / weakref 等失败均为 V2 属性与句柄语义缺口。

## 修复内容

1. **删除垫片 crate `wjsm-backend-wasm-v2`**（整个 crate 只有一行 re-export，
   其唯一作用是传播 feature——而 Cargo 本就支持 crate feature 直接启用非可选依赖的
   feature，垫片没有存在必要）。`wjsm-runtime` 的 feature 直连转发：
   ```toml
   managed-heap-v2 = ["wjsm-backend-wasm/managed-heap-v2", "wjsm-runtime-support/managed-heap-v2"]
   ```
   runtime 内 4 处 `wjsm_backend_wasm_v2::backend::*` cfg 分叉坍缩为单一
   `wjsm_backend_wasm::*` 调用（V1/V2 差异本就在 wjsm-backend-wasm 内部 cfg）。
2. **`wjsm-gc-bench` 改为 opt-in**：依赖不再带 feature；新增自身
   `managed-heap-v2 = ["wjsm-runtime/managed-heap-v2"]`；lib 模块整体
   `#[cfg(feature)]` 门控（feature 关闭时编译为空 crate，与 `heap_access_v2.rs`
   等 V2 测试文件同一模式）；bin 与 `contracts` 测试 target 声明
   `required-features = ["managed-heap-v2"]`——`--workspace` 构建时自动跳过，
   显式 `cargo run -p wjsm-gc-bench` 会得到标准的 required-features 报错，
   保证性能证据不可能在错误的堆上产出（ADR 0010）。
3. **CI 更新**：`zgc-nightly.yml` 与 `zgc-capability-matrix.yml` 中全部 8 处
   gc-bench 构建/运行命令补 `--features managed-heap-v2`。
4. **删除 `crates/wjsm-runtime/tests/wasm_env_async_corruption.rs`**（上个会话
   基于错误理论创建的未跟踪复现测试，依赖外部网络 httpbin.org，前提已被证伪）。
5. 修复 `wjsm-backend-wasm/tests/integration/gc_alloc_window.rs` 在 V2 feature 下的
   unused-import 警告（`HashSet` 导入与使用同步 cfg 门控）。

## 验证（2026-07-21）

```
cargo nextest run --workspace   # ×3
Summary: 1714 tests run: 1714 passed, 2 skipped   (23.1s / 22.3s / 23.3s)
cargo build --workspace                            # 0 warnings
cargo build --workspace --features managed-heap-v2 # 编译通过（9 条既有 V2 WIP dead-code 警告，见下）
cargo run -p wjsm-gc-bench --features managed-heap-v2 -- capabilities  # 正常产出 JSON
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(heap_platform) | test(bitmap_simd)'  # 4/4
```

测试总数 1832 → 1714：V2 门控测试（heap_access_v2 / zgc_v2 / g1_v2 等）在默认构建面
不再编译，改由显式 `--features managed-heap-v2` 运行（CI capability matrix 与 Task 15
工作流负责）；另删除 1 个错误前提测试。

### 环境噪声（验证期间发现并排除）

- 一个 2026-07-20 遗留的 agent-browser 无头 Chrome GPU 进程持续占用 252% CPU
  （累计 1301 CPU 分钟），10 核机器 load 13.8，导致 2.5s 级 fixture 越过 3s 硬门禁
  出现批量 TIMEOUT 假象。已终止。
- `/tmp` tmpfs 被历史 wjsm 产物占满（wjsm-test-cache 4.3G 等），已清理。
- 诊断超时类失败前先 `uptime` + `df /tmp`。

## 遗留问题（不属于本修复，移交 V2 工作流）

1. `--features managed-heap-v2` 下 wjsm-runtime 有 9 条 dead-code 警告
   （`attach_heap`/`recolor_live_obj_table_entries` 等 Task 16-23 在途脚手架）。
2. `cargo fmt --check` 在已提交的 V2 代码（runtime_gc/zgc/heap 等 ~70 处）上有漂移。
3. `runtime_typedarray.rs` 存在未提交的 V2 WIP diff（typedarray V2 句柄路径），未动。
4. V2 全量缺口（regex/typedarray/streams/vm 等）即 Task 15 的剩余工作，本次泄漏
   恰好提前暴露了完整清单——30/31 文档中的失败列表可直接作为 Task 15 验收清单。

## 规则沉淀

工作区任何 crate 都**不得在依赖声明中无条件启用 `managed-heap-v2`**（或其他改变
行为的私有 feature）。需要该 feature 的 crate 一律：自身声明同名转发 feature +
`required-features`（bin/test target）或模块级 `#[cfg]` 门控（lib）。已写入 CLAUDE.md。
