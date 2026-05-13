# ES Builtins Phase 2: Number + Boolean — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the complete ECMAScript `Number` and `Boolean` built-in objects in the wjsm JavaScript engine.

**Architecture:** Number and Boolean are both callable functions (type coercion) and constructors. At compile time, `Number.isNaN(x)` and `Boolean(value)` calls are intercepted by `builtin_from_static_member` and `builtin_from_global_ident` respectively. The type coercion behavior (`Number(x)`, `Boolean(x)`) uses existing `to_number` / `to_boolean` runtime primitives. Static constants (Number.EPSILON, Number.MAX_VALUE, etc.) are initialized as data segment constants like Math constants. Prototype methods use Type 3 `(i64)->i64` signature with `this` as first arg.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_static_member, builtin_from_global_ident
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — host function implementations + imports array

**Design decisions:**
- `Number(value)` / `Boolean(value)` as coercion: uses existing `to_number` / abstract `ToBoolean` logic already present in the runtime — we just need a Builtin that delegates to them
- `Number` constructor with `new`: creates a Number wrapper object (host object with internal `[[NumberData]]` slot)
- `Boolean` constructor with `new`: creates a Boolean wrapper object (host object with internal `[[BooleanData]]` slot)
- `Number.prototype.toString(radix)`: uses Rust's built-in number formatting with radix support
- `Number.prototype.toFixed(digits)`: uses Rust's format! macro with precision
- `Number.isNaN` vs global `isNaN`: Number.isNaN does NOT coerce — only returns true for actual NaN
- `Number.isFinite` vs global `isFinite`: Number.isFinite does NOT coerce

---

### Task 1: Add Number + Boolean Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add Number + Boolean variants to Builtin enum**

After the last Math variant (`MathTrunc`), add:

```rust
    // ── Number constructor and methods ──────────────────────────────────
    NumberConstructor,
    NumberIsNaN,
    NumberIsFinite,
    NumberIsInteger,
    NumberIsSafeInteger,
    NumberParseInt,
    NumberParseFloat,
    NumberProtoToString,
    NumberProtoValueOf,
    NumberProtoToFixed,
    NumberProtoToExponential,
    NumberProtoToPrecision,
    // ── Boolean constructor and methods ─────────────────────────────────
    BooleanConstructor,
    BooleanProtoToString,
    BooleanProtoValueOf,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::NumberConstructor => "Number",
            Self::NumberIsNaN => "Number.isNaN",
            Self::NumberIsFinite => "Number.isFinite",
            Self::NumberIsInteger => "Number.isInteger",
            Self::NumberIsSafeInteger => "Number.isSafeInteger",
            Self::NumberParseInt => "Number.parseInt",
            Self::NumberParseFloat => "Number.parseFloat",
            Self::NumberProtoToString => "Number.prototype.toString",
            Self::NumberProtoValueOf => "Number.prototype.valueOf",
            Self::NumberProtoToFixed => "Number.prototype.toFixed",
            Self::NumberProtoToExponential => "Number.prototype.toExponential",
            Self::NumberProtoToPrecision => "Number.prototype.toPrecision",
            Self::BooleanConstructor => "Boolean",
            Self::BooleanProtoToString => "Boolean.prototype.toString",
            Self::BooleanProtoValueOf => "Boolean.prototype.valueOf",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add Number and Boolean builtin variants"
```

---

### Task 2: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add Number as global ident and static member**

In `builtin_from_global_ident`, add:
```rust
        "Number" => Some(Builtin::NumberConstructor),
        "Boolean" => Some(Builtin::BooleanConstructor),
```

In `builtin_from_static_member`, add under the `"Math"` arm:
```rust
        "Number" => match property {
            "isNaN" => Some(Builtin::NumberIsNaN),
            "isFinite" => Some(Builtin::NumberIsFinite),
            "isInteger" => Some(Builtin::NumberIsInteger),
            "isSafeInteger" => Some(Builtin::NumberIsSafeInteger),
            "parseInt" => Some(Builtin::NumberParseInt),
            "parseFloat" => Some(Builtin::NumberParseFloat),
            _ => None,
        },
```

- [ ] **Step 2: Add Number.prototype method recognition**

In `lower_call_expr`, after the String.prototype method optimization block, add a Number.prototype method recognition block following the same pattern as Object.prototype:

