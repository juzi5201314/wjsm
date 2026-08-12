# 启动快照与嵌入工件

wjsm 默认使用启动快照（startup snapshot）加速冷启动：构建期把 primordial JS heap 状态序列化成二进制，嵌入到可执行文件中，运行时直接反序列化恢复，跳过重复的 builtin bootstrap。

## 控制

| 来源 | 作用 |
| --- | --- |
| `WJSM_STARTUP_SNAPSHOT=0`/`false`/`off` | 禁用快照，走 cold bootstrap |
| `WJSM_STARTUP_SNAPSHOT_DEBUG=1` | 输出快照诊断信息 |

默认开启。禁用后每次启动都重新执行 primordial bootstrap（分配 `Array.prototype`、`Object.prototype`、注册方法等），冷启动会明显变慢。

```bash
# 正常使用（快照开启）
wjsm run app.js

# 禁用快照，用于 A/B benchmark
WJSM_STARTUP_SNAPSHOT=0 wjsm run app.js

# 查看快照诊断
WJSM_STARTUP_SNAPSHOT_DEBUG=1 wjsm run app.js
```

## 快照内容

快照捕获的是 seed 模块引导后的 primordial 堆状态：

- **对象堆字节**：`memory[object_heap_start..heap_ptr]` 的原始拷贝
- **句柄相对偏移**：`obj_table[0..count]`，null 槽编码为 `u32::MAX`
- **runtime 字符串**：35 个固定偏移的 primordial 字符串
- **native callable 表**：58 个无状态 `SnapshotNativeCallable` 变体的判别式表

不捕获的内容：timer、microtask、promise、scheduler、worker、用户对象、side table。这些在新实例里保持零值。

## 格式版本与 ABI hash

快照格式有版本号（`SNAPSHOT_FORMAT_VERSION`）和 ABI hash 两层校验：

- **格式版本**：wire 结构变更时递增。不匹配则报 `native snapshot format version mismatch`。
- **ABI hash**：由 `support_abi_union_hash` + `builtin_js_bundle_hash` + `compatibility_fingerprint` 组合。不匹配时静默回退到 cold bootstrap，不污染 stderr。

ABI hash 的输入包括：NaN-box 常量、heap type tags、primordial 字符串内容和偏移、58 个 `SnapshotNativeCallable` discriminants、property slot 常量。

新增 builtin 或 primordial 字符串时必须更新 ABI hash 输入，否则快照会静默不匹配而走 cold bootstrap。

## 恢复流程

1. 从嵌入字节读取 header，校验格式版本和 ABI hash。
2. 按当前模块的 `__object_heap_start` 重定位对象字节。
3. 重定位句柄表。
4. 执行当前模块的 `__wjsm_init_function_props`（幂等）。
5. 进入用户 `main`。

restore 路径是 1:1 原位恢复，不是克隆。连续创建多个 Realm 时，每次 restore 都从原始嵌入字节开始。

## 禁用快照的场景

- **benchmark**：测量 cold bootstrap 开销，对比快照加速效果。
- **调试 bootstrap 逻辑**：变更 builtin 初始化代码后，需要走 cold 路径验证。
- **快照不匹配排查**：`WJSM_STARTUP_SNAPSHOT_DEBUG=1` 查看诊断信息。

## 深入了解

- [构建工件索引](../../internals/reference/artifact-index.md)
- [核心不变量](../../internals/reference/invariants.md)
- [ADR 0003: Startup Snapshot Boundary](../../../../adr/0003-startup-snapshot-boundary.md)
