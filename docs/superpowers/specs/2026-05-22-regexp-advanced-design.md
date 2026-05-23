# RegExp 高级特性 — 设计文档

日期：2026-05-22
状态：待审批

## 1. 动机

当前 wjsm 的 RegExp 实现仅覆盖基础功能（字面量创建、`test()`、`exec()`、`match()`、`replace()`、`search()`、`split()`）。缺失 ES2018 引入的三个高级特性：

- **Lookbehind 断言** (`(?<=...)` / `(?<!...)`)
- **命名捕获组** (`(?<name>...)`)
- **Unicode 属性转义** (`\p{...}` / `\P{...}`)

顺带实现 ES2022 的 `d` 标志（`hasIndices`），因其实现成本极低（纯数据组装）。

## 2. 当前状态

### 2.1 已有基础设施

| 层 | 状态 |
|---|---|
| 语法解析 | swc_core 解析全部 ES2018+ regex 语法 → `Lit::Regex` |
| IR | `Constant::RegExp { pattern, flags }` → 常量池 |
| Semantic | `/pattern/flags` 字面量 → `Constant::RegExp`；`.test()`/`.exec()` → `CallBuiltin` |
| Backend | 字面量 → data section 字符串 + `regex_create` 调用；方法调用 → `regex_test`/`regex_exec` import |
| Runtime 引擎 | `regress` 0.11 原生支持全部 ES2018 语法 + `v` 标志 |

### 2.2 当前 `regex_exec` 结果

```
[full_match, group1, group2, ...]
```

**缺失的 ES 规范属性**：`.index`、`.input`、`.groups`、`.indices`。

### 2.3 当前 `process_replacement`

支持 `$$`、`$&`、`` $` ``、`$'`、`$1`–`$99`。**缺失** `$<name>`。

### 2.4 当前标志处理

`regex_create` 将 flags 字符串原样传给 `regress::Regex::with_flags()`。regress 静默忽略不认识的标志（`g`、`y`、`d`）。**缺失** ES 规范要求的重复/非法标志校验 → `SyntaxError`。

### 2.5 影响的文件

- `crates/wjsm-runtime/src/host_imports/primitive_core.rs` — `regex_create`、`regex_exec`、`string_match`、`string_replace`（`process_replacement`）
- `crates/wjsm-runtime/src/host_imports/string_methods.rs` — `string_match_all`

无 IR / semantic / backend 改动。

## 3. 设计

### 3.1 `regex_exec` — 补全结果属性

在构建捕获组数组 `[full_match, group1, ...]` 之后，追加以下属性：

#### `.index`

```rust
let index_val = value::encode_f64(m.start() as f64);
define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "index", index_val);
```

#### `.input`

```rust
let input_val = store_runtime_string(&caller, s.clone());
define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "input", input_val);
```

#### `.groups`

```rust
// NamedGroups 无 ExactSizeIterator，先 collect 到 Vec
let named: Vec<(&str, Option<std::ops::Range<usize>>)> = m.named_groups().collect();
if !named.is_empty() {
    let groups_obj = alloc_host_object_from_caller(&mut caller, named.len() as u32);
    for (name, range) in named {
        let val = match range {
            Some(r) => store_runtime_string(&caller, s[r].to_string()),
            None => value::encode_undefined(),
        };
        define_host_data_property_from_caller(&mut caller, groups_obj, name, val);
    }
    define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "groups", groups_obj);
} else {
    define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "groups", value::encode_undefined());
}
```

注意：`Match::named_groups()` 返回 `NamedGroups` 迭代器（`Item = (&str, Option<Range>)`），未实现 `ExactSizeIterator`。因此先 collect 到 `Vec` 以获取长度并迭代。

#### `.indices`（仅 `d` 标志）

```rust
if flags.contains('d') {
    let indices_arr = alloc_array(&mut caller, group_count as u32);
    let indices_ptr = resolve_array_ptr(&mut caller, indices_arr);
    for i in 0..group_count {
        let elem = match m.group(i) {
            Some(range) => {
                let pair = alloc_array(&mut caller, 2);
                let pair_ptr = resolve_array_ptr(&mut caller, pair);
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
                    let pair_ptr = resolve_array_ptr(&mut caller, pair);
                    write_array_elem(&mut caller, pair_ptr, 0, value::encode_f64(r.start as f64));
                    write_array_elem(&mut caller, pair_ptr, 1, value::encode_f64(r.end as f64));
                    write_array_length(&mut caller, pair_ptr, 2);
                    pair
                }
                None => value::encode_undefined(),
            };
            define_host_data_property_from_caller(&mut caller, ig, name, val);
        }
        define_host_data_property_from_caller(&mut caller, indices_ptr as i64, "groups", ig);
    } else {
        define_host_data_property_from_caller(&mut caller, indices_ptr as i64, "groups", value::encode_undefined());
    }
    define_host_data_property_from_caller(&mut caller, arr_ptr as i64, "indices", indices_arr);
}
```

### 3.2 `string_match` — 非全局路径补全

`String.prototype.match(regexp)` 非全局模式下返回同 `exec()` —— 需同样加上 `.index`、`.input`、`.groups`、`.indices`。

全局模式返回纯字符串数组，规范不要求这些属性 —— 无需修改。

### 3.3 `string_match_all` — 补全 `.groups`、`.indices`

当前 `matchAll` 已设置 `.index` 和 `.input`，缺 `.groups` 和 `.indices`。逻辑同 3.1。

