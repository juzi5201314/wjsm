# 生成文件与缓存边界

这一章说明构建系统生成的文件和运行时缓存的边界。

## 构建期生成

开发构建当前不生成嵌入工件。native cache 在磁盘缓存目录可解析时按需落盘（`WJSM_CACHE_DIR` > XDG/HOME 回落，空串禁用）。

`build.rs` 生成的测试函数写入 `$OUT_DIR`，不提交到仓库。`include!` 宏在编译期把生成文件包含进测试模块。

## 运行时缓存

| 工件 | 位置 | 键 | 生命周期 |
| --- | --- | --- | --- |
| Native image cache | `${cache_dir}/*.wnat` | artifact digest + native ABI + codegen source hash + target + Cranelift + settings | 进程间持久 |
| Builtin IR 段缓存 | `${cache_dir}/builtin_ir/` | sha256(ABI hash ‖ debug ‖ builtin canonical 名) | 进程间持久 |
| 输入寻址 artifact 缓存 | `${cache_dir}/artifact/` | sha256(源码闭包读集 ‖ 选项 ‖ 语义 ABI) | 进程间持久 |
| Portable `.wjsm` | 用户指定路径 | — | 用户管理 |

## 缓存可重建性

所有缓存都是可重建的派生数据，不是 `.wjsm` 用户制品。删除或 prune 只会让下一次运行重新编译。

损坏、stale 或权限不安全的 cache entry 会被 invalidated，运行时不会执行未通过校验的 bytes。

## 生成物与临时文件

- 构建产物在 `target/`。
- 本手册的构建输出写 `/tmp`。
- 不提交 `docs/book/book/`。
- 临时验证脚本不落在仓库里，用 `-e` 传内联源码或写 `/tmp`。

## 深入了解

- [缓存实现](../tooling/cache.md)
- [仓库布局](../foundations/repository-layout.md)
- [构建工件索引](../reference/artifact-index.md)
