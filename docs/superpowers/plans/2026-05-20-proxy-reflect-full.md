# wjsm Proxy 13个陷阱与 Reflect API 完整实现执行计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `wjsm` 运行时全面实现 13 个 Proxy 陷阱以及与之对应的完整 13 个 `Reflect` 静态方法，确保严谨的对象可扩展性（Extensibility）校验与 Proxy 不变式（Invariants）校验。

**Architecture:** 采用 Option 1 (Side Table in RuntimeState) 来存储不可扩展对象，在 `native_call_fn` 中检测 `new_target` 以路由 `construct` 陷阱，在 `core.rs` 中抽象属性定义逻辑 `define_property_internal`。

**Tech Stack:** Rust, WebAssembly, wasmtime, swc_core.

---

## User Review Required

> [!NOTE]
> 本计划不需要用户做过多决策，我们已选定：
> 1. 可扩展性侧表存储于 `RuntimeState::non_extensible_handles` 中，避开 GC 堆分配的改动。
> 2. Proxy 的 `new` 调用（构造函数拦截）在 `native_call_fn` 宿主层面捕获，无需改动 IR 或 WASM 编译器后端。

---

## Proposed Changes

### [wjsm-runtime]

#### [MODIFY] [lib.rs](file:///home/soeur/project/wjsm/crates/wjsm-runtime/src/lib.rs)
- 在 `RuntimeState` 结构体中新增 `non_extensible_handles: Arc<Mutex<HashSet<u32>>>`。
- 在 `execute_with_writer` / `RuntimeState` 实例化处将其初始化并克隆 to `store`。

#### [MODIFY] [core.rs](file:///home/soeur/project/wjsm/crates/wjsm-runtime/src/host_imports/core.rs)
- 提取通用的 `define_property_internal(caller, target, name_id, desc_val) -> Result<bool, String>`，使得 `Object.defineProperty` 与 `Reflect.defineProperty` 共享相同的只读/不可配置/不可扩展校验规则。
- 在 `is_callable_in_runtime` 函数中加入递归的 Proxy Target 级校验。

#### [MODIFY] [proxy_traps.rs](file:///home/soeur/project/wjsm/crates/wjsm-runtime/src/host_imports/proxy_traps.rs)
- 补全基础陷阱（如 `getPrototypeOf`, `setPrototypeOf`, `isExtensible`, `preventExtensions`, `getOwnPropertyDescriptor`, `defineProperty`, `ownKeys`）的底层宿主拦截逻辑。

#### [MODIFY] [promise_async.rs](file:///home/soeur/project/wjsm/crates/wjsm-runtime/src/host_imports/promise_async.rs)
- 在 `native_call_fn` 中，若有 `new_target` 压栈，则路由到 Proxy 的 `construct` 陷阱。
- 补全 `Reflect.apply`, `Reflect.construct` 以及 argumentsList 类数组对象元素的展开逻辑 `extract_array_like_elements`。
- 实现 `Reflect` 原型链循环检测 `is_prototype_circular_chain` 逻辑，预防死循环。

---

## Bite-Sized Tasks

### Task 1: 状态扩展与运行时可调用性校验 `is_callable_in_runtime`

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs:160-170,430-496`
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs:1-100`

- [ ] **Step 1: 在 `RuntimeState` 结构体中新增 `non_extensible_handles` 字段并完成初始化**
  修改 `crates/wjsm-runtime/src/lib.rs` 的 `struct RuntimeState`：
  ```rust
  non_extensible_handles: Arc<Mutex<HashSet<u32>>>,
  ```
  在 `execute_with_writer` 中初始化它：
  ```rust
  let non_extensible_handles = Arc::new(Mutex::new(HashSet::new()));
  ```
  并在 `RuntimeState` 实例化（包括 `store` 创建时）传入。
- [ ] **Step 2: 在 `core.rs` 中编写或重构 `is_callable_in_runtime` 递归判断**
  ```rust
  pub(crate) fn is_callable_in_runtime(caller: &mut Caller<'_, RuntimeState>, val: i64) -> bool {
      if value::is_function(val) || value::is_closure(val) {
          return true;
      }
      if value::is_proxy(val) {
          let handle = value::decode_proxy_handle(val) as usize;
          let entry = {
              let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
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
- [ ] **Step 3: 运行 `cargo check` 确保能够编译通过**
  Run: `cargo check`
  Expected: SUCCESS

---

### Task 2: 提取并重构属性定义逻辑 `define_property_internal`

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs:100-300`

