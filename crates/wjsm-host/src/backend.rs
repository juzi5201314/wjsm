//! 编译后端契约：IR → 制品 → 同步执行。
//!
//! 后端只暴露稳定的编译与执行边界；portable artifact 与 native image 的所有权
//! 由对应后端实现，host crate 不依赖任何具体 runtime。
//! 静态分发由 CLI/调用方完成，trait 不做 `dyn`，避免热路径 vtable 开销。

use std::io::Write;

use anyhow::Result;
use wjsm_ir::Program;

/// 编译后端契约。
pub trait JsBackend {
    /// 后端编译制品。
    type Artifact: Send;
    /// 后端专属执行配置。
    type ExecOptions;

    /// 后端名称。
    fn name(&self) -> &'static str;

    /// IR → 制品；`debug` 控制语句级调试插桩。
    fn compile(&self, program: &Program, debug: bool) -> Result<Self::Artifact>;

    /// 制品的持久化字节；不可序列化的后端返回 `None`。
    fn artifact_bytes(artifact: &Self::Artifact) -> Option<&[u8]>;

    /// 同步执行制品并返回输出诊断。
    fn execute<W: Write>(
        &self,
        artifact: &Self::Artifact,
        options: Self::ExecOptions,
        writer: W,
    ) -> Result<(W, Vec<u8>)>;
}
