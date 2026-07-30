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

目录由 `WJSM_CACHE_DIR` 决定，未设置时回落到 `$HOME/.cache/wjsm`。把 `WJSM_CACHE_DIR` 设为空字符串，或 `HOME` 未设置，缓存即禁用。详见[环境变量](../configuration/environment-variables.md)。

缓存放的是 Wasmtime 编译结果，不是你的 `.wasm` 产物。删掉它只会让下一次运行重新编译，不会丢失 `wjsm build` 写出的文件。

> <details><summary>这个缓存和你「想缓存的东西」可能不一样</summary>
>
> 很多用户第一次看到 `cache stats` 显示 1.2 GB 的缓存时会惊讶：明明我只是 `wjsm run` 跑过几次小程序。原因是这里的「缓存」是 Wasmtime 自己的 cwasm 缓存——把 WASM 字节码编成机器码的结果，跨文件、跨项目共享。
>
> 这条缓存的设计是「反正 Cranelift 编译 WASM 也要花时间，结果存起来下次直接用」。它和 wjsm 的 IR 缓存、build 缓存是两件不同的事——wjsm 不维护后两者，每次都重新生成。
>
> 什么情况下手动 `cache clear`？
>
> - 磁盘不够了。
> - 怀疑缓存内容和新版本 wjsm 不兼容（一般不需要——哈希键会自动失配）。
> - 调试时想强制重编译。
>
> </details>

## 深入了解

- [编译缓存的键计算与失效规则](../../internals/tooling/cache.md)
- [生成文件与缓存边界](../../internals/build-release/generated-artifacts.md)
