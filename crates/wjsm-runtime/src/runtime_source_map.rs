//! WASM source map 解析与 backtrace 格式化。
//!
//! 从 WASM 模块的 "wjsm_sourcemap" custom section 解析函数源码位置映射，
//! 并在运行时 trap 时将 wasmtime WasmBacktrace 格式化为 JS 风格的堆栈跟踪。

use std::collections::HashMap;
use wasmtime::WasmBacktrace;

/// 函数源码位置映射（从 "wjsm_sourcemap" custom section 解析）。
#[derive(Debug, Clone, Default)]
pub(crate) struct SourceMapInfo {
    /// 源文件路径。
    pub source_file: Option<String>,
    /// WASM 函数索引 → (line, col)。
    pub entries: HashMap<u32, (u32, u32)>,
}

impl SourceMapInfo {
    /// 从 WASM 字节中解析 "wjsm_sourcemap" custom section。
    /// 格式：source_file_len(u32 LE) + source_file_bytes + num_entries(u32 LE)
    ///       + [func_idx(u32 LE), line(u32 LE), col(u32 LE)] * num_entries
    pub fn parse_from_wasm(wasm_bytes: &[u8]) -> Option<Self> {
        let data = find_custom_section(wasm_bytes, "wjsm_sourcemap")?;
        Self::parse_section_data(data)
    }

    fn parse_section_data(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        let mut offset = 0usize;

        // source_file_len (u32 LE)
        let sf_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        let source_file = if sf_len > 0 && offset + sf_len <= data.len() {
            Some(String::from_utf8_lossy(&data[offset..offset + sf_len]).into_owned())
        } else {
            None
        };
        offset += sf_len;

        // num_entries (u32 LE)
        if offset + 4 > data.len() {
            return Some(SourceMapInfo {
                source_file,
                entries: HashMap::new(),
            });
        }
        let num_entries = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;

        let mut entries = HashMap::with_capacity(num_entries as usize);
        for _ in 0..num_entries {
            if offset + 12 > data.len() {
                break;
            }
            let func_idx = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let line = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let col = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            entries.insert(func_idx, (line, col));
            offset += 12;
        }

        Some(SourceMapInfo {
            source_file,
            entries,
        })
    }

    /// 查找 WASM 函数索引对应的源码位置。
    pub fn lookup(&self, func_idx: u32) -> Option<(u32, u32)> {
        self.entries.get(&func_idx).copied()
    }
}

/// 从 WASM 字节中查找指定名称的 custom section，返回其 data 部分。
fn find_custom_section<'a>(wasm_bytes: &'a [u8], target_name: &str) -> Option<&'a [u8]> {
    // WASM magic + version = 8 bytes
    if wasm_bytes.len() < 8 {
        return None;
    }
    let mut offset = 8usize;

    while offset < wasm_bytes.len() {
        // section id (1 byte)
        let section_id = wasm_bytes[offset];
        offset += 1;

        // section size (LEB128)
        let (size, consumed) = read_leb128(wasm_bytes, offset)?;
        offset += consumed;

        let section_end = offset + size as usize;
        if section_end > wasm_bytes.len() {
            return None;
        }

        if section_id == 0 {
            // custom section: name_len (LEB128) + name + data
            let (name_len, name_consumed) = read_leb128(wasm_bytes, offset)?;
            let name_start = offset + name_consumed;
            let name_end = name_start + name_len as usize;
            if name_end > section_end {
                return None;
            }
            let name = String::from_utf8_lossy(&wasm_bytes[name_start..name_end]);
            if name == target_name {
                return Some(&wasm_bytes[name_end..section_end]);
            }
        }

        offset = section_end;
    }

    None
}

/// 读取 LEB128 编码的 u32，返回 (value, bytes_consumed)。
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