```rust
                    // Number.prototype method call optimization
                    if let Some(num_builtin) =
                        builtin_from_number_proto_method(&prop_ident.sym)
                    {
                        this_val = self.lower_expr(&member_expr.obj, block)?;
                        let mut builtin_args = vec![this_val];
                        for arg in &call.args {
                            builtin_args.push(self.lower_expr(&arg.expr, block)?);
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: num_builtin,
                                args: builtin_args,
                            },
                        );
                        return Ok(dest);
                    }
```

- [ ] **Step 3: Add helper functions**

Add `builtin_from_number_proto_method` and `builtin_from_boolean_proto_method`:

```rust
fn builtin_from_number_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "toString" => Some(NumberProtoToString),
        "valueOf" => Some(NumberProtoValueOf),
        "toFixed" => Some(NumberProtoToFixed),
        "toExponential" => Some(NumberProtoToExponential),
        "toPrecision" => Some(NumberProtoToPrecision),
        _ => None,
    }
}

fn builtin_from_boolean_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "toString" => Some(BooleanProtoToString),
        "valueOf" => Some(BooleanProtoValueOf),
        _ => None,
    }
}
```

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add Number and Boolean call recognition"
```

---

### Task 3: Register WASM types and imports

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM type for NumberProtoToFixed/ToExponential/ToPrecision (need radix/digits param)**

After Type 22:
```rust
        // Type 23: (i64, i64) -> (i64) — Number.prototype.toFixed(receiver, digits)
        types.ty().function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
```

- [ ] **Step 2: Add import declarations (indices 228-242)**

```rust
        // ── Number imports (indices 228-239) ──
        imports.import("env", "number_constructor", EntityType::Function(3));
        imports.import("env", "number_is_nan", EntityType::Function(3));
        imports.import("env", "number_is_finite", EntityType::Function(3));
        imports.import("env", "number_is_integer", EntityType::Function(3));
        imports.import("env", "number_is_safe_integer", EntityType::Function(3));
        imports.import("env", "number_parse_int", EntityType::Function(3));
        imports.import("env", "number_parse_float", EntityType::Function(3));
        imports.import("env", "number_proto_to_string", EntityType::Function(23));
        imports.import("env", "number_proto_value_of", EntityType::Function(3));
        imports.import("env", "number_proto_to_fixed", EntityType::Function(23));
        imports.import("env", "number_proto_to_exponential", EntityType::Function(23));
        imports.import("env", "number_proto_to_precision", EntityType::Function(23));
        // ── Boolean imports (indices 240-242) ──
        imports.import("env", "boolean_constructor", EntityType::Function(3));
        imports.import("env", "boolean_proto_to_string", EntityType::Function(3));
        imports.import("env", "boolean_proto_value_of", EntityType::Function(3));
```

- [ ] **Step 3: Add builtin_arity entries**

```rust
        Builtin::NumberConstructor => ("number_constructor", 1),
        Builtin::NumberIsNaN => ("number_is_nan", 1),
        Builtin::NumberIsFinite => ("number_is_finite", 1),
        Builtin::NumberIsInteger => ("number_is_integer", 1),
        Builtin::NumberIsSafeInteger => ("number_is_safe_integer", 1),
        Builtin::NumberParseInt => ("number_parse_int", 1),
        Builtin::NumberParseFloat => ("number_parse_float", 1),
        Builtin::NumberProtoToString => ("number_proto_to_string", 2),
        Builtin::NumberProtoValueOf => ("number_proto_value_of", 1),
        Builtin::NumberProtoToFixed => ("number_proto_to_fixed", 2),
        Builtin::NumberProtoToExponential => ("number_proto_to_exponential", 2),
        Builtin::NumberProtoToPrecision => ("number_proto_to_precision", 2),
        Builtin::BooleanConstructor => ("boolean_constructor", 1),
        Builtin::BooleanProtoToString => ("boolean_proto_to_string", 1),
        Builtin::BooleanProtoValueOf => ("boolean_proto_value_of", 1),