### 3.4 `process_replacement` — `$<name>` 支持

在 `$` 解析的 match 分支中添加：

```rust
'<' => {
    // $<name> → named capture group
    if let Some(close) = chars[i + 2..].iter().position(|&c| c == '>') {
        let name: String = chars[i + 2..i + 2 + close].iter().collect();
        // 需要在调用侧传入 named_groups，此处仅描述逻辑
        // 查找命名组并替换
        i += 3 + close; // skip past $<name>
    } else {
        // 未闭合的 $<，保持原样
        result.push('$');
        result.push('<');
        i += 2;
    }
}
```

注意：当前 `process_replacement` 签名只接受 `captures: &[Option<Range>]`（按索引），不支持按名称查找。需要额外传入命名组映射或 `&Match`。

**方案**：扩展 `process_replacement` 签名，增加 `named_groups: &[(String, Option<Range>)]` 参数（预先从 `Match` 提取）。或者传入整个 `&Match` 引用。

推荐传 `&Match`，可以同时访问索引组 (`group(i)`) 和命名组 (`named_group(name)`)，简化调用侧。
### 3.4.1 `call_replace_func` — 补全 `groups` 参数

函数替换 `str.replace(/re/, (match, p1, ..., offset, string, groups) => ...)` 的最后一个参数应为命名捕获组对象。当前 `call_replace_func` 参数列表止于 `offset + string`。

**修改**：在 `offset` 和 `string` 参数之后追加 `groups` 参数。若无名分组则传 `undefined`，否则构造同名于 3.1 的 groups 对象。

注意：这会使 `args_count` 从 `1 + captures + 1 + 1` 变为 `1 + captures + 1 + 1 + 1`。

### 3.5 标志校验

在 `regex_create`（import 109）中，`regress::Regex::with_flags()` 调用之前：

```rust
const VALID_FLAGS: &[char] = &['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'];

// 检查非法标志
for c in flags.chars() {
    if !VALID_FLAGS.contains(&c) {
        *caller.data().runtime_error.lock().unwrap() =
            Some(format!("SyntaxError: Invalid regular expression flags: '{}'", c));
        return value::encode_undefined();
    }
}

// 检查重复标志
let mut seen = [false; 128];
for c in flags.chars() {
    let idx = c as usize;
    if idx < 128 {
        if seen[idx] {
            *caller.data().runtime_error.lock().unwrap() =
                Some(format!("SyntaxError: Duplicate regular expression flag: '{}'", c));
            return value::encode_undefined();
        }
        seen[idx] = true;
    }
}
```

然后将仅引擎相关标志（`i`、`m`、`s`、`u`、`v`）传给 regress：

```rust
let engine_flags: String = flags.chars().filter(|c| matches!(c, 'i' | 'm' | 's' | 'u' | 'v')).collect();
match regress::Regex::with_flags(&pattern, &engine_flags) { ... }
```

`RegexEntry.flags` 保存完整 flags（含 `g`/`y`/`d`），供后续 lastIndex 和 indices 逻辑使用。

### 3.6 `d` 标志的 `lastIndex` 行为

`d` 标志不影响匹配行为，仅影响结果格式。lastIndex 更新逻辑无需修改。

## 4. 不改动的部分

| 项目 | 原因 |
|---|---|
| IR / semantic / backend | regress 引擎已支持全部语法，无需 IR 层改动 |
| `replaceAll` 的 RegExp 支持 | 独立功能缺口，非本次范围 |
| test262 集成 | 无 test262 RegExp 目录，可后续建立 |
| `regress` 升级 | 当前 0.11.1 已满足全部需求 |
| `RegExp` 构造函数路径 (`new RegExp(...)`) | 走通用构造函数路径，非本次重点 |

## 5. 测试策略

### 5.1 Happy-path Fixtures

每个特性至少一个 E2E fixture（`fixtures/happy/` + `.expected`）：

| Fixture | 覆盖 |
|---|---|
| `regexp_named_groups.js` | 命名捕获组创建 + exec 结果 `.groups` |
| `regexp_lookbehind.js` | 正/反向 lookbehind 断言匹配 |
| `regexp_unicode_prop.js` | `\p{Letter}` / `\P{...}` 匹配 |
| `regexp_dotall.js` | `s` 标志下 `.` 匹配换行 |
| `regexp_hasindices.js` | `d` 标志下 `.indices` 数组 |
| `regexp_replace_named.js` | `replace()` 中 `$<name>` 引用 |
| `regexp_flags_invalid.js` (error) | 非法/重复标志 → SyntaxError |

### 5.2 现有 Fixtures 回归

确保现有 RegExp fixtures 全部通过：

```
regex_literal, regex_exec, regex_test, regex_invalid, string_match, string_match_global
```

### 5.3 边界情况

- 无名分组时 `.groups` 为 `undefined`（非空对象）
- 未参与匹配的命名组 → `undefined`
- `$<name>` 中 name 不存在 → 空字符串（按 ES 规范）
- lookbehind 中的捕获组能正确提取
- Unicode 属性转义匹配多字节字符

## 6. 实现顺序

1. 标志校验（`regex_create`）
2. `regex_exec`：`.index` + `.input` + `.groups`
3. `process_replacement`：`$<name>`
4. `string_match` 非全局路径：同 exec 补全
5. `matchAll`：`.groups`
6. `d` 标志：`.indices`（含 `.indices.groups`）
7. Fixture 编写 + 全量回归测试
