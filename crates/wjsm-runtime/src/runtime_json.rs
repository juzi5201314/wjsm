//! SIMD-accelerated JSON parser and value builder for JSON.parse implementation
//!
//! Implements ECMAScript §24.5.1 JSON.parse(text, reviver).
//! SIMD acceleration inspired by sonic-rs techniques:
//! - StringBlock: parallel quote/backslash/control detection (32 bytes at once, AVX2)
//! - NonspaceBitmap: cached 64-byte whitespace bitmap for skip_whitespace

use crate::*;
use wasmtime::{AsContextMut, Caller};

// ── SIMD helpers ──────────────────────────────

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// 64-byte nonspace bitmap for whitespace skipping.
/// Bit i is set if byte at position (base + i) is NOT whitespace.
struct NonspaceBitmap {
    bits: u64,
    base: usize,
}

#[cfg(target_arch = "x86_64")]
fn compute_nonspace_bits_avx2(input: &[u8], base: usize) -> u64 {
    if base + 64 > input.len() {
        return compute_nonspace_bits_scalar(input, base);
    }
    // SAFETY: caller guarantees base + 64 <= input.len(), so 64 bytes are readable.
    unsafe {
        let ptr = input[base..].as_ptr();
        let v1 = _mm256_loadu_si256(ptr as *const __m256i);
        let v2 = _mm256_loadu_si256(ptr.add(32) as *const __m256i);

        let sp = _mm256_set1_epi8(0x20);
        let tab = _mm256_set1_epi8(0x09);
        let nl = _mm256_set1_epi8(0x0a);
        let cr = _mm256_set1_epi8(0x0d);

        let m1 = _mm256_or_si256(
            _mm256_or_si256(_mm256_cmpeq_epi8(v1, sp), _mm256_cmpeq_epi8(v1, tab)),
            _mm256_or_si256(_mm256_cmpeq_epi8(v1, nl), _mm256_cmpeq_epi8(v1, cr)),
        );
        let m2 = _mm256_or_si256(
            _mm256_or_si256(_mm256_cmpeq_epi8(v2, sp), _mm256_cmpeq_epi8(v2, tab)),
            _mm256_or_si256(_mm256_cmpeq_epi8(v2, nl), _mm256_cmpeq_epi8(v2, cr)),
        );
        // m1/m2: whitespace positions = 0xFF → movemask bit = 1
        // We want nonspace = !whitespace
        let ws1 = _mm256_movemask_epi8(m1) as u64;
        let ws2 = _mm256_movemask_epi8(m2) as u64;
        !(ws1 | (ws2 << 32))
    }
}

#[inline(always)]
fn compute_nonspace_bits_scalar(input: &[u8], base: usize) -> u64 {
    let mut bits: u64 = 0;
    let end = (base + 64).min(input.len());
    for i in base..end {
        let b = input[i];
        if b != b' ' && b != b'\t' && b != b'\n' && b != b'\r' {
            bits |= 1u64 << (i - base);
        }
    }
    bits
}

