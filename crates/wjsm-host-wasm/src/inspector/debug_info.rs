//! 解析 `wjsm_debug` custom section；缺失时回退 `wjsm_sourcemap`。
//!
//! Backend 编码格式（version=1，见 `compiler_data.rs`）：
//! ```text
//! version: u32 LE (=1)
//! source_file_len: u32 LE + bytes
//! num_line_entries: u32 LE
//! [func_idx:u32, wasm_pc:u32, line:u32, col:u32] * N
//! num_local_entries: u32 LE
//! [func_idx:u32, local_idx:u32, name_len:u32, name_utf8] * M
//! num_debugger_pcs: u32 LE
//! [func_idx:u32, wasm_pc:u32] * K
//! ```

use crate::runtime_source_map::SourceMapInfo;
use std::collections::HashMap;

/// 行映射条目。
#[derive(Debug, Clone)]
pub(crate) struct LineEntry {
    pub func_idx: u32,
    pub wasm_pc: u32,
    pub line: u32,
    pub col: u32,
}

/// 局部变量名条目。
#[derive(Debug, Clone)]
pub(crate) struct LocalEntry {
    pub func_idx: u32,
    pub local_idx: u32,
    pub name: String,
}

/// 调试用脚本元数据。
#[derive(Debug, Clone, Default)]
pub(crate) struct DebugInfo {
    /// 源文件路径或 URL。
    pub source_url: String,
    /// 源码文本（供 `Debugger.getScriptSource`）；可能为空。
    pub source_text: String,
    /// 行映射表。
    pub line_entries: Vec<LineEntry>,
    /// 局部变量名。
    pub local_entries: Vec<LocalEntry>,
    /// debugger 语句 PC。
    pub debugger_pcs: Vec<(u32, u32)>,
    /// WASM 函数索引 → (1-based line, 1-based col) 快速表（取每个 func 首条 line entry）。
    pub func_entries: HashMap<u32, (u32, u32)>,
}

impl DebugInfo {
    /// 优先 `wjsm_debug`，否则 `wjsm_sourcemap`。
    pub fn parse_from_wasm(wasm_bytes: &[u8]) -> Self {
        if let Some(info) = Self::parse_wjsm_debug(wasm_bytes) {
            return info;
        }
        Self::from_sourcemap(SourceMapInfo::parse_from_wasm(wasm_bytes))
    }

    fn from_sourcemap(sm: Option<SourceMapInfo>) -> Self {
        let Some(sm) = sm else {
            return Self {
                source_url: "file://main.js".to_string(),
                source_text: String::new(),
                line_entries: Vec::new(),
                local_entries: Vec::new(),
                debugger_pcs: Vec::new(),
                func_entries: HashMap::new(),
            };
        };
        let source_url = sm
            .source_file
            .clone()
            .unwrap_or_else(|| "file://main.js".to_string());
        let source_text = load_source_text(&source_url).unwrap_or_default();
        Self {
            source_url,
            source_text,
            line_entries: Vec::new(),
            local_entries: Vec::new(),
            debugger_pcs: Vec::new(),
            func_entries: sm.entries,
        }
    }

    fn parse_wjsm_debug(wasm_bytes: &[u8]) -> Option<Self> {
        let data = find_custom_section(wasm_bytes, "wjsm_debug")?;
        Self::parse_wjsm_debug_payload(data)
    }

    /// 供测试直接解析段 payload。
    pub fn parse_wjsm_debug_payload(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        let mut offset = 0usize;
        let version = read_u32(data, &mut offset)?;
        if version != 1 {
            return None;
        }
        let source_url = read_len_string(data, &mut offset)?;

        let num_line = read_u32(data, &mut offset)? as usize;
        let mut line_entries = Vec::with_capacity(num_line);
        let mut func_entries = HashMap::new();
        for _ in 0..num_line {
            let func_idx = read_u32(data, &mut offset)?;
            let wasm_pc = read_u32(data, &mut offset)?;
            let line = read_u32(data, &mut offset)?;
            let col = read_u32(data, &mut offset)?;
            func_entries.entry(func_idx).or_insert((line, col));
            line_entries.push(LineEntry {
                func_idx,
                wasm_pc,
                line,
                col,
            });
        }

        let num_locals = read_u32(data, &mut offset).unwrap_or(0) as usize;
        let mut local_entries = Vec::with_capacity(num_locals);
        for _ in 0..num_locals {
            let func_idx = read_u32(data, &mut offset)?;
            let local_idx = read_u32(data, &mut offset)?;
            let name = read_len_string(data, &mut offset)?;
            local_entries.push(LocalEntry {
                func_idx,
                local_idx,
                name,
            });
        }

        let num_dbg = read_u32(data, &mut offset).unwrap_or(0) as usize;
        let mut debugger_pcs = Vec::with_capacity(num_dbg);
        for _ in 0..num_dbg {
            let func_idx = read_u32(data, &mut offset)?;
            let wasm_pc = read_u32(data, &mut offset)?;
            debugger_pcs.push((func_idx, wasm_pc));
        }

        let source_url = if source_url.is_empty() {
            "file://main.js".to_string()
        } else {
            source_url
        };
        let source_text = load_source_text(&source_url).unwrap_or_default();
        Some(Self {
            source_url,
            source_text,
            line_entries,
            local_entries,
            debugger_pcs,
            func_entries,
        })
    }

