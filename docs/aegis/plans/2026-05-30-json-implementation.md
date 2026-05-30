# JSON Implementation Plan (Revised v3)

## Goal

Implement complete ES-compliant `JSON.parse(text, reviver)` and `JSON.stringify(value, replacer, space)` by replacing the stub implementation, fixing all spec deviations, and eliminating every TODO placeholder. Includes SIMD-accelerated parser for high-throughput JSON text processing.

## Architecture

Linear compilation pipeline: `source → parser → semantic → IR → backend → WASM → runtime`. JSON functions are builtins that compile to host imports.

### Current bottleneck (root cause)

Both `JSON.parse` and `JSON.stringify` are registered as single-parameter `(i64) → i64` WASM host imports, so optional arguments (`reviver`, `replacer`, `space`) are silently discarded. This breaks every feature that depends on those arguments.

### Resolution path

1. Expand backend type/signatures to carry all parameters
2. Implement a SIMD-accelerated recursive-descent JSON text parser with full ECMAScript JSON grammar
3. Implement `InternalizeJSONProperty` reviver walk per ES §24.5.1
4. Implement `SerializeJSONProperty` per ES §24.5.2 with `toJSON`, replacer (array + function), and `space`
5. Update 22+ fixtures

## Tech Stack

- Rust 2024 edition
- wasmtime for WASM execution
- NaN-boxed value encoding (`i64`)
- WASM host imports for runtime operations
- Hand-written recursive descent parser with SIMD hot-path acceleration (inspired by sonic-rs techniques, zero external dependency)

**为什么不用 sonic-rs：**
sonic-rs 解析到 `sonic_rs::Value` DOM 后仍需完整遍历转换为 WASM 堆对象，双重内存 + 重依赖（serde + faststr + simdutf8 + ...），对典型的 `JSON.parse` 场景收益极微（瓶颈在堆分配和 reviver 遍历，不在 tokenize）。手写 parser 可以控制每一步内存行为，同时在热路径引入 SIMD 加速。

**SIMD 策略（借鉴 sonic-rs）：**
1. **StringBlock 模式**：AVX2 SIMD 并行扫描 32 字节的 quote/backslash/control char，长字符串解析跳过逐字节检查。使用 `is_x86_feature_detected!("avx2")` 运行时检测，scalar fallback 保证安全。
2. **whitespace 位图缓存**：64 字节窗口的 `nonspace_bits` 位图，有限次标量预热后切入 SIMD 批量跳过。
3. **补充平面字符直接输出**：ES 规范操作 UTF-16 码元，但 JSON 输出是 UTF-8，补充平面字符直接写入而非 `\uXXXX` surrogate pair

## Baseline / Authority Refs

- AGENTS.md: wjsm project conventions
- ECMAScript 2024 §24.5.1 JSON.parse(text, reviver)
- ECMAScript 2024 §24.5.2 JSON.stringify(value, replacer, space)
- ECMAScript 2024 §7.1.17 Number::toString
- Current state: `JSON.parse` is stub (returns raw string), `JSON.stringify` has 7 spec deviations

## Compatibility Boundary

- WASM contract: host import type signatures must match `types.ty().function(...)` indices in `compiler_core.rs`
- NaN-boxed encoding: all JS values are `i64` with tag bits
- Existing 22+ JSON fixtures must pass after update
- No breaking changes to other builtins
- No new external crate dependencies

## Plan Pressure Test

| Check | Result |
|-------|--------|
| Owner / contract / retirement | `JSON.parse` stub → full parser; `JSON.stringify` partial → complete |
| Verification scope | All JSON fixtures, ES spec compliance, no regressions |
| Task executability | Infrastructure exists (`alloc_host_object`, `call_wasm_callback`, `write_object_property_by_name_id`, `reflect_delete_property_impl`) |
| **Pressure result** | **proceed** |

## Plan-Time Complexity Check