- [ ] **Step 1: 提取 `define_property_internal` 并实现完整的不变式校验**
  ```rust
  fn define_property_internal(
      caller: &mut Caller<'_, RuntimeState>,
      target: i64,
      name_id: i32,
      desc_val: i64,
  ) -> Result<bool, String> {
      // 1. 若 target 为 Proxy，则分发 defineProperty 陷阱
      if value::is_proxy(target) {
          let handle = value::decode_proxy_handle(target) as usize;
          let entry = {
              let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
              table.get(handle).cloned()
          };
          if let Some(entry) = entry {
              if entry.revoked {
                  return Err("TypeError: Cannot perform 'defineProperty' on a proxy that has been revoked".to_string());
              }
              if let Some(handler_ptr) = resolve_handle(caller, entry.handler) {
                  let trap = read_object_property_by_name(caller, handler_ptr, "defineProperty")
                      .unwrap_or_else(value::encode_undefined);
                  if !value::is_undefined(trap) && !value::is_null(trap) {
                      let prop = property_key_value(caller, name_id);
                      let result = call_trap_with_args(caller, trap, entry.handler, &[entry.target, prop, desc_val]);
                      let success = nanbox_to_bool(result);
                      // Invariant check: if success is true, validate target invariants
                      if success {
                          let ext = is_extensible_impl(caller, entry.target);
                          let prop_name = read_string(caller, name_id as u32).unwrap_or_default();
                          let Some(t_ptr) = resolve_handle(caller, entry.target) else { return Ok(true); };
                          let Some(name_c) = find_memory_c_string(caller, &prop_name) else { return Ok(true); };
                          let exists = find_property_slot_by_name_id(caller, t_ptr, name_c).is_some();
                          if !ext && !exists {
                              return Err("TypeError: Proxy defineProperty invariant violated: target is not extensible and property does not exist".to_string());
                          }
                      }
                      return Ok(success);
                  }
              }
              // 退回 target 递归定义
              return define_property_internal(caller, entry.target, name_id, desc_val);
          }
      }

      // 2. 普通对象定义
      let Some(ptr) = resolve_handle(caller, target) else {
          return Ok(false);
      };
      
      let ext = is_extensible_impl(caller, target);
      let prop_name = read_string(caller, name_id as u32).unwrap_or_default();
      let Some(name_c) = find_memory_c_string(caller, &prop_name) else {
          return Ok(false);
      };
      let slot = find_property_slot_by_name_id(caller, ptr, name_c);
      
      if !ext && slot.is_none() {
          return Ok(false);
      }

      let desc_ptr = resolve_handle(caller, desc_val);
      let mut val = value::encode_undefined();
      let mut flags = constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
      if let Some(dp) = desc_ptr {
          if let Some(v) = read_object_property_by_name(caller, dp, "value") {
              val = v;
          }
          if let Some(w) = read_object_property_by_name(caller, dp, "writable") {
              if !nanbox_to_bool(w) {
                  flags &= !constants::FLAG_WRITABLE;
              }
          }
          if let Some(c) = read_object_property_by_name(caller, dp, "configurable") {
              if !nanbox_to_bool(c) {
                  flags &= !constants::FLAG_CONFIGURABLE;
              }
          }
          if let Some(e) = read_object_property_by_name(caller, dp, "enumerable") {
              if !nanbox_to_bool(e) {
                  flags &= !constants::FLAG_ENUMERABLE;
              }
          }
      }

      if let Some((_, orig_flags, orig_val)) = slot {
          let orig_configurable = (orig_flags & constants::FLAG_CONFIGURABLE) != 0;
          if !orig_configurable {
              let new_configurable = (flags & constants::FLAG_CONFIGURABLE) != 0;
              let new_enumerable = (flags & constants::FLAG_ENUMERABLE) != 0;
              let orig_enumerable = (orig_flags & constants::FLAG_ENUMERABLE) != 0;
              if new_configurable || (new_enumerable != orig_enumerable) {
                  return Ok(false);
              }
              let orig_writable = (orig_flags & constants::FLAG_WRITABLE) != 0;
              let new_writable = (flags & constants::FLAG_WRITABLE) != 0;
              if !orig_writable && new_writable {
                  return Ok(false);
              }
              if !orig_writable && orig_val != val && !value::is_undefined(val) {
                  return Ok(false);
              }
          }
      }

      write_object_property_by_name_id(caller, ptr, target, name_c as u32, val, flags);
      Ok(true)
  }
  ```
- [ ] **Step 2: 重构 `Object.defineProperty` 与 `Reflect.defineProperty`，调用共享函数**
  在 `core.rs` 或 `promise_async.rs` 中重写 `reflect_define_property_fn`：
  ```rust
  |mut caller, target, prop, descriptor| -> i64 {
      let Ok(prop_name) = render_value(&mut caller, prop) else { return value::encode_bool(false); };
      let Some(name_id) = find_memory_c_string(&mut caller, &prop_name) else { return value::encode_bool(false); };
      match define_property_internal(&mut caller, target, name_id, descriptor) {
          Ok(success) => value::encode_bool(success),
          Err(err) => {
              set_runtime_error(caller.data(), err);
              value::encode_bool(false)
          }
      }
  }
  ```
- [ ] **Step 3: 运行 `cargo test` 验证现存的 `define_property` 相关测试仍然通过**
  Run: `cargo test`
  Expected: PASS

---

