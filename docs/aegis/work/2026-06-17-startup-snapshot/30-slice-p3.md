# Slice Card: P3 - 固定 primordial 字符串表

- **Goal**: 将 Array.prototype 方法名等 34 个 primordial 字符串固定在 data section 固定偏移，使不同编译产物的 name_id 一致，作为 snapshot ABI hash 输入。
- **Parent plan**: `docs/aegis/plans/2026-06-17-startup-snapshot.md` P3
- **Files**:
  - modify: `crates/wjsm-ir/src/constants.rs` — 新增所有 primordial string 偏移常量，更新 `USER_STRING_START`
  - modify: `crates/wjsm-backend-wasm/src/compiler_module.rs` — 预写 primordial strings 到 data section，填充 `string_ptr_cache`
  - add test: `crates/wjsm-backend-wasm/tests/primordial_strings.rs` — 两次不同源码编译后 primordial offset 完全一致
- **Boundary**: 不改变 `intern_data_string` 签名；不改变 `compile_bootstrap_once_function`/`compile_init_function_props_function` 的代码（它们通过 cache 命中拿到固定 offset）；不改变 runtime 行为；fixture 输出不变。
- **Verification**:
  ```bash
  cargo nextest run -p wjsm-backend-wasm -E 'test(primordial_strings) or test(startup_bootstrap_exports) or test(compile_exports_runtime_contract)'
  cargo nextest run -E 'test(happy__)'
  ```
- **Stop**: 全部测试通过后切 P4。