| Signal | Observation | Decision |
|--------|-------------|----------|
| Target files | 8 files across 4 crates | Multi-phase |
| Large files | `compiler_builtins.rs` (2093L), `host_import_registry.rs` (2425L), `runtime_render.rs` (842L) | Surgical edits only |
| New owner | `runtime_json.rs` (new module, ~650L with SIMD) | **Create new file** — parser + reviver walk + heap construction + SIMD helpers + delete helper |
| Add-in-place risk | Low for metadata/signature changes; Medium for stringify logic in `runtime_render.rs` | Extract stringify helpers to new functions in `runtime_render.rs` |
| Better file boundary | `runtime_json.rs` owns parser + build_wasm_value + reviver walk + delete_property_by_name_id; `runtime_render.rs` owns stringify + escape | **Add owner file** for parse path, edit-in-place for stringify |
| **Recommendation** | Metadata/signature: **edit-in-place**; Parser + stringify helpers: **add owner file / new functions** |

---

## ES Spec Compliance Checklist

| Spec Requirement | Covered | Deviation | Task |
|------------------|---------|-----------|------|
| JSON.parse trailing whitespace check | ✅ | — | 9 |
| JSON.parse duplicate keys: last wins | ✅ | — (via `write_object_property_by_name_id`) | 9 |
| JSON.parse reviver this=holder | ✅ | — | 9 |
| JSON.parse reviver returns root value directly | ✅ | — (use `apply_reviver` return, not re-read heap) | 9 |
| JSON.parse SyntaxError via `set_runtime_error` | ✅ | ⚠️ Error is string, not Error object (runtime limitation) | 9 |
| JSON.parse reviver array: undefined → write undefined | ✅ | ⚠️ Dense arrays can't create holes; writes undefined instead of deleting (documented deviation) | 9 |
| JSON.parse non-string input ToString | ✅ | ⚠️ Uses `eval_to_string` approximation (documented deviation) | 9 |
| JSON.stringify supplementary plane chars: direct UTF-8 | ✅ | — | 11 |
| JSON.stringify replacer array: skip non-String/Number | ✅ | — (ES §24.5.2 step 4.b: only String and Number added) | 11 |
| JSON.stringify replacer array preserves insertion order | ✅ | — (uses `Vec<String>`, not `HashSet`) | 11 |
| JSON.stringify `toJSON` this=value | ✅ | — | 11 |
| JSON.stringify -0 → "0" (not "-0") | ✅ | — | 11 |
| JSON.stringify NaN/±Infinity → null | ✅ | — | 11 |
| JSON.stringify BigInt → TypeError | ✅ | — | 11 |
| JSON.stringify space: UTF-16 code unit limit | ✅ | ⚠️ Counts Unicode scalars not UTF-16 code units (extremely rare edge case) | 11 |

---

## Files

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/wjsm-semantic/src/builtins.rs` | Modify | Update parameter count metadata |
| `crates/wjsm-backend-wasm/src/lib.rs` | Modify | Update import signature count table |
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | **No change needed** | Type 16 and Type 2 already exist |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | Modify | Expand JSON builtins to emit all parameters |
| `crates/wjsm-backend-wasm/src/host_import_registry.rs` | Modify | Update type indices |
| `crates/wjsm-runtime/src/lib.rs` | Modify | Add `mod runtime_json;` + `use runtime_json::*;` |
| `crates/wjsm-runtime/src/runtime_json.rs` | **Create** | SIMD-accelerated JSON parser + reviver walk + heap construction + delete helper |
| `crates/wjsm-runtime/src/host_imports/timers_arrays.rs` | Modify | Wire `json_parse`; update `json_stringify` to 3 args |
| `crates/wjsm-runtime/src/runtime_render.rs` | Modify | Full JSON.stringify with toJSON, replacer (Vec), space |
| `fixtures/happy/json_*.expected` | Update | 22+ fixtures |

---

## Task 1: Update Semantic Parameter Counts

**Files:** `crates/wjsm-semantic/src/builtins.rs`

**Why:** Semantic layer declares parameter counts for builtins.

**Verification:** `cargo check -p wjsm-semantic`

**Steps:**
- [ ] Change `Builtin::JsonStringify => ("JSON.stringify", 3)` and `Builtin::JsonParse => ("JSON.parse", 2)`
- [ ] Verify: `cargo check -p wjsm-semantic`
- [ ] Commit: `git commit -m "feat: update JSON builtin parameter counts for ES compliance"`

---

## Task 2: Update Backend Import Signature Count Table

**Files:** `crates/wjsm-backend-wasm/src/lib.rs`

**Why:** Backend count table must match semantic layer.

**Verification:** `cargo check -p wjsm-backend-wasm`

**Steps:**
- [ ] Change `Builtin::JsonStringify => ("JSON.stringify", 3)` and `Builtin::JsonParse => ("JSON.parse", 2)`
- [ ] Verify: `cargo check -p wjsm-backend-wasm`
- [ ] Commit: `git commit -m "feat: update JSON builtin signature count table"`

---

## Task 3: Update Backend Emission Logic

**Files:** `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

