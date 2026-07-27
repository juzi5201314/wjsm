//! SIMD 加速 JSON 解析器与 JSON.parse（ES §24.5.1）后端无关实现。
//!
//! SIMD 技术借鉴 sonic-rs：
//! - StringBlock：32 字节并行 quote/backslash/control 检测（AVX2）
//! - NonspaceBitmap：64 字节 whitespace 位图缓存加速 skip_whitespace
//!
//! 解析产出 `wjsm_host::JsonValue` 中间表示；物化为 JS 值经
//! `ExecContext::json_materialize`（后端各自实现堆分配）。

use wjsm_host::{ExecContext, JsonValue, RuntimeString, Value};
use wjsm_ir::{constants, value};

// ── SIMD helpers ──────────────────────────────

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// 64 字节 nonspace 位图（bit i 表示 base+i 处字节非空白）。
struct NonspaceBitmap {
    bits: u64,
    base: usize,
}

#[cfg(target_arch = "x86_64")]
fn compute_nonspace_bits_avx2(input: &[u8], base: usize) -> u64 {
    // SAFETY: 调用方保证 base+64 <= input.len() 且 AVX2 已检测。
    unsafe {
        let ptr = input.as_ptr().add(base);
        let mut bits = 0u64;
        for half in 0..2 {
            let chunk = _mm256_loadu_si256(ptr.add(half * 32) as *const __m256i);
            let space = _mm256_cmpeq_epi8(chunk, _mm256_set1_epi8(b' ' as i8));
            let tab = _mm256_cmpeq_epi8(chunk, _mm256_set1_epi8(b'\t' as i8));
            let nl = _mm256_cmpeq_epi8(chunk, _mm256_set1_epi8(b'\n' as i8));
            let cr = _mm256_cmpeq_epi8(chunk, _mm256_set1_epi8(b'\r' as i8));
            let ws = _mm256_or_si256(_mm256_or_si256(space, tab), _mm256_or_si256(nl, cr));
            let ws_mask = _mm256_movemask_epi8(ws) as u32;
            bits |= ((!ws_mask) as u64) << (half * 32);
        }
        bits
    }
}

#[inline(always)]
#[allow(clippy::needless_range_loop)]
fn compute_nonspace_bits_scalar(input: &[u8], base: usize) -> u64 {
    let mut bits = 0u64;
    for i in 0..64 {
        let b = input[base + i];
        if !matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
            bits |= 1 << i;
        }
    }
    bits
}

/// AVX2 可用时用 SIMD，否则 scalar。
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

/// StringBlock：32 字节并行 quote/backslash/control 检测。
/// AVX2-only；调用方必须先 `is_x86_feature_detected!("avx2")`。
#[cfg(target_arch = "x86_64")]
struct StringBlock {
    quote_bits: u32,
    backslash_bits: u32,
    control_bits: u32,
}

#[cfg(target_arch = "x86_64")]
impl StringBlock {
    /// SAFETY: 调用方保证 `ptr` 起至少 32 字节可读且 AVX2 已检测。
    #[target_feature(enable = "avx2")]
    unsafe fn new_avx2(ptr: *const u8) -> Self {
        // SAFETY: 函数契约保证 32 字节可读。
        unsafe {
            let chunk = _mm256_loadu_si256(ptr as *const __m256i);
            let quote = _mm256_cmpeq_epi8(chunk, _mm256_set1_epi8(b'"' as i8));
            let backslash = _mm256_cmpeq_epi8(chunk, _mm256_set1_epi8(b'\\' as i8));
            // control: byte < 0x20（无符号）。i8 域内 unsigned < 0x20 等价于
            // max(chunk, 0x1F) == 0x1F ⇔ chunk <= 0x1F（unsigned）。
            let limit = _mm256_set1_epi8(0x1F);
            let maxed = _mm256_max_epu8(chunk, limit);
            let control = _mm256_cmpeq_epi8(maxed, limit);
            Self {
                quote_bits: _mm256_movemask_epi8(quote) as u32,
                backslash_bits: _mm256_movemask_epi8(backslash) as u32,
                control_bits: _mm256_movemask_epi8(control) as u32,
            }
        }
    }

