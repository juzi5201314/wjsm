# wjsm Proxy 13个陷阱与 Reflect API 完整实现设计文档

本文档定义了在 `wjsm` 运行时中全面实现 13 个 Proxy 陷阱以及与之对应的 `Reflect` 静态方法的设计方案。

## 1. 整体架构与状态扩展

为了符合 ECMAScript 规范对 Proxy 不变式（Invariants）及对象可扩展性（Extensibility）的严谨要求，我们需要在运行时状态中引入新的侧表，并扩展 callability（可调用性）的检测规则。

### 1.1 `RuntimeState` 状态扩展
在 `crates/wjsm-runtime/src/lib.rs` 中，为 `RuntimeState` 增加 `non_extensible_handles` 侧表：
```rust
struct RuntimeState {
    // ... 现有字段 ...
    /// 记录被 `preventExtensions` 标记为不可扩展对象的 handle 集合
    non_extensible_handles: Arc<Mutex<HashSet<u32>>>,
}
```

### 1.2 运行时可调用性校验 `is_callable_in_runtime`
在运行时层，对于 Proxy 类型的对象，需要沿着 `[[ProxyTarget]]` 递归检查其最终目标是否可调用：
```rust
pub(crate) fn is_callable_in_runtime(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
    if value::is_callable(val) {
        return true;
    }
    if value::is_proxy(val) {
        let handle = value::decode_proxy_handle(val) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().unwrap();
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if !entry.revoked {
                return is_callable_in_runtime(caller, entry.target);
            }
        }
    }
    false
}
```

### 1.3 类数组参数解析 `extract_array_like_elements`
为了支持 `Reflect.apply` 和 `Reflect.construct` 的 argumentsList 参数，以及 `ownKeys` 的返回值解析，我们在运行时增加提取类数组对象所有元素的辅助函数：
- 若为 `TAG_ARRAY`：直接读取其连续的元素内存。
- 否则：通过 `read_object_property_by_name(caller, ptr, "length")` 提取长度，若为数字则循环读取 `0..length` 属性。

---

## 2. 13 个 Proxy 陷阱与 Reflect 静态方法设计

下面定义所有 13 个内置操作的具体运行逻辑 and 陷阱分发规则：

### 1. `Reflect.get` / `get` 陷阱 (已基本实现)
- **Reflect Signature**: `Reflect.get(target, propertyKey, receiver)`
- **Behavior**:
  - 若 `target` 是 Proxy，触发 `get` 陷阱。若无陷阱或被撤销，递归调用 `Reflect.get`。
  - 否则，读取 `target` 的对应属性值并返回。

### 2. `Reflect.set` / `set` 陷阱 (已基本实现)
- **Reflect Signature**: `Reflect.set(target, propertyKey, value, receiver)`
- **Behavior**:
  - 若 `target` 是 Proxy，触发 `set` 陷阱，返回布尔值。
  - 否则，在 `target` 上写入/重写属性值，并返回 `true`。

### 3. `Reflect.has` / `has` 陷阱 (已基本实现)
- **Reflect Signature**: `Reflect.has(target, propertyKey)`
- **Behavior**:
  - 若 `target` 是 Proxy，触发 `has` 陷阱。
  - 否则，检查属性是否存在于原型链中，返回布尔值。

### 4. `Reflect.deleteProperty` / `deleteProperty` 陷阱 (已基本实现)
- **Reflect Signature**: `Reflect.deleteProperty(target, propertyKey)`
- **Behavior**:
  - 若 `target` 是 Proxy，触发 `deleteProperty` 陷阱。
  - 否则，若属性不可配置，返回 `false`；否则从对象中 swap-remove 属性，返回 `true`。

### 5. `Reflect.getPrototypeOf` / `getPrototypeOf` 陷阱 (已基本实现)
- **Reflect Signature**: `Reflect.getPrototypeOf(target)`
- **Behavior**:
  - 若 `target` 是 Proxy，触发 `getPrototypeOf` 陷阱。
  - 否则，读取对象的 `proto_handle`（0..4字节），转化为 nanbox 对象返回。