/// Compute nonspace bits using AVX2 if available, otherwise scalar.
#[cfg(target_arch = "x86_64")]
fn compute_nonspace_bits(input: &[u8], base: usize) -> u64 {
    if is_x86_feature_detected!("avx2") {
        compute_nonspace_bits_avx2(input, base)
    } else {
        compute_nonspace_bits_scalar(input, base)
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn compute_nonspace_bits(input: &[u8], base: usize) -> u64 {
    compute_nonspace_bits_scalar(input, base)
}

/// StringBlock: parallel detection of quotes, backslashes, and control chars.
/// AVX2-only; callers must gate behind `is_x86_feature_detected!("avx2")`.
#[cfg(target_arch = "x86_64")]
struct StringBlock {
    quote_bits: u32,
    backslash_bits: u32,
    control_bits: u32,
}

#[cfg(target_arch = "x86_64")]
impl StringBlock {
    /// Load 32 bytes and compute bitmasks for structural characters.
    ///
    /// # Safety
    /// - Caller must ensure `ptr` points to at least 32 readable bytes.
    /// - Caller must ensure AVX2 is available (via `is_x86_feature_detected!("avx2")`).
    unsafe fn new_avx2(ptr: *const u8) -> Self {
        // SAFETY: The caller of this `unsafe fn` has already ensured (see `# Safety` docs above):
        // - `ptr` points to at least 32 readable bytes (no out-of-bounds on loads)
        // - AVX2 is available (via `is_x86_feature_detected!("avx2")`), satisfying the
        //   `#[target_feature(enable = "avx2")]` requirement for all intrinsics below.
        // This explicit block is required under Rust 2024 edition (unsafe_op_in_unsafe_fn).
        unsafe {
            let v = _mm256_loadu_si256(ptr as *const __m256i);
            let quote = _mm256_set1_epi8(b'"' as i8);
            let bs = _mm256_set1_epi8(b'\\' as i8);
            // 控制字符阈值 0x20：0x20 > v 对 v in [0x00..=0x1F] 为 true
            // AND 排除高位字节（0x80..0xFF）：有符号比较时 0x20 > negative_byte 为 true
            // 所以必须 AND `(v >= 0)` 即 `v > -1` 来排除 UTF-8 continuation bytes
            let ctrl_thresh = _mm256_set1_epi8(0x20);
            let non_neg = _mm256_set1_epi8(-1); // -1 = 0xFF，cmpgt(v, -1) = (v > -1) = (v >= 0)
            let q_bits = _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, quote)) as u32;
            let bs_bits = _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, bs)) as u32;
            // control = (0x20 > v) AND (v >= 0)
            let ctrl_bits = _mm256_movemask_epi8(_mm256_and_si256(
                _mm256_cmpgt_epi8(ctrl_thresh, v),
                _mm256_cmpgt_epi8(v, non_neg),
            )) as u32;
            Self {
                quote_bits: q_bits,
                backslash_bits: bs_bits,
                control_bits: ctrl_bits,
            }
        }
    }

    fn has_quote_first(&self) -> bool {
        if self.quote_bits == 0 {
            return false;
        }
        if self.backslash_bits == 0 {
            return true;
        }
        self.quote_bits.trailing_zeros() < self.backslash_bits.trailing_zeros()
    }

    fn quote_index(&self) -> usize {
        self.quote_bits.trailing_zeros() as usize
    }

    fn has_backslash(&self) -> bool {
        self.backslash_bits != 0
    }
    #[allow(dead_code)]
    fn has_control(&self) -> bool {
        self.control_bits != 0
    }
}

// ── Internal parsed value representation ──

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

pub(crate) struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
    nonspace: NonspaceBitmap, // 缓存当前 64 字节对齐窗口的 nonspace 位图；skip_whitespace 中按需更新，避免重复 compute
}