**Why:** Backend emission currently only emits the first argument. Need to emit all parameters.

**Verification:** `cargo check -p wjsm-backend-wasm`

**Steps:**
- [ ] Split the combined `Builtin::Fetch | Builtin::JsonStringify | Builtin::JsonParse` match arm into three separate arms:
  - `Builtin::Fetch` — unchanged, single arg
  - `Builtin::JsonStringify` — emit 3 args (value, replacer?, space?) with undefined defaults
  - `Builtin::JsonParse` — emit 2 args (text, reviver?) with undefined default
- [ ] Verify: `cargo check -p wjsm-backend-wasm`
- [ ] Commit: `git commit -m "feat: emit multi-parameter calls for JSON builtins"`

---

## Task 4: Update Host Import Registry Type Indices

**Files:** `crates/wjsm-backend-wasm/src/host_import_registry.rs`

**Why:** Host import registry maps builtin names to WASM type indices.

**Known indices:** Type 16: `(i64, i64, i64) → (i64)`; Type 2: `(i64, i64) → (i64)`

**Verification:** `cargo check -p wjsm-backend-wasm`

**Steps:**
- [ ] Change `json_stringify` type_idx to 16; `json_parse` type_idx to 2
- [ ] Verify: `cargo check -p wjsm-backend-wasm`
- [ ] Commit: `git commit -m "feat: update JSON host import type signatures"`

---

## Task 5: Create SIMD-Accelerated JSON Parser Module

**Files:** `crates/wjsm-runtime/src/runtime_json.rs` (create), `crates/wjsm-runtime/src/lib.rs` (modify)

**Why:** Core of the implementation — recursive descent parser with SIMD hot paths.

**SIMD Design (reviewed and verified):**

1. **AVX2 runtime detection**: Use `is_x86_feature_detected!("avx2")` inside `#[cfg(target_arch = "x86_64")]` functions. If AVX2 unavailable at runtime, fall back to scalar. No illegal instruction risk.

2. **StringBlock**: Parallel quote/backslash/control char detection. Control char threshold is `0x20` (not `0x1f` — `0x1f` itself must be detected). AND mask excludes high bytes (0x80..0xFF UTF-8 continuation bytes) which would otherwise be falsely detected as control chars since `0x20 > negative_signed_byte` is true.

3. **Whitespace SIMD**: Initial scalar loop is **bounded** to ≤8 iterations to drain leading whitespace up to the next 64-byte alignment boundary, then SIMD batch takes over. Unbounded scalar loop would consume all whitespace before SIMD ever runs.

4. **String fast path**: Tracks `start_pos` (position after opening quote). When quote found, copies `self.input[start_pos..end]` (not `self.input[self.pos..end]` which truncates prior chunks).

**Verification:** `cargo check -p wjsm-runtime`

**Steps:**

- [ ] Create `crates/wjsm-runtime/src/runtime_json.rs`:

```rust
//! SIMD-accelerated JSON parser and value builder for JSON.parse implementation
//!
//! Implements ECMAScript §24.5.1 JSON.parse(text, reviver).
//! SIMD acceleration inspired by sonic-rs techniques:
//! - StringBlock: parallel quote/backslash/control detection (32 bytes at once, AVX2)
//! - NonspaceBitmap: cached 64-byte whitespace bitmap for skip_whitespace

use wasmtime::Caller;
use crate::*;

// ── SIMD helpers ──────────────────────────────────────────────────────

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
    nonspace: NonspaceBitmap,
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
```

- [ ] Modify `crates/wjsm-runtime/src/lib.rs`: Add `mod runtime_json;` and `use runtime_json::*;`
- [ ] Verify: `cargo check -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: add SIMD-accelerated JSON parser module skeleton"`

---

## Task 6: Implement SIMD-Accelerated String Parsing

**Files:** `crates/wjsm-runtime/src/runtime_json.rs`

**Why:** String parsing is the hottest path. SIMD StringBlock scanning processes 32 bytes at once.

