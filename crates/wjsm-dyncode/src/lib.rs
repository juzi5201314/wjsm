//! 动态代码编译编排（后端无关的编译管线入口）。
//!
//! 本 crate 集中管理 `parse → lower → compile` 的编译编排，把编译管线
//! （parser / semantic / backend）的调用从执行引擎（`wjsm-host-wasm`）中抽离。
//! 它不依赖运行时状态（`RuntimeState`），只做源码到 WASM 字节的转换。
//!
//! # 与 wjsm-host-wasm 的关系
//!
//! host-wasm 是 wasmtime 执行引擎（含 eval/vm 解释器，它们需要 `Caller<RuntimeState>`）。
//! 本 crate 提供**编译入口**，host-wasm 与 CLI 复用它，避免各自重复编排编译管线。

use anyhow::Result;

/// 编译 JS/TS 源码到 WASM 字节码。
///
/// `parse_module → lower_module → compile` 流程。供执行引擎测试及外部集成复用，
/// 避免重复定义编译编排。
pub fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    wjsm_backend_wasm::compile(&program)
}

/// 带调试插桩的编译（语句 `DebugCheck` + `wjsm_debug` 段 + `debug_break` 调用）。
///
/// 供 `--inspect` / 测试路径使用。
pub fn compile_source_with_debug(source: &str, filename: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module_with_debug_source(
        module,
        false,
        Some(std::sync::Arc::<str>::from(source)),
        filename,
        true,
    )?;
    wjsm_backend_wasm::compile_with_options(
        &program,
        wjsm_backend_wasm::CompileOptions { debug: true },
    )
}