impl<'a> JsonParser<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            nonspace: NonspaceBitmap {
                bits: 0,
                base: usize::MAX,
            },
        }
    }

    // ── SIMD-accelerated whitespace skipping ──
    // 设计：先做 ≤8 次标量检查（排空到 64 字节对齐边界），然后切入 SIMD 批量跳过

    fn skip_whitespace(&mut self) {
        // 快路径：逐字节检查，最多 8 次（排空到 64 字节对齐边界）
        let limit = (self.pos + 8).min(self.input.len());
        while self.pos < limit {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => return,
            }
        }
        // 批量路径：使用缓存的 64B nonspace 位图加速（跨多次 skip 命中同一窗口时避免重复计算）
        while self.pos + 64 <= self.input.len() {
            let base = self.pos & !63;
            if base != self.nonspace.base {
                // 窗口切换或首次：计算并缓存（SIMD 或 scalar）
                let bits = compute_nonspace_bits(self.input, base);
                self.nonspace = NonspaceBitmap { bits, base };
            }
            let bits = self.nonspace.bits;
            let offset = self.pos - base;
            let mask = bits >> offset;
            if mask != 0 {
                self.pos += mask.trailing_zeros() as usize;
                return;
            }
            // 当前 64 字节窗口全是空白，跳到下一个窗口
            self.pos = base + 64;
        }
        // 尾部逐字节
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let ch = self.input.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        match self.next() {
            Some(ch) if ch == expected => Ok(()),
            Some(ch) => Err(format!(
                "Expected '{}', found '{}'",
                expected as char, ch as char
            )),
            None => Err(format!(
                "Expected '{}', found end of input",
                expected as char
            )),
        }
    }

    pub(crate) fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'n') => self.parse_null(),
            Some(b't') => self.parse_true(),
            Some(b'f') => self.parse_false(),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(ch) => Err(format!("Unexpected character: {}", ch as char)),
            None => Err("Unexpected end of input".to_string()),
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, String> {
        if self.input[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(JsonValue::Null)
        } else {
            Err("Expected 'null'".to_string())
        }
    }

    fn parse_true(&mut self) -> Result<JsonValue, String> {
        if self.input[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(JsonValue::Bool(true))
        } else {
            Err("Expected 'true'".to_string())
        }
    }

    fn parse_false(&mut self) -> Result<JsonValue, String> {
        if self.input[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(JsonValue::Bool(false))
        } else {
            Err("Expected 'false'".to_string())
        }
    }
    fn parse_string(&mut self) -> Result<String, String> {
        if self.next() != Some(b'"') {
            return Err("Expected '\"'".to_string());
        }

        let start_pos = self.pos; // 位置在 '"' 之后

        // ── SIMD 快路径（AVX2）──
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                let mut simd_result = None;
                // SAFETY: AVX2 feature detected at runtime.
                unsafe { self.parse_string_simd(&mut simd_result) }?;
                if let Some(s) = simd_result {
                    return Ok(s); // SIMD 完整解析，直接返回
                }
                // SIMD 未完成（遇到转义或尾部），fall through 到慢路径
                // 此时 self.pos 已被重置为 start_pos
            }
        }

        let _ = start_pos; // referenced in SIMD fallback comment above; keeps binding for doc intent across cfg (silences unused_variable when !avx2 or avx2 not detected)

        // ── 慢路径：逐字节处理（含转义序列） ──
        let mut result = String::new();
        loop {
            match self.next() {
                None => return Err("Unterminated string".to_string()),
                Some(b'"') => return Ok(result),
                Some(b'\\') => match self.next() {
                    None => return Err("Unterminated escape sequence".to_string()),
                    Some(b'"') => result.push('"'),
                    Some(b'\\') => result.push('\\'),
                    Some(b'/') => result.push('/'),
                    Some(b'b') => result.push('\u{0008}'),
                    Some(b'f') => result.push('\u{000C}'),
                    Some(b'n') => result.push('\n'),
                    Some(b'r') => result.push('\r'),
                    Some(b't') => result.push('\t'),
                    Some(b'u') => {
                        let code_point = self.parse_hex_escape()?;
                        if (0xD800..=0xDBFF).contains(&code_point) {
                            if self.next() != Some(b'\\') {
                                return Err("Expected '\\' before low surrogate".to_string());
                            }
                            if self.next() != Some(b'u') {
                                return Err("Expected 'u' before low surrogate".to_string());
                            }
                            let low = self.parse_hex_escape()?;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Err("Invalid low surrogate".to_string());
                            }
                            let full = 0x10000 + ((code_point - 0xD800) << 10) + (low - 0xDC00);
                            match char::from_u32(full) {
                                Some(ch) => result.push(ch),
                                None => return Err("Invalid surrogate pair code point".to_string()),
                            }
                        } else if (0xDC00..=0xDFFF).contains(&code_point) {
                            return Err("Unexpected low surrogate".to_string());
                        } else {
                            match char::from_u32(code_point) {
                                Some(ch) => result.push(ch),
                                None => return Err("Invalid unicode escape".to_string()),
                            }
                        }
                    }
                    Some(ch) => return Err(format!("Invalid escape sequence: \\{}", ch as char)),
                },
                Some(ch) if ch < 0x20 => {
                    return Err(format!("Control character in string: 0x{:02X}", ch));
                }
                Some(ch) => {
                    if ch < 0x80 {
                        result.push(ch as char);
                    } else {
                        let start = self.pos - 1;
                        let width = match ch {
                            0xC0..=0xDF => 2,
                            0xE0..=0xEF => 3,
                            0xF0..=0xFF => 4,
                            _ => return Err("Invalid UTF-8 leading byte".to_string()),
                        };
                        if start + width > self.input.len() {
                            return Err("Incomplete UTF-8 sequence".to_string());
                        }
                        for i in 1..width {
                            let byte = self.input[start + i];
                            if (byte & 0xC0) != 0x80 {
                                return Err("Invalid UTF-8 continuation byte".to_string());
                            }
                        }
                        self.pos = start + width;
                        match std::str::from_utf8(&self.input[start..self.pos]) {
                            Ok(s) => result.push_str(s),
                            Err(_) => return Err("Invalid UTF-8 sequence".to_string()),
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn parse_string_simd(&mut self, result_out: &mut Option<String>) -> Result<(), String> {
        let start_pos = self.pos;
        while self.pos + 32 <= self.input.len() {
            // SAFETY: We are inside `unsafe fn parse_string_simd` (guarded by `is_x86_feature_detected!("avx2")` in the caller).
            // `self.pos + 32 <= self.input.len()` guarantees at least 32 readable bytes from `self.input[self.pos..]`.
            // This explicit unsafe block is required under Rust 2024 `unsafe_op_in_unsafe_fn`.
            let block = unsafe { StringBlock::new_avx2(self.input[self.pos..].as_ptr()) };
            // 只检查在第一个 quote/backslash 之前的控制字符
            let first_structural =
                (block.quote_bits | block.backslash_bits).trailing_zeros() as usize;
            // 当 block 没有引号/反斜杠时，trailing_zeros() 返回 32
            // 1u32 << 32 是 UB，必须用 u32::MAX 作为全量掩码
            let mask = if first_structural >= 32 {
                u32::MAX
            } else {
                (1u32 << first_structural) - 1
            };
            let control_before_structural = block.control_bits & mask;
            if control_before_structural != 0 {
                let idx = control_before_structural.trailing_zeros() as usize;
                let ch = self.input[self.pos + idx];
                return Err(format!("Control character in string: 0x{:02X}", ch));
            }
            if block.has_quote_first() {
                let idx = block.quote_index();
                let end = self.pos + idx;
                let s = String::from_utf8(self.input[start_pos..end].to_vec())
                    .map_err(|_| "Invalid UTF-8 in string".to_string())?;
                self.pos = end + 1; // 跳过引号
                *result_out = Some(s);
                return Ok(());
            }
            if block.has_backslash() {
                // 有转义 → 重置 pos 到 start_pos，交给慢路径从头处理
                self.pos = start_pos;
                return Ok(()); // result_out 仍为 None → 慢路径接管
            }
            // 无特殊字符，前进 32 字节
            self.pos += 32;
        }
        // SIMD 扫描完毕但未找到引号或转义，交给慢路径
        self.pos = start_pos;
        Ok(())
    }

    fn parse_hex_escape(&mut self) -> Result<u32, String> {
        let mut hex = 0u32;
        for _ in 0..4 {
            match self.next() {
                Some(ch) if ch.is_ascii_hexdigit() => {
                    let digit = if ch.is_ascii_digit() {
                        ch - b'0'
                    } else if ch.is_ascii_lowercase() {
                        ch - b'a' + 10
                    } else {
                        ch - b'A' + 10
                    };
                    hex = (hex << 4) | (digit as u32);
                }
                Some(ch) => return Err(format!("Invalid hex digit: {}", ch as char)),
                None => return Err("Unexpected end in unicode escape".to_string()),
            }
        }
        Ok(hex)
    }
    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;

        if self.peek() == Some(b'-') {
            self.next();
        }

        match self.peek() {
            Some(b'0') => {
                self.next();
            }
            Some(b'1'..=b'9') => {
                self.next();
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.next();
                }
            }
            _ => return Err("Invalid number".to_string()),
        }

        if self.peek() == Some(b'.') {
            self.next();
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("Invalid number".to_string());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.next();
            }
        }

        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.next();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.next();
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("Invalid number".to_string());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.next();
            }
        }

        let slice = &self.input[start..self.pos];
        let s = std::str::from_utf8(slice).map_err(|_| "Invalid UTF-8 in number".to_string())?;
        let value = s.parse::<f64>().map_err(|_| "Invalid number".to_string())?;
        Ok(JsonValue::Number(value))
    }
    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.expect(b'[')?;
        let mut elems = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some(b']') {
                self.next();
                return Ok(JsonValue::Array(elems));
            }
            if !elems.is_empty() {
                self.expect(b',')?;
                self.skip_whitespace();
                if self.peek() == Some(b']') {
                    // 严格拒绝尾随逗号（ES JSON 规范要求）
                    return Err("Trailing comma in array".to_string());
                }
            }
            elems.push(self.parse_value()?);
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.expect(b'{')?;
        let mut pairs = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some(b'}') {
                self.next();
                return Ok(JsonValue::Object(pairs));
            }
            if !pairs.is_empty() {
                self.expect(b',')?;
                self.skip_whitespace();
                if self.peek() == Some(b'}') {
                    // 严格拒绝尾随逗号（ES JSON 规范要求）
                    return Err("Trailing comma in object".to_string());
                }
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value = self.parse_value()?;
            pairs.push((key, value));
        }
    }
}
/// Delete an object property by name_id using swap-remove.
/// Based on the same pattern as `reflect_delete_property_impl`.
fn delete_property_by_name_id<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name_id: u32,
) {
    let obj_ptr =
        match resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize) {
            Some(p) => p,
            None => return,
        };
    let Some((slot_offset, flags, _val)) =
        find_property_slot_by_name_id_with_env(ctx, env, obj_ptr, name_id)
    else {
        return;
    };
    // 只删除 configurable 属性
    if (flags & constants::FLAG_CONFIGURABLE) == 0 {
        return;
    }
    let data = env.memory.data_mut(&mut *ctx);
    if obj_ptr + 16 > data.len() || slot_offset + 32 > data.len() {
        return;
    }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12],
        data[obj_ptr + 13],
        data[obj_ptr + 14],
        data[obj_ptr + 15],
    ]) as usize;
    if num_props == 0 {
        return;
    }
    let last_slot_offset = obj_ptr + 16 + (num_props - 1) * 32;
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props as u32 - 1).to_le_bytes());
    if slot_offset != last_slot_offset {
        data.copy_within(last_slot_offset..last_slot_offset + 32, slot_offset);
    }
}

