# ABI Hash 与兼容性指纹

这一章说明 ABI 哈希的组成和兼容性指纹的计算。

## 哈希组成

`combined_abi_external_input(engine)` 把三项哈希合成单个 `u64`：

```rust
support_abi_union_hash()      // 三种 GC flavor 的 support ABI
builtin_js_bundle_hash()     // builtin_js manifest 里的 .js 文件
compatibility_fingerprint(engine)  // wasmtime engine 配置指纹
```

任一项变化，合成哈希变化，快照失配。

## support_abi_union_hash

三种 GC flavor（mark-sweep、g1、zgc）的 support module 各有 ABI hash。union hash 是它们的组合，让一个 embedded snapshot 能匹配任意 GC 选择——快照恢复后根据当前 GC 加载对应 support cwasm。

## builtin_js_bundle_hash

`builtin_js::manifest::BUILTIN_JS_FILES` 是 `(name, source)` 序列。每个 `.js` 文件的名字和内容参与哈希：

```rust
for (name, source) in builtin_js::manifest::BUILTIN_JS_FILES {
    name.hash(&mut h);
    source.hash(&mut h);
}
```

改 builtin JS（修内置 polyfill、加新方法等）会让快照失配。

## compatibility_fingerprint

`compatibility_fingerprint(engine)` 由唯一 engine-config owner 计算，不把 wasmtime 类型泄漏到 snapshot-format crate。它包含 compiler 选择、opt level、epoch、memory reservation、guest debug 等影响代码生成的配置。

`WASMTIME_VERSION = "43.0.2"` 也参与 fingerprint，跨 wasmtime 版本会失配。

## final hash

`startup_snapshot_abi_hash(engine)` 调用 `wjsm_snapshot_format::abi_hash_with_external_input(combined_abi_external_input(engine))`，把合成外部输入与 snapshot format 自身的稳定基线组合，得到最终 ABI hash。这个 hash 写入快照 header 的 `abi_hash` 字段。

## 失配的行为

失配时 `embedded_startup_snapshot_view` 返回 `None`，走 cold bootstrap。`WJSM_STARTUP_SNAPSHOT_DEBUG=1` 会在 stderr 打印诊断信息。

## 深入了解

- [启动快照边界](startup-snapshot.md)
- [冷启动与失配处理](cold-start-and-mismatch.md)
- [构建期嵌入工件](embedded-artifacts.md)
