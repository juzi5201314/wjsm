# 预编译执行与磁盘缓存

Direct Cranelift 切换后，不再有隐藏的 `__run-precompiled`、`--precompiled` 或 `PrecompiledEntry`。CLI 也没有 `hide = true` 的内部命令把 raw WASM 交给子进程 `deserialize_file`。

## 现在怎么跳过重复编译

可分发制品是 portable `.wjsm`，不是预编译 WASM：

```bash
wjsm build app.ts -o /tmp/app.wjsm
wjsm run /tmp/app.wjsm
```

`build` 只做到 verified IR + manifest。`run` 在当前宿主上把 artifact 编成 native image。默认不落盘；要跨进程复用机器码，显式打开磁盘缓存：

```bash
export WJSM_CACHE_DIR=~/.cache/wjsm
wjsm run /tmp/app.wjsm
```

命中时 `NativeImageRepository` 从 `${WJSM_CACHE_DIR}/<digest>.wnat` 加载 image。key 绑定 artifact digest、native ABI、codegen hash、target、Cranelift 版本和 settings，不是文件 mtime。

## 测试路径

fixture 走 `run_file_in_process`，与 CLI 共用 `NativeRuntime`。测试进程默认不设 `WJSM_CACHE_DIR`，因此每个用例都冷编译。需要磁盘复用时由个别测试自己设置（例如 `tests/cluster_ipc.rs` 的 `/tmp/wjsm-test-cache`）。

`--watch` 是父进程监听、子进程整段重跑，不是预编译 handoff。

## 同宿主 native executable

`wjsm build --format native-executable` 把预链 `wjsm-exec`、预编译 `NativeObject` 与制品内源码快照打进真实 ELF/PE。启动走 `CompiledImage::load`，跳过主程序 codegen；运行时解析只读快照。这不是磁盘 cache，也不是把 `.wnat` 改后缀。详见 [ADR 0016](../../../../adr/0016-native-executable-stub-overlay.md) 与 [ADR 0017](../../../../adr/0017-native-executable-source-snapshot.md)。

## 深入了解

- [源码输入与编译编排](source-input.md)
- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 Portable `.wjsm` 制品](../../user/output/portable-artifacts.md)