### Task 3: 实现可扩展性侧表逻辑（isExtensible 与 preventExtensions）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1713-1726`

- [ ] **Step 1: 实现普通对象的可扩展性校验与防改写逻辑**
  ```rust
  fn is_extensible_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> bool {
      if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) {
          return false;
      }
      let handle = if value::is_object(target) {
          value::decode_object_handle(target)
      } else if value::is_array(target) {
          value::decode_array_handle(target)
      } else if value::is_proxy(target) {
          value::decode_proxy_handle(target)
      } else {
          return true;
      };
      let set = caller.data().non_extensible_handles.lock().expect("non_extensible_handles mutex");
      !set.contains(&handle)
  }

  fn prevent_extensions_impl(caller: &mut Caller<'_, RuntimeState>, target: i64) -> bool {
      if !value::is_object(target) && !value::is_array(target) && !value::is_function(target) {
          return false;
      }
      let handle = if value::is_object(target) {
          value::decode_object_handle(target)
      } else if value::is_array(target) {
          value::decode_array_handle(target)
      } else if value::is_proxy(target) {
          value::decode_proxy_handle(target)
      } else {
          return false;
      };
      let mut set = caller.data().non_extensible_handles.lock().expect("non_extensible_handles mutex");
      set.insert(handle);
      true
  }
  ```
- [ ] **Step 2: 绑定 `isExtensible` 与 `preventExtensions` 陷阱与 Reflect 静态接口**
  在 `promise_async.rs` 中重写 `reflect_is_extensible_fn` 和 `reflect_prevent_extensions_fn`：
  （同上文中的 detail）
- [ ] **Step 3: 运行 `cargo test` 确认代码编译无误且现有用例测试通过**
  Run: `cargo test`
  Expected: PASS

---

### Task 4: 实现原型链修改（setPrototypeOf）与循环检测

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1706-1712`

- [ ] **Step 1: 编写原型链循环检测 `is_prototype_circular_chain` 辅助函数**
  （同上文中的 detail）
- [ ] **Step 2: 实现 `Reflect.setPrototypeOf` 并融入 Proxy 陷阱与不变式校验**
  （同上文中的 detail）
- [ ] **Step 3: 运行 `cargo test` 确认通过**
  Run: `cargo test`
  Expected: PASS

---

### Task 5: 补全 Reflect.apply, Reflect.construct 及其构造调用拦截

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1240-1300,1630-1675`

- [ ] **Step 1: 实现 argumentsList 类数组对象展开逻辑 `extract_array_like_elements`**
  （同上文中的 detail）
- [ ] **Step 2: 重构 `Reflect.apply` 与 `Reflect.construct` 支持完整陷阱分发**
  （同上文中的 detail）
- [ ] **Step 3: 重构 `native_call_fn` 拦截 `new_target` 构造上下文路由到 `construct` 陷阱**
  根据 Spec 第 3 部分，修改 `native_call_fn` 处理 `is_proxy(callable)` 的分支逻辑。
- [ ] **Step 4: 运行 `cargo test` 确认无编译错误并且通过**
  Run: `cargo test`
  Expected: PASS

---

### Task 6: 补开 ownKeys 陷阱、不变式检查与编写 Happy/Error Fixture

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1808-1820`
- Create: `fixtures/happy/proxy_traps_full.js`
- Create: `fixtures/happy/proxy_traps_full.expected`

- [ ] **Step 1: 重构 `Reflect.ownKeys` 并应用 Proxy `ownKeys` 陷阱与展开逻辑**
  （同上文中的 detail）
- [ ] **Step 2: 编写完整的 `proxy_traps_full.js` 测试，覆盖 13 个陷阱和 Reflect API**
  编写包含 `Reflect.isExtensible`, `Reflect.preventExtensions`, `Reflect.setPrototypeOf`, `Reflect.ownKeys`, `Reflect.construct` 以及 Proxy 所有 13 个陷阱调用的专项 Happy Fixture。
- [ ] **Step 3: 运行测试并自动构建 snapshot 期望文件**
  Run: `WJSM_UPDATE_FIXTURES=1 cargo test`
  Expected: E2E 自动成功更新，单元与集成测试均 PASS。
- [ ] **Step 4: Commit 所有改动**
  Run: `git add . && git commit -m "feat: complete all 13 proxy traps and 13 reflect api with full invariants checks"`
  Expected: SUCCESS

---

## Verification Plan

### Automated Tests
- 执行 `cargo test` 运行所有现存的 happy/errors 路径用例，确保没有任何 regression。
- 运行 `cargo run -- run fixtures/happy/proxy_traps_full.js`，验证输出是否与 JS Proxy 行为完全一致。
- 运行 test262 的 Proxy/Reflect 相关套件。

### Manual Verification
- 检查 `wjsm` 是否对非对象抛出正确的 `TypeError`（比如 `Reflect.setPrototypeOf(null, {})`）。
- 检查原型链循环引用时 `Reflect.setPrototypeOf` 是否正确返回 `false`。
