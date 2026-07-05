# T5.5 GC 回归矩阵覆盖审计

日期：2026-07-05  
范围：`fixtures/happy/gc_*`、`fixtures/happy/*{weak,async,streams,typedarray,fetch}*`，并补充历史缺口 fixture。  
非范围：T5.4 footprint bench / runtime stats-hist、P6 ADR。

## 审计输入

- 计划：`docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T5.5。
- P3 evidence：`docs/aegis/work/2026-07-03-gc-v2/90-evidence.md` P3 T3.0-T3.8。
- P4 evidence：`docs/aegis/work/2026-07-03-gc-v2/90-evidence.md` P4 T4.1-T4.6。
- fixture 集：`fixtures/happy/gc_*`、`fixtures/happy/*weak*`、`fixtures/happy/*async*`、`fixtures/happy/*streams*`、`fixtures/happy/*typedarray*`、`fixtures/happy/*fetch*`，并交叉查看 `*proxy*`、`*closure*`、`*bound*`。

## 历史缺口勾选清单

| 历史缺口 | 状态 | 覆盖 fixture / evidence | 审计结论 |
|---|---:|---|---|
| 侧表强 root：Map/Set/Proxy 可达时内部值不可被扫掉 | 已覆盖 | `fixtures/happy/gc_side_table_roots.js` | Map value、Set value、Proxy target 在 churn 后仍可读。 |
| 侧表 owner reachability：不可达 Map/Set 不能永久保活 key/value | 已覆盖 | `fixtures/happy/gc_map_set_owner_reachability.js` | FinalizationRegistry 能观察不可达 Map/Set 内值清理。 |
| TypedArray/DataView viewed buffer 侧表引用 | 已覆盖 | `fixtures/happy/gc_typedarray_dataview_side_refs.js` | 丢弃原始 `ArrayBuffer` 后，TypedArray/DataView 跨 GC 仍读到 77/88。 |
| Streams BYOB pending view / promise side roots | 已覆盖 | `fixtures/happy/streams_byob_gc_pending_view.js` | pending BYOB view 在 5000 次分配和后续 respond 后仍有效。 |
| Fetch/Streams 集成在三算法 support 下不退化 | 已覆盖 | `fixtures/happy/streams_fetch_body_data_url.js`、`fixtures/happy/fetch_data_url_init.js` | data: URL fetch body、init path 在三算法矩阵中通过。 |
| 长期 churn / 碎片 / 不 OOM 前一致性 | 已覆盖 | `fixtures/happy/gc_fragmentation_churn.js`、P3 T3.6 evidence | 长期对象/数组 churn 后 survivor 输出稳定；G1 mixed 对该 fixture 有专项验证。 |
| G1 young 跨代引用 | 已覆盖 | `fixtures/happy/gc_g1_young_churn.js`、P3 T3.4 evidence | 老/长期对象 property 与固定数组元素指向 young child，多轮 churn 后仍正确。 |
| G1 concurrent mark / cleanup | 已覆盖 | `fixtures/happy/gc_g1_concurrent_mark_churn.js`、P3 T3.5 evidence | old 引用保活、断开后 cleanup、后续 churn 正常。 |
| ZGC mark / WeakRef cleanup 顺序 | 已覆盖 | P4 T4.3 evidence；`weakref_gc` / `finalization_registry_cleanup` / `gc_map_set_owner_reachability` 矩阵 | MarkEnd 先 weak/side-table cleanup 再发布 handle，ZGC 相关 weak fixtures 已纳入矩阵。 |
| ZGC relocate 后 host 读/写旧位置一致性 | 已覆盖 | P4 T4.4 evidence；`gc_typedarray_dataview_side_refs`、`gc_fragmentation_churn` 矩阵 | Relocate 跳过 active page；typedarray side-ref 与 fragmentation churn 已在 ZGC 下通过。 |
| WeakRef / FinalizationRegistry | 已覆盖 | `fixtures/happy/weakref_gc.js`、`fixtures/happy/finalization_registry_cleanup.js`、`fixtures/happy/weak_collections_gc.js` | weak deref 清空、finalization cleanup、WeakMap/WeakSet key cleanup 均有 fixture。 |
| async/await 跨 GC root | 已覆盖 | `fixtures/happy/gc_async_await.js`、`fixtures/happy/async_closure_capture*.js` | await 前后对象和 closure capture 已有 fixture；`gc_async_await` 纳入矩阵。 |
| safepoint spill / local roots | 已覆盖 | `fixtures/happy/gc_safepoint_local.js`、`fixtures/happy/gc_spill_stress.js`、`fixtures/happy/gc_const_safepoint.js`、`fixtures/happy/gc_prop_access_safepoint.js` | local/const/property safepoint 保护已有专门 fixture。 |
| 函数属性 / closure env roots | 已覆盖 | `fixtures/happy/gc_function_props_survive.js`、`fixtures/happy/gc_bound_proxy_closure_churn.js` | 函数属性多轮 GC 和新增 closure env 多轮 GC 均覆盖。 |
| BoundFunction 多轮 GC 存活 | 新增覆盖 | `fixtures/happy/gc_bound_proxy_closure_churn.js` | 旧覆盖只有 `timer_bound_callback.js` 的基础 bound callback；新增 fixture 在多轮 `gc()` 后通过 bound timer callback 观察 bound this/args。 |
| Proxy target/handler 多轮 GC 存活 | 新增覆盖 | `fixtures/happy/gc_bound_proxy_closure_churn.js`；既有 `gc_side_table_roots.js` | 新增 fixture 用 proxy get/set handler 持有 `state`，多轮 GC 后读写仍一致。 |
| Closure env 多轮 GC 存活 | 新增覆盖 | `fixtures/happy/gc_bound_proxy_closure_churn.js`；既有 `async_closure_capture*.js` | 新增 fixture 的 closure env 在 6 轮分配 + `gc()` 后继续累加。 |
| Bound/Proxy/Closure 同 fixture 多轮 GC 交叉覆盖 | 新增覆盖 | `fixtures/happy/gc_bound_proxy_closure_churn.js` | 原 fixture 分散覆盖，缺少三类 side-table/function-like 值同场多轮 GC；已补齐。 |

## 新增 fixture

新增文件：

- `fixtures/happy/gc_bound_proxy_closure_churn.js`
- `fixtures/happy/gc_bound_proxy_closure_churn.expected`

覆盖点：

1. `BoundRecord`：`timerCallback.bind(receiver, argBox, 2)` 在 6 轮分配与显式 `gc()` 后仍通过 timer 回调输出 `109`，证明 bound this 与 bound args 仍可达。
2. `ProxyEntry`：proxy handler 捕获 `state`，target/handler 经过多轮 GC 后 `get`/`set` 输出 `14`、`42`。
3. `ClosureEntry`：closure env 持有嵌套对象与 hit counter，多轮 GC 后输出 `39`、`37`。

说明：未修改或弱化任何旧 fixture；新增 fixture 使用 `timer_bound_callback.js` 已覆盖的 bound callback dispatch 路径，专注验证 BoundFunction side-table roots 和多轮 GC 存活。

## 三算法验证矩阵

本轮矩阵选取新增 fixture 与相关历史缺口代表 fixture：

- `happy__gc_bound_proxy_closure_churn`
- `happy__gc_side_table_roots`
- `happy__gc_typedarray_dataview_side_refs`
- `happy__gc_map_set_owner_reachability`
- `happy__weakref_gc`
- `happy__finalization_registry_cleanup`
- `happy__weak_collections_gc`
- `happy__gc_async_await`
- `happy__streams_byob_gc_pending_view`
- `happy__gc_g1_young_churn`
- `happy__gc_g1_concurrent_mark_churn`
- `happy__gc_fragmentation_churn`
- `happy__streams_fetch_body_data_url`
- `happy__fetch_data_url_init`

| 算法 | 命令 | 结果 |
|---|---|---|
| mark-sweep | `cargo nextest run -E 'test(happy__gc_bound_proxy_closure_churn) + test(happy__gc_side_table_roots) + test(happy__gc_typedarray_dataview_side_refs) + test(happy__gc_map_set_owner_reachability) + test(happy__weakref_gc) + test(happy__finalization_registry_cleanup) + test(happy__weak_collections_gc) + test(happy__gc_async_await) + test(happy__streams_byob_gc_pending_view) + test(happy__gc_g1_young_churn) + test(happy__gc_g1_concurrent_mark_churn) + test(happy__gc_fragmentation_churn) + test(happy__streams_fetch_body_data_url) + test(happy__fetch_data_url_init)'` | 14 passed, 727 skipped |
| G1 | `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__gc_bound_proxy_closure_churn) + test(happy__gc_side_table_roots) + test(happy__gc_typedarray_dataview_side_refs) + test(happy__gc_map_set_owner_reachability) + test(happy__weakref_gc) + test(happy__finalization_registry_cleanup) + test(happy__weak_collections_gc) + test(happy__gc_async_await) + test(happy__streams_byob_gc_pending_view) + test(happy__gc_g1_young_churn) + test(happy__gc_g1_concurrent_mark_churn) + test(happy__gc_fragmentation_churn) + test(happy__streams_fetch_body_data_url) + test(happy__fetch_data_url_init)'` | 14 passed, 727 skipped |
| ZGC | `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__gc_bound_proxy_closure_churn) + test(happy__gc_side_table_roots) + test(happy__gc_typedarray_dataview_side_refs) + test(happy__gc_map_set_owner_reachability) + test(happy__weakref_gc) + test(happy__finalization_registry_cleanup) + test(happy__weak_collections_gc) + test(happy__gc_async_await) + test(happy__streams_byob_gc_pending_view) + test(happy__gc_g1_young_churn) + test(happy__gc_g1_concurrent_mark_churn) + test(happy__gc_fragmentation_churn) + test(happy__streams_fetch_body_data_url) + test(happy__fetch_data_url_init)'` | 14 passed, 727 skipped |

## 剩余缺口结论

T5.5 指定的历史缺口已逐项有 fixture 或 P3/P4 evidence 支撑。新增 `gc_bound_proxy_closure_churn` 后，未发现仍缺少可运行覆盖的明确项。

未覆盖范围按任务边界保留：未运行全 workspace；未做外部网络 fetch；未扩展到完整 Test262/WPT 矩阵。以上不是本 T5.5 历史缺口清单的剩余缺口。