fn make_exception(caller: &mut Caller<'_, RuntimeState>, name: &str, message: String) -> i64 {
    let message_val = store_runtime_string(caller, message.clone());
    let error_obj = create_error_object(caller, name, message_val);
    let mut errors = caller.data().error_table.lock().expect("error table mutex");
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: name.to_string(),
        message,
        value: error_obj,
    });
    value::encode_exception(idx)
}

fn json_parse_to_string(caller: &mut Caller<'_, RuntimeState>, value: i64) -> Result<String, i64> {
    if value::is_string(value) {
        return Ok(read_runtime_string(caller, value));
    }
    if value::is_symbol(value) {
        return Err(make_exception(
            caller,
            "TypeError",
            "Cannot convert a Symbol to a string".to_string(),
        ));
    }
    if value::is_bigint(value) {
        let handle = value::decode_bigint_handle(value) as usize;
        let table = caller
            .data()
            .bigint_table
            .lock()
            .expect("bigint_table mutex");
        return Ok(table
            .get(handle)
            .map(|bigint| bigint.to_string())
            .unwrap_or_default());
    }
    if value::is_f64(value)
        || value::is_bool(value)
        || value::is_null(value)
        || value::is_undefined(value)
    {
        return Ok(eval_to_string(caller, value));
    }
    if value::is_js_object(value) {
        // 同步 ToPrimitive 不支持回调；走 async 路径或调用方传入字符串。
        return Ok("[object Object]".to_string());
    }
    Ok(eval_to_string(caller, value))
}