    pub fn lookup_func(&self, func_idx: u32) -> Option<(u32, u32)> {
        self.func_entries.get(&func_idx).copied()
    }

    /// 按 (func, wasm_pc) 查找最近的行映射。
    pub fn lookup_pc(&self, func_idx: u32, wasm_pc: u32) -> Option<(u32, u32)> {
        let mut best: Option<&LineEntry> = None;
        for e in &self.line_entries {
            if e.func_idx != func_idx || e.wasm_pc > wasm_pc {
                continue;
            }
            if best.is_none_or(|b| e.wasm_pc >= b.wasm_pc) {
                best = Some(e);
            }
        }
        best.map(|e| (e.line, e.col))
            .or_else(|| self.lookup_func(func_idx))
    }

    #[allow(dead_code)] // CDP Scope 枚举局部变量时使用
    pub fn locals_for_func(&self, func_idx: u32) -> impl Iterator<Item = &LocalEntry> {
        self.local_entries
            .iter()
            .filter(move |e| e.func_idx == func_idx)
    }

    /// 是否包含 debugger 语句映射（诊断用）。
    pub fn has_debugger_pcs(&self) -> bool {
        !self.debugger_pcs.is_empty()
    }
}

fn load_source_text(source_url: &str) -> Option<String> {
    let path = source_url.strip_prefix("file://").unwrap_or(source_url);
    std::fs::read_to_string(path).ok()
}

fn read_u32(data: &[u8], offset: &mut usize) -> Option<u32> {
    if *offset + 4 > data.len() {
        return None;
    }
    let v = u32::from_le_bytes([
        data[*offset],
        data[*offset + 1],
        data[*offset + 2],
        data[*offset + 3],
    ]);
    *offset += 4;
    Some(v)
}

fn read_len_string(data: &[u8], offset: &mut usize) -> Option<String> {
    let len = read_u32(data, offset)? as usize;
    if *offset + len > data.len() {
        return None;
    }
    let s = String::from_utf8_lossy(&data[*offset..*offset + len]).into_owned();
    *offset += len;
    Some(s)
}

/// 从 WASM 字节中查找指定名称的 custom section，返回其 data 部分。
pub(crate) fn find_custom_section<'a>(wasm_bytes: &'a [u8], target_name: &str) -> Option<&'a [u8]> {
    if wasm_bytes.len() < 8 {
        return None;
    }
    let mut offset = 8usize;
    while offset < wasm_bytes.len() {
        let section_id = wasm_bytes[offset];
        offset += 1;
        let (size, consumed) = read_leb128(wasm_bytes, offset)?;
        offset += consumed;
        let section_end = offset + size as usize;
        if section_end > wasm_bytes.len() {
            return None;
        }
        if section_id == 0 {
            let (name_len, name_consumed) = read_leb128(wasm_bytes, offset)?;
            let name_start = offset + name_consumed;
            let name_end = name_start + name_len as usize;
            if name_end > section_end {
                return None;
            }
            let name = std::str::from_utf8(&wasm_bytes[name_start..name_end]).ok()?;
            if name == target_name {
                return Some(&wasm_bytes[name_end..section_end]);
            }
        }
        offset = section_end;
    }
    None
}

fn read_leb128(data: &[u8], offset: usize) -> Option<(u32, usize)> {
    let mut result = 0u32;
    let mut shift = 0u32;
    let mut i = 0usize;
    loop {
        if offset + i >= data.len() {
            return None;
        }
        let byte = data[offset + i];
        i += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 32 {
            return None;
        }
    }
    Some((result, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wjsm_debug_v1_backend_format() {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        let url = b"main.js";
        data.extend_from_slice(&(url.len() as u32).to_le_bytes());
        data.extend_from_slice(url);
        // 1 line entry
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&10u32.to_le_bytes()); // func
        data.extend_from_slice(&0u32.to_le_bytes()); // pc
        data.extend_from_slice(&3u32.to_le_bytes()); // line
        data.extend_from_slice(&1u32.to_le_bytes()); // col
        // 1 local
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&10u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let name = b"x";
        data.extend_from_slice(&(name.len() as u32).to_le_bytes());
        data.extend_from_slice(name);
        // 0 debugger pcs
        data.extend_from_slice(&0u32.to_le_bytes());

        let info = DebugInfo::parse_wjsm_debug_payload(&data).unwrap();
        assert_eq!(info.source_url, "main.js");
        assert_eq!(info.lookup_func(10), Some((3, 1)));
        let locals: Vec<_> = info.locals_for_func(10).collect();
        assert_eq!(locals.len(), 1);
        assert_eq!(locals[0].name, "x");
        assert_eq!(locals[0].local_idx, 0);
        assert_eq!(locals[0].func_idx, 10);
        assert_eq!(info.lookup_pc(10, 0), Some((3, 1)));
        assert!(!info.has_debugger_pcs());
        assert_eq!(info.line_entries[0].func_idx, 10);
        assert_eq!(info.line_entries[0].wasm_pc, 0);
        assert_eq!(info.line_entries[0].line, 3);
        assert_eq!(info.line_entries[0].col, 1);
    }
}
