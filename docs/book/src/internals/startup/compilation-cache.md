# 编译缓存

这一章说明 `compile_or_load_cached` 如何缓存用户 WASM 的编译产物。

## 缓存 key

缓存 key 是 WASM 字节内容的 SipHash，前缀 `wasmtime-43`（纳入 wasmtime 版本避免跨版本冲突）：

```rust
"wasmtime-43".hash(&mut hasher);
wasm_bytes.hash(&mut hasher);
let key = format!("{:016x}", hasher.finish());
```

key 不受文件 mtime 影响，与 wasmtime 内置 cache 的 debug_assertions mtime keying 不同。

## 缓存目录

`module_cache_dir()` 解析缓存目录：

1. `WJSM_CACHE_DIR` 非空 → 使用该路径。
2. `WJSM_CACHE_DIR` 为空或未设置 → 回落 `$HOME/.cache/wjsm`。
3. 两者都不可用 → 返回 `None`，缓存禁用，直接 `Module::new` 编译。

`WJSM_CACHE_DIR=`（空值）不禁用缓存，仍回落到 `$HOME/.cache/wjsm`。

## 命中路径

缓存文件存在时，`Module::deserialize_file` 走 mmap 零拷贝加载，跳过 Cranelift 编译。这是冷启动的主要加速点。

如果缓存文件损坏或 engine 配置不匹配，删除文件后重新编译。

## 未命中路径

`Module::new` 编译 WASM，然后 `engine.precompile_module` 生成 cwasm，best-effort 写入缓存目录。写入失败不影响执行。

## 与启动快照的关系

编译缓存和启动快照是两套独立的机制：编译缓存跳过用户代码的编译，启动快照跳过 builtin JS 的执行。两者都加速启动，但对象不同。

## 深入了解

- [启动路径概览](startup-path.md)
- [Engine 配置与池化](engine-pool.md)
- [启动快照边界](startup-snapshot.md)
