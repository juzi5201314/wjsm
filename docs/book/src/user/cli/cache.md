# cache

查看或清空编译产物缓存。

```bash
wjsm cache stats
wjsm cache clear
```

`stats` 打印缓存目录、条目数和总字节数：

```text
Cache directory: /home/you/.cache/wjsm
Entries: 2522
Bytes: 1219209192
```

缓存被禁用时，第一行是 `Cache disabled`，条目和字节都为 0。

`clear` 删除缓存条目并报告删除数量：

```text
Cleared 1 cache entries
```

## 缓存位置

目录由 `WJSM_CACHE_DIR` 决定，未设置时回落到 `$HOME/.cache/wjsm`。把 `WJSM_CACHE_DIR` 设为空字符串，
或 `HOME` 未设置，缓存即禁用。详见[环境变量](../configuration/environment-variables.md)。

缓存放的是 Wasmtime 编译结果，不是你的 `.wasm` 产物。删掉它只会让下一次运行重新编译，不会丢失
`wjsm build` 写出的文件。

## 深入了解

- [编译缓存的键计算与失效规则](../../internals/tooling/cache.md)
- [生成文件与缓存边界](../../internals/build-release/generated-artifacts.md)
