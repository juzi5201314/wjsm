# ES Builtins Phase 5: Date — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the complete ECMAScript `Date` built-in object in the wjsm JavaScript engine.

**Architecture:** Date stores a single f64 value representing milliseconds since Unix epoch (1970-01-01T00:00:00.000Z), compatible with NaN-boxing. All getter/setter methods decode/encode this timestamp into year/month/day/hour/minute/second/millisecond components using the `chrono` crate. `Date.now()` returns the current system time. `Date.parse()` and `Date.UTC()` are static methods. The constructor supports multiple overloads: `new Date()`, `new Date(value)`, `new Date(dateString)`, `new Date(year, month, ...)`.

**Tech Stack:** Rust, wasmtime, chrono, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-runtime/Cargo.toml` — add chrono dependency
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_global_ident, builtin_from_static_member
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — Date host functions + imports

**Design decisions:**
- Date objects store timestamp as f64 in milliseconds. `NaN` timestamp means invalid date.
- Use `chrono` crate for calendar calculations (year/month/day/hour/minute/second/millisecond, day of week, timezone offset)
- All Date.prototype.get* methods return local time values; getUTC* methods return UTC values
- `Date.prototype.getTimezoneOffset()` returns minutes west of UTC using chrono's local time offset
- `Date.prototype.toString()` / `toDateString()` / `toTimeString()` / `toISOString()` / `toUTCString()` / `toJSON()` use chrono formatting
- `Date.now()` uses `std::time::SystemTime::now()` (already imported)
- `Date.parse()` for MVP: handle ISO 8601 format (YYYY-MM-DDTHH:mm:ss.sssZ) using chrono::NaiveDateTime::parse_from_str
- `Date.UTC(year, month, ...)` constructs a timestamp from UTC components
- Constructor with multiple args: `new Date(year, month, day, hours, minutes, seconds, ms)` — month is 0-indexed

---

### Task 0: Add chrono dependency

**Files:**
- Modify: `crates/wjsm-runtime/Cargo.toml`

- [ ] **Step 1: Add chrono to runtime Cargo.toml**

```toml
chrono = "0.4"
```

- [ ] **Step 2: Verify build**

Run: `cargo check -p wjsm-runtime`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-runtime/Cargo.toml Cargo.lock
git commit -m "chore: add chrono dependency for Date implementation"
```

---

### Task 1: Add Date Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add Date variants to Builtin enum**

After the last Set variant:

