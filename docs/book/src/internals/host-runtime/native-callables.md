# NativeCallable 注册表

这一章说明 IR 的 `Builtin` 枚举如何映射到运行时函数。

## 两层映射

| 层 | 位置 | 职责 |
| --- | --- | --- |
| codegen | `compiler_builtins_*.rs` | `Builtin` → WASM function index |
| runtime | `runtime_builtins.rs` | WASM function index → Rust 函数 |

codegen 在 Pass 1 预留 host import 索引，函数体编译时直接引用。运行时 `runtime_linker.rs` 把 Rust 函数注册到 wasmtime Linker，两侧通过 import 名对齐。

## Builtin 分派

`compiler_builtins_core.rs` 的 `compile_builtin_core` 是分派入口，按 Builtin variant 选择生成路径：

- `ConsoleLog` / `ConsoleError` / ... → 影子栈传参 + `Call console_*`
- `Fetch` → `Call fetch`
- `NewObject` / `NewArray` → `Call obj_new` / `Call arr_new`
- `PromiseResolve` / `PromiseThen` → `Call promise_resolve` / `promise_then`

按域分组的实现文件：

| 文件 | 域 |
| --- | --- |
| `compiler_builtins_core.rs` | Console ~ EnumeratorKey |
| `compiler_builtins_collections.rs` | Map/Set/WeakMap |
| `compiler_builtins_string_math.rs` | String/Math/Number |
| `compiler_builtins_async_proxy.rs` | Promise/Proxy/Reflect/async |
| `compiler_builtins_runtime.rs` | 模块/eval/require |

## 返回值处理

`BuiltinDispatch::Handled` 表示 codegen 已生成完整调用序列，`NeedsFallback` 表示需要默认处理。`dest: Option<ValueId>` 为 `None` 时 builtin 只有副作用，不产生值——codegen 跳过结果赋值。

## 深入了解

- [Host Import 注册与包装层](host-imports.md)
- [Instruction 与 Constant 的 CallBuiltin](../ir/instructions-and-constants.md)
- [语义层如何选择 Builtin 拦截](../frontend/expressions-and-statements.md)
