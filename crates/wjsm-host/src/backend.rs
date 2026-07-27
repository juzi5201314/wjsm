//! 编译后端契约：IR → 制品 → 执行。
//!
//! 为未来 llvm/c/jit 等后端设计：
//! - `Artifact` 因后端而异（wasm 字节 / native 镜像 / C 源码包）；
//! - `ExecOptions` 是后端专属执行配置（后端无关的 argv/env/fs 等由各 Options 自带）；
//! - 静态分发（CLI enum match）：trait 不做 `dyn`，零冷/热路径开销。

use std::io::Write;

use anyhow::Result;
use wjsm_ir::Program;

/// 编译后端契约。
///
/// 每个后端实现本 trait，CLI 按 `Target` enum 静态分发到具体后端。
/// 不使用 `dyn`——所有调用经 `match target` 单态化。
pub trait JsBackend {
    /// 后端编译制品（wasm 字节 / native 镜像 / C 源码包等）。
    type Artifact: Send;
    /// 后端专属执行配置（wasm 后端用 `RuntimeOptions`，native 后端可能用 argv/env 等）。
    type ExecOptions;

    /// 后端名称（`"wasm"` / `"jit"` / `"llvm"` 等）。
    fn name(&self) -> &'static str;

    /// IR → 制品；`debug` 控制语句级调试插桩。
    fn compile(&self, program: &Program, debug: bool) -> Result<Self::Artifact>;

    /// 制品的持久化字节（`build -o` 写盘）；不可序列化的后端返回 `None`。
    fn artifact_bytes(artifact: &Self::Artifact) -> Option<&[u8]>;

    /// 执行制品；输出到 `writer`，返回 `(writer, diagnostics)`。
    ///
    /// 返回的 Future 不要求 `Send`：CLI 用单线程 `block_on` 驱动，
    /// 各后端（含 wasmtime）的执行 future 在该线程上完成。
    fn execute<'a, W: Write + 'a>(
        &'a self,
        artifact: &'a Self::Artifact,
        options: Self::ExecOptions,
        writer: W,
    ) -> impl Future<Output = Result<(W, Vec<u8>)>> + 'a;
}
