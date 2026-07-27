//! `wjsm-host-wasm` 的 `JsBackend` 实现：wasm 编译 + wasmtime 执行。

use std::io::Write;

use anyhow::Result;
use wjsm_host::JsBackend;
use wjsm_ir::Program;

use crate::{CompileOptions, RuntimeOptions, compile_with_options, execute_with_writer_with_options};

/// wasmtime 后端：IR → wasm 字节 → wasmtime 执行。
pub struct WasmBackend;

impl JsBackend for WasmBackend {
    type Artifact = Vec<u8>;
    type ExecOptions = RuntimeOptions;

    fn name(&self) -> &'static str {
        "wasm"
    }

    fn compile(&self, program: &Program, debug: bool) -> Result<Self::Artifact> {
        compile_with_options(program, CompileOptions { debug })
    }

    fn artifact_bytes(artifact: &Self::Artifact) -> Option<&[u8]> {
        Some(artifact)
    }

    fn execute<'a, W: Write + 'a>(
        &'a self,
        artifact: &'a Self::Artifact,
        options: Self::ExecOptions,
        writer: W,
    ) -> impl Future<Output = Result<(W, Vec<u8>)>> + 'a {
        execute_with_writer_with_options(artifact, writer, options)
    }
}