/// 将 wasmtime WasmBacktrace 格式化为 JS 风格的堆栈跟踪字符串。
///
/// 输出格式：
/// ```text
///     at functionName (sourceFile:line:col)
///     at anonymous (sourceFile:line:col)
/// ```
pub(crate) fn format_backtrace(
    backtrace: &WasmBacktrace,
    source_map: Option<&SourceMapInfo>,
) -> String {
    let frames = backtrace.frames();
    if frames.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::with_capacity(frames.len());
    let source_file = source_map.and_then(|m| m.source_file.as_deref());

    for frame in frames {
        let func_name = frame.func_name().unwrap_or("<anonymous>");
        let func_idx = frame.func_index();

        let location = if let Some(sm) = source_map {
            if let Some((line, col)) = sm.lookup(func_idx) {
                let file = source_file.unwrap_or("unknown");
                format!("{file}:{line}:{col}")
            } else {
                let func_offset = frame.func_offset().unwrap_or(0);
                let file = source_file.unwrap_or("unknown");
                format!("{file}:{func_offset}")
            }
        } else {
            let func_offset = frame.func_offset().unwrap_or(0);
            format!("<wasm>:{func_offset}")
        };

        let entry = format!("    at {func_name} ({location})");
        // 压缩连续重复帧：相同行合并为 "entry (xN)"。
        if let Some(last) = lines.last_mut() {
            if let Some((prefix, count_str)) = last.rsplit_once(" (x") {
                if prefix == &entry {
                    let count: u32 = count_str.trim_end_matches(')').parse().unwrap_or(1);
                    *last = format!("{entry} (x{})", count + 1);
                    continue;
                }
            } else if last == &entry {
                *last = format!("{entry} (x2)");
                continue;
            }
        }
        lines.push(entry);
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_section() {
        // source_file_len=0, num_entries=0
        let data = [0, 0, 0, 0, 0, 0, 0, 0];
        let info = SourceMapInfo::parse_section_data(&data).unwrap();
        assert!(info.source_file.is_none());
        assert!(info.entries.is_empty());
    }

    #[test]
    fn parse_with_source_file() {
        // source_file="test.ts" (7 bytes), num_entries=0
        let mut data = Vec::new();
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(b"test.ts");
        data.extend_from_slice(&0u32.to_le_bytes());
        let info = SourceMapInfo::parse_section_data(&data).unwrap();
        assert_eq!(info.source_file.as_deref(), Some("test.ts"));
        assert!(info.entries.is_empty());
    }

    #[test]
    fn parse_with_entries() {
        let mut data = Vec::new();
        // source_file = "input.js" (8 bytes)
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(b"input.js");
        // num_entries = 2
        data.extend_from_slice(&2u32.to_le_bytes());
        // entry 1: func_idx=10, line=5, col=1
        data.extend_from_slice(&10u32.to_le_bytes());
        data.extend_from_slice(&5u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        // entry 2: func_idx=20, line=10, col=3
        data.extend_from_slice(&20u32.to_le_bytes());
        data.extend_from_slice(&10u32.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());

        let info = SourceMapInfo::parse_section_data(&data).unwrap();
        assert_eq!(info.source_file.as_deref(), Some("input.js"));
        assert_eq!(info.entries.len(), 2);
        assert_eq!(info.lookup(10), Some((5, 1)));
        assert_eq!(info.lookup(20), Some((10, 3)));
        assert_eq!(info.lookup(99), None);
    }

    #[test]
    fn leb128_basic() {
        // 0 → [0x00]
        assert_eq!(read_leb128(&[0x00], 0), Some((0, 1)));
        // 1 → [0x01]
        assert_eq!(read_leb128(&[0x01], 0), Some((1, 1)));
        // 127 → [0x7F]
        assert_eq!(read_leb128(&[0x7F], 0), Some((127, 1)));
        // 128 → [0x80, 0x01]
        assert_eq!(read_leb128(&[0x80, 0x01], 0), Some((128, 2)));
        // 300 → [0xAC, 0x02]
        assert_eq!(read_leb128(&[0xAC, 0x02], 0), Some((300, 2)));
    }
}
