//! SIMD-accelerated JSON parser and value builder for JSON.parse implementation
//!
//! Implements ECMAScript §24.5.1 JSON.parse(text, reviver).
//! SIMD acceleration inspired by sonic-rs techniques:
//! - StringBlock: parallel quote/backslash/control detection (32 bytes at once, AVX2)
//! - NonspaceBitmap: cached 64-byte whitespace bitmap for skip_whitespace

use wasmtime::Caller;
use crate::*;

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
        // SAFETY: We just verified AVX2 is available.
        unsafe { compute_nonspace_bits_avx2(input, base) }
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
            let ctrl_bits = _mm256_movemask_epi8(
                _mm256_and_si256(_mm256_cmpgt_epi8(ctrl_thresh, v), _mm256_cmpgt_epi8(v, non_neg))
            ) as u32;
            Self { quote_bits: q_bits, backslash_bits: bs_bits, control_bits: ctrl_bits }
        }
    }

    fn has_quote_first(&self) -> bool {
        if self.quote_bits == 0 { return false; }
        if self.backslash_bits == 0 { return true; }
        self.quote_bits.trailing_zeros() < self.backslash_bits.trailing_zeros()
    }

    fn quote_index(&self) -> usize {
        self.quote_bits.trailing_zeros() as usize
    }

    fn has_backslash(&self) -> bool { self.backslash_bits != 0 }
    fn has_control(&self) -> bool { self.control_bits != 0 }
}

// ── Internal parsed value representation ──

#[derive(Debug, Clone)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
    nonspace: NonspaceBitmap,   // TODO: wire caching in later task (currently compute_nonspace_bits called directly in skip_whitespace; field kept for exact Task 5 skeleton compliance)
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            nonspace: NonspaceBitmap { bits: 0, base: usize::MAX },
        }
    }

    // ── SIMD-accelerated whitespace skipping ──
    // 设计：先做 ≤8 次标量检查（排空到 64 字节对齐边界），然后切入 SIMD 批量跳过

    fn skip_whitespace(&mut self) {
        // 快路径：逐字节检查，最多 8 次（对齐到 64 字节边界）
        let limit = (self.pos + 8).min(self.input.len());
        while self.pos < limit {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => return,
            }
        }
        // 批量路径：使用位图加速
        while self.pos + 64 <= self.input.len() {
            let base = self.pos & !63;
            let bits = compute_nonspace_bits(self.input, base);
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

    fn peek(&self) -> Option<u8> { self.input.get(self.pos).copied() }

    fn next(&mut self) -> Option<u8> {
        let ch = self.input.get(self.pos).copied();
        if ch.is_some() { self.pos += 1; }
        ch
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        match self.next() {
            Some(ch) if ch == expected => Ok(()),
            Some(ch) => Err(format!("Expected '{}', found '{}'", expected as char, ch as char)),
            None => Err(format!("Expected '{}', found end of input", expected as char)),
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
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
}