```rust
    // ── Date constructor and methods ────────────────────────────────────
    DateConstructor,
    DateNow,
    DateParse,
    DateUTC,
    DateProtoGetDate,
    DateProtoGetDay,
    DateProtoGetFullYear,
    DateProtoGetHours,
    DateProtoGetMilliseconds,
    DateProtoGetMinutes,
    DateProtoGetMonth,
    DateProtoGetSeconds,
    DateProtoGetTime,
    DateProtoGetTimezoneOffset,
    DateProtoGetUTCDate,
    DateProtoGetUTCDay,
    DateProtoGetUTCFullYear,
    DateProtoGetUTCHours,
    DateProtoGetUTCMilliseconds,
    DateProtoGetUTCMinutes,
    DateProtoGetUTCMonth,
    DateProtoGetUTCSeconds,
    DateProtoSetDate,
    DateProtoSetFullYear,
    DateProtoSetHours,
    DateProtoSetMilliseconds,
    DateProtoSetMinutes,
    DateProtoSetMonth,
    DateProtoSetSeconds,
    DateProtoSetTime,
    DateProtoSetUTCDate,
    DateProtoSetUTCFullYear,
    DateProtoSetUTCHours,
    DateProtoSetUTCMilliseconds,
    DateProtoSetUTCMinutes,
    DateProtoSetUTCMonth,
    DateProtoSetUTCSeconds,
    DateProtoToDateString,
    DateProtoToISOString,
    DateProtoToJSON,
    DateProtoToString,
    DateProtoToTimeString,
    DateProtoToUTCString,
    DateProtoValueOf,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::DateConstructor => "Date",
            Self::DateNow => "Date.now",
            Self::DateParse => "Date.parse",
            Self::DateUTC => "Date.UTC",
            Self::DateProtoGetDate => "Date.prototype.getDate",
            Self::DateProtoGetDay => "Date.prototype.getDay",
            Self::DateProtoGetFullYear => "Date.prototype.getFullYear",
            Self::DateProtoGetHours => "Date.prototype.getHours",
            Self::DateProtoGetMilliseconds => "Date.prototype.getMilliseconds",
            Self::DateProtoGetMinutes => "Date.prototype.getMinutes",
            Self::DateProtoGetMonth => "Date.prototype.getMonth",
            Self::DateProtoGetSeconds => "Date.prototype.getSeconds",
            Self::DateProtoGetTime => "Date.prototype.getTime",
            Self::DateProtoGetTimezoneOffset => "Date.prototype.getTimezoneOffset",
            Self::DateProtoGetUTCDate => "Date.prototype.getUTCDate",
            Self::DateProtoGetUTCDay => "Date.prototype.getUTCDay",
            Self::DateProtoGetUTCFullYear => "Date.prototype.getUTCFullYear",
            Self::DateProtoGetUTCHours => "Date.prototype.getUTCHours",
            Self::DateProtoGetUTCMilliseconds => "Date.prototype.getUTCMilliseconds",
            Self::DateProtoGetUTCMinutes => "Date.prototype.getUTCMinutes",
            Self::DateProtoGetUTCMonth => "Date.prototype.getUTCMonth",
            Self::DateProtoGetUTCSeconds => "Date.prototype.getUTCSeconds",
            Self::DateProtoSetDate => "Date.prototype.setDate",
            Self::DateProtoSetFullYear => "Date.prototype.setFullYear",
            Self::DateProtoSetHours => "Date.prototype.setHours",
            Self::DateProtoSetMilliseconds => "Date.prototype.setMilliseconds",
            Self::DateProtoSetMinutes => "Date.prototype.setMinutes",
            Self::DateProtoSetMonth => "Date.prototype.setMonth",
            Self::DateProtoSetSeconds => "Date.prototype.setSeconds",
            Self::DateProtoSetTime => "Date.prototype.setTime",
            Self::DateProtoSetUTCDate => "Date.prototype.setUTCDate",
            Self::DateProtoSetUTCFullYear => "Date.prototype.setUTCFullYear",
            Self::DateProtoSetUTCHours => "Date.prototype.setUTCHours",
            Self::DateProtoSetUTCMilliseconds => "Date.prototype.setUTCMilliseconds",
            Self::DateProtoSetUTCMinutes => "Date.prototype.setUTCMinutes",
            Self::DateProtoSetUTCMonth => "Date.prototype.setUTCMonth",
            Self::DateProtoSetUTCSeconds => "Date.prototype.setUTCSeconds",
            Self::DateProtoToDateString => "Date.prototype.toDateString",
            Self::DateProtoToISOString => "Date.prototype.toISOString",
            Self::DateProtoToJSON => "Date.prototype.toJSON",
            Self::DateProtoToString => "Date.prototype.toString",
            Self::DateProtoToTimeString => "Date.prototype.toTimeString",
            Self::DateProtoToUTCString => "Date.prototype.toUTCString",
            Self::DateProtoValueOf => "Date.prototype.valueOf",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add Date builtin variants"
```

---

### Task 2: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add Date as global ident and static member**

In `builtin_from_global_ident`:
```rust
        "Date" => Some(Builtin::DateConstructor),
```

In `builtin_from_static_member`, add under the `"Set"` or last arm:
```rust
        "Date" => match property {
            "now" => Some(Builtin::DateNow),
            "parse" => Some(Builtin::DateParse),
            "UTC" => Some(Builtin::DateUTC),
            _ => None,
        },
```

- [ ] **Step 2: Add Date prototype method helper**

