# `cache`

管理当前宿主派生的 native image cache。磁盘缓存默认关闭；`run` / `build` 只有在设置了 `WJSM_CACHE_DIR` 时才会读写 `.wnat` 条目。

```bash
export WJSM_CACHE_DIR=~/.cache/wjsm
wjsm cache stats
wjsm cache clear
wjsm cache prune --max-bytes 1G
wjsm cache --dir /tmp/wjsm-cache stats
```

未设置 `WJSM_CACHE_DIR` 时必须传 `--dir`，否则命令报 `native cache directory is not configured`。`--dir` 只影响这次 `cache` 子命令，不会替 `run` 打开编译缓存。

cache key 绑定 portable artifact digest、native ABI、native codegen source hash、target、Cranelift 版本和 codegen settings。

打开磁盘缓存后还有自动 LRU：默认上限 256 MiB，可用 `WJSM_CACHE_MAX_BYTES` 覆盖；`0` 关闭自动淘汰。淘汰范围是顶层 `*.wnat` 与 `builtin_ir/*.bin`，不删除同目录下的其它文件。

cache 是可重建的派生数据，不是 `.wjsm` 用户制品。删除或 prune 只会让下一次运行重新编译；损坏、stale 或权限不安全的条目会被 invalidated，运行时不会执行未通过校验的 bytes。

## 深入了解

- [缓存实现](../../internals/tooling/cache.md)
- [预编译执行与磁盘缓存](../../internals/tooling/precompiled-execution.md)
- [环境变量](../configuration/environment-variables.md)
