# `cache`

管理磁盘编译缓存：native image（`*.wnat`）、builtin IR 段（`builtin_ir/*.bin`）与输入寻址 artifact 缓存（`artifact/*.wjsm` + `artifact/*.dep`）。

```bash
wjsm cache stats
wjsm cache clear
wjsm cache prune --max-bytes 1G
wjsm cache --dir /tmp/wjsm-cache stats
```

目录解析顺序：`--dir` > `WJSM_CACHE_DIR` > `${XDG_CACHE_HOME}/wjsm` > `${HOME}/.cache/wjsm`。`WJSM_CACHE_DIR` 设为空串表示显式禁用磁盘缓存，此时必须传 `--dir`，否则命令报错。`--dir` 只影响这次 `cache` 子命令，不会改变 `run` 的缓存目录。

native image 的 cache key 绑定 portable artifact digest、native ABI、native codegen source hash、target、Cranelift 版本和 codegen settings。artifact 缓存的 key 绑定源码闭包读集、resolution options 与语义 ABI 指纹，详见[编译缓存](../../internals/startup/compilation-cache.md)。

磁盘缓存默认启用并自动 LRU：默认上限 256 MiB，可用 `WJSM_CACHE_MAX_BYTES` 覆盖；`0` 关闭自动淘汰。stats/clear/prune 与自动淘汰的范围一致：顶层 `*.wnat`、`builtin_ir/*.bin` 与 `artifact/*.{wjsm,dep}`，不删除同目录下的其它文件。

cache 是可重建的派生数据，不是 `.wjsm` 用户制品。删除或 prune 只会让下一次运行重新编译；损坏、stale 或权限不安全的条目会被 invalidated，运行时不会执行未通过校验的 bytes。

## 深入了解

- [缓存实现](../../internals/tooling/cache.md)
- [预编译执行与磁盘缓存](../../internals/tooling/precompiled-execution.md)
- [环境变量](../configuration/environment-variables.md)
