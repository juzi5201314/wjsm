//! `wjsm-backend-jit` 的 `JsBackend` 实现：保持 stub 状态，与现状用户可见错误一致。

use std::io::Write;

use anyhow::{Result, bail};
use wjsm_host::JsBackend;
use wjsm_ir::Program;

/// JIT 后端 stub：编译与执行均返回未实现错误，与现状用户可见行为一致。
pub struct JitBackend;

impl JsBackend for JitBackend {
    type Artifact = Vec<u8>;
    type ExecOptions = ();

    fn name(&self) -> &'static str {
        "jit"
    }

    fn compile(&self, _program: &Program, _debug: bool) -> Result<Self::Artifact> {
        bail!("JIT backend is not implemented yet")
    }

    fn artifact_bytes(_artifact: &Self::Artifact) -> Option<&[u8]> {
        None
    }

    async fn execute<'a, W: Write + 'a>(
        &'a self,
        _artifact: &'a Self::Artifact,
        _options: Self::ExecOptions,
        _writer: W,
    ) -> Result<(W, Vec<u8>)> {
        bail!("JIT backend is not implemented yet")
    }
}