```rust
fn builtin_from_date_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "getDate" => Some(DateProtoGetDate),
        "getDay" => Some(DateProtoGetDay),
        "getFullYear" => Some(DateProtoGetFullYear),
        "getHours" => Some(DateProtoGetHours),
        "getMilliseconds" => Some(DateProtoGetMilliseconds),
        "getMinutes" => Some(DateProtoGetMinutes),
        "getMonth" => Some(DateProtoGetMonth),
        "getSeconds" => Some(DateProtoGetSeconds),
        "getTime" => Some(DateProtoGetTime),
        "getTimezoneOffset" => Some(DateProtoGetTimezoneOffset),
        "getUTCDate" => Some(DateProtoGetUTCDate),
        "getUTCDay" => Some(DateProtoGetUTCDay),
        "getUTCFullYear" => Some(DateProtoGetUTCFullYear),
        "getUTCHours" => Some(DateProtoGetUTCHours),
        "getUTCMilliseconds" => Some(DateProtoGetUTCMilliseconds),
        "getUTCMinutes" => Some(DateProtoGetUTCMinutes),
        "getUTCMonth" => Some(DateProtoGetUTCMonth),
        "getUTCSeconds" => Some(DateProtoGetUTCSeconds),
        "setDate" => Some(DateProtoSetDate),
        "setFullYear" => Some(DateProtoSetFullYear),
        "setHours" => Some(DateProtoSetHours),
        "setMilliseconds" => Some(DateProtoSetMilliseconds),
        "setMinutes" => Some(DateProtoSetMinutes),
        "setMonth" => Some(DateProtoSetMonth),
        "setSeconds" => Some(DateProtoSetSeconds),
        "setTime" => Some(DateProtoSetTime),
        "setUTCDate" => Some(DateProtoSetUTCDate),
        "setUTCFullYear" => Some(DateProtoSetUTCFullYear),
        "setUTCHours" => Some(DateProtoSetUTCHours),
        "setUTCMilliseconds" => Some(DateProtoSetUTCMilliseconds),
        "setUTCMinutes" => Some(DateProtoSetUTCMinutes),
        "setUTCMonth" => Some(DateProtoSetUTCMonth),
        "setUTCSeconds" => Some(DateProtoSetUTCSeconds),
        "toDateString" => Some(DateProtoToDateString),
        "toISOString" => Some(DateProtoToISOString),
        "toJSON" => Some(DateProtoToJSON),
        "toString" => Some(DateProtoToString),
        "toTimeString" => Some(DateProtoToTimeString),
        "toUTCString" => Some(DateProtoToUTCString),
        "valueOf" => Some(DateProtoValueOf),
        _ => None,
    }
}
```

- [ ] **Step 3: Add Date prototype call optimization in lower_call_expr**

