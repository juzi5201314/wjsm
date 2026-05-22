# RegExp 高级特性 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 补全 RegExp 运行时对 ES2018 命名捕获组/lookbehind/Unicode 属性转义 及 ES2022 `d` 标志的支持——所有语法引擎已支持，仅需 runtime 暴露数据。

**Architecture:** 纯 runtime 改动，集中在 `primitive_core.rs`（`regex_create`、`regex_exec`、`string_match`、`string_replace`）和 `string_methods.rs`（`matchAll`）。regress 引擎已原生支持全部目标语法，工作为：在 match 结果上附加 `.index`/`.input`/`.groups`/`.indices` 属性，补全 `$<name>` 替换和函数替换的 `groups` 参数，校验 RegExp 标志。

**Tech Stack:** Rust 2024, regress 0.11, wasmtime 43

---

### Task 1: 标志校验（`regex_create`）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:423-425`

在 `regress::Regex::with_flags()` 调用前插入标志校验，并将仅引擎相关标志（`i`/`m`/`s`/`u`/`v`）传给 regress，`RegexEntry.flags` 保留完整 flags。

- [ ] **Step 1: 添加标志校验 + 过滤引擎标志**

找到 `regex_create_fn` 闭包中 `// 编译正则表达式` 注释行（约 line 424）。将：

```rust
            // 编译正则表达式
            match regress::Regex::with_flags(&pattern, flags.as_str()) {
```

替换为完整校验 + 过滤版本。具体改动见下方完整代码块。

在 `regex_create_fn` 闭包内，`let flags = ...into_owned();`（line 422）之后，`// 编译正则表达式` 注释（line 424）之前，加入校验逻辑。将 `regress::Regex::with_flags(&pattern, flags.as_str())` 改为 `regress::Regex::with_flags(&pattern, &engine_flags)`。

完整改动后的 `regex_create_fn`（lines 382-449）：

```rust
    // ── Import 109: regex_create(i32, i32, i32, i32) → i64 ──────────────────────
    let regex_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         pat_ptr: i32,
         pat_len: i32,
         flags_ptr: i32,
         flags_len: i32|
         -> i64 {
            // 从 WASM 内存读取 pattern 和 flags
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return value::encode_undefined();
            };
            let data = memory.data(&caller);

            let pat_start = pat_ptr as usize;
            let pat_end = (pat_ptr as usize).saturating_add(pat_len as usize);
            if pat_end > data.len() {
                return value::encode_undefined();
            }
            let pat_bytes = &data[pat_start..pat_end];
            let pattern = String::from_utf8_lossy(if pat_bytes.ends_with(&[0]) {
                &pat_bytes[..pat_bytes.len() - 1]
            } else {
                pat_bytes
            })
            .into_owned();

            let flags_start = flags_ptr as usize;
            let flags_end = (flags_ptr as usize).saturating_add(flags_len as usize);
            if flags_end > data.len() {
                return value::encode_undefined();
            }
            let flags_bytes = &data[flags_start..flags_end];
            let flags = String::from_utf8_lossy(if flags_bytes.ends_with(&[0]) {
                &flags_bytes[..flags_bytes.len() - 1]
            } else {
                flags_bytes
            })
            .into_owned();

            // 标志校验
            const VALID_FLAGS: &[char] = &['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'];
            let mut seen = [false; 128u8 as usize];
            for c in flags.chars() {
                if !VALID_FLAGS.contains(&c) {
                    *caller.data().runtime_error.lock().unwrap() =
                        Some(format!(
                            "SyntaxError: Invalid regular expression flag: '{}'",
                            c
                        ));
                    return value::encode_undefined();
                }
                let idx = c as usize;
                if idx < seen.len() {
                    if seen[idx] {
                        *caller.data().runtime_error.lock().unwrap() =
                            Some(format!(
                                "SyntaxError: Duplicate regular expression flag: '{}'",
                                c
                            ));
                        return value::encode_undefined();
                    }
                    seen[idx] = true;
                }
            }

            // 仅将引擎相关标志传给 regress
            let engine_flags: String = flags
                .chars()
                .filter(|c| matches!(c, 'i' | 'm' | 's' | 'u' | 'v'))
                .collect();

            // 编译正则表达式
            match regress::Regex::with_flags(&pattern, &engine_flags) {
                Ok(compiled) => {
                    let mut table = caller.data_mut().regex_table.lock().unwrap();
                    let handle = table.len() as u32;
                    table.push(RegexEntry {
                        pattern,
                        flags,
                        compiled,
                        last_index: 0,
                    });
                    value::encode_regexp_handle(handle)
                }
                Err(e) => {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some(format!("SyntaxError: Invalid regular expression: {}", e));
                    value::encode_undefined()
                }
            }
        },
    );
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过，无错误。

- [ ] **Step 3: 创建错误路径 fixture**

创建 `fixtures/errors/regexp_flags_invalid.js`：
```js
var r = /test/xx;
```

创建 `fixtures/errors/regexp_flags_invalid.expected`：
```
exit_code: 2
--- stdout ---