**Key SIMD fix**: Fast path tracks `start_pos` from the opening quote. When quote found at offset `idx`, copies `input[start_pos..self.pos + idx]` — the complete string content from opening to closing quote, not just the last chunk.

**Verification:** `cargo check -p wjsm-runtime`

**Steps:**

- [ ] Add `parse_string` and `parse_hex_escape` methods:

```rust
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

        // ── 慢路径：逐字节处理（含转义序列） ──
        let mut result = String::new();
        loop {
            match self.next() {
                None => return Err("Unterminated string".to_string()),
                Some(b'"') => return Ok(result),
                Some(b'\\') => {
                    match self.next() {
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
                    }
                }
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
            let block = StringBlock::new_avx2(self.input[self.pos..].as_ptr());
            // 只检查在第一个 quote/backslash 之前的控制字符
            let first_structural = (block.quote_bits | block.backslash_bits).trailing_zeros() as usize;
            // 当 block 没有引号/反斜杠时，trailing_zeros() 返回 32
            // 1u32 << 32 是 UB，必须用 u32::MAX 作为全量掩码
            let mask = if first_structural >= 32 { u32::MAX } else { (1u32 << first_structural) - 1 };
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
                    let digit = if ch.is_ascii_digit() { ch - b'0' }
                    else if ch.is_ascii_lowercase() { ch - b'a' + 10 }
                    else { ch - b'A' + 10 };
                    hex = (hex << 4) | (digit as u32);
                }
                Some(ch) => return Err(format!("Invalid hex digit: {}", ch as char)),
                None => return Err("Unexpected end in unicode escape".to_string()),
            }
        }
        Ok(hex)
    }
```

- [ ] Verify: `cargo check -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: implement SIMD-accelerated JSON string parsing"`

---

## Task 7: Implement JSON Number Parsing

**Files:** `crates/wjsm-runtime/src/runtime_json.rs`

**Steps:**
- [ ] Add `parse_number` method (same as v2 plan — extracts slice, delegates to `f64::from_str`)
- [ ] Verify: `cargo check -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: implement JSON number parsing"`

---

## Task 8: Implement JSON Array and Object Parsing

**Files:** `crates/wjsm-runtime/src/runtime_json.rs`

**Steps:**
- [ ] Add `parse_array` and `parse_object` methods (same as v2 — reject trailing commas per spec)
- [ ] Verify: `cargo check -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: implement JSON array and object parsing"`

---

## Task 9: Implement WASM Heap Construction + Reviver Walk + Helpers

**Files:** `crates/wjsm-runtime/src/runtime_json.rs`

