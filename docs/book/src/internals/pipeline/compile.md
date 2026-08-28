# Native 编译阶段

这一章说明 verified portable artifact 如何变成当前宿主的 native image，以及磁盘缓存何时介入。

## 编译入口

CLI 不再按 `Target::Wasm` / `Target::Jit` 分发。`run` / `run_file_in_process` 把已验证的 `PortableArtifact` 交给 `NativeImageRepository::prepare`：

1. 用 `NativeCacheKey` 查进程内 Weak 池；
2. 若 `cache_dir` 有值，再查 `${WJSM_CACHE_DIR}/<digest>.wnat`；
3. 都未命中则 `NativeCompiler::compile(artifact)`，从 IR 直接生成当前宿主 object。

`NativeCompiler::compile` 在 `wjsm-backend-native`：输入是 artifact 里的 `Program`，输出是 `NativeObject`。没有 `compile_program_to_wasm`，也没有 Wasmtime deserialize。

## 磁盘缓存

`cache_dir` 来自 `NativeRuntimeConfig`。CLI 与 in-process 测试经 `resolve_cache_dir()` 传入（`WJSM_CACHE_DIR` > XDG/HOME 回落，空串禁用）。缓存可用时 miss 写入 `.wnat`，损坏 / stale / 权限不安全的条目 invalidated 后重编译；写入失败静默降级，不影响编译。

## 后端侧入口

`wjsm-backend-native` 暴露的编译入口按用途分开：

| 函数 | 用途 |
| --- | --- |
| `NativeCompiler::compile` | Normal 模式，产出 `NativeObject` |
| `NativeImageRepository::prepare` | 进程内池 + 可选磁盘 cache + compile |
| `dump-clif` / `disasm` | 诊断：CLIF 文本或当前宿主反汇编 |

Normal 模式与 eval 模式的差别不只是入口不同：normal 模式有独立 image，eval 模式共享当前 realm 的 vmctx。

## debug 插桩的传递

`--inspect` / `--inspect-brk` 会让 `Cli::wants_debug_codegen()` 为 true，该标记同时传给 lowering（发射 `DebugCheck`）和 codegen（生成 debug 段与 epoch interruption 检查）。两侧必须同时开启，否则断点无法映射回源码位置。

## 深入了解

- [编译器内部结构](../backend/compiler-architecture.md)
- [编译缓存](../startup/compilation-cache.md)
- [Normal 与 Eval 编译模式](../backend/normal-and-eval-modes.md)
- [Import、Export 与 ABI](../backend/imports-exports-and-abi.md)
- [多后端边界与 JsBackend 契约](../backend/multi-backend-boundary.md)