    fn has_quote_first(&self) -> bool {
        let bs = self.backslash_bits;
        let q = self.quote_bits;
        q != 0 && (bs == 0 || q.trailing_zeros() < bs.trailing_zeros())
    }

    fn quote_index(&self) -> usize {
        self.quote_bits.trailing_zeros() as usize
    }

    fn has_backslash(&self) -> bool {
        self.backslash_bits != 0
    }
}

// ── Parser ──

/// JSON 文本解析器（字节驱动，UTF-16 完整保留）。
pub struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
    /// 缓存当前 64 字节对齐窗口的 nonspace 位图；skip_whitespace 中按需更新，避免重复 compute
    nonspace: NonspaceBitmap,
}

impl<'a> JsonParser<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            nonspace: NonspaceBitmap {
                bits: 0,
                base: usize::MAX,
            },
        }
    }

    /// 当前游标（供调用方检查 trailing content）。
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// 输入总长（供调用方检查 trailing content）。
    pub fn input_len(&self) -> usize {
        self.input.len()
    }

    // ── SIMD-accelerated whitespace skipping ──
    // 设计：先做 ≤8 次标量检查（排空到 64 字节对齐边界），然后切入 SIMD 批量跳过

    pub fn skip_whitespace(&mut self) {
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

    pub fn parse_value(&mut self) -> Result<JsonValue, String> {
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

    fn parse_string(&mut self) -> Result<RuntimeString, String> {
        if self.next() != Some(b'"') {
            return Err("Expected '\"'".to_string());
        }

        let start_pos = self.pos; // 位置在 '"' 之后

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                let mut simd_result = None;
                // SAFETY: AVX2 feature detected at runtime.
                unsafe { self.parse_string_simd(&mut simd_result) }?;
                if let Some(s) = simd_result {
                    return Ok(s);
                }
            }
        }

        let _ = start_pos;

        let mut units = Vec::new();
        loop {
            match self.next() {
                None => return Err("Unterminated string".to_string()),
                Some(b'"') => return Ok(RuntimeString::from_utf16_units(units)),
                Some(b'\\') => match self.next() {
                    None => return Err("Unterminated escape sequence".to_string()),
                    Some(b'"') => units.push(b'"' as u16),
                    Some(b'\\') => units.push(b'\\' as u16),
                    Some(b'/') => units.push(b'/' as u16),
                    Some(b'b') => units.push(0x0008),
                    Some(b'f') => units.push(0x000C),
                    Some(b'n') => units.push(b'\n' as u16),
                    Some(b'r') => units.push(b'\r' as u16),
                    Some(b't') => units.push(b'\t' as u16),
                    Some(b'u') => units.push(self.parse_hex_escape()? as u16),
                    Some(ch) => return Err(format!("Invalid escape sequence: \\{}", ch as char)),
                },
                Some(ch) if ch < 0x20 => {
                    return Err(format!("Control character in string: 0x{:02X}", ch));
                }
                Some(ch) => {
                    if ch < 0x80 {
                        units.push(ch as u16);
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
                        let scalar = std::str::from_utf8(&self.input[start..self.pos])
                            .map_err(|_| "Invalid UTF-8 sequence".to_string())?;
                        units.extend(scalar.encode_utf16());
                    }
                }
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn parse_string_simd(
        &mut self,
        result_out: &mut Option<RuntimeString>,
    ) -> Result<(), String> {
        let start_pos = self.pos;
        while self.pos + 32 <= self.input.len() {
            // SAFETY: We are inside `unsafe fn parse_string_simd` (guarded by `is_x86_feature_detected!("avx2")` in the caller).
            // `self.pos + 32 <= self.input.len()` guarantees at least 32 readable bytes from `self.input[self.pos..]`.
            // This explicit unsafe block is required under Rust 2024 `unsafe_op_in_unsafe_fn`.
            let block = unsafe { StringBlock::new_avx2(self.input[self.pos..].as_ptr()) };
            let first_structural =
                (block.quote_bits | block.backslash_bits).trailing_zeros() as usize;
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
                let s = std::str::from_utf8(&self.input[start_pos..end])
                    .map_err(|_| "Invalid UTF-8 in string".to_string())?;
                self.pos = end + 1;
                *result_out = Some(RuntimeString::from_utf8_str(s));
                return Ok(());
            }
            if block.has_backslash() {
                self.pos = start_pos;
                return Ok(());
            }
            self.pos += 32;
        }
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

/// 完整解析一段 JSON 文本（含 trailing content 检查）。
pub fn parse_json_text(input: &str) -> Result<JsonValue, String> {
    let mut parser = JsonParser::new(input.as_bytes());
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos < parser.input.len() {
        return Err("Unexpected trailing content".to_string());
    }
    Ok(value)
}

// ── JSON.parse 输入 ToString ──

/// JSON.parse 第一参数的同步 ToString（不再入回调；对象直接 "[object Object]"）。
pub fn json_parse_to_string_impl<E: ExecContext>(
    ctx: &mut E,
    value: Value,
) -> Result<String, Value> {
    if value::is_string(value) {
        return Ok(ctx.get_runtime_string(value).to_utf8_lossy());
    }
    if value::is_symbol(value) {
        return Err(make_json_exception(
            ctx,
            "TypeError",
            "Cannot convert a Symbol to a string",
        ));
    }
    if value::is_bigint(value) {
        return Ok(ctx
            .read_bigint(value)
            .map(|bigint| bigint.to_string())
            .unwrap_or_default());
    }
    if value::is_f64(value)
        || value::is_bool(value)
        || value::is_null(value)
        || value::is_undefined(value)
    {
        return Ok(ctx.value_to_display_string(value));
    }
    if value::is_js_object(value) {
        // 同步 ToPrimitive 不支持回调；走 async 路径或调用方传入字符串。
        return Ok("[object Object]".to_string());
    }
    Ok(ctx.value_to_display_string(value))
}

/// JSON.parse 第一参数的异步 ToString（对象再入 toString/valueOf）。
async fn json_parse_to_string_async<E: ExecContext>(
    ctx: &mut E,
    value: Value,
) -> Result<String, Value> {
    if value::is_string(value) {
        return Ok(ctx.get_runtime_string(value).to_utf8_lossy());
    }
    if value::is_symbol(value) {
        return Err(make_json_exception(
            ctx,
            "TypeError",
            "Cannot convert a Symbol to a string",
        ));
    }
    if value::is_bigint(value) {
        return Ok(ctx
            .read_bigint(value)
            .map(|bigint| bigint.to_string())
            .unwrap_or_default());
    }
    if value::is_f64(value)
        || value::is_bool(value)
        || value::is_null(value)
        || value::is_undefined(value)
    {
        return Ok(ctx.value_to_display_string(value));
    }
    if value::is_js_object(value) {
        for method_name in ["toString", "valueOf"] {
            let Some(method) = ctx.read_property_for_render(value, method_name) else {
                continue;
            };
            if !ctx.is_callable(method) {
                continue;
            }
            let Ok(result) = ctx.call_js_async(method, value, &[]).await else {
                continue;
            };
            if value::is_exception(result) {
                return Err(result);
            }
            if !value::is_js_object(result) {
                return Box::pin(json_parse_to_string_async(ctx, result)).await;
            }
        }
        return Ok("[object Object]".to_string());
    }
    Ok(ctx.value_to_display_string(value))
}

/// 命名异常构造（error_table 压入 + TAG_EXCEPTION 编码）。
fn make_json_exception<E: ExecContext>(ctx: &mut E, name: &str, message: &str) -> Value {
    let message_val = ctx.store_string(message);
    let error_obj = ctx.create_error_object(name, message_val, value::encode_undefined());
    ctx.push_exception(name, message, error_obj)
}

// ── reviver ──

/// ES §24.5.1 InternalizeJSONProperty：对已物化对象递归应用 reviver。
async fn apply_reviver<E: ExecContext>(
    ctx: &mut E,
    reviver: Value,
    holder: Value,
    key: &str,
    val: Value,
) -> Value {
    if value::is_object(val) {
        return apply_reviver_object(ctx, reviver, val, key).await;
    }
    if value::is_array(val)
        && let Some(len) = ctx.array_read_length(val)
    {
        for i in 0..len {
            let elem_val = ctx
                .array_elem_at(val, i)
                .unwrap_or_else(value::encode_undefined);
            let new_val =
                Box::pin(apply_reviver(ctx, reviver, val, &i.to_string(), elem_val)).await;
            if value::is_exception(new_val) {
                return new_val;
            }
            if value::is_undefined(new_val) {
                ctx.array_write_hole(val, i);
            } else {
                ctx.array_write_elem(val, i, new_val);
            }
        }
    }
    let key_str = ctx.store_string(key);
    let args = [key_str, val];
    match ctx.call_js_async(reviver, holder, &args).await {
        Ok(result) => result,
        Err(_) => value::encode_undefined(),
    }
}

/// 对象分支：遍历 own 槽位递归 internalize，undefined 删除、否则重定义。
async fn apply_reviver_object<E: ExecContext>(
    ctx: &mut E,
    reviver: Value,
    object: Value,
    key: &str,
) -> Value {
    let handle = value::decode_handle(object);
    let slots = ctx.own_property_entries(handle);
    for (property_key, _) in slots {
        let Some((prop_value, flags, _getter, _setter)) =
            ctx.get_own_property_slot(handle, property_key)
        else {
            continue;
        };
        let Some(name) = ctx.property_key_string(property_key) else {
            // Symbol 键：跳过
            continue;
        };
        let new_value = Box::pin(apply_reviver(ctx, reviver, object, &name, prop_value)).await;
        if value::is_exception(new_value) {
            return new_value;
        }
        if value::is_undefined(new_value) {
            let _ = ctx.delete_property_by_name_id(handle, property_key);
        } else {
            ctx.define_data_property_with_flags(handle, property_key, new_value, flags);
        }
    }
    let key_str = ctx.store_string(key);
    let args = [key_str, object];
    match ctx.call_js_async(reviver, object, &args).await {
        Ok(result) => result,
        Err(_) => value::encode_undefined(),
    }
}

// ── 入口 ──

/// ES §24.5.1 JSON.parse(text, reviver) — async 完整路径。
pub async fn json_parse_impl<E: ExecContext>(ctx: &mut E, text: Value, reviver: Value) -> Value {
    let text_str = match json_parse_to_string_async(ctx, text).await {
        Ok(text) => text,
        Err(exception) => return exception,
    };

    let mut parser = JsonParser::new(text_str.as_bytes());
    match parser.parse_value() {
        Ok(json_value) => {
            parser.skip_whitespace();
            if parser.pos < parser.input.len() {
                return make_json_exception(ctx, "SyntaxError", "Unexpected trailing content");
            }

            let wasm_value = ctx.json_materialize(&json_value);

            if ctx.is_callable(reviver) {
                let root = ctx.alloc_object(1);
                let empty_key = ctx.intern_property_key("");
                let root_handle = value::decode_handle(root);
                ctx.define_data_property_with_flags(
                    root_handle,
                    empty_key,
                    wasm_value,
                    (constants::FLAG_CONFIGURABLE
                        | constants::FLAG_ENUMERABLE
                        | constants::FLAG_WRITABLE) as u32,
                );
                apply_reviver(ctx, reviver, root, "", wasm_value).await
            } else {
                wasm_value
            }
        }
        Err(error) => make_json_exception(ctx, "SyntaxError", &error),
    }
}

/// JSON.parse 同步路径（无 reviver 再入；eval 解释器等冷路径用）。
pub fn json_parse_sync_impl<E: ExecContext>(ctx: &mut E, text: Value, _reviver: Value) -> Value {
    let text_str = match json_parse_to_string_impl(ctx, text) {
        Ok(text) => text,
        Err(exception) => return exception,
    };

    let mut parser = JsonParser::new(text_str.as_bytes());
    match parser.parse_value() {
        Ok(json_value) => {
            parser.skip_whitespace();
            if parser.pos < parser.input.len() {
                return make_json_exception(ctx, "SyntaxError", "Unexpected trailing content");
            }

            ctx.json_materialize(&json_value)
        }
        Err(error) => make_json_exception(ctx, "SyntaxError", &error),
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
        assert_eq!(parse("3.25").unwrap(), JsonValue::Number(3.25));
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