--- stderr ---
SyntaxError: Invalid regular expression flag: 'x'
```

- [ ] **Step 4: 运行 fixture 验证**

```bash
cargo run -- run fixtures/errors/regexp_flags_invalid.js 2>&1
```
预期：exit code 2，stderr 包含 "Invalid regular expression flag: 'x'"。

- [ ] **Step 5: 验证重复标志**

```bash
cargo run -- run -e '/test/gg' 2>&1
```
预期：exit code 2，stderr 包含 "Duplicate regular expression flag: 'g'"。

- [ ] **Step 6: 更新 .expected（如需要）**

如果 fixture 输出正确，确认 `.expected` 内容匹配。如有差异运行：
```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration 2>&1
```

- [ ] **Step 7: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/primitive_core.rs fixtures/errors/regexp_flags_invalid.js fixtures/errors/regexp_flags_invalid.expected
git commit -m "feat: add RegExp flag validation (invalid/duplicate flags -> SyntaxError)"
```

---

### Task 2: `regex_exec` — 补全 `.index`、`.input`、`.groups`

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:572-589`

在 `regex_exec` 的 match 成功分支中，构建数组后追加属性设置。

- [ ] **Step 1: 在 `regex_exec` 数组构建后追加 `.index`、`.input`、`.groups`**

找到 `regex_exec_fn` 闭包中 `Some(m) => {` 分支（约 line 563）。在 `write_array_length` 调用（line 588）之后、`arr` 返回（line 589）之前，插入属性设置代码。

将 lines 563-590 的 `Some(m) => { ... arr }` 块改为：

```rust
                Some(m) => {
                    // 更新 lastIndex（全局或粘性模式）
                    if is_global || is_sticky {
                        let mut table = caller.data().regex_table.lock().unwrap();
                        if let Some(e) = table.get_mut(handle as usize) {
                            e.last_index = m.end() as i64;
                        }
                    }

                    // 构建结果数组 [full_match, group1, group2, ...]
                    let group_count = m.captures.len() + 1;
                    let arr = alloc_array(&mut caller, group_count as u32);
                    let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                        return value::encode_null();
                    };

                    for i in 0..group_count {
                        let elem = if let Some(range) = m.group(i) {
                            let group_str = &s[range];
                            store_runtime_string(&caller, group_str.to_string())
                        } else {
                            value::encode_undefined()
                        };
                        write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                    }
                    write_array_length(&mut caller, arr_ptr, group_count as u32);

                    // 设置 .index 属性
                    let index_val = value::encode_f64(m.start() as f64);
                    let _ = define_host_data_property_from_caller(
                        &mut caller, arr_ptr as i64, "index", index_val,
                    );

                    // 设置 .input 属性
                    let input_val = store_runtime_string(&caller, s.clone());
                    let _ = define_host_data_property_from_caller(
                        &mut caller, arr_ptr as i64, "input", input_val,
                    );

                    // 设置 .groups 属性
                    let named: Vec<(&str, Option<std::ops::Range<usize>>)> =
                        m.named_groups().collect();
                    if !named.is_empty() {
                        let groups_obj =
                            alloc_host_object_from_caller(&mut caller, named.len() as u32);
                        for (name, range) in named {
                            let val = match range {
                                Some(r) => {
                                    store_runtime_string(&caller, s[r].to_string())
                                }
                                None => value::encode_undefined(),
                            };
                            let _ = define_host_data_property_from_caller(
                                &mut caller, groups_obj, name, val,
                            );
                        }
                        let _ = define_host_data_property_from_caller(
                            &mut caller, arr_ptr as i64, "groups", groups_obj,
                        );
                    } else {
                        let _ = define_host_data_property_from_caller(
                            &mut caller, arr_ptr as i64, "groups", value::encode_undefined(),
                        );
                    }

                    arr
                }
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过。

- [ ] **Step 3: 创建 happy-path fixture — 命名捕获组**

创建 `fixtures/happy/regexp_named_groups.js`：
```js
var re = /(?<year>\d{4})-(?<month>\d{2})-(?<day>\d{2})/;
var m = re.exec("2026-05-22");
console.log(m[0]);
console.log(m[1]);
console.log(m[2]);
console.log(m[3]);
console.log(JSON.stringify(m.groups));
```

- [ ] **Step 4: 运行 fixture 并生成 .expected**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration 2>&1
```

- [ ] **Step 5: 验证 .expected 内容**

检查 `fixtures/happy/regexp_named_groups.expected` 包含正确的 groups JSON。

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/primitive_core.rs fixtures/happy/regexp_named_groups.js fixtures/happy/regexp_named_groups.expected
git commit -m "feat: add .index, .input, .groups properties to regex_exec result"
```

---

### Task 3: `process_replacement` — `$<name>` 支持

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:741-824`（`process_replacement` 函数签名和 `$` 解析）
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:934-949,959-973`（调用侧传 `&Match`）

将 `process_replacement` 签名从接受 `captures` + `match_start` + `match_end` 改为接受 `&regress::Match`，在 `$` 解析中添加 `$<name>` 分支。

- [ ] **Step 1: 修改 `process_replacement` 函数签名和实现**

将完整的 `process_replacement` 函数（lines 741-824）替换为：

