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
wjsm run /tmp/app.wjsm    # 磁盘缓存默认可用（~/.cache/wjsm，可用 WJSM_CACHE_DIR 覆盖）
```

命中时 `NativeImageRepository` 从 `${cache_dir}/<digest>.wnat` 加载 image。key 绑定 artifact digest、native ABI、codegen hash、target、Cranelift 版本和 settings，不是文件 mtime。

## 测试路径

fixture 走 `run_file_in_process`，与 CLI 共用 `NativeRuntime`。测试进程通过 `tests/support/test_env.rs` 把 `WJSM_CACHE_DIR` 重定向到 `/tmp` 下的进程隔离目录，避免污染用户缓存，也保证跨测试运行不吃到旧缓存。

`--watch` 是父进程监听、子进程整段重跑，不是预编译 handoff。

## 同宿主 native executable

`wjsm build --format native-executable` 把预链 `wjsm-exec`、预编译 `NativeObject` 与制品内源码快照打进真实 ELF/PE。overlay 正文整层 zstd。启动走 `CompiledImage::load`，跳过主程序 codegen；运行时解析只读快照。这不是磁盘 cache，也不是把 `.wnat` 改后缀。详见 [ADR 0016](../../../../adr/0016-native-executable-stub-overlay.md)、[ADR 0017](../../../../adr/0017-native-executable-source-snapshot.md)、[ADR 0018](../../../../adr/0018-native-executable-zstd-payload.md) 与 [ADR 0019](../../../../adr/0019-native-executable-application-contract.md)。

## 深入了解

- [源码输入与编译编排](source-input.md)
- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 Portable `.wjsm` 制品](../../user/output/portable-artifacts.md)