```

- [ ] **Step 4: Add builtin_func_indices entries**

```rust
        builtin_func_indices.insert(Builtin::NumberConstructor, 228);
        builtin_func_indices.insert(Builtin::NumberIsNaN, 229);
        builtin_func_indices.insert(Builtin::NumberIsFinite, 230);
        builtin_func_indices.insert(Builtin::NumberIsInteger, 231);
        builtin_func_indices.insert(Builtin::NumberIsSafeInteger, 232);
        builtin_func_indices.insert(Builtin::NumberParseInt, 233);
        builtin_func_indices.insert(Builtin::NumberParseFloat, 234);
        builtin_func_indices.insert(Builtin::NumberProtoToString, 235);
        builtin_func_indices.insert(Builtin::NumberProtoValueOf, 236);
        builtin_func_indices.insert(Builtin::NumberProtoToFixed, 237);
        builtin_func_indices.insert(Builtin::NumberProtoToExponential, 238);
        builtin_func_indices.insert(Builtin::NumberProtoToPrecision, 239);
        builtin_func_indices.insert(Builtin::BooleanConstructor, 240);
        builtin_func_indices.insert(Builtin::BooleanProtoToString, 241);
        builtin_func_indices.insert(Builtin::BooleanProtoValueOf, 242);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register Number and Boolean WASM imports"