async fn json_parse_to_string_async(
    caller: &mut Caller<'_, RuntimeState>,
    value: i64,
) -> Result<String, i64> {
    if value::is_string(value) {
        return Ok(read_runtime_string(caller, value));
    }
    if value::is_symbol(value) {
        return Err(make_exception(
            caller,
            "TypeError",
            "Cannot convert a Symbol to a string".to_string(),
        ));
    }
    if value::is_bigint(value) {
        let handle = value::decode_bigint_handle(value) as usize;
        let table = caller
            .data()
            .bigint_table
            .lock()
            .expect("bigint_table mutex");
        return Ok(table
            .get(handle)
            .map(|bigint| bigint.to_string())
            .unwrap_or_default());
    }
    if value::is_f64(value)
        || value::is_bool(value)
        || value::is_null(value)
        || value::is_undefined(value)
    {
        return Ok(eval_to_string(caller, value));
    }
    if value::is_js_object(value)
        && let Some(ptr) = resolve_handle(caller, value)
    {
        for method_name in ["toString", "valueOf"] {
            let method = read_object_property_by_name(caller, ptr, method_name)
                .unwrap_or_else(value::encode_undefined);
            if !is_callable_in_runtime(caller, method) {
                continue;
            }
            let Ok(result) = call_wasm_callback_async(caller, method, value, &[]).await else {
                continue;
            };
            if value::is_exception(result) {
                return Err(result);
            }
            if !value::is_js_object(result) {
                return Box::pin(json_parse_to_string_async(caller, result)).await;
            }
        }
        return Ok("[object Object]".to_string());
    }
    Ok(eval_to_string(caller, value))
}

