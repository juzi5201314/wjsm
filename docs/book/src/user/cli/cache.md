# `cache`

管理当前宿主派生的 native image cache：

```bash
wjsm cache stats
wjsm cache clear
wjsm cache prune --max-bytes 1G
wjsm cache --dir /tmp/wjsm-cache stats
```

`--dir` 覆盖 `WJSM_CACHE_DIR`。cache key 绑定 portable artifact digest、native ABI、native codegen source hash、target、Cranelift 版本和 codegen settings。

cache 是可重建的派生数据，不是 `.wjsm` 用户制品。删除或 prune 只会让下一次运行重新编译；损坏、stale 或权限不安全的条目会被 invalidated，运行时不会执行未通过校验的 bytes。