**Why:** Parsed JSON values must be converted to WASM heap objects/arrays. Reviver walk per ES §24.5.1. Need `delete_property_by_name_id` helper (doesn't exist yet).

**ES Spec Compliance (verified by reviewer):**
1. ✅ Trailing whitespace check after `parse_value`
2. ✅ Duplicate keys: `write_object_property_by_name_id` (last-wins, updates existing slot)
3. ✅ Reviver `this` = holder
4. ✅ Reviver return value used directly (not re-read from heap)
5. ✅ `eval_to_string` for non-string JSON.parse input (better than `render_value`)
6. ⚠️ SyntaxError via `set_runtime_error(string)` — not a proper Error object (runtime limitation)
7. ⚠️ Array reviver undefined → writes undefined (dense arrays don't support holes)
8. ✅ `delete_property_by_name_id` helper created (based on `reflect_delete_property_impl` swap-remove pattern)
9. ✅ Uses `read_string_bytes_mem(caller, &env.memory, name_id)` (existing function)

**Verification:** `cargo check -p wjsm-runtime`

**Steps:**

- [ ] Add `delete_property_by_name_id` helper:

```rust
/// Delete an object property by name_id using swap-remove.
/// Based on the same pattern as `reflect_delete_property_impl`.
fn delete_property_by_name_id<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    name_id: u32,
) {
    let obj_ptr = match resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize) {
        Some(p) => p,
        None => return,
    };
    let Some((slot_offset, flags, _val)) = find_property_slot_by_name_id_with_env(ctx, env, obj_ptr, name_id)
    else { return };
    // 只删除 configurable 属性
    if (flags & constants::FLAG_CONFIGURABLE) == 0 { return; }
    let data = env.memory.data_mut(&mut *ctx);
    if obj_ptr + 16 > data.len() || slot_offset + 32 > data.len() { return; }
    let num_props = u32::from_le_bytes([
        data[obj_ptr + 12], data[obj_ptr + 13], data[obj_ptr + 14], data[obj_ptr + 15],
    ]) as usize;
    if num_props == 0 { return; }
    let last_slot_offset = obj_ptr + 16 + (num_props - 1) * 32;
    data[obj_ptr + 12..obj_ptr + 16].copy_from_slice(&(num_props as u32 - 1).to_le_bytes());
    if slot_offset != last_slot_offset {
        data.copy_within(last_slot_offset..last_slot_offset + 32, slot_offset);
    }
}
```

- [ ] Add `build_wasm_value` using `write_object_property_by_name_id` for last-wins:

```rust
fn build_wasm_value(caller: &mut Caller<'_, RuntimeState>, json_value: &JsonValue) -> i64 {
    match json_value {
        JsonValue::Null => value::encode_null(),
        JsonValue::Bool(b) => value::encode_bool(*b),
        JsonValue::Number(n) => value::encode_f64(*n),
        JsonValue::String(s) => store_runtime_string(caller, s.clone()),
        JsonValue::Array(elements) => {
            let arr = alloc_array(caller, elements.len() as u32);
            if let Some(ptr) = resolve_array_ptr(caller, arr) {
                for (i, elem) in elements.iter().enumerate() {
                    let elem_val = build_wasm_value(caller, elem);
                    write_array_elem(caller, ptr, i as u32, elem_val);
                }
                write_array_length(caller, ptr, elements.len() as u32);
            }
            arr
        }
        JsonValue::Object(properties) => {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let obj = alloc_host_object(caller, &env, properties.len() as u32);
            let obj_ptr = resolve_handle(caller, obj);
            if let Some(ptr) = obj_ptr {
                for (key, val) in properties {
                    let val_encoded = build_wasm_value(caller, val);
                    let name_id = find_memory_c_string_with_env(caller, &env, key)
                        .or_else(|| alloc_heap_c_string_with_env(caller, &env, key));
                    if let Some(nid) = name_id {
                        let flags = constants::FLAG_CONFIGURABLE
                            | constants::FLAG_ENUMERABLE
                            | constants::FLAG_WRITABLE;
                        write_object_property_by_name_id(caller, ptr, obj, nid, val_encoded, flags);
                    }
                }
            }
            obj
        }
    }
}
```

- [ ] Add `apply_reviver` with correct `this=holder` and `delete_property_by_name_id`:

```rust
fn apply_reviver(
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
                let new_val = apply_reviver(caller, reviver, val, &i.to_string(), elem_val);
                // ⚠️ Deviation: dense arrays don't support holes.
                // ES §24.5.1 says "delete the element" (creating a hole),
                // but wjsm dense arrays can't represent holes.
                // We write undefined instead, which differs for `in`/enumeration.
                write_array_elem(caller, ptr, i as u32, new_val);
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
                        data[obj_ptr + 12], data[obj_ptr + 13],
                        data[obj_ptr + 14], data[obj_ptr + 15],
                    ]) as usize;
                    for i in 0..num_props {
                        let slot_off = obj_ptr + 16 + i * 32;
                        if slot_off + 32 > data.len() { continue; }
                        let name_id = u32::from_le_bytes([
                            data[slot_off], data[slot_off + 1],
                            data[slot_off + 2], data[slot_off + 3],
                        ]);
                        let prop_val = i64::from_le_bytes(
                            data[slot_off + 8..slot_off + 16].try_into().unwrap()
                        );
                        props.push((name_id, prop_val));
                    }
                }
            }
            for (name_id, prop_val) in &props {
                let name_bytes = read_string_bytes_mem(caller, &env.memory, *name_id);
                let name = String::from_utf8_lossy(&name_bytes);
                let new_val = apply_reviver(caller, reviver, val, &name, *prop_val);
                if value::is_undefined(new_val) {
                    delete_property_by_name_id(caller, &env, val, *name_id);
                } else {
                    let obj_ptr2 = resolve_handle(caller, val).unwrap_or(0);
                    if obj_ptr2 != 0 {
                        write_object_property_by_name_id(
                            caller, obj_ptr2, val, *name_id, new_val,
                            constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
                        );
                    }
                }
            }
        }
    }
    // ES §24.5.1: Call(reviver, holder, «key, value»)
    let key_str = store_runtime_string(caller, key.to_string());
    call_wasm_callback(caller, reviver, holder, &[key_str, val])
        .unwrap_or_else(|_| value::encode_undefined())
}
```

- [ ] Add `json_parse_to_wasm` with trailing check and direct reviver return:

```rust
pub fn json_parse_to_wasm(
    caller: &mut Caller<'_, RuntimeState>,
    text: i64,
    reviver: i64,
) -> i64 {
    let text_str = if value::is_string(text) {
        if value::is_runtime_string_handle(text) {
            let handle = value::decode_runtime_string_handle(text) as usize;
            caller.data().runtime_strings.lock()
                .expect("runtime strings mutex")
                .get(handle).cloned().unwrap_or_default()
        } else {
            read_string(caller, value::decode_string_ptr(text)).unwrap_or_default()
        }
    } else {
        // 非字符串输入：使用 eval_to_string（比 render_value 更接近 ES ToString）
        eval_to_string(caller, text)
    };

    let mut parser = JsonParser::new(text_str.as_bytes());
    match parser.parse_value() {
        Ok(json_value) => {
            // ES 规范：解析完 value 后，剩余内容必须全是空白
            parser.skip_whitespace();
            if parser.pos < parser.input.len() {
                set_runtime_error(caller.data(), "SyntaxError: Unexpected trailing content".to_string());
                return value::encode_undefined();
            }

            let wasm_value = build_wasm_value(caller, &json_value);

            if is_callable_in_runtime(caller, reviver) {
                let env = WasmEnv::from_caller(caller).expect("WasmEnv");
                let root = alloc_host_object(caller, &env, 1);
                // 设置 root[""] = wasm_value
                let empty_name_id = find_memory_c_string_with_env(caller, &env, "")
                    .or_else(|| alloc_heap_c_string_with_env(caller, &env, ""));
                if let Some(nid) = empty_name_id {
                    let root_ptr = resolve_handle(caller, root);
                    if let Some(ptr) = root_ptr {
                        write_object_property_by_name_id(
                            caller, ptr, root, nid, wasm_value,
                            constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
                        );
                    }
                }
                // ES §24.5.1: 遍历后调用 reviver("", value)，this=root
                // 直接使用 apply_reviver 的返回值，不从 heap 重新读取
                let result = apply_reviver(caller, reviver, root, "", wasm_value);
                result
            } else {
                wasm_value
            }
        }
        Err(e) => {
            // ⚠️ Deviation: 使用 set_runtime_error(string) 而非创建 Error 对象
            // 创建真正的 SyntaxError 对象需要重新设计 runtime error 机制
            set_runtime_error(caller.data(), format!("SyntaxError: {}", e));
            value::encode_undefined()
        }
    }
}
```

- [ ] Verify: `cargo check -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: implement heap construction, reviver walk, delete helper, trailing check"`

---

## Task 10: Wire JSON.parse Host Import

**Files:** `crates/wjsm-runtime/src/host_imports/timers_arrays.rs`

**Steps:**
- [ ] Update `json_stringify` to accept 3 params
- [ ] Replace `json_parse` stub with `runtime_json::json_parse_to_wasm`
- [ ] Verify: `cargo build -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: wire JSON.parse and JSON.stringify to full implementations"`

---

## Task 11: Implement Full JSON.stringify

**Files:** `crates/wjsm-runtime/src/runtime_render.rs`

**Key fixes from review:**
1. ✅ `json_escape_string`: supplementary plane chars output directly as UTF-8 (not surrogate pairs)
2. ✅ `build_replacer_whitelist`: returns `Vec<String>` (not `HashSet`) to preserve insertion order
3. ✅ `build_replacer_whitelist`: skips Symbol and non-String/Number elements (ES §24.5.2 step 4.b)
4. ✅ `build_space_string`: uses `n.trunc() as i32` for proper ToIntegerOrInfinity; ⚠️ `chars().take(10)` counts Unicode scalars not UTF-16 code units (deviation documented)
5. ✅ Keep `runtime_json_stringify` as single-param backward-compat wrapper

**Verification:** `cargo build -p wjsm-runtime`

**Steps:**
- [ ] Add `json_escape_string` (supplementary plane chars directly output, no surrogate pair encoding)
- [ ] Add `build_space_string` (ToIntegerOrInfinity for numbers, chars().take(10) for strings)
- [ ] Add `build_replacer_whitelist` returning `Vec<String>` with linear `contains` check
- [ ] Add `runtime_json_stringify_full` and `serialize_json_property` (full ES §24.5.2)
- [ ] Add `get_to_json` helper
- [ ] Remove old `runtime_json_stringify_inner`
- [ ] Verify: `cargo build -p wjsm-runtime`
- [ ] Commit: `git commit -m "feat: implement full ES-compliant JSON.stringify"`

---

## Task 12: Update JSON Fixtures

**Steps:**
- [ ] Run: `WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__json_)'`
- [ ] Verify: `cargo nextest run -E 'test(happy__json_)'`
- [ ] Commit: `git commit -m "test: update JSON fixture expectations"`

---

## Task 13: Final Verification

**Steps:**
- [ ] Full build: `cargo build --all`
- [ ] All tests: `cargo nextest run -E 'test(happy__)'`
- [ ] Manual verification:
  ```bash
  cargo run -- eval 'console.log(JSON.stringify({a: 1, b: [2, 3]}))'
  cargo run -- eval 'console.log(JSON.parse("{\"x\": 42}"))'
  cargo run -- eval 'console.log(JSON.stringify(NaN))'
  cargo run -- eval 'console.log(JSON.parse("123   "))'   # trailing OK
  cargo run -- eval 'console.log(JSON.parse("123abc"))'   # SyntaxError
  ```
- [ ] Commit: `git commit -m "feat: complete JSON implementation with SIMD acceleration"`

---

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| WASM Type Mismatch | Low | Type 16 and Type 2 verified |
| AVX2 unavailable at runtime | Low | `is_x86_feature_detected!("avx2")` with scalar fallback |
| SIMD control char false positives | Low | AND mask excludes high bytes (0x80..0xFF) |
| SIMD string truncation | Low | Fixed: uses `start_pos..end` range |
| Dense array hole deviation | Medium | Documented; undefined written instead of deleting |
| `delete_property_by_name_id` correctness | Medium | Based on proven `reflect_delete_property_impl` swap-remove pattern |
| SyntaxError string vs object | Medium | Documented deviation; requires future runtime redesign |
| Fixture breakage | Medium | Update via `WJSM_UPDATE_FIXTURES=1` |
| Replacer HashSet → Vec | Low | `Vec<String>` with linear contains; small N |
| Reviver return value | Low | Fixed: use `apply_reviver` return directly |

---

## Retirement

| Old code | Location | Replacement | Status |
|----------|----------|-------------|--------|
| `json_parse` stub | `timers_arrays.rs` | `runtime_json::json_parse_to_wasm` | **Replaced** |
| `runtime_json_stringify_inner` | `runtime_render.rs:387-563` | `serialize_json_property` | **Replaced** |
| Single-parameter emission | `compiler_builtins.rs` | Multi-parameter branches | **Replaced** |
| Old fixture expectations | 22+ `.expected` files | Updated | **Updated** |

---

## Summary

13-task plan implementing **ES-compliant** `JSON.parse` and `JSON.stringify` with **SIMD-accelerated parsing**:

| # | Task |
|---|------|
| 1-4 | Metadata/signature/emission updates |
| 5 | SIMD parser module skeleton (AVX2 + scalar fallback) |
| 6 | SIMD string parsing (start_pos tracking, StringBlock) |
| 7 | Number parsing |
| 8 | Array/object parsing |
| 9 | Heap construction + reviver walk + delete helper + trailing check |
| 10 | Wire host imports |
| 11 | Full JSON.stringify (Vec whitelist, direct UTF-8 escaping) |
| 12 | Update fixtures |
| 13 | Final verification |

**ES spec deviations documented (not claimed as compliant):**
- ⚠️ SyntaxError is string, not Error object (runtime limitation)
- ⚠️ Reviver array undefined: writes undefined instead of deleting (dense array limitation)
- ⚠️ JSON.parse non-string input: `eval_to_string` approximation (not full ToString)
- ⚠️ Space string: counts Unicode scalars, not UTF-16 code units