```rust
            /// 处理 JavaScript 替换模式：$$, $&, $`, $', $n, $nn, $<name>
            fn process_replacement(
                replace_str: &str,
                s: &str,
                m: &regress::Match,
            ) -> String {
                let match_start = m.start();
                let match_end = m.end();
                let mut result = String::new();
                let chars: Vec<char> = replace_str.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    if chars[i] == '$' && i + 1 < chars.len() {
                        let next = chars[i + 1];
                        match next {
                            '$' => {
                                result.push('$');
                                i += 2;
                            }
                            '&' => {
                                result.push_str(&s[match_start..match_end]);
                                i += 2;
                            }
                            '`' => {
                                result.push_str(&s[..match_start]);
                                i += 2;
                            }
                            '\'' => {
                                result.push_str(&s[match_end..]);
                                i += 2;
                            }
                            '<' => {
                                // $<name> → named capture group
                                if let Some(close_pos) =
                                    chars[i + 2..].iter().position(|&c| c == '>')
                                {
                                    let name: String =
                                        chars[i + 2..i + 2 + close_pos].iter().collect();
                                    if let Some(range) = m.named_group(&name) {
                                        result.push_str(&s[range]);
                                    }
                                    // 命名组不存在或未匹配 → 空字符串（ES 规范）
                                    i += 3 + close_pos; // skip past $<name>
                                } else {
                                    // 未闭合的 $<，保持原样
                                    result.push('$');
                                    result.push('<');
                                    i += 2;
                                }
                            }
                            '0'..='9' => {
                                // $n or $nn → captured group
                                let mut group_num = (next as u8 - b'0') as usize;
                                let mut consumed = 2;
                                // ECMAScript: $0 不是特殊模式，应保持字面量
                                if group_num == 0 {
                                    result.push('$');
                                    result.push('0');
                                    i += 2;
                                    continue;
                                }
                                // 检查是否为两位数 $nn
                                if i + 2 < chars.len()
                                    && let Some('0'..='9') = chars.get(i + 2)
                                {
                                    let next_digit = (chars[i + 2] as u8 - b'0') as usize;
                                    let two_digit = group_num * 10 + next_digit;
                                    // $00 不是特殊模式，只有 $01-$99 是
                                    if two_digit > 0 && two_digit <= m.captures.len() {
                                        group_num = two_digit;
                                        consumed = 3;
                                    }
                                }
                                // 获取捕获组（group_num ≥ 1）
                                if group_num <= m.captures.len() {
                                    if let Some(range) = m.group(group_num) {
                                        result.push_str(&s[range]);
                                    }
                                } else {
                                    result.push('$');
                                    result.push(next);
                                }
                                i += consumed;
                            }
                            _ => {
                                result.push('$');
                                result.push(next);
                                i += 2;
                            }
                        }
                    } else {
                        result.push(chars[i]);
                        i += 1;
                    }
                }
                result
            }