```

---

### Task 4: Implement Number + Boolean host functions in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add Number host functions**

Insert before the `let imports = [` line:

```rust
    // ── Number host functions ────────────────────────────────────────────
    let number_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if value::is_undefined(val) || value::is_null(val) {
            return 0.0f64.to_bits() as i64;
        }
        if value::is_boolean(val) {
            return if value::is_truthy(val) { 1.0f64.to_bits() as i64 } else { 0.0f64.to_bits() as i64 };
        }
        if value::is_number(val) { return val; }
        if value::is_string(val) {
            if let Some(s) = read_value_string_bytes(&mut caller, val) {
                let trimmed = String::from_utf8_lossy(&s).trim().to_string();
                if trimmed.is_empty() { return 0.0f64.to_bits() as i64; }
                if let Ok(n) = trimmed.parse::<f64>() { return n.to_bits() as i64; }
                return f64::NAN.to_bits() as i64;
            }
        }
        f64::NAN.to_bits() as i64
    });
    let number_is_nan_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if value::is_number(val) {
            let f = f64::from_bits(val as u64);
            value::encode_bool(f.is_nan())
        } else {
            value::encode_bool(false)
        }
    });
    let number_is_finite_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if value::is_number(val) {
            let f = f64::from_bits(val as u64);
            value::encode_bool(f.is_finite())
        } else {
            value::encode_bool(false)
        }
    });
    let number_is_integer_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if value::is_number(val) {
            let f = f64::from_bits(val as u64);
            value::encode_bool(f.is_finite() && f == f.trunc())
        } else {
            value::encode_bool(false)
        }
    });
    let number_is_safe_integer_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if value::is_number(val) {
            let f = f64::from_bits(val as u64);
            value::encode_bool(f.is_finite() && f == f.trunc() && f >= -(2.0f64.powi(53) - 1.0) && f <= 2.0f64.powi(53) - 1.0)
        } else {
            value::encode_bool(false)
        }
    });
    let number_parse_int_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if let Some(s) = read_value_string_bytes(&mut caller, val) {
            let s = String::from_utf8_lossy(&s);
            let trimmed = s.trim_start();
            if trimmed.is_empty() { return f64::NAN.to_bits() as i64; }
            // Simple integer parsing: scan for leading digits, optional sign
            let chars: Vec<char> = trimmed.chars().collect();
            let mut i = 0;
            let sign = if i < chars.len() && chars[i] == '-' { i += 1; -1.0 } else if i < chars.len() && chars[i] == '+' { i += 1; 1.0 } else { 1.0 };
            if i < chars.len() && chars[i] == '0' && i + 1 < chars.len() && (chars[i+1] == 'x' || chars[i+1] == 'X') {
                // hex
                i += 2;
                let hex_str: String = chars[i..].iter().take_while(|c| c.is_ascii_hexdigit()).collect();
                if hex_str.is_empty() { return f64::NAN.to_bits() as i64; }
                if let Ok(n) = i64::from_str_radix(&hex_str, 16) {
                    return (sign * n as f64).to_bits() as i64;
                }
                return f64::NAN.to_bits() as i64;
            }
            let num_str: String = chars[i..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if num_str.is_empty() { return f64::NAN.to_bits() as i64; }
            if let Ok(n) = num_str.parse::<f64>() {
                return (sign * n).to_bits() as i64;
            }
        }
        f64::NAN.to_bits() as i64
    });
    let number_parse_float_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        if let Some(s) = read_value_string_bytes(&mut caller, val) {
            let s = String::from_utf8_lossy(&s);
            let trimmed = s.trim();
            if trimmed.is_empty() { return f64::NAN.to_bits() as i64; }
            if let Ok(n) = trimmed.parse::<f64>() { return n.to_bits() as i64; }
        }
        f64::NAN.to_bits() as i64
    });
    let number_proto_to_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, radix: i64| -> i64 {
        let n = f64::from_bits(receiver as u64);
        let r = f64::from_bits(radix as u64) as u32;
        let radix = if r < 2 || r > 36 { 10 } else { r };
        let s = if radix == 10 {
            if n.is_nan() { "NaN".to_string() }
            else if n.is_infinite() { if n.is_sign_negative() { "-Infinity".to_string() } else { "Infinity".to_string() } }
            else { format!("{}", n) }
        } else {
            // Simplified: convert integer part
            if n.is_nan() { "NaN".to_string() }
            else if n.is_infinite() { if n.is_sign_negative() { "-Infinity".to_string() } else { "Infinity".to_string() } }
            else {
                let int_part = n.trunc() as i64;
                let sign = if int_part < 0 { "-" } else { "" };
                let abs_val = int_part.unsigned_abs();
                let mut result = String::new();
                let mut v = abs_val;
                if v == 0 { result.push('0'); }
                let digits = "0123456789abcdefghijklmnopqrstuvwxyz";
                while v > 0 {
                    result.push(digits.chars().nth((v % radix as u64) as usize).unwrap());
                    v /= radix as u64;
                }
                format!("{}{}", sign, result.chars().rev().collect::<String>())
            }
        };
        store_runtime_string(&mut caller, s)
    });
    let number_proto_value_of_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        receiver
    });
    let number_proto_to_fixed_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, digits: i64| -> i64 {
        let n = f64::from_bits(receiver as u64);
        let d = (f64::from_bits(digits as u64) as i32).max(0).min(100) as usize;
        if n.is_nan() { return receiver; }
        if n.is_infinite() { return receiver; }
        let s = format!("{:.prec$}", n, prec = d);
        store_runtime_string_from_str(&mut caller, &s)
    });
    let number_proto_to_exponential_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, digits: i64| -> i64 {
        let n = f64::from_bits(receiver as u64);
        if n.is_nan() || n.is_infinite() { return receiver; }
        let s = if digits == value::encode_undefined() {
            format!("{:e}", n)
        } else {
            let d = (f64::from_bits(digits as u64) as i32).max(0).min(100) as usize;
            format!("{:.prec$e}", n, prec = d)
        };
        store_runtime_string_from_str(&mut caller, &s)
    });
    let number_proto_to_precision_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64, digits: i64| -> i64 {
        let n = f64::from_bits(receiver as u64);
        if n.is_nan() || n.is_infinite() { return receiver; }
        let s = if digits == value::encode_undefined() {
            format!("{}", n)
        } else {
            let d = (f64::from_bits(digits as u64) as i32).max(1).min(100) as usize;
            format!("{:.prec$}", n, prec = d)
        };
        store_runtime_string_from_str(&mut caller, &s)
    });

    // ── Boolean host functions ───────────────────────────────────────────
    let boolean_constructor_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
        value::encode_bool(value::is_truthy(val))
    });
    let boolean_proto_to_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        if value::is_truthy(receiver) {
            store_runtime_string_from_str(&mut caller, "true")
        } else {
            store_runtime_string_from_str(&mut caller, "false")
        }
    });
    let boolean_proto_value_of_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        receiver
    });
```

- [ ] **Step 2: Add imports to the imports array**

After the last Math import (index 227):
```rust
        // ── Number imports (228-239) ──
        number_constructor_fn.into(),       // 228
        number_is_nan_fn.into(),            // 229
        number_is_finite_fn.into(),         // 230
        number_is_integer_fn.into(),        // 231
        number_is_safe_integer_fn.into(),   // 232
        number_parse_int_fn.into(),         // 233
        number_parse_float_fn.into(),       // 234
        number_proto_to_string_fn.into(),   // 235
        number_proto_value_of_fn.into(),    // 236
        number_proto_to_fixed_fn.into(),    // 237
        number_proto_to_exponential_fn.into(), // 238
        number_proto_to_precision_fn.into(),   // 239
        // ── Boolean imports (240-242) ──
        boolean_constructor_fn.into(),      // 240
        boolean_proto_to_string_fn.into(),  // 241
        boolean_proto_value_of_fn.into(),   // 242