fn build_wasm_value(caller: &mut Caller<'_, RuntimeState>, json_value: &JsonValue) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    build_wasm_value_with_env(caller, &env, json_value)
}

pub(crate) fn build_wasm_value_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    json_value: &JsonValue,
) -> i64 {
    match json_value {
        JsonValue::Null => value::encode_null(),
        JsonValue::Bool(b) => value::encode_bool(*b),
        JsonValue::Number(n) => value::encode_f64(*n),
        JsonValue::String(s) => {
            let text = s.clone();
            let state = ctx.as_context().data();
            store_runtime_string_in_state(state, text)
        }
        JsonValue::Array(elements) => {
            let arr = alloc_array_with_env(ctx, env, elements.len() as u32);
            if let Some(ptr) = resolve_array_ptr_with_env(ctx, env, arr) {
                for (i, elem) in elements.iter().enumerate() {
                    let elem_val = build_wasm_value_with_env(ctx, env, elem);
                    write_array_elem_with_env(ctx, env, ptr, i as u32, elem_val);
                }
                write_array_length_with_env(ctx, env, ptr, elements.len() as u32);
            }
            arr
        }
        JsonValue::Object(properties) => {
            let obj = alloc_host_object(ctx, env, properties.len() as u32);
            for (key, val) in properties {
                let val_encoded = build_wasm_value_with_env(ctx, env, val);
                let _ = define_host_data_property_with_env(ctx, env, obj, key, val_encoded);
            }
            obj
        }
    }
}