```

- [ ] **Step 2: 更新调用侧 — 全局替换路径**

找到全局替换路径中对 `process_replacement` 的调用（约 lines 934-949）。原来：

```rust
                        let captures: Vec<Option<std::ops::Range<usize>>> =
                            (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                        let replaced = if is_func_replace {
                            call_replace_func(
                                &mut caller,
                                replace,
                                &s,
                                m.start(),
                                m.end(),
                                &captures,
                            )
                        } else {
                            let replace_str = get_string_value(&mut caller, replace);
                            process_replacement(&replace_str, &s, m.start(), m.end(), &captures)
                        };
```

改为：

```rust
                        let captures: Vec<Option<std::ops::Range<usize>>> =
                            (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                        let replaced = if is_func_replace {
                            call_replace_func(
                                &mut caller,
                                replace,
                                &s,
                                m.start(),
                                m.end(),
                                &captures,
                            )
                        } else {
                            let replace_str = get_string_value(&mut caller, replace);
                            process_replacement(&replace_str, &s, &m)
                        };
```

- [ ] **Step 3: 更新调用侧 — 单次替换路径**

找到单次替换路径中对 `process_replacement` 的调用（约 lines 959-973）。同样将：

```rust
                            let captures: Vec<Option<std::ops::Range<usize>>> =
                                (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                            let replaced = if is_func_replace {
                                call_replace_func(
                                    &mut caller,
                                    replace,
                                    &s,
                                    m.start(),
                                    m.end(),
                                    &captures,
                                )
                            } else {
                                let replace_str = get_string_value(&mut caller, replace);
                                process_replacement(&replace_str, &s, m.start(), m.end(), &captures)
                            };
```

改为：

```rust
                            let captures: Vec<Option<std::ops::Range<usize>>> =
                                (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                            let replaced = if is_func_replace {
                                call_replace_func(
                                    &mut caller,
                                    replace,
                                    &s,
                                    m.start(),
                                    m.end(),
                                    &captures,
                                )
                            } else {
                                let replace_str = get_string_value(&mut caller, replace);
                                process_replacement(&replace_str, &s, &m)
                            };
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过。

- [ ] **Step 5: 创建 happy-path fixture — `$<name>` 替换**

创建 `fixtures/happy/regexp_replace_named.js`：
```js
var re = /(?<first>\w+)\s+(?<last>\w+)/;
var s = "John Doe";
var r = s.replace(re, "$<last>, $<first>");
console.log(r);
// 测试不存在的命名组 → 空字符串
var r2 = s.replace(re, "$<unknown>");
console.log(r2);
```

- [ ] **Step 6: 运行并生成 .expected**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration 2>&1
```

- [ ] **Step 7: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/primitive_core.rs fixtures/happy/regexp_replace_named.js fixtures/happy/regexp_replace_named.expected
git commit -m "feat: add $<name> named group references in string.replace()"
```

---

### Task 4: `call_replace_func` — 补全 `groups` 参数

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:827-914`（`call_replace_func` 签名 + 参数写入）
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:934-945,959-968`（调用侧传 `groups`）
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:988-998`（字符串替换路径调用侧）

函数替换 `str.replace(/re/, (match, p1, ..., offset, string, groups) => ...)` 最后一个参数应为命名捕获组对象。

- [ ] **Step 1: 修改 `call_replace_func` 签名和参数写入**

将 `call_replace_func`（lines 827-914）的签名添加 `named_groups_obj: i64` 参数，并在 `args_count` 中 +1，在 `offset` 和 `string` 参数之后写入 `groups`。

完整替换后的函数：

```rust
            /// 调用替换函数并返回替换字符串
            fn call_replace_func(
                caller: &mut Caller<'_, RuntimeState>,
                func: i64,
                s: &str,
                match_start: usize,
                match_end: usize,
                captures: &[Option<std::ops::Range<usize>>],
                named_groups_obj: i64,
            ) -> String {
                let capture_count = captures.len().saturating_sub(1);
                let args_count = 1 + capture_count + 1 + 1 + 1; // matched + captures + offset + string + groups

                let shadow_sp_global = caller
                    .get_export("__shadow_sp")
                    .and_then(|e| e.into_global())
                    .unwrap();
                let shadow_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .unwrap();

                let mut arg_idx = 0;

                // 1. matched substring
                let matched_val =
                    store_runtime_string(&*caller, s[match_start..match_end].to_string());
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &matched_val.to_le_bytes(),
                    )
                    .unwrap();
                arg_idx += 1;

                // 2. capture groups (从 group 1 开始)
                for i in 1..=capture_count {
                    let capture_val = if let Some(Some(range)) = captures.get(i) {
                        store_runtime_string(&*caller, s[range.clone()].to_string())
                    } else {
                        value::encode_undefined()
                    };
                    memory
                        .write(
                            &mut *caller,
                            (shadow_sp + arg_idx * 8) as usize,
                            &capture_val.to_le_bytes(),
                        )
                        .unwrap();
                    arg_idx += 1;
                }

                // 3. offset
                let offset_val = value::encode_f64(match_start as f64);
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &offset_val.to_le_bytes(),
                    )
                    .unwrap();
                arg_idx += 1;

                // 4. original string
                let string_val = store_runtime_string(&*caller, s.to_string());
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &string_val.to_le_bytes(),
                    )
                    .unwrap();
                arg_idx += 1;

                // 5. named groups object
                memory
                    .write(
                        &mut *caller,
                        (shadow_sp + arg_idx * 8) as usize,
                        &named_groups_obj.to_le_bytes(),
                    )
                    .unwrap();

                let result = resolve_and_call(
                    caller,
                    func,
                    value::encode_undefined(),
                    0,
                    args_count as i32,
                );

                get_string_value(caller, result)
            }
```

- [ ] **Step 2: 添加 groups 对象构建辅助闭包**

在 `string_replace_fn` 闭包内、`process_replacement` 和 `call_replace_func` 定义之前（约 line 740），添加一个辅助闭包用于构建 groups 对象：

```rust
            /// 从 Match 构建命名捕获组对象
            let build_groups_obj =
                |caller: &mut Caller<'_, RuntimeState>, m: &regress::Match| -> i64 {
                    let named: Vec<(&str, Option<std::ops::Range<usize>>)> =
                        m.named_groups().collect();
                    if named.is_empty() {
                        return value::encode_undefined();
                    }
                    let obj = alloc_host_object_from_caller(caller, named.len() as u32);
                    for (name, range) in named {
                        let val = match range {
                            Some(r) => store_runtime_string(caller, s[r].to_string()),
                            None => value::encode_undefined(),
                        };
                        let _ = define_host_data_property_from_caller(caller, obj, name, val);
                    }
                    obj
                };
