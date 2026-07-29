# Host、Runtime 与 Builtins

这一部分讲执行侧：宿主契约、运行时状态、builtins 语义算法与 Runtime facade。

`wjsm-host` 定义后端无关的 trait 契约；`wjsm-builtins` 在这些 trait 上实现 ECMAScript / WHATWG 语义算法，以 `<E: ExecContext>` 泛型单态化；`wjsm-host-wasm` 是 wasmtime 后端的具体实现，也是当前唯一的执行后端。`wjsm-runtime` 是只做 re-export 的 facade。
