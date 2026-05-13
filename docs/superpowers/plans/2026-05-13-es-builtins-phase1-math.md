# ES Builtins Phase 1: Math Object — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the complete ECMAScript `Math` built-in object (~35 static methods + 8 constants) in the wjsm JavaScript engine.

**Architecture:** Math is a namespace object (not a constructor). All methods are stateless pure functions. At compile time, `Math.abs(x)` calls are intercepted by the semantic layer via `builtin_from_static_member("Math", "abs")` and lowered to `CallBuiltin(MathAbs, ...)`. At runtime, each method is a host function (`Func::wrap`) operating on f64 values. Constants (Math.PI, Math.E, etc.) are initialized as data segment constants in the WASM backend's main function preamble.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_static_member
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices, constant initialization
- `crates/wjsm-runtime/src/lib.rs` — host function implementations + imports array

**Design decisions:**
- Math methods use existing WASM type signatures: Type 2 `(i64,i64)->i64` for binary, Type 3 `(i64)->i64` for unary
- Math constants are pre-computed f64 values stored as data segment constants, emitted via `Instruction::Const { Constant::Number(...) }` + `Instruction::StoreVar` in main()
- `Math.random()` uses `rand` crate (add to Cargo.toml)
- `Math.imul` returns i32 result packed as i64 (f64 bits)
- `Math.clz32` returns i32 result packed as i64

---

### Task 0: Add rand dependency

**Files:**
- Modify: `crates/wjsm-runtime/Cargo.toml`

- [ ] **Step 1: Add rand to runtime Cargo.toml**

```toml
# Add under [dependencies]
rand = "0.8"
```

- [ ] **Step 2: Verify build**