async fn apply_reviver_async(
    caller: &mut Caller<'_, RuntimeState>,
    reviver: i64,
    holder: i64,
    key: &str,
    val: i64,
) -> i64 {
    if value::is_array(val) {
        if let Some(ptr) = resolve_array_ptr(caller, val) {
            let len = match read_array_length(caller, ptr) {
                Some(n) => n,
                None => return value::encode_undefined(),
            };
            for i in 0..len {
                let elem_val = match read_array_elem(caller, ptr, i) {
                    Some(v) => v,
                    None => continue,
                };
                let new_val = Box::pin(apply_reviver_async(
                    caller,
                    reviver,
                    val,
                    &i.to_string(),
                    elem_val,
                ))
                .await;
                if value::is_undefined(new_val) {
                    write_array_elem(caller, ptr, i, value::encode_undefined());
                } else {
                    write_array_elem(caller, ptr, i, new_val);
                }
            }
        }
    } else if value::is_object(val) {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        if let Some(obj_ptr) = resolve_handle(caller, val) {
            let mut props: Vec<(u32, i64)> = Vec::new();
            {
                let data = env.memory.data(&*caller);
                if obj_ptr + 16 <= data.len() {
                    let num_props = u32::from_le_bytes([
                        data[obj_ptr + 12],
                        data[obj_ptr + 13],
                        data[obj_ptr + 14],
                        data[obj_ptr + 15],
                    ]) as usize;
                    for i in 0..num_props {
                        let slot_off = obj_ptr + 16 + i * 32;
                        if slot_off + 32 > data.len() {
                            continue;
                        }
                        let name_id = u32::from_le_bytes([
                            data[slot_off],
                            data[slot_off + 1],
                            data[slot_off + 2],
                            data[slot_off + 3],
                        ]);
                        let prop_val = i64::from_le_bytes([
                            data[slot_off + 8],
                            data[slot_off + 9],
                            data[slot_off + 10],
                            data[slot_off + 11],
                            data[slot_off + 12],
                            data[slot_off + 13],
                            data[slot_off + 14],
                            data[slot_off + 15],
                        ]);
                        props.push((name_id, prop_val));
                    }
                }
            }
            for (name_id, prop_val) in &props {
                let name_bytes = read_string_bytes_mem(caller, &env.memory, *name_id);
                let name = String::from_utf8_lossy(&name_bytes);
                let new_val =
                    Box::pin(apply_reviver_async(caller, reviver, val, &name, *prop_val)).await;
                if value::is_undefined(new_val) {
                    delete_property_by_name_id(caller, &env, val, *name_id);
                } else {
                    let obj_ptr2 = resolve_handle(caller, val).unwrap_or(0);
                    if obj_ptr2 != 0 {
                        write_object_property_by_name_id(
                            caller,
                            obj_ptr2,
                            val,
                            *name_id,
                            new_val,
                            constants::FLAG_CONFIGURABLE
                                | constants::FLAG_ENUMERABLE
                                | constants::FLAG_WRITABLE,
                        );
                    }
                }
            }
        }
    }
    let key_str = store_runtime_string(caller, key.to_string());
    call_wasm_callback_async(caller, reviver, holder, &[key_str, val])
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

pub async fn json_parse_to_wasm_async(
    caller: &mut Caller<'_, RuntimeState>,
    text: i64,
    reviver: i64,
) -> i64 {
    let text_str = match json_parse_to_string_async(caller, text).await {
        Ok(text) => text,
        Err(exception) => return exception,
    };

    let mut parser = JsonParser::new(text_str.as_bytes());
    match parser.parse_value() {
        Ok(json_value) => {
            parser.skip_whitespace();
            if parser.pos < parser.input.len() {
                return make_exception(
                    caller,
                    "SyntaxError",
                    "Unexpected trailing content".to_string(),
                );
            }

            let wasm_value = build_wasm_value(caller, &json_value);

            if is_callable_in_runtime(caller, reviver) {
                let env = WasmEnv::from_caller(caller).expect("WasmEnv");
                let root = alloc_host_object(caller, &env, 1);
                let empty_name_id = find_memory_c_string_with_env(caller, &env, "")
                    .or_else(|| alloc_heap_c_string_with_env(caller, &env, ""));
                if let Some(nid) = empty_name_id {
                    let root_ptr = resolve_handle(caller, root);
                    if let Some(ptr) = root_ptr {
                        write_object_property_by_name_id(
                            caller,
                            ptr,
                            root,
                            nid,
                            wasm_value,
                            constants::FLAG_CONFIGURABLE
                                | constants::FLAG_ENUMERABLE
                                | constants::FLAG_WRITABLE,
                        );
                    }
                }
                apply_reviver_async(caller, reviver, root, "", wasm_value).await
            } else {
                wasm_value
            }
        }
        Err(error) => make_exception(caller, "SyntaxError", error),
    }
}

pub fn json_parse_to_wasm(caller: &mut Caller<'_, RuntimeState>, text: i64, _reviver: i64) -> i64 {
    let text_str = match json_parse_to_string(caller, text) {
        Ok(text) => text,
        Err(exception) => return exception,
    };

    let mut parser = JsonParser::new(text_str.as_bytes());
    match parser.parse_value() {
        Ok(json_value) => {
            parser.skip_whitespace();
            if parser.pos < parser.input.len() {
                return make_exception(
                    caller,
                    "SyntaxError",
                    "Unexpected trailing content".to_string(),
                );
            }

            let wasm_value = build_wasm_value(caller, &json_value);

            wasm_value
        }
        Err(error) => make_exception(caller, "SyntaxError", error),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<JsonValue, String> {
        let mut p = JsonParser::new(s.as_bytes());
        let v = p.parse_value()?;
        p.skip_whitespace();
        if p.pos != p.input.len() {
            return Err("trailing content".into());
        }
        Ok(v)
    }

    #[test]
    fn test_parse_null_true_false() {
        assert!(matches!(parse("null").unwrap(), JsonValue::Null));
        assert!(matches!(parse("true").unwrap(), JsonValue::Bool(true)));
        assert!(matches!(parse("false").unwrap(), JsonValue::Bool(false)));
        assert!(parse(" null ").is_ok());
    }

    #[test]
    fn test_parse_numbers() {
        assert_eq!(parse("0").unwrap(), JsonValue::Number(0.0));
        assert_eq!(parse("-42").unwrap(), JsonValue::Number(-42.0));
        assert_eq!(parse("3.14").unwrap(), JsonValue::Number(3.14));
        assert_eq!(parse("1e3").unwrap(), JsonValue::Number(1000.0));
        assert_eq!(parse("1.5e-2").unwrap(), JsonValue::Number(0.015));
        assert!(parse("01").is_err()); // leading zero
        assert!(parse("1.").is_err()); // trailing dot
        assert!(parse("-01").is_err());
    }

    #[test]
    fn test_parse_strings_and_escapes() {
        assert_eq!(
            parse(r#""hello""#).unwrap(),
            JsonValue::String("hello".into())
        );
        assert_eq!(
            parse(r#""a\nb\tc""#).unwrap(),
            JsonValue::String("a\nb\tc".into())
        );
        assert_eq!(
            parse(r#""\\ \" \/""#).unwrap(),
            JsonValue::String(r#"\ " /"#.into())
        );
        // unicode + surrogate not fully exercised here but basic ok
        assert!(parse("\"\\u0041\"").is_ok());
    }

    #[test]
    fn test_parse_arrays() {
        let v = parse("[1,2,3]").unwrap();
        if let JsonValue::Array(a) = v {
            assert_eq!(a.len(), 3);
        } else {
            panic!();
        }
        assert!(parse("[]").is_ok());
        assert!(parse("[1,]").is_err()); // trailing comma rejected
        assert!(parse("[1,2").is_err()); // unterm
    }

    #[test]
    fn test_parse_objects() {
        let v = parse(r#"{"a":1,"b":true}"#).unwrap();
        if let JsonValue::Object(o) = v {
            assert_eq!(o.len(), 2);
        } else {
            panic!();
        }
        assert!(parse("{}").is_ok());
        assert!(parse(r#"{"a":1,}"#).is_err()); // trailing
    }

    #[test]
    fn test_parse_errors_and_trailing() {
        assert!(parse("{not json").is_err());
        assert!(parse("1 2").is_err()); // trailing content after value
        assert!(parse("").is_err());
    }

    #[test]
    fn test_skip_whitespace_and_cache() {
        // 多次 skip 应命中/更新缓存窗口
        let mut p = JsonParser::new(b"   \n\t  1");
        p.skip_whitespace();
        assert_eq!(p.pos, 7); // 跳过所有 ws
        // 再调用一次（已在值前）
        p.skip_whitespace();
        assert_eq!(p.pos, 7);
    }

    #[test]
    fn test_parse_deeply_nested_for_coverage() {
        // 增加一些分支覆盖（对象套数组等）
        let s = r#"{"a":[1,{"x":null},true],"b":false}"#;
        assert!(parse(s).is_ok());
    }
}