```

- [ ] **Step 3: 更新调用侧 — 全局替换路径**

在全局替换循环中（约 line 937），`call_replace_func` 调用增加 groups 参数：

```rust
                        let captures: Vec<Option<std::ops::Range<usize>>> =
                            (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                        let replaced = if is_func_replace {
                            let groups_obj = build_groups_obj(&mut caller, &m);
                            call_replace_func(
                                &mut caller,
                                replace,
                                &s,
                                m.start(),
                                m.end(),
                                &captures,
                                groups_obj,
                            )
                        } else {
```

- [ ] **Step 4: 更新调用侧 — 单次替换路径**

同样在单次替换路径（约 line 962）：

```rust
                            let captures: Vec<Option<std::ops::Range<usize>>> =
                                (0..m.captures.len() + 1).map(|i| m.group(i)).collect();
                            let replaced = if is_func_replace {
                                let groups_obj = build_groups_obj(&mut caller, &m);
                                call_replace_func(
                                    &mut caller,
                                    replace,
                                    &s,
                                    m.start(),
                                    m.end(),
                                    &captures,
                                    groups_obj,
                                )
                            } else {
```

- [ ] **Step 5: 更新字符串替换调用侧**

字符串替换路径（约 line 991）传 `value::encode_undefined()` 作为 groups：

```rust
                    let replaced = if is_func_replace {
                        let captures = vec![Some(pos..pos + search_str.len())];
                        call_replace_func(
                            &mut caller,
                            replace,
                            &s,
                            pos,
                            pos + search_str.len(),
                            &captures,
                            value::encode_undefined(),
                        )
                    } else {
```

- [ ] **Step 6: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过。

- [ ] **Step 7: 更新 replace_named fixture 增加函数替换测试**

在 `fixtures/happy/regexp_replace_named.js` 末尾追加：
```js
// 函数替换接收 groups 参数
var re2 = /(?<a>\d+)\+(?<b>\d+)/;
var r3 = "3+5".replace(re2, function(match, p1, p2, offset, str, groups) {
    return (Number(groups.a) + Number(groups.b)).toString();
});
console.log(r3);
```

- [ ] **Step 8: 更新 .expected**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration 2>&1
```
验证 `.expected` 包含 `"8"`。

- [ ] **Step 9: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/primitive_core.rs fixtures/happy/regexp_replace_named.js fixtures/happy/regexp_replace_named.expected
git commit -m "feat: add groups argument to replacement function callbacks"
```

---

### Task 5: `string_match` 非全局路径 — 补全 `.index`、`.input`、`.groups`

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs:706-728`（非全局匹配路径）

非全局模式下 `string_match` 返回同 `exec()` 的结果数组。全局模式返回纯字符串数组无需修改。

- [ ] **Step 1: 修改 `string_match` 非全局路径**

找到 `string_match_fn` 闭包中 `} else { // 非全局：返回 exec 结果` 分支（约 line 704-728）。在 `write_array_length` 之后、`arr` 返回之前，加上同 Task 2 的属性设置。

将 lines 706-728 的 `Some(m) => { ... arr }` 块改为：

```rust
                    Some(m) => {
                        let group_count = m.captures.len() + 1;
                        let arr = alloc_array(&mut caller, group_count as u32);
                        let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) else {
                            return value::encode_null();
                        };
                        for i in 0..group_count {
                            let elem = if let Some(range) = m.group(i) {
                                let group_str = &s[range];
                                store_runtime_string(&caller, group_str.to_string())
                            } else {
                                value::encode_undefined()
                            };
                            write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                        }
                        write_array_length(&mut caller, arr_ptr, group_count as u32);

                        // 设置 .index
                        let index_val = value::encode_f64(m.start() as f64);
                        let _ = define_host_data_property_from_caller(
                            &mut caller, arr_ptr as i64, "index", index_val,
                        );
                        // 设置 .input
                        let input_val = store_runtime_string(&caller, s.clone());
                        let _ = define_host_data_property_from_caller(
                            &mut caller, arr_ptr as i64, "input", input_val,
                        );
                        // 设置 .groups
                        let named: Vec<(&str, Option<std::ops::Range<usize>>)> =
                            m.named_groups().collect();
                        if !named.is_empty() {
                            let groups_obj =
                                alloc_host_object_from_caller(&mut caller, named.len() as u32);
                            for (name, range) in named {
                                let val = match range {
                                    Some(r) => store_runtime_string(&caller, s[r].to_string()),
                                    None => value::encode_undefined(),
                                };
                                let _ = define_host_data_property_from_caller(
                                    &mut caller, groups_obj, name, val,
                                );
                            }
                            let _ = define_host_data_property_from_caller(
                                &mut caller, arr_ptr as i64, "groups", groups_obj,
                            );
                        } else {
                            let _ = define_host_data_property_from_caller(
                                &mut caller, arr_ptr as i64, "groups", value::encode_undefined(),
                            );
                        }

                        arr
                    }
```

- [ ] **Step 2: 同样修改非 RegExp 路径的 `string_match`**

非 RegExp 参数时走 `if !value::is_regexp(regexp)` 分支（约 line 612），内部也有一个 `match entry.compiled.find(&s)` 的 `Some(m) => { ... arr }` 块（lines 632-648）。同样需要在 `write_array_length` 后加 `.index`、`.input`、`.groups`。但那块没有 `has_indices` 需求（隐式创建的 RegExp 无 flags）。

在 line 647 `write_array_length(&mut caller, arr_ptr, group_count as u32);` 之后、`return arr;` 之前插入：

```rust
                                // .index 和 .input
                                let index_val = value::encode_f64(m.start() as f64);
                                let _ = define_host_data_property_from_caller(
                                    &mut caller, arr_ptr as i64, "index", index_val,
                                );
                                let input_val = store_runtime_string(&caller, s.clone());
                                let _ = define_host_data_property_from_caller(
                                    &mut caller, arr_ptr as i64, "input", input_val,
                                );
                                // .groups（隐式创建的 RegExp 无命名组，传 undefined）
                                let _ = define_host_data_property_from_caller(
                                    &mut caller, arr_ptr as i64, "groups", value::encode_undefined(),
                                );
```

- [ ] **Step 3: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过。

- [ ] **Step 4: 运行现有测试回归**

```bash
cargo test -p wjsm --test integration 2>&1
```
预期：全部现有 RegExp fixtures 通过（`regex_literal`、`regex_exec`、`regex_test`、`string_match`、`string_match_global`）。

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/primitive_core.rs
git commit -m "feat: add .index, .input, .groups to string_match non-global results"
```

---

### Task 6: `matchAll` — 补全 `.groups`

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/string_methods.rs:208-213`

`matchAll` 已设 `.index` 和 `.input`，缺 `.groups`。

- [ ] **Step 1: 在 `matchAll` 循环中添加 `.groups`**

找到 `string_match_all_fn` 闭包中的循环（约 line 193-218）。在 `.input` 设置（line 213）之后，添加 `.groups` 设置（与 Task 2 相同逻辑）。

将 lines 208-214 的代码块扩展：

```rust
                write_array_length(&mut caller, arr_ptr, group_count as u32);
                let match_start = m.group(0).map(|r| r.start).unwrap_or(0);
                let index_val = value::encode_f64(match_start as f64);
                let input_val = store_runtime_string(&caller, s.clone());
                let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "index", index_val);
                let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "input", input_val);
                // 设置 .groups
                let named: Vec<(&str, Option<std::ops::Range<usize>>)> = m.named_groups().collect();
                if !named.is_empty() {
                    let groups_obj = alloc_host_object_from_caller(&mut caller, named.len() as u32);
                    for (name, range) in named {
                        let val = match range {
                            Some(r) => store_runtime_string(&caller, s[r].to_string()),
                            None => value::encode_undefined(),
                        };
                        let _ = define_host_data_property_from_caller(&mut caller, groups_obj, name, val);
                    }
                    let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "groups", groups_obj);
                } else {
                    let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "groups", value::encode_undefined());
                }
                results.push(arr);
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过。

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/string_methods.rs
git commit -m "feat: add .groups property to matchAll result elements"
```

---

### Task 7: `d` 标志 — `.indices`（含 `.indices.groups`）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs` — `regex_exec` 和 `string_match` 非全局路径
- Modify: `crates/wjsm-runtime/src/host_imports/string_methods.rs` — `matchAll`

`regex_exec`、`string_match` 非全局、`matchAll` 的结果需在 `d` 标志激活时附加 `.indices` 数组。

- [ ] **Step 1: 在 `regex_exec` 中添加 `.indices` 逻辑**

在 `regex_exec` 的 `.groups` 设置（Task 2 添加的代码）之后、`arr` 返回之前，插入 `.indices` 逻辑。

在 `regex_exec_fn` 的 `Some(m) => { ... }` 分支中，`.groups` 设置之后：

```rust
                    // 设置 .indices（仅 d 标志）
                    if entry.flags.contains('d') {
                        let indices_arr =
                            alloc_array(&mut caller, group_count as u32);
                        let indices_ptr =
                            resolve_array_ptr(&mut caller, indices_arr)
                            .unwrap_or(0);
                        for i in 0..group_count {
                            let elem = match m.group(i) {
                                Some(range) => {
                                    let pair = alloc_array(&mut caller, 2);
                                    let pair_ptr =
                                        resolve_array_ptr(&mut caller, pair)
                                        .unwrap_or(0);
                                    write_array_elem(
                                        &mut caller, pair_ptr, 0,
                                        value::encode_f64(range.start as f64),
                                    );
                                    write_array_elem(
                                        &mut caller, pair_ptr, 1,
                                        value::encode_f64(range.end as f64),
                                    );
                                    write_array_length(&mut caller, pair_ptr, 2);
                                    pair
                                }
                                None => value::encode_undefined(),
                            };
                            write_array_elem(
                                &mut caller, indices_ptr, i as u32, elem,
                            );
                        }
                        write_array_length(
                            &mut caller, indices_ptr, group_count as u32,
                        );
                        // indices.groups
                        let named: Vec<(&str, Option<std::ops::Range<usize>>)> =
                            m.named_groups().collect();
                        if !named.is_empty() {
                            let ig = alloc_host_object_from_caller(
                                &mut caller, named.len() as u32,
                            );
                            for (name, range) in named {
                                let val = match range {
                                    Some(r) => {
                                        let pair = alloc_array(&mut caller, 2);
                                        let pair_ptr =
                                            resolve_array_ptr(&mut caller, pair)
                                            .unwrap_or(0);
                                        write_array_elem(
                                            &mut caller, pair_ptr, 0,
                                            value::encode_f64(r.start as f64),
                                        );
                                        write_array_elem(
                                            &mut caller, pair_ptr, 1,
                                            value::encode_f64(r.end as f64),
                                        );
                                        write_array_length(
                                            &mut caller, pair_ptr, 2,
                                        );
                                        pair
                                    }
                                    None => value::encode_undefined(),
                                };
                                let _ = define_host_data_property_from_caller(
                                    &mut caller, ig, name, val,
                                );
                            }
                            let _ = define_host_data_property_from_caller(
                                &mut caller, indices_ptr as i64, "groups", ig,
                            );
                        } else {
                            let _ = define_host_data_property_from_caller(
                                &mut caller,
                                indices_ptr as i64,
                                "groups",
                                value::encode_undefined(),
                            );
                        }
                        let _ = define_host_data_property_from_caller(
                            &mut caller, arr_ptr as i64, "indices", indices_arr,
                        );
                    }
```

注意：`entry.flags` 需要从闭包捕获中获取。在 `regex_exec_fn` 中，`entry` 是在 `Some(m) =>` 之前 clone 出来的变量，已包含 flags。可直接用 `entry.flags.contains('d')`。

- [ ] **Step 2: 在 `string_match` 非全局路径添加 `.indices`**

同样在 `string_match_fn` 的非 RegExp 路径和 RegExp 非全局路径两个 `Some(m) =>` 块中，`.groups` 设置之后加 `.indices` 逻辑。

对于 RegExp 非全局路径（`} else { // 非全局` 分支，约 line 705），`entry` 变量可用（已在 scope 中）。在 `.groups` 设置之后：

```rust
                        // 设置 .indices（仅 d 标志）
                        if entry.flags.contains('d') {
                            let indices_arr =
                                alloc_array(&mut caller, group_count as u32);
                            let indices_ptr =
                                resolve_array_ptr(&mut caller, indices_arr)
                                .unwrap_or(0);
                            for i in 0..group_count {
                                let elem = match m.group(i) {
                                    Some(range) => {
                                        let pair = alloc_array(&mut caller, 2);
                                        let pair_ptr =
                                            resolve_array_ptr(&mut caller, pair)
                                            .unwrap_or(0);
                                        write_array_elem(
                                            &mut caller, pair_ptr, 0,
                                            value::encode_f64(range.start as f64),
                                        );
                                        write_array_elem(
                                            &mut caller, pair_ptr, 1,
                                            value::encode_f64(range.end as f64),
                                        );
                                        write_array_length(&mut caller, pair_ptr, 2);
                                        pair
                                    }
                                    None => value::encode_undefined(),
                                };
                                write_array_elem(
                                    &mut caller, indices_ptr, i as u32, elem,
                                );
                            }
                            write_array_length(
                                &mut caller, indices_ptr, group_count as u32,
                            );
                            // indices.groups
                            let named: Vec<(&str, Option<std::ops::Range<usize>>)> =
                                m.named_groups().collect();
                            if !named.is_empty() {
                                let ig = alloc_host_object_from_caller(
                                    &mut caller, named.len() as u32,
                                );
                                for (name, range) in named {
                                    let val = match range {
                                        Some(r) => {
                                            let pair = alloc_array(&mut caller, 2);
                                            let pair_ptr =
                                                resolve_array_ptr(&mut caller, pair)
                                                .unwrap_or(0);
                                            write_array_elem(
                                                &mut caller, pair_ptr, 0,
                                                value::encode_f64(r.start as f64),
                                            );
                                            write_array_elem(
                                                &mut caller, pair_ptr, 1,
                                                value::encode_f64(r.end as f64),
                                            );
                                            write_array_length(
                                                &mut caller, pair_ptr, 2,
                                            );
                                            pair
                                        }
                                        None => value::encode_undefined(),
                                    };
                                    let _ = define_host_data_property_from_caller(
                                        &mut caller, ig, name, val,
                                    );
                                }
                                let _ = define_host_data_property_from_caller(
                                    &mut caller, indices_ptr as i64, "groups", ig,
                                );
                            } else {
                                let _ = define_host_data_property_from_caller(
                                    &mut caller,
                                    indices_ptr as i64,
                                    "groups",
                                    value::encode_undefined(),
                                );
                            }
                            let _ = define_host_data_property_from_caller(
                                &mut caller, arr_ptr as i64, "indices", indices_arr,
                            );
                        }
```

对于非 RegExp 路径（隐式创建的 RegExp 无标志），不加 `.indices`。

- [ ] **Step 3: 在 `matchAll` 中添加 `.indices`**

在 `string_match_all_fn` 的循环中，`.groups` 设置（Task 6 添加）之后。`entry` 变量已在 scope：

```rust
                // 设置 .indices（仅 d 标志）
                if entry.flags.contains('d') {
                    let indices_arr = alloc_array(&mut caller, group_count as u32);
                    let indices_ptr = resolve_array_ptr(&mut caller, indices_arr).unwrap_or(0);
                    for i in 0..group_count {
                        let elem = match m.group(i) {
                            Some(range) => {
                                let pair = alloc_array(&mut caller, 2);
                                let pair_ptr = resolve_array_ptr(&mut caller, pair).unwrap_or(0);
                                write_array_elem(&mut caller, pair_ptr, 0, value::encode_f64(range.start as f64));
                                write_array_elem(&mut caller, pair_ptr, 1, value::encode_f64(range.end as f64));
                                write_array_length(&mut caller, pair_ptr, 2);
                                pair
                            }
                            None => value::encode_undefined(),
                        };
                        write_array_elem(&mut caller, indices_ptr, i as u32, elem);
                    }
                    write_array_length(&mut caller, indices_ptr, group_count as u32);
                    // indices.groups
                    let named: Vec<(&str, Option<std::ops::Range<usize>>)> = m.named_groups().collect();
                    if !named.is_empty() {
                        let ig = alloc_host_object_from_caller(&mut caller, named.len() as u32);
                        for (name, range) in named {
                            let val = match range {
                                Some(r) => {
                                    let pair = alloc_array(&mut caller, 2);
                                    let pair_ptr = resolve_array_ptr(&mut caller, pair).unwrap_or(0);
                                    write_array_elem(&mut caller, pair_ptr, 0, value::encode_f64(r.start as f64));
                                    write_array_elem(&mut caller, pair_ptr, 1, value::encode_f64(r.end as f64));
                                    write_array_length(&mut caller, pair_ptr, 2);
                                    pair
                                }
                                None => value::encode_undefined(),
                            };
                            let _ = define_host_data_property_from_caller(&mut caller, ig, name, val);
                        }
                        let _ = define_host_data_property_from_caller(&mut caller, indices_ptr as i64, "groups", ig);
                    } else {
                        let _ = define_host_data_property_from_caller(&mut caller, indices_ptr as i64, "groups", value::encode_undefined());
                    }
                    let _ = define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "indices", indices_arr);
                }
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p wjsm-runtime 2>&1
```
预期：编译通过。

- [ ] **Step 5: 创建 fixture — `hasIndices`**

创建 `fixtures/happy/regexp_hasindices.js`：
```js
var re = /(?<x>a)(b)/d;
var m = re.exec("ab");
console.log(m.indices[0][0], m.indices[0][1]);
console.log(m.indices[1][0], m.indices[1][1]);
console.log(m.indices[2][0], m.indices[2][1]);
console.log(JSON.stringify(m.indices.groups));
```

- [ ] **Step 6: 生成 .expected**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration 2>&1
```
验证 `.expected` 内容正确。

- [ ] **Step 7: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/primitive_core.rs crates/wjsm-runtime/src/host_imports/string_methods.rs fixtures/happy/regexp_hasindices.js fixtures/happy/regexp_hasindices.expected
git commit -m "feat: add .indices array (d flag / hasIndices) to exec, match, matchAll results"
```

---

### Task 8: 补充 Fixtures + 全量回归

**Files:**
- Create: `fixtures/happy/regexp_lookbehind.js` + `.expected`
- Create: `fixtures/happy/regexp_unicode_prop.js` + `.expected`
- Create: `fixtures/happy/regexp_dotall.js` + `.expected`

- [ ] **Step 1: lookbehind fixture**

创建 `fixtures/happy/regexp_lookbehind.js`：
```js
// 正向后顾断言
var re1 = /(?<=a)b/;
console.log(re1.test("ab"));  // true
console.log(re1.test("cb"));  // false

// 反向后顾断言
var re2 = /(?<!a)b/;
console.log(re2.test("ab"));  // false
console.log(re2.test("cb"));  // true

// lookbehind 中的捕获组
var re3 = /(?<=(a))(b)/;
var m = re3.exec("ab");
console.log(m[0]);  // b
console.log(m[1]);  // a
console.log(m[2]);  // b
```

- [ ] **Step 2: Unicode 属性转义 fixture**

创建 `fixtures/happy/regexp_unicode_prop.js`：
```js
// \p{Letter} 匹配字母
var re1 = /\p{Letter}+/u;
console.log(re1.test("hello"));  // true
console.log(re1.test("123"));    // false

// \P{Letter} 匹配非字母
var re2 = /\P{Letter}+/u;
console.log(re2.test("123"));    // true

// \p{Script=Latin}
var re3 = /\p{Script=Latin}+/u;
console.log(re3.test("café"));   // true
```

- [ ] **Step 3: dotAll fixture**

创建 `fixtures/happy/regexp_dotall.js`：
```js
// s 标志下 . 匹配换行
var re = /a.b/s;
console.log(re.test("a\nb"));  // true

// 无 s 标志下 . 不匹配换行
var re2 = /a.b/;
console.log(re2.test("a\nb"));  // false
```

- [ ] **Step 4: 生成 .expected 并运行全量回归**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration 2>&1
```
预期：全部通过（新建 + 现有 6 个 RegExp fixtures）。

- [ ] **Step 5: 运行 semantic snapshot 测试确保无 IR 变更**

```bash
cargo test -p wjsm-semantic --test lowering_snapshots 2>&1
```
预期：全部通过（本次无 IR 变更，应不影响 snapshot）。

- [ ] **Step 6: Commit**

```bash
git add fixtures/happy/regexp_lookbehind.js fixtures/happy/regexp_lookbehind.expected fixtures/happy/regexp_unicode_prop.js fixtures/happy/regexp_unicode_prop.expected fixtures/happy/regexp_dotall.js fixtures/happy/regexp_dotall.expected
git commit -m "feat: add E2E fixtures for lookbehind, unicode property escapes, and dotAll flag"
```