Run: `cargo check -p wjsm-runtime`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-runtime/Cargo.toml Cargo.lock
git commit -m "chore: add rand dependency for Math.random()"
```

---

### Task 1: Add Math Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add Math builtin variants to the Builtin enum**

Find the `pub enum Builtin {` block (around line 661). Add after the last String builtin variant (`StringFromCodePoint`):

```rust
    // ── Math static methods ────────────────────────────────────────────
    MathAbs,
    MathAcos,
    MathAcosh,
    MathAsin,
    MathAsinh,
    MathAtan,
    MathAtanh,
    MathAtan2,
    MathCbrt,
    MathCeil,
    MathClz32,
    MathCos,
    MathCosh,
    MathExp,
    MathExpm1,
    MathFloor,
    MathFround,
    MathHypot,
    MathImul,
    MathLog,
    MathLog1p,
    MathLog10,
    MathLog2,
    MathMax,
    MathMin,
    MathPow,
    MathRandom,
    MathRound,
    MathSign,
    MathSin,
    MathSinh,
    MathSqrt,
    MathTan,
    MathTanh,
    MathTrunc,
```

- [ ] **Step 2: Add Display impl entries**

Find the `impl fmt::Display for Builtin` block (around line 863). Add after the last String entry:

```rust
            Self::MathAbs => "Math.abs",
            Self::MathAcos => "Math.acos",
            Self::MathAcosh => "Math.acosh",
            Self::MathAsin => "Math.asin",
            Self::MathAsinh => "Math.asinh",
            Self::MathAtan => "Math.atan",
            Self::MathAtanh => "Math.atanh",
            Self::MathAtan2 => "Math.atan2",
            Self::MathCbrt => "Math.cbrt",
            Self::MathCeil => "Math.ceil",
            Self::MathClz32 => "Math.clz32",
            Self::MathCos => "Math.cos",
            Self::MathCosh => "Math.cosh",
            Self::MathExp => "Math.exp",
            Self::MathExpm1 => "Math.expm1",
            Self::MathFloor => "Math.floor",
            Self::MathFround => "Math.fround",
            Self::MathHypot => "Math.hypot",
            Self::MathImul => "Math.imul",
            Self::MathLog => "Math.log",
            Self::MathLog1p => "Math.log1p",
            Self::MathLog10 => "Math.log10",
            Self::MathLog2 => "Math.log2",
            Self::MathMax => "Math.max",
            Self::MathMin => "Math.min",
            Self::MathPow => "Math.pow",
            Self::MathRandom => "Math.random",
            Self::MathRound => "Math.round",
            Self::MathSign => "Math.sign",
            Self::MathSin => "Math.sin",
            Self::MathSinh => "Math.sinh",
            Self::MathSqrt => "Math.sqrt",
            Self::MathTan => "Math.tan",
            Self::MathTanh => "Math.tanh",
            Self::MathTrunc => "Math.trunc",
```

- [ ] **Step 3: Build check**

Run: `cargo check -p wjsm-ir`
Expected: compiles successfully (warning: variants not yet handled in match arms in backend)

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add Math builtin variants to Builtin enum"
```

---

### Task 2: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add Math static member mappings**

Find the `fn builtin_from_static_member` function (around line 13541). Add a new `"Math"` match arm after the `"String"` arm:

```rust
        "Math" => match property {
            "abs" => Some(Builtin::MathAbs),
            "acos" => Some(Builtin::MathAcos),
            "acosh" => Some(Builtin::MathAcosh),
            "asin" => Some(Builtin::MathAsin),
            "asinh" => Some(Builtin::MathAsinh),
            "atan" => Some(Builtin::MathAtan),
            "atanh" => Some(Builtin::MathAtanh),
            "atan2" => Some(Builtin::MathAtan2),
            "cbrt" => Some(Builtin::MathCbrt),
            "ceil" => Some(Builtin::MathCeil),
            "clz32" => Some(Builtin::MathClz32),
            "cos" => Some(Builtin::MathCos),
            "cosh" => Some(Builtin::MathCosh),
            "exp" => Some(Builtin::MathExp),
            "expm1" => Some(Builtin::MathExpm1),
            "floor" => Some(Builtin::MathFloor),
            "fround" => Some(Builtin::MathFround),
            "hypot" => Some(Builtin::MathHypot),
            "imul" => Some(Builtin::MathImul),
            "log" => Some(Builtin::MathLog),
            "log1p" => Some(Builtin::MathLog1p),
            "log10" => Some(Builtin::MathLog10),
            "log2" => Some(Builtin::MathLog2),
            "max" => Some(Builtin::MathMax),
            "min" => Some(Builtin::MathMin),
            "pow" => Some(Builtin::MathPow),
            "random" => Some(Builtin::MathRandom),
            "round" => Some(Builtin::MathRound),
            "sign" => Some(Builtin::MathSign),
            "sin" => Some(Builtin::MathSin),
            "sinh" => Some(Builtin::MathSinh),
            "sqrt" => Some(Builtin::MathSqrt),
            "tan" => Some(Builtin::MathTan),
            "tanh" => Some(Builtin::MathTanh),
            "trunc" => Some(Builtin::MathTrunc),
            _ => None,
        },
```

- [ ] **Step 2: Build check**

Run: `cargo check -p wjsm-semantic`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add Math static method recognition"
```

---

### Task 3: Register WASM types and imports for Math

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM function type for Math.hypot (variadic-like via shadow stack)**

Find the type section in `new_with_data_base` (around line 500). After the last type definition (Type 20 for regex_create), add:

```rust
        // Type 21: (i32, i32) -> (i64) — Math.max/Math.min (variadic via shadow stack)
        //   param 0 = args_base_ptr (i32), param 1 = args_count (i32)
        types.ty().function(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
        // Type 22: () -> (i64) — Math.random (no args)
        types.ty().function(vec![], vec![ValType::I64]);
```

- [ ] **Step 2: Add import declarations**

Find the import section (around line 577). After the last import (string_from_code_point, index 192), add:

```rust
        // ── Math imports (indices 193-227) ──
        // Import 193: math_abs: (i64) -> (i64)
        imports.import("env", "math_abs", EntityType::Function(3));
        // Import 194: math_acos: (i64) -> (i64)
        imports.import("env", "math_acos", EntityType::Function(3));
        // Import 195: math_acosh: (i64) -> (i64)
        imports.import("env", "math_acosh", EntityType::Function(3));
        // Import 196: math_asin: (i64) -> (i64)
        imports.import("env", "math_asin", EntityType::Function(3));
        // Import 197: math_asinh: (i64) -> (i64)
        imports.import("env", "math_asinh", EntityType::Function(3));
        // Import 198: math_atan: (i64) -> (i64)
        imports.import("env", "math_atan", EntityType::Function(3));
        // Import 199: math_atanh: (i64) -> (i64)
        imports.import("env", "math_atanh", EntityType::Function(3));
        // Import 200: math_atan2: (i64, i64) -> (i64)
        imports.import("env", "math_atan2", EntityType::Function(2));
        // Import 201: math_cbrt: (i64) -> (i64)
        imports.import("env", "math_cbrt", EntityType::Function(3));
        // Import 202: math_ceil: (i64) -> (i64)
        imports.import("env", "math_ceil", EntityType::Function(3));
        // Import 203: math_clz32: (i64) -> (i64)
        imports.import("env", "math_clz32", EntityType::Function(3));
        // Import 204: math_cos: (i64) -> (i64)
        imports.import("env", "math_cos", EntityType::Function(3));
        // Import 205: math_cosh: (i64) -> (i64)
        imports.import("env", "math_cosh", EntityType::Function(3));
        // Import 206: math_exp: (i64) -> (i64)
        imports.import("env", "math_exp", EntityType::Function(3));
        // Import 207: math_expm1: (i64) -> (i64)
        imports.import("env", "math_expm1", EntityType::Function(3));
        // Import 208: math_floor: (i64) -> (i64)
        imports.import("env", "math_floor", EntityType::Function(3));
        // Import 209: math_fround: (i64) -> (i64)
        imports.import("env", "math_fround", EntityType::Function(3));
        // Import 210: math_hypot: (i32, i32) -> (i64) — variadic via shadow stack
        imports.import("env", "math_hypot", EntityType::Function(21));
        // Import 211: math_imul: (i64, i64) -> (i64)
        imports.import("env", "math_imul", EntityType::Function(2));
        // Import 212: math_log: (i64) -> (i64)
        imports.import("env", "math_log", EntityType::Function(3));
        // Import 213: math_log1p: (i64) -> (i64)
        imports.import("env", "math_log1p", EntityType::Function(3));
        // Import 214: math_log10: (i64) -> (i64)
        imports.import("env", "math_log10", EntityType::Function(3));
        // Import 215: math_log2: (i64) -> (i64)
        imports.import("env", "math_log2", EntityType::Function(3));
        // Import 216: math_max: (i32, i32) -> (i64) — variadic via shadow stack
        imports.import("env", "math_max", EntityType::Function(21));
        // Import 217: math_min: (i32, i32) -> (i64) — variadic via shadow stack
        imports.import("env", "math_min", EntityType::Function(21));
        // Import 218: math_pow: (i64, i64) -> (i64)
        imports.import("env", "math_pow", EntityType::Function(2));
        // Import 219: math_random: () -> (i64)
        imports.import("env", "math_random", EntityType::Function(22));
        // Import 220: math_round: (i64) -> (i64)
        imports.import("env", "math_round", EntityType::Function(3));
        // Import 221: math_sign: (i64) -> (i64)
        imports.import("env", "math_sign", EntityType::Function(3));
        // Import 222: math_sin: (i64) -> (i64)
        imports.import("env", "math_sin", EntityType::Function(3));
        // Import 223: math_sinh: (i64) -> (i64)
        imports.import("env", "math_sinh", EntityType::Function(3));
        // Import 224: math_sqrt: (i64) -> (i64)
        imports.import("env", "math_sqrt", EntityType::Function(3));
        // Import 225: math_tan: (i64) -> (i64)
        imports.import("env", "math_tan", EntityType::Function(3));
        // Import 226: math_tanh: (i64) -> (i64)
        imports.import("env", "math_tanh", EntityType::Function(3));
        // Import 227: math_trunc: (i64) -> (i64)
        imports.import("env", "math_trunc", EntityType::Function(3));
```

- [ ] **Step 3: Add builtin_arity entries**

Find the `pub fn builtin_arity` function (around line 7118). Add after the last entry:

```rust
        Builtin::MathAbs => ("math_abs", 1),
        Builtin::MathAcos => ("math_acos", 1),
        Builtin::MathAcosh => ("math_acosh", 1),
        Builtin::MathAsin => ("math_asin", 1),
        Builtin::MathAsinh => ("math_asinh", 1),
        Builtin::MathAtan => ("math_atan", 1),
        Builtin::MathAtanh => ("math_atanh", 1),
        Builtin::MathAtan2 => ("math_atan2", 2),
        Builtin::MathCbrt => ("math_cbrt", 1),
        Builtin::MathCeil => ("math_ceil", 1),
        Builtin::MathClz32 => ("math_clz32", 1),
        Builtin::MathCos => ("math_cos", 1),
        Builtin::MathCosh => ("math_cosh", 1),
        Builtin::MathExp => ("math_exp", 1),
        Builtin::MathExpm1 => ("math_expm1", 1),
        Builtin::MathFloor => ("math_floor", 1),
        Builtin::MathFround => ("math_fround", 1),
        Builtin::MathHypot => ("math_hypot", 2),
        Builtin::MathImul => ("math_imul", 2),
        Builtin::MathLog => ("math_log", 1),
        Builtin::MathLog1p => ("math_log1p", 1),
        Builtin::MathLog10 => ("math_log10", 1),
        Builtin::MathLog2 => ("math_log2", 1),
        Builtin::MathMax => ("math_max", 2),
        Builtin::MathMin => ("math_min", 2),
        Builtin::MathPow => ("math_pow", 2),
        Builtin::MathRandom => ("math_random", 0),
        Builtin::MathRound => ("math_round", 1),
        Builtin::MathSign => ("math_sign", 1),
        Builtin::MathSin => ("math_sin", 1),
        Builtin::MathSinh => ("math_sinh", 1),
        Builtin::MathSqrt => ("math_sqrt", 1),
        Builtin::MathTan => ("math_tan", 1),
        Builtin::MathTanh => ("math_tanh", 1),
        Builtin::MathTrunc => ("math_trunc", 1),
```

- [ ] **Step 4: Add builtin_func_indices entries**

Find the `builtin_func_indices` HashMap initialization (around line 1061). Add after the last insert:

```rust
        builtin_func_indices.insert(Builtin::MathAbs, 193);
        builtin_func_indices.insert(Builtin::MathAcos, 194);
        builtin_func_indices.insert(Builtin::MathAcosh, 195);
        builtin_func_indices.insert(Builtin::MathAsin, 196);
        builtin_func_indices.insert(Builtin::MathAsinh, 197);
        builtin_func_indices.insert(Builtin::MathAtan, 198);
        builtin_func_indices.insert(Builtin::MathAtanh, 199);
        builtin_func_indices.insert(Builtin::MathAtan2, 200);
        builtin_func_indices.insert(Builtin::MathCbrt, 201);
        builtin_func_indices.insert(Builtin::MathCeil, 202);
        builtin_func_indices.insert(Builtin::MathClz32, 203);
        builtin_func_indices.insert(Builtin::MathCos, 204);
        builtin_func_indices.insert(Builtin::MathCosh, 205);
        builtin_func_indices.insert(Builtin::MathExp, 206);
        builtin_func_indices.insert(Builtin::MathExpm1, 207);
        builtin_func_indices.insert(Builtin::MathFloor, 208);
        builtin_func_indices.insert(Builtin::MathFround, 209);
        builtin_func_indices.insert(Builtin::MathHypot, 210);
        builtin_func_indices.insert(Builtin::MathImul, 211);
        builtin_func_indices.insert(Builtin::MathLog, 212);
        builtin_func_indices.insert(Builtin::MathLog1p, 213);
        builtin_func_indices.insert(Builtin::MathLog10, 214);
        builtin_func_indices.insert(Builtin::MathLog2, 215);
        builtin_func_indices.insert(Builtin::MathMax, 216);
        builtin_func_indices.insert(Builtin::MathMin, 217);
        builtin_func_indices.insert(Builtin::MathPow, 218);
        builtin_func_indices.insert(Builtin::MathRandom, 219);
        builtin_func_indices.insert(Builtin::MathRound, 220);
        builtin_func_indices.insert(Builtin::MathSign, 221);
        builtin_func_indices.insert(Builtin::MathSin, 222);
        builtin_func_indices.insert(Builtin::MathSinh, 223);
        builtin_func_indices.insert(Builtin::MathSqrt, 224);
        builtin_func_indices.insert(Builtin::MathTan, 225);
        builtin_func_indices.insert(Builtin::MathTanh, 226);
        builtin_func_indices.insert(Builtin::MathTrunc, 227);
```

- [ ] **Step 5: Build check**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles (warning: unused import variants until runtime is updated)

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register Math WASM imports and type signatures"
```

---

### Task 4: Implement Math host functions in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add `use rand::Rng;` import**

At the top of the file, after `use std::time::{Duration, Instant};`, add:

```rust
use rand::Rng;
```

- [ ] **Step 2: Add Math host function implementations**

Insert before the `let imports = [` line (around line 7199). Add all Math host functions:

```rust
    // ── Math host functions ──────────────────────────────────────────────
    let math_abs_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.abs().to_bits() as i64
    });
    let math_acos_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.acos().to_bits() as i64
    });
    let math_acosh_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.acosh().to_bits() as i64
    });
    let math_asin_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.asin().to_bits() as i64
    });
    let math_asinh_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.asinh().to_bits() as i64
    });
    let math_atan_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.atan().to_bits() as i64
    });
    let math_atanh_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.atanh().to_bits() as i64
    });
    let math_atan2_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, y: i64, x: i64| -> i64 {
        let yf = f64::from_bits(y as u64);
        let xf = f64::from_bits(x as u64);
        yf.atan2(xf).to_bits() as i64
    });
    let math_cbrt_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.cbrt().to_bits() as i64
    });
    let math_ceil_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.ceil().to_bits() as i64
    });
    let math_clz32_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        let n = (xf as u32).leading_zeros();
        (n as f64).to_bits() as i64
    });
    let math_cos_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.cos().to_bits() as i64
    });
    let math_cosh_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.cosh().to_bits() as i64
    });
    let math_exp_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.exp().to_bits() as i64
    });
    let math_expm1_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.exp_m1().to_bits() as i64
    });
    let math_floor_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.floor().to_bits() as i64
    });
    let math_fround_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        ((xf as f32) as f64).to_bits() as i64
    });
    let math_hypot_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
        let mut sum = 0.0f64;
        for i in 0..args_count as u32 {
            let val = read_shadow_arg(&mut caller, args_base, i);
            let vf = f64::from_bits(val as u64);
            sum += vf * vf;
        }
        sum.sqrt().to_bits() as i64
    });
    let math_imul_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let af = f64::from_bits(a as u64) as i32;
        let bf = f64::from_bits(b as u64) as i32;
        ((af.wrapping_mul(bf)) as f64).to_bits() as i64
    });
    let math_log_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.ln().to_bits() as i64
    });
    let math_log1p_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.ln_1p().to_bits() as i64
    });
    let math_log10_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.log10().to_bits() as i64
    });
    let math_log2_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.log2().to_bits() as i64
    });
    let math_max_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
        if args_count == 0 {
            return f64::NEG_INFINITY.to_bits() as i64;
        }
        let mut max_val = f64::NEG_INFINITY;
        for i in 0..args_count as u32 {
            let val = read_shadow_arg(&mut caller, args_base, i);
            let vf = f64::from_bits(val as u64);
            if vf.is_nan() {
                return val;
            }
            if vf > max_val || (vf == 0.0 && max_val == 0.0 && vf.to_bits() != max_val.to_bits()) {
                max_val = vf;
            }
        }
        max_val.to_bits() as i64
    });
    let math_min_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
        if args_count == 0 {
            return f64::INFINITY.to_bits() as i64;
        }
        let mut min_val = f64::INFINITY;
        for i in 0..args_count as u32 {
            let val = read_shadow_arg(&mut caller, args_base, i);
            let vf = f64::from_bits(val as u64);
            if vf.is_nan() {
                return val;
            }
            if vf < min_val || (vf == 0.0 && min_val == 0.0 && vf.to_bits() != min_val.to_bits()) {
                min_val = vf;
            }
        }
        min_val.to_bits() as i64
    });
    let math_pow_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, base: i64, exp: i64| -> i64 {
        let bf = f64::from_bits(base as u64);
        let ef = f64::from_bits(exp as u64);
        bf.powf(ef).to_bits() as i64
    });
    let math_random_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| -> i64 {
        let mut rng = rand::thread_rng();
        rng.gen::<f64>().to_bits() as i64
    });
    let math_round_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.round().to_bits() as i64
    });
    let math_sign_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        if xf.is_nan() {
            x.to_bits() as i64
        } else if xf == 0.0 {
            xf.to_bits() as i64
        } else if xf.is_sign_positive() {
            1.0f64.to_bits() as i64
        } else {
            (-1.0f64).to_bits() as i64
        }
    });
    let math_sin_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.sin().to_bits() as i64
    });
    let math_sinh_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.sinh().to_bits() as i64
    });
    let math_sqrt_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.sqrt().to_bits() as i64
    });
    let math_tan_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.tan().to_bits() as i64
    });
    let math_tanh_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.tanh().to_bits() as i64
    });
    let math_trunc_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, x: i64| -> i64 {
        let xf = f64::from_bits(x as u64);
        xf.trunc().to_bits() as i64
    });
```

- [ ] **Step 3: Add Math imports to the imports array**

Find the `let imports = [` array (around line 7199). Add after the last entry (string_from_code_point, index 192):

```rust
        // ── Math imports (indices 193-227) ──
        math_abs_fn.into(),          // 193
        math_acos_fn.into(),         // 194
        math_acosh_fn.into(),        // 195
        math_asin_fn.into(),         // 196
        math_asinh_fn.into(),        // 197
        math_atan_fn.into(),         // 198
        math_atanh_fn.into(),        // 199
        math_atan2_fn.into(),        // 200
        math_cbrt_fn.into(),         // 201
        math_ceil_fn.into(),         // 202
        math_clz32_fn.into(),        // 203
        math_cos_fn.into(),          // 204
        math_cosh_fn.into(),         // 205
        math_exp_fn.into(),          // 206
        math_expm1_fn.into(),        // 207
        math_floor_fn.into(),        // 208
        math_fround_fn.into(),       // 209
        math_hypot_fn.into(),        // 210
        math_imul_fn.into(),         // 211
        math_log_fn.into(),          // 212
        math_log1p_fn.into(),        // 213
        math_log10_fn.into(),        // 214
        math_log2_fn.into(),         // 215
        math_max_fn.into(),          // 216
        math_min_fn.into(),          // 217
        math_pow_fn.into(),          // 218
        math_random_fn.into(),       // 219
        math_round_fn.into(),        // 220
        math_sign_fn.into(),         // 221
        math_sin_fn.into(),          // 222
        math_sinh_fn.into(),         // 223
        math_sqrt_fn.into(),         // 224
        math_tan_fn.into(),          // 225
        math_tanh_fn.into(),         // 226
        math_trunc_fn.into(),        // 227
```

- [ ] **Step 4: Add Math constants to main() initialization in WASM backend**

Find the global constant initialization in the semantic layer's `lower_module` method (around line 1596, in `crates/wjsm-semantic/src/lib.rs`). After the Infinity initialization, add:

```rust
        // Math constants
        let math_constants: [(&str, f64); 8] = [
            ("$0.Math_E", std::f64::consts::E),
            ("$0.Math_LN10", std::f64::consts::LN_10),
            ("$0.Math_LN2", std::f64::consts::LN_2),
            ("$0.Math_LOG10E", std::f64::consts::LOG10_E),
            ("$0.Math_LOG2E", std::f64::consts::LOG2_E),
            ("$0.Math_PI", std::f64::consts::PI),
            ("$0.Math_SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
            ("$0.Math_SQRT2", std::f64::consts::SQRT_2),
        ];
        for (name, value) in &math_constants {
            let c = self.module.add_constant(Constant::Number(*value));
            let v = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const { dest: v, constant: c },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar { name: name.to_string(), value: v },
            );
        }
```

- [ ] **Step 5: Full build check**

Run: `cargo check`
Expected: compiles successfully across all crates

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/lib.rs crates/wjsm-semantic/src/lib.rs
git commit -m "feat(runtime): implement all Math host functions and constants"
```

---

### Task 5: Add Math test fixtures

**Files:**
- Create: `fixtures/happy/math_abs.js`
- Create: `fixtures/happy/math_abs.expected`
- Create: `fixtures/happy/math_trig.js`
- Create: `fixtures/happy/math_trig.expected`
- Create: `fixtures/happy/math_min_max.js`
- Create: `fixtures/happy/math_min_max.expected`
- Create: `fixtures/happy/math_random.js`
- Create: `fixtures/happy/math_random.expected`
- Create: `fixtures/happy/math_constants.js`
- Create: `fixtures/happy/math_constants.expected`

- [ ] **Step 1: Write math_abs test**

`fixtures/happy/math_abs.js`:
```js
console.log(Math.abs(-5));
console.log(Math.abs(3.14));
console.log(Math.abs(-0));
console.log(Math.abs(0));
console.log(Math.abs(-Infinity));
```

`fixtures/happy/math_abs.expected`:
```
5
3.14
0
0
Infinity
```

- [ ] **Step 2: Write math_trig test**

`fixtures/happy/math_trig.js`:
```js
console.log(Math.sin(0));
console.log(Math.cos(0));
console.log(Math.tan(0));
console.log(Math.floor(Math.sin(Math.PI / 2)));
console.log(Math.floor(Math.cos(Math.PI)));
console.log(Math.sqrt(16));
```

`fixtures/happy/math_trig.expected`:
```
0
1
0
1
-1
4
```

- [ ] **Step 3: Write math_min_max test**

`fixtures/happy/math_min_max.js`:
```js
console.log(Math.max(1, 2, 3));
console.log(Math.min(1, 2, 3));
console.log(Math.max(-1, -2, -3));
console.log(Math.min(-1, -2, -3));
```

`fixtures/happy/math_min_max.expected`:
```
3
1
-1
-3
```

- [ ] **Step 4: Write math_random test**

`fixtures/happy/math_random.js`:
```js
var r = Math.random();
console.log(r >= 0 && r < 1);
```

`fixtures/happy/math_random.expected`:
```
true
```

- [ ] **Step 5: Write math_constants test**

`fixtures/happy/math_constants.js`:
```js
console.log(Math.PI > 3.14 && Math.PI < 3.15);
console.log(Math.E > 2.71 && Math.E < 2.72);
console.log(Math.SQRT2 > 1.41 && Math.SQRT2 < 1.42);
```

`fixtures/happy/math_constants.expected`:
```
true
true
true
```

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: all math fixture tests pass

- [ ] **Step 7: Commit**

```bash
git add fixtures/happy/math_abs.js fixtures/happy/math_abs.expected \
        fixtures/happy/math_trig.js fixtures/happy/math_trig.expected \
        fixtures/happy/math_min_max.js fixtures/happy/math_min_max.expected \
        fixtures/happy/math_random.js fixtures/happy/math_random.expected \
        fixtures/happy/math_constants.js fixtures/happy/math_constants.expected
git commit -m "test: add Math built-in test fixtures"
```

---

### Task 6: Run full test suite and fix regressions

- [ ] **Step 1: Run all existing tests**

Run: `cargo test`
Expected: all existing tests still pass, no regressions

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy --all-targets`
Expected: no new warnings

- [ ] **Step 3: Fix any issues found**

If any test fails or clippy warning appears, fix and re-run.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore: fix any test regressions from Math implementation"
```