# 快照、缓存与嵌入工件问题

## 缓存位置不对

`wjsm cache stats` 打印当前生效目录：

```text
Cache directory: /home/you/.cache/wjsm
Entries: 2522
Bytes: 1219209192
```

`WJSM_CACHE_DIR` 非空时优先，否则回落 `$HOME/.cache/wjsm`。把变量设为空字符串不会禁用缓存，只是跳过这个来源；只有 `HOME` 也不可用时才显示 `Cache disabled`。

## 缓存占用过大

编译缓存按内容哈希累积，不自动淘汰：

```bash
wjsm cache clear
```

## 启动变慢

先确认启动快照没被关掉。`WJSM_STARTUP_SNAPSHOT` 取 `0`、`false`、`off` 会禁用它，其他值（含未设置）都启用。

排查快照本身是否被使用：

```bash
WJSM_STARTUP_SNAPSHOT_DEBUG=1 wjsm run app.js
```

## 换了 wjsm 版本后行为异常

快照与预编译 support 模块带 ABI 指纹，版本不匹配时运行时会自行回退到冷启动路径，不会加载过期字节。若怀疑残留，清一次缓存重跑。

## 深入了解

- [启动快照边界](../../internals/startup/startup-snapshot.md)
- [ABI Hash 与兼容性指纹](../../internals/startup/abi-hash.md)
- [缓存实现](../../internals/tooling/cache.md)