```

- [ ] **Step 3: Add `store_runtime_string_from_str` helper if not exists**

Check if a helper like `store_runtime_string_from_str` exists. If not, add it near `store_runtime_string`:

```rust
fn store_runtime_string_from_str(caller: &mut Caller<'_, RuntimeState>, s: &str) -> i64 {
    let mut strings = caller.data().runtime_strings.lock().expect("runtime_strings mutex");
    let handle = strings.len() as u32;
    strings.push(s.to_string());
    value::encode_handle(value::TAG_STRING, handle)
}
```

- [ ] **Step 4: Add Number constants to main() initialization in semantic layer**

In `crates/wjsm-semantic/src/lib.rs`, after Math constants block:

```rust
        // Number constants
        let number_constants: [(&str, f64); 8] = [
            ("$0.Number_EPSILON", f64::EPSILON),
            ("$0.Number_MAX_VALUE", f64::MAX),
            ("$0.Number_MIN_VALUE", f64::MIN_POSITIVE),
            ("$0.Number_MAX_SAFE_INTEGER", 9007199254740991.0),
            ("$0.Number_MIN_SAFE_INTEGER", -9007199254740991.0),
            ("$0.Number_NaN", f64::NAN),
            ("$0.Number_NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("$0.Number_POSITIVE_INFINITY", f64::INFINITY),
        ];
        for (name, value) in &number_constants {
            let c = self.module.add_constant(Constant::Number(*value));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry, Instruction::Const { dest: v, constant: c },
            );
            self.current_function.append_instruction(
                entry, Instruction::StoreVar { name: name.to_string(), value: v },
            );
        }
```

- [ ] **Step 5: Full build check and commit**

Run: `cargo check`
Expected: compiles across all crates

```bash
git add crates/wjsm-runtime/src/lib.rs crates/wjsm-semantic/src/lib.rs
git commit -m "feat(runtime): implement Number and Boolean host functions and constants"
```

---

### Task 5: Add test fixtures

**Files:**
- Create: `fixtures/happy/number_methods.js` + `.expected`
- Create: `fixtures/happy/number_constants.js` + `.expected`
- Create: `fixtures/happy/boolean_basic.js` + `.expected`

- [ ] **Step 1: number_methods test**

`fixtures/happy/number_methods.js`:
```js
console.log(Number.isNaN(NaN));
console.log(Number.isNaN(1));
console.log(Number.isFinite(42));
console.log(Number.isFinite(Infinity));
console.log(Number.isInteger(3.0));
console.log(Number.isInteger(3.14));
console.log((3.14159).toFixed(2));
console.log(Number.parseInt("42"));
console.log(Number.parseFloat("3.14"));
```

`fixtures/happy/number_methods.expected`:
```
true
false
true
false
true
false
3.14
42
3.14
```

- [ ] **Step 2: number_constants test**

`fixtures/happy/number_constants.js`:
```js
console.log(Number.MAX_VALUE > 0);
console.log(Number.MIN_VALUE > 0);
console.log(Number.MAX_SAFE_INTEGER > 0);
```

`fixtures/happy/number_constants.expected`:
```
true
true
true
```

- [ ] **Step 3: boolean_basic test**

`fixtures/happy/boolean_basic.js`:
```js
console.log(Boolean(1));
console.log(Boolean(0));
console.log(Boolean(""));
console.log(Boolean("hello"));
console.log(true.toString());
console.log(false.valueOf());
```

`fixtures/happy/boolean_basic.expected`:
```
true
false
false
true
true
false
```

- [ ] **Step 4: Run tests and commit**

Run: `cargo test`
Expected: all new fixture tests pass

```bash
git add fixtures/happy/number_methods.js fixtures/happy/number_methods.expected \
        fixtures/happy/number_constants.js fixtures/happy/number_constants.expected \
        fixtures/happy/boolean_basic.js fixtures/happy/boolean_basic.expected
git commit -m "test: add Number and Boolean test fixtures"
```