After the Set.prototype handling block, add a Date.prototype block.

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add Date call recognition"
```

---

### Task 3: Register WASM types and imports for Date

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM types for Date (variadic constructor)**

After Type 24:
```rust
        // Type 25: (i32, i32) -> (i64) — Date constructor with multiple args via shadow stack
        types.ty().function(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
```

- [ ] **Step 2: Add import declarations (indices 272-313)**

```rust
        // ── Date imports (indices 272-313) ──
        // Static methods
        imports.import("env", "date_now", EntityType::Function(22));           // () -> i64
        imports.import("env", "date_parse", EntityType::Function(3));          // (i64) -> i64
        imports.import("env", "date_utc", EntityType::Function(25));           // (i32,i32) -> i64
        // Constructor: (i32,i32) -> i64 via shadow stack (args: up to 7 values)
        imports.import("env", "date_constructor", EntityType::Function(25));
        // Getter methods: (i64 receiver) -> i64
        imports.import("env", "date_proto_get_date", EntityType::Function(3));
        imports.import("env", "date_proto_get_day", EntityType::Function(3));
        imports.import("env", "date_proto_get_full_year", EntityType::Function(3));
        imports.import("env", "date_proto_get_hours", EntityType::Function(3));
        imports.import("env", "date_proto_get_milliseconds", EntityType::Function(3));
        imports.import("env", "date_proto_get_minutes", EntityType::Function(3));
        imports.import("env", "date_proto_get_month", EntityType::Function(3));
        imports.import("env", "date_proto_get_seconds", EntityType::Function(3));
        imports.import("env", "date_proto_get_time", EntityType::Function(3));
        imports.import("env", "date_proto_get_timezone_offset", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_date", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_day", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_full_year", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_hours", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_milliseconds", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_minutes", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_month", EntityType::Function(3));
        imports.import("env", "date_proto_get_utc_seconds", EntityType::Function(3));
        // Setter methods: (i64 receiver, i64 value) -> i64
        imports.import("env", "date_proto_set_date", EntityType::Function(2));
        imports.import("env", "date_proto_set_full_year", EntityType::Function(2));
        imports.import("env", "date_proto_set_hours", EntityType::Function(2));
        imports.import("env", "date_proto_set_milliseconds", EntityType::Function(2));
        imports.import("env", "date_proto_set_minutes", EntityType::Function(2));
        imports.import("env", "date_proto_set_month", EntityType::Function(2));
        imports.import("env", "date_proto_set_seconds", EntityType::Function(2));
        imports.import("env", "date_proto_set_time", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_date", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_full_year", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_hours", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_milliseconds", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_minutes", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_month", EntityType::Function(2));
        imports.import("env", "date_proto_set_utc_seconds", EntityType::Function(2));
        // toString methods: (i64 receiver) -> i64
        imports.import("env", "date_proto_to_date_string", EntityType::Function(3));
        imports.import("env", "date_proto_to_iso_string", EntityType::Function(3));
        imports.import("env", "date_proto_to_json", EntityType::Function(3));
        imports.import("env", "date_proto_to_string", EntityType::Function(3));
        imports.import("env", "date_proto_to_time_string", EntityType::Function(3));
        imports.import("env", "date_proto_to_utc_string", EntityType::Function(3));
        imports.import("env", "date_proto_value_of", EntityType::Function(3));
```

- [ ] **Step 3: Add builtin_arity entries**

```rust
        Builtin::DateConstructor => ("date_constructor", 7),
        Builtin::DateNow => ("date_now", 0),
        Builtin::DateParse => ("date_parse", 1),
        Builtin::DateUTC => ("date_utc", 7),
        Builtin::DateProtoGetDate => ("date_proto_get_date", 1),
        Builtin::DateProtoGetDay => ("date_proto_get_day", 1),
        Builtin::DateProtoGetFullYear => ("date_proto_get_full_year", 1),
        Builtin::DateProtoGetHours => ("date_proto_get_hours", 1),
        Builtin::DateProtoGetMilliseconds => ("date_proto_get_milliseconds", 1),
        Builtin::DateProtoGetMinutes => ("date_proto_get_minutes", 1),
        Builtin::DateProtoGetMonth => ("date_proto_get_month", 1),
        Builtin::DateProtoGetSeconds => ("date_proto_get_seconds", 1),
        Builtin::DateProtoGetTime => ("date_proto_get_time", 1),
        Builtin::DateProtoGetTimezoneOffset => ("date_proto_get_timezone_offset", 1),
        Builtin::DateProtoGetUTCDate => ("date_proto_get_utc_date", 1),
        Builtin::DateProtoGetUTCDay => ("date_proto_get_utc_day", 1),
        Builtin::DateProtoGetUTCFullYear => ("date_proto_get_utc_full_year", 1),
        Builtin::DateProtoGetUTCHours => ("date_proto_get_utc_hours", 1),
        Builtin::DateProtoGetUTCMilliseconds => ("date_proto_get_utc_milliseconds", 1),
        Builtin::DateProtoGetUTCMinutes => ("date_proto_get_utc_minutes", 1),
        Builtin::DateProtoGetUTCMonth => ("date_proto_get_utc_month", 1),
        Builtin::DateProtoGetUTCSeconds => ("date_proto_get_utc_seconds", 1),
        Builtin::DateProtoSetDate => ("date_proto_set_date", 2),
        Builtin::DateProtoSetFullYear => ("date_proto_set_full_year", 2),
        Builtin::DateProtoSetHours => ("date_proto_set_hours", 2),
        Builtin::DateProtoSetMilliseconds => ("date_proto_set_milliseconds", 2),
        Builtin::DateProtoSetMinutes => ("date_proto_set_minutes", 2),
        Builtin::DateProtoSetMonth => ("date_proto_set_month", 2),
        Builtin::DateProtoSetSeconds => ("date_proto_set_seconds", 2),
        Builtin::DateProtoSetTime => ("date_proto_set_time", 2),
        Builtin::DateProtoSetUTCDate => ("date_proto_set_utc_date", 2),
        Builtin::DateProtoSetUTCFullYear => ("date_proto_set_utc_full_year", 2),
        Builtin::DateProtoSetUTCHours => ("date_proto_set_utc_hours", 2),
        Builtin::DateProtoSetUTCMilliseconds => ("date_proto_set_utc_milliseconds", 2),
        Builtin::DateProtoSetUTCMinutes => ("date_proto_set_utc_minutes", 2),
        Builtin::DateProtoSetUTCMonth => ("date_proto_set_utc_month", 2),
        Builtin::DateProtoSetUTCSeconds => ("date_proto_set_utc_seconds", 2),
        Builtin::DateProtoToDateString => ("date_proto_to_date_string", 1),
        Builtin::DateProtoToISOString => ("date_proto_to_iso_string", 1),
        Builtin::DateProtoToJSON => ("date_proto_to_json", 1),
        Builtin::DateProtoToString => ("date_proto_to_string", 1),
        Builtin::DateProtoToTimeString => ("date_proto_to_time_string", 1),
        Builtin::DateProtoToUTCString => ("date_proto_to_utc_string", 1),
        Builtin::DateProtoValueOf => ("date_proto_value_of", 1),
```

- [ ] **Step 4: Add builtin_func_indices entries (272-313)**

```rust
        builtin_func_indices.insert(Builtin::DateNow, 272);
        builtin_func_indices.insert(Builtin::DateParse, 273);
        builtin_func_indices.insert(Builtin::DateUTC, 274);
        builtin_func_indices.insert(Builtin::DateConstructor, 275);
        builtin_func_indices.insert(Builtin::DateProtoGetDate, 276);
        builtin_func_indices.insert(Builtin::DateProtoGetDay, 277);
        builtin_func_indices.insert(Builtin::DateProtoGetFullYear, 278);
        builtin_func_indices.insert(Builtin::DateProtoGetHours, 279);
        builtin_func_indices.insert(Builtin::DateProtoGetMilliseconds, 280);
        builtin_func_indices.insert(Builtin::DateProtoGetMinutes, 281);
        builtin_func_indices.insert(Builtin::DateProtoGetMonth, 282);
        builtin_func_indices.insert(Builtin::DateProtoGetSeconds, 283);
        builtin_func_indices.insert(Builtin::DateProtoGetTime, 284);
        builtin_func_indices.insert(Builtin::DateProtoGetTimezoneOffset, 285);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCDate, 286);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCDay, 287);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCFullYear, 288);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCHours, 289);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCMilliseconds, 290);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCMinutes, 291);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCMonth, 292);
        builtin_func_indices.insert(Builtin::DateProtoGetUTCSeconds, 293);
        builtin_func_indices.insert(Builtin::DateProtoSetDate, 294);
        builtin_func_indices.insert(Builtin::DateProtoSetFullYear, 295);
        builtin_func_indices.insert(Builtin::DateProtoSetHours, 296);
        builtin_func_indices.insert(Builtin::DateProtoSetMilliseconds, 297);
        builtin_func_indices.insert(Builtin::DateProtoSetMinutes, 298);
        builtin_func_indices.insert(Builtin::DateProtoSetMonth, 299);
        builtin_func_indices.insert(Builtin::DateProtoSetSeconds, 300);
        builtin_func_indices.insert(Builtin::DateProtoSetTime, 301);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCDate, 302);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCFullYear, 303);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCHours, 304);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCMilliseconds, 305);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCMinutes, 306);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCMonth, 307);
        builtin_func_indices.insert(Builtin::DateProtoSetUTCSeconds, 308);
        builtin_func_indices.insert(Builtin::DateProtoToDateString, 309);
        builtin_func_indices.insert(Builtin::DateProtoToISOString, 310);
        builtin_func_indices.insert(Builtin::DateProtoToJSON, 311);
        builtin_func_indices.insert(Builtin::DateProtoToString, 312);
        builtin_func_indices.insert(Builtin::DateProtoToTimeString, 313);
        builtin_func_indices.insert(Builtin::DateProtoToUTCString, 314);
        builtin_func_indices.insert(Builtin::DateProtoValueOf, 315);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register Date WASM imports"
