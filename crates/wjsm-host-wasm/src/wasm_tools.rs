//! WASM 字节层工具：WAT 打印、验证、section 尺寸统计、import 名枚举、
//! shared memory64 构造。
//!
//! 这些能力此前散落在 CLI（wasmprinter/wasmparser 直接依赖）与 facade 测试中；
//! 收缩后 host-wasm 是唯一持有 wasm 工具链依赖的 crate，外部一律经
//! `wjsm_runtime::*` re-export 使用。

use anyhow::Result;
use wasmtime::{MemoryType, SharedMemory};

use crate::heap::SharedHeapMemory;

/// 用 wasmprinter 输出 WAT 字符串。`name_unnamed(true)` 始终启用，
/// 使合成函数获得 `$fN` 名称；`skeleton` 为 true 时省略指令体。
pub fn dump_wat(wasm: &[u8], skeleton: bool) -> Result<String> {
    use wasmprinter::{Config, PrintFmtWrite};
    let mut cfg = Config::new();
    cfg.name_unnamed(true);
    if skeleton {
        cfg.print_skeleton(true);
    }
    let mut dst = String::new();
    cfg.print(wasm, &mut PrintFmtWrite(&mut dst))?;
    Ok(dst)
}

/// 验证 WASM 字节流；非法时返回带解析位置的错误。
pub fn validate_wasm(bytes: &[u8]) -> Result<()> {
    wasmparser::validate(bytes)?;
    Ok(())
}

/// 按 section 统计 WASM 字节尺寸；所有 code entry 聚合为一条 `Code`。
pub fn wasm_section_sizes(bytes: &[u8]) -> Result<Vec<(String, usize)>> {
    let mut sizes: Vec<(String, usize)> = Vec::new();
    let mut code_size: usize = 0;

    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload?;
        use wasmparser::Payload::*;
        let (name, size) = match payload {
            TypeSection(s) => ("Type", s.range().len()),
            ImportSection(s) => ("Import", s.range().len()),
            FunctionSection(s) => ("Function", s.range().len()),
            TableSection(s) => ("Table", s.range().len()),
            MemorySection(s) => ("Memory", s.range().len()),
            GlobalSection(s) => ("Global", s.range().len()),
            ExportSection(s) => ("Export", s.range().len()),
            StartSection { range, .. } => ("Start", range.len()),
            ElementSection(s) => ("Element", s.range().len()),
            CodeSectionEntry(s) => {
                code_size += s.range().len();
                continue;
            }
            DataSection(s) => ("Data", s.range().len()),
            CustomSection(s) => ("Custom", s.range().len()),
            _ => continue,
        };
        sizes.push((name.to_string(), size));
    }
    if code_size > 0 {
        sizes.push(("Code".to_string(), code_size));
    }
    Ok(sizes)
}

/// 枚举 WASM 模块 import 名（不含 module 前缀），供测试断言 ABI 契约。
pub fn wasm_import_names(bytes: &[u8]) -> Vec<String> {
    wasmparser::Parser::new(0)
        .parse_all(bytes)
        .filter_map(Result::ok)
        .filter_map(|payload| match payload {
            wasmparser::Payload::ImportSection(section) => Some(section),
            _ => None,
        })
        .flat_map(|section| section.into_imports().filter_map(Result::ok))
        .map(|import| import.name.to_string())
        .collect()
}

/// 用 canonical artifact engine 构造 shared memory64 对象堆内存。
pub fn new_shared_heap_memory(min_pages: u64, max_pages: u64) -> Result<SharedHeapMemory> {
    let engine = crate::engine_config::EngineConfig::artifact().build()?;
    let ty = MemoryType::builder()
        .memory64(true)
        .shared(true)
        .min(min_pages)
        .max(Some(max_pages))
        .build()?;
    Ok(SharedHeapMemory::new(SharedMemory::new(&engine, ty)?))
}