### 6. `Reflect.setPrototypeOf` / `setPrototypeOf` 陷阱
- **Reflect Signature**: `Reflect.setPrototypeOf(target, prototype)`
- **Behavior**:
  - `target` 非对象抛 `TypeError`；`prototype` 非对象且非 null 抛 `TypeError`。
  - 若为 Proxy，分发 `setPrototypeOf` 陷阱，校验陷阱返回值（若返回 true 且 target 不可扩展，则要求 prototype 必须与 target 的现有原型一致）。
  - 若为普通对象：
    - 若 `target` 不可扩展，且 `prototype` 不等于其当前原型，返回 `false`。
    - **原型链循环检测**：通过不断获取 `prototype` 的原型，检查是否存在 `target`。若存在循环，返回 `false`。
    - 写入 `proto_handle` 对应的 4 字节，返回 `true`。

### 7. `Reflect.isExtensible` / `isExtensible` 陷阱
- **Reflect Signature**: `Reflect.isExtensible(target)`
- **Behavior**:
  - `target` 非对象抛 `TypeError`。
  - 若为 Proxy，分发 `isExtensible` 陷阱。**不变式校验**：陷阱的布尔返回值必须与 `isExtensible(target)` 的真实结果完全一致，否则抛出 `TypeError`。
  - 若为普通对象，查询 `non_extensible_handles` 侧表，若不存在则返回 `true`，存在则返回 `false`。

### 8. `Reflect.preventExtensions` / `preventExtensions` 陷阱
- **Reflect Signature**: `Reflect.preventExtensions(target)`
- **Behavior**:
  - `target` 非对象抛 `TypeError`。
  - 若为 Proxy，分发 `preventExtensions` 陷阱。**不变式校验**：如果陷阱返回 `true`，但此时 `isExtensible(target)` 仍为 `true`，必须抛出 `TypeError`。
  - 若为普通对象，将它的 handle ID 插入 `non_extensible_handles`，并返回 `true`。

### 9. `Reflect.getOwnPropertyDescriptor` / `getOwnPropertyDescriptor` 陷阱
- **Reflect Signature**: `Reflect.getOwnPropertyDescriptor(target, propertyKey)`
- **Behavior**:
  - 若为 Proxy，分发 `getOwnPropertyDescriptor` 陷阱，将返回的描述符对象转换为标准 nanbox 描述符对象（包含 value, writable, enumerable, configurable 等）。
  - 否则，读取对象的特定属性插槽，生成对应的描述符对象并返回。

### 10. `Reflect.defineProperty` / `defineProperty` 陷阱
- **Reflect Signature**: `Reflect.defineProperty(target, propertyKey, attributes)`
- **Behavior**:
  - 统一在 `core.rs` 中抽象出一个 `define_property_internal(caller, target, name_id, desc_val) -> Result<bool, String>`。
  - 如果是 Proxy，分发 `defineProperty` 陷阱并强类型转换返回值。
  - 内部实现（普通对象）：
    - 解析 `attributes` 描述符对象的 `value`、`writable`、`configurable` 等属性。
    - 若属性原本存在且不可配置，校验其属性变更是否违反只读/不可配置不变式（若是则抛出对应异常或返回 `false`）。
    - 若属性原本不存在且 `target` 不可扩展，返回 `false`。
    - 将更新后的 flags 和 value 写入属性插槽中。

### 11. `Reflect.ownKeys` / `ownKeys` 陷阱
- **Reflect Signature**: `Reflect.ownKeys(target)`
- **Behavior**:
  - 若为 Proxy，分发 `ownKeys` 陷阱。
  - 使用 `extract_array_like_elements` 将陷阱返回的类数组展开为 key 列表。
  - **不变式校验**：
    - 每个 key 必须为 String 或 Symbol 类型。
    - 目标对象的所有不可配置属性必须包含在返回列表中。
    - 如果目标对象不可扩展，返回列表必须包含且仅包含目标对象的所有自有属性。
  - 否则（普通对象），通过 `collect_own_property_names` 收集自有属性列表，并构造成 `TAG_ARRAY` 返回。

### 12. `Reflect.apply` / `apply` 陷阱
- **Reflect Signature**: `Reflect.apply(target, thisArgument, argumentsList)`
- **Behavior**:
  - 校验 `target` 是否可调用（使用 `is_callable_in_runtime`）。
  - 通过 `extract_array_like_elements` 提取参数列表。
  - 将所有提取到的参数写入 shadow stack，调用 `resolve_and_call(caller, target, thisArgument, args_base, args_count)`。

### 13. `Reflect.construct` / `construct` 陷阱
- **Reflect Signature**: `Reflect.construct(target, argumentsList[, newTarget])`
- **Behavior**:
  - 校验 `target` 和 `newTarget` 是否可调用。若 `newTarget` 缺省，则默认为 `target`。
  - 若 `target` 是 Proxy，分发 `construct` 陷阱。校验其返回值必须是 Object 类型。
  - 否则，提取 `newTarget.prototype` 作为新对象的原型（若非对象则默认使用 `Object.prototype`）。
  - 分配新对象，并调用 `resolve_and_call` 执行构造逻辑，最终返回该实例。

---

## 3. `new Proxy` 静态调用分发设计 (方案 A 落地)

对于 `new proxy(...)` 和 `Reflect.construct`，我们需要保证 Proxy 的 `construct` 陷阱能被正确路由。

在 `crates/wjsm-runtime/src/host_imports/promise_async.rs` 的 `native_call_fn` 中进行以下拦截：
```rust
let new_tgt = caller.data().new_target.get();
caller.data().new_target.set(value::encode_undefined());

if value::is_proxy(callable) {
    if !value::is_undefined(new_tgt) {
        // 这是构造调用！路由到 Proxy 的 construct 陷阱
        let handle = value::decode_proxy_handle(callable) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().unwrap();
            table.get(handle).cloned()
        };
        if let Some(entry) = entry {
            if entry.revoked {
                set_runtime_error(caller.data(), "TypeError: Cannot perform 'construct' on a proxy that has been revoked".to_string());
                return value::encode_undefined();
            }
            if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct")
                    .unwrap_or_else(value::encode_undefined);
                if !value::is_undefined(trap) && !value::is_null(trap) {
                    // 构建参数数组
                    let arr = alloc_array(&mut caller, args_count as u32);
                    for i in 0..args_count {
                        let arg = read_shadow_arg(&mut caller, args_base, i as u32);
                        set_array_elem(&mut caller, arr, i, arg);
                    }
                    // 调用 construct 陷阱：trap_args = [target, arr, new_target]
                    let result = call_trap_with_args(&mut caller, trap, entry.handler, &[entry.target, arr, new_tgt]);
                    if !is_js_object(result) {
                        set_runtime_error(caller.data(), "TypeError: Proxy construct trap returned non-object".to_string());
                    }
                    return result;
                }
            }
            // 退回到对 target 的构造调用
            caller.data().new_target.set(new_tgt); // 恢复构造上下文
            return resolve_and_call(&mut caller, entry.target, value::encode_undefined(), args_base, args_count);
        }
    } else {
        // 普通函数调用，路由到 Proxy 的 apply 陷阱（保留现有逻辑）
        // ...
    }
}
```

---

## 4. 验证规划与测试套件

我们将在 `fixtures/happy/` 下增加丰富的 Proxy 专项测试：
1. `proxy_traps_extensibility.js`：测试 `isExtensible`、`preventExtensions` 及对应陷阱。
2. `proxy_traps_prototype.js`：测试 `getPrototypeOf`、`setPrototypeOf` 原型链循环检测以及对应的陷阱与不变式约束。
3. `proxy_traps_define_property.js`：测试 `defineProperty` 属性描述符及不变式检查。
4. `proxy_traps_own_keys.js`：测试 `ownKeys` 展开及不变式检查。
5. `proxy_traps_construct.js`：测试 `new Proxy(...)` 构造调用和 `Reflect.construct`。
6. `reflect_api_full.js`：测试 Reflect API 的 13 个静态方法。

测试执行命令：
```bash
# 运行单元测试与集成测试
cargo test
# 运行 test262 中相关的 Proxy / Reflect 测试
cargo run -p wjsm-test262 -- run --suite test/built-ins/Proxy --all --plain
cargo run -p wjsm-test262 -- run --suite test/built-ins/Reflect --all --plain
```