```

---

### Task 4: Implement Date host functions in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add chrono imports**

```rust
use chrono::{DateTime, Datelike, Duration, NaiveDateTime, Timelike, Utc, Local, TimeZone};
```

- [ ] **Step 2: Add Date helper functions**

```rust
fn ms_to_datetime_utc(ms: f64) -> Option<DateTime<Utc>> {
    if ms.is_nan() || ms.is_infinite() { return None; }
    let secs = (ms / 1000.0).trunc() as i64;
    let nsecs = ((ms % 1000.0) * 1_000_000.0) as u32;
    DateTime::from_timestamp(secs, nsecs)
}

fn ms_to_datetime_local(ms: f64) -> Option<DateTime<Local>> {
    ms_to_datetime_utc(ms).map(|utc| utc.with_timezone(&Local))
}

fn make_date(ms: f64) -> i64 {
    if ms.is_nan() { f64::NAN.to_bits() as i64 }
    else { ms.to_bits() as i64 }
}

fn read_shadow_arg_i64(caller: &mut Caller<'_, RuntimeState>, base: i32, idx: u32) -> i64 {
    let mem = caller.get_export("memory").unwrap().into_memory().unwrap();
    let offset = base as usize + idx as usize * 8;
    let mut bytes = [0u8; 8];
    mem.read(caller, offset, &mut bytes).unwrap();
    i64::from_le_bytes(bytes)
}
```

- [ ] **Step 3: Implement Date host functions**

Insert before the `let imports = [` line:

```rust
    // ── Date host functions ──────────────────────────────────────────────
    let date_now_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>| -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        (now.as_millis() as f64).to_bits() as i64
    });
    let date_parse_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, date_str: i64| -> i64 {
        let s = if let Some(bytes) = read_value_string_bytes(&mut caller, date_str) {
            String::from_utf8_lossy(&bytes).to_string()
        } else {
            return f64::NAN.to_bits() as i64;
        };
        // Try ISO 8601: "YYYY-MM-DDTHH:mm:ss.sssZ"
        if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.fZ") {
            let ms = dt.and_utc().timestamp_millis() as f64;
            return ms.to_bits() as i64;
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%SZ") {
            let ms = dt.and_utc().timestamp_millis() as f64;
            return ms.to_bits() as i64;
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d") {
            let ms = dt.and_utc().timestamp_millis() as f64;
            return ms.to_bits() as i64;
        }
        f64::NAN.to_bits() as i64
    });
    let date_utc_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
        let n = args_count.min(7) as u32;
        let mut vals = [0.0f64; 7];
        for i in 0..n {
            vals[i as usize] = f64::from_bits(read_shadow_arg_i64(&mut caller, args_base, i) as u64);
        }
        // Date.UTC(year, month, day=1, hours=0, minutes=0, seconds=0, ms=0)
        let year = vals[0] as i32;
        let month = vals.get(1).copied().unwrap_or(0.0) as u32;
        let day = vals.get(2).copied().unwrap_or(1.0) as u32;
        let hour = vals.get(3).copied().unwrap_or(0.0) as u32;
        let min = vals.get(4).copied().unwrap_or(0.0) as u32;
        let sec = vals.get(5).copied().unwrap_or(0.0) as u32;
        let ms = vals.get(6).copied().unwrap_or(0.0) as u32;
        if let Some(dt) = Utc.with_ymd_and_hms(year, month + 1, day, hour, min, sec).single() {
            let ts = dt.timestamp_millis() as f64 + ms as f64;
            ts.to_bits() as i64
        } else {
            f64::NAN.to_bits() as i64
        }
    });
    let date_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
        if args_count == 0 {
            return date_now_fn.call(&mut caller, &[], &mut []).unwrap();
        }
        if args_count == 1 {
            let arg = read_shadow_arg_i64(&mut caller, args_base, 0);
            if value::is_string(arg) {
                return date_parse_fn.call(&mut caller, &[wasmtime::Val::I64(arg)], &mut []).unwrap();
            }
            let ts = f64::from_bits(arg as u64);
            return make_date(ts);
        }
        // Multiple args: year, month, day, ...
        date_utc_fn.call(&mut caller, &[wasmtime::Val::I32(args_base), wasmtime::Val::I32(args_count)], &mut []).unwrap()
    });
    // Getters — local time
    let date_proto_get_date_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { (dt.day() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_day_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { (dt.weekday().num_days_from_sunday() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_full_year_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { (dt.year() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_hours_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { (dt.hour() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_milliseconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { ((dt.nanosecond() / 1_000_000) as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_minutes_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { (dt.minute() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_month_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { ((dt.month0()) as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_seconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) { (dt.second() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_time_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        receiver
    });
    let date_proto_get_timezone_offset_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_local(ms) {
            let offset = dt.offset().local_minus_utc() / 60;
            ((-offset) as f64).to_bits() as i64
        } else { receiver }
    });
    // Getters — UTC
    let date_proto_get_utc_date_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { (dt.day() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_day_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { (dt.weekday().num_days_from_sunday() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_full_year_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { (dt.year() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_hours_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { (dt.hour() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_milliseconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { ((dt.nanosecond() / 1_000_000) as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_minutes_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { (dt.minute() as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_month_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { ((dt.month0()) as f64).to_bits() as i64 } else { receiver }
    });
    let date_proto_get_utc_seconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) { (dt.second() as f64).to_bits() as i64 } else { receiver }
    });
    // Setters — simplified: return new timestamp
    let date_proto_set_time_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, _receiver: i64, time: i64| -> i64 {
        time
    });
    let date_proto_set_full_year_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, year: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let y = f64::from_bits(year as u64) as i32;
        if let Some(dt) = ms_to_datetime_local(ms) {
            if let Some(new_dt) = dt.with_year(y) {
                return (new_dt.timestamp_millis() as f64 + (dt.nanosecond() % 1_000_000) as f64 / 1_000_000.0).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_month_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, month: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let m = f64::from_bits(month as u64) as u32;
        if let Some(dt) = ms_to_datetime_local(ms) {
            if let Some(new_dt) = dt.with_month0(m) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_date_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, date: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let d = f64::from_bits(date as u64) as u32;
        if let Some(dt) = ms_to_datetime_local(ms) {
            if let Some(new_dt) = dt.with_day(d) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_hours_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, hours: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let h = f64::from_bits(hours as u64) as u32;
        if let Some(dt) = ms_to_datetime_local(ms) {
            if let Some(new_dt) = dt.with_hour(h) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_minutes_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, minutes: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let m = f64::from_bits(minutes as u64) as u32;
        if let Some(dt) = ms_to_datetime_local(ms) {
            if let Some(new_dt) = dt.with_minute(m) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_seconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, seconds: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let s = f64::from_bits(seconds as u64) as u32;
        if let Some(dt) = ms_to_datetime_local(ms) {
            if let Some(new_dt) = dt.with_second(s) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_milliseconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, ms_val: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let extra_ms = f64::from_bits(ms_val as u64);
        if let Some(dt) = ms_to_datetime_local(ms) {
            let new_ms = dt.timestamp_millis() as f64 - (dt.nanosecond() as f64 / 1_000_000.0) + extra_ms;
            return new_ms.to_bits() as i64;
        }
        receiver
    });
    // UTC setters — similar pattern
    let date_proto_set_utc_full_year_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, year: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let y = f64::from_bits(year as u64) as i32;
        if let Some(dt) = ms_to_datetime_utc(ms) {
            if let Some(new_dt) = dt.with_year(y) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_utc_month_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, month: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let m = f64::from_bits(month as u64) as u32;
        if let Some(dt) = ms_to_datetime_utc(ms) {
            if let Some(new_dt) = dt.with_month0(m) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_utc_date_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, date: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let d = f64::from_bits(date as u64) as u32;
        if let Some(dt) = ms_to_datetime_utc(ms) {
            if let Some(new_dt) = dt.with_day(d) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_utc_hours_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, hours: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let h = f64::from_bits(hours as u64) as u32;
        if let Some(dt) = ms_to_datetime_utc(ms) {
            if let Some(new_dt) = dt.with_hour(h) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_utc_minutes_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, minutes: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let m = f64::from_bits(minutes as u64) as u32;
        if let Some(dt) = ms_to_datetime_utc(ms) {
            if let Some(new_dt) = dt.with_minute(m) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_utc_seconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, seconds: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let s = f64::from_bits(seconds as u64) as u32;
        if let Some(dt) = ms_to_datetime_utc(ms) {
            if let Some(new_dt) = dt.with_second(s) {
                return (new_dt.timestamp_millis() as f64).to_bits() as i64;
            }
        }
        receiver
    });
    let date_proto_set_utc_milliseconds_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64, ms_val: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        let extra_ms = f64::from_bits(ms_val as u64);
        if let Some(dt) = ms_to_datetime_utc(ms) {
            let new_ms = dt.timestamp_millis() as f64 - (dt.nanosecond() as f64 / 1_000_000.0) + extra_ms;
            return new_ms.to_bits() as i64;
        }
        receiver
    });
    // toString methods
    let date_proto_to_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if ms.is_nan() { return store_runtime_string_from_str(&mut caller, "Invalid Date"); }
        if let Some(dt) = ms_to_datetime_local(ms) {
            let s = dt.format("%a %b %d %Y %H:%M:%S GMT%z").to_string();
            store_runtime_string_from_str(&mut caller, &s)
        } else {
            store_runtime_string_from_str(&mut caller, "Invalid Date")
        }
    });
    let date_proto_to_date_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if ms.is_nan() { return store_runtime_string_from_str(&mut caller, "Invalid Date"); }
        if let Some(dt) = ms_to_datetime_local(ms) {
            let s = dt.format("%a %b %d %Y").to_string();
            store_runtime_string_from_str(&mut caller, &s)
        } else {
            store_runtime_string_from_str(&mut caller, "Invalid Date")
        }
    });
    let date_proto_to_time_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if ms.is_nan() { return store_runtime_string_from_str(&mut caller, "Invalid Date"); }
        if let Some(dt) = ms_to_datetime_local(ms) {
            let s = dt.format("%H:%M:%S GMT%z").to_string();
            store_runtime_string_from_str(&mut caller, &s)
        } else {
            store_runtime_string_from_str(&mut caller, "Invalid Date")
        }
    });
    let date_proto_to_iso_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if ms.is_nan() { return store_runtime_string_from_str(&mut caller, "Invalid Date"); }
        if let Some(dt) = ms_to_datetime_utc(ms) {
            let s = dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            store_runtime_string_from_str(&mut caller, &s)
        } else {
            store_runtime_string_from_str(&mut caller, "Invalid Date")
        }
    });
    let date_proto_to_utc_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let ms = f64::from_bits(receiver as u64);
        if ms.is_nan() { return store_runtime_string_from_str(&mut caller, "Invalid Date"); }
        if let Some(dt) = ms_to_datetime_utc(ms) {
            let s = dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            store_runtime_string_from_str(&mut caller, &s)
        } else {
            store_runtime_string_from_str(&mut caller, "Invalid Date")
        }
    });
    let date_proto_to_json_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        date_proto_to_iso_string_fn.call(&mut caller, &[wasmtime::Val::I64(receiver)], &mut []).unwrap()
    });
    let date_proto_value_of_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        receiver
    });
```

- [ ] **Step 4: Add imports to the imports array**

After the last Set import (index 271):
```rust
        // ── Date imports (272-315) ──
        date_now_fn.into(),                       // 272
        date_parse_fn.into(),                     // 273
        date_utc_fn.into(),                       // 274
        date_constructor_fn.into(),               // 275
        date_proto_get_date_fn.into(),            // 276
        date_proto_get_day_fn.into(),             // 277
        date_proto_get_full_year_fn.into(),       // 278
        date_proto_get_hours_fn.into(),           // 279
        date_proto_get_milliseconds_fn.into(),    // 280
        date_proto_get_minutes_fn.into(),         // 281
        date_proto_get_month_fn.into(),           // 282
        date_proto_get_seconds_fn.into(),         // 283
        date_proto_get_time_fn.into(),            // 284
        date_proto_get_timezone_offset_fn.into(), // 285
        date_proto_get_utc_date_fn.into(),        // 286
        date_proto_get_utc_day_fn.into(),         // 287
        date_proto_get_utc_full_year_fn.into(),   // 288
        date_proto_get_utc_hours_fn.into(),       // 289
        date_proto_get_utc_milliseconds_fn.into(),// 290
        date_proto_get_utc_minutes_fn.into(),     // 291
        date_proto_get_utc_month_fn.into(),       // 292
        date_proto_get_utc_seconds_fn.into(),     // 293
        date_proto_set_date_fn.into(),            // 294
        date_proto_set_full_year_fn.into(),       // 295
        date_proto_set_hours_fn.into(),           // 296
        date_proto_set_milliseconds_fn.into(),    // 297
        date_proto_set_minutes_fn.into(),         // 298
        date_proto_set_month_fn.into(),           // 299
        date_proto_set_seconds_fn.into(),         // 300
        date_proto_set_time_fn.into(),            // 301
        date_proto_set_utc_date_fn.into(),        // 302
        date_proto_set_utc_full_year_fn.into(),   // 303
        date_proto_set_utc_hours_fn.into(),       // 304
        date_proto_set_utc_milliseconds_fn.into(),// 305
        date_proto_set_utc_minutes_fn.into(),     // 306
        date_proto_set_utc_month_fn.into(),       // 307
        date_proto_set_utc_seconds_fn.into(),     // 308
        date_proto_to_date_string_fn.into(),      // 309
        date_proto_to_iso_string_fn.into(),       // 310
        date_proto_to_json_fn.into(),             // 311
        date_proto_to_string_fn.into(),           // 312
        date_proto_to_time_string_fn.into(),      // 313
        date_proto_to_utc_string_fn.into(),       // 314
        date_proto_value_of_fn.into(),            // 315
```

- [ ] **Step 5: Full build check and commit**

Run: `cargo check`
Expected: compiles

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): implement Date constructor and all prototype methods"
```

---

### Task 5: Add Date test fixtures

**Files:**
- Create: `fixtures/happy/date_basic.js` + `.expected`
- Create: `fixtures/happy/date_now.js` + `.expected`

- [ ] **Step 1: date_basic test**

`fixtures/happy/date_basic.js`:
```js
var d = new Date(2023, 0, 15);
console.log(d.getFullYear());
console.log(d.getMonth());
console.log(d.getDate());
console.log(d.getDay());
```

`fixtures/happy/date_basic.expected`:
```
2023
0
15
0
```

- [ ] **Step 2: date_now test**

`fixtures/happy/date_now.js`:
```js
var now = Date.now();
console.log(now > 0);
```

`fixtures/happy/date_now.expected`:
```
true
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test`
Expected: new fixture tests pass

```bash
git add fixtures/happy/date_basic.js fixtures/happy/date_basic.expected \
        fixtures/happy/date_now.js fixtures/happy/date_now.expected
git commit -m "test: add Date test fixtures"
```