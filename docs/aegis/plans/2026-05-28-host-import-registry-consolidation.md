# Plan: Host Import Registry Consolidation

**Date:** 2026-05-28
**Goal:** 把 `wjsm-backend-wasm` 中 host import 的名字、顺序、WASM type、Builtin 绑定与特殊索引查询收敛到单一 owner，删除 `HOST_IMPORT_NAMES`、硬编码 `func_idx`、魔数范围与过时编号注释，避免后续新增/调整 host import 时再次出现索引漂移和错误 `call`。

**Architecture:** 只重构 backend 的 host import owner 与索引派生链；runtime 继续按名字链接，不改变 JS 语义、不改变 host import 名字和签名。

**Tech Stack:** Rust 2024、`wasm-encoder`、`wasmparser`、`wjsm-ir::Builtin`、`wasmtime`。

**Baseline/Authority Refs:**
- Spec: `docs/aegis/specs/2026-05-28-host-import-registry-design.md`
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-backend-wasm/src/compiler_core.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- `crates/wjsm-backend-wasm/src/compiler_instructions.rs`
- `crates/wjsm-backend-wasm/src/compiler_control.rs`
- `crates/wjsm-backend-wasm/src/compiler_data.rs`
- `crates/wjsm-backend-wasm/src/compiler_module.rs`
- `crates/wjsm-ir/src/builtin.rs`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/host_imports/`

**Compatibility Boundary:**
- **Must NOT change** runtime 通过 `linker.define("env", name, ...)` 按名字链接的机制。
- **Must NOT change** 任一现有 host import 的名字与函数签名。
- **Must NOT reorder** 当前 host import 声明顺序；本计划只收敛 owner，不洗牌。
- **Must remove** backend 中所有 imported host function 的数字 `Call(N)`、`unwrap_or(N)`、`= N` 初始化、`50..=76` 这类顺序假设。
- **Must keep** backend 内部 helper（如 `$obj_new`、`$obj_get`、`$to_int32`）与 imported host function 索引分离。

**Verification:**
- `cargo test -p wjsm-backend-wasm --test host_import_registry`
- `cargo check -p wjsm-ir -p wjsm-runtime`
- `cargo nextest run -E 'test(happy__hello) | test(happy__closure_counter) | test(happy__object_spread) | test(happy__new_target_constructor_context) | test(happy__proxy_traps_full) | test(happy__eval_scope_record) | test(happy__typedarray_full) | test(happy__throw_uncaught)'`
- Harness 搜索验证：backend 不再保留 host import 魔数；runtime 不再保留 `Import N` 这类编号 authority 注释

---

## Plan Basis

### Problem
当前同一组 host import 真相分散在：
1. `HOST_IMPORT_NAMES`
2. `compiler_core.rs` 里的 `imports.import(...)`
3. `Builtin::import_name()` + `ALL_BUILTINS`（IR 层携带 backend 关注点，是层边界泄露）
4. `Compiler` 初始化里的 `*_func_idx = 22/35/320/...`
5. 多个 codegen 文件中的 `Call(16)`、`Call(17)`、`unwrap_or(313)`、`50..=76`
6. runtime 大量 `Import 321`、`index 358` 之类注释

真正出错的是 backend 继续按过期数字索引发 `call`；runtime 实际按名字链接，本身不是根因。

### Success Evidence
- backend 新增/调整 host import 时只需要改一个 owner 文件
- 关键 call site 全部走命名 lookup
- registry 缺项时明确报错，不再默默 fallback 到错误数字
- 现有关键 fixtures 通过

---

## Files

| Kind | Path | Reason |
|------|------|--------|
| create | `crates/wjsm-backend-wasm/src/host_import_registry.rs` | 新 canonical owner：host import 规格、键、分组、lookup builder |
| modify | `crates/wjsm-backend-wasm/src/lib.rs` | 删除 `HOST_IMPORT_NAMES`，接入 registry 模块与新字段 |
| modify | `crates/wjsm-backend-wasm/src/compiler_core.rs` | 用 registry 生成 import section / export / import count / lookup |
| modify | `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | 去掉 builtin fallback 魔数 |
| modify | `crates/wjsm-backend-wasm/src/compiler_instructions.rs` | 去掉 `Call(16)` / `Call(17)` 与特殊 import 魔数 |
| modify | `crates/wjsm-backend-wasm/src/compiler_control.rs` | 去掉 `CreateException` fallback 魔数 |
| modify | `crates/wjsm-backend-wasm/src/compiler_data.rs` | 去掉 `AbortShadowStackOverflow` fallback 魔数 |
| modify | `crates/wjsm-backend-wasm/src/compiler_module.rs` | 去掉 `50..=76` 区间与 `obj_spread` 手写位置查找 |
| modify | `crates/wjsm-backend-wasm/src/compiler_helpers.rs` | 去掉 `gc_collect_func_idx` / `proxy_trap_get_func_idx` 等直接字段引用 |
| modify | `crates/wjsm-backend-wasm/src/compiler_array_helpers.rs` | 去掉 `gc_collect_func_idx` / `obj_get_by_index_func_idx` / `typedarray_set_by_index_func_idx` 等直接字段引用 |
| modify | `crates/wjsm-ir/src/builtin.rs` | 删除只服务旧 backend 映射的 `import_name()` / `ALL_BUILTINS`（若确认无其他引用） |
| create | `crates/wjsm-backend-wasm/tests/host_import_registry.rs` | registry 不变量和编译产物验证 |
| modify | `crates/wjsm-runtime/src/lib.rs` | 去除编号 authority 注释 |
| modify | `crates/wjsm-runtime/src/host_imports/*.rs` | 去除编号 authority 注释，保留分组说明 |

---

## Plan Pressure Test

- **Owner / contract / retirement:** 新 owner 是 `host_import_registry.rs`；旧 owner（数组、名字反查、数字 fallback）一次性退休。
- **Verification scope:** registry 不变量、backend compile 结果、8 个高价值 fixtures、runtime 编译。
- **Task executability:** 可以按“建 owner → 切换生成链 → 清理 call site → 清理 runtime 注释”顺序独立落地。
- **Pressure result:** proceed

## Plan-Time Complexity Check

- **Target files:** `lib.rs` / `compiler_core.rs` / `compiler_builtins.rs` / `compiler_instructions.rs` / `compiler_control.rs` / `compiler_data.rs` / `compiler_module.rs` / `compiler_helpers.rs` / `compiler_array_helpers.rs` / runtime `host_imports/*.rs`
- **Existing size / shape signals:** owner 分散、魔数泄漏、多文件同时知道 import 排序
- **Owner fit:** 必须新建 owner 文件，不能继续往大文件里塞数组与索引知识
- **Add-in-place risk:** 高
- **Better file boundary:** `host_import_registry.rs`
- **Recommendation:** add owner file

---

## Tasks

### Task 1: 建立 registry owner 与最小不变量测试
**Files:**
- `crates/wjsm-backend-wasm/src/host_import_registry.rs`
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-backend-wasm/tests/host_import_registry.rs`

**Why:** 先建立单一权威，再让其他文件依赖它；否则后续改动仍会继续猜顺序。

**Impact/Compatibility:** 仅新增 owner 与测试，不删除旧逻辑；行为风险低。

**Repair Track:** 根因是 owner 分散；此任务把 owner 固化到单文件。

**Retirement Track:** 本任务不删除旧数组，只建立替代物并验证其不变量。

**Verification:** `cargo test -p wjsm-backend-wasm --test host_import_registry registry_has_unique_names_and_keys -- --exact`

- [ ] **Write test**
  - 新建 `crates/wjsm-backend-wasm/tests/host_import_registry.rs`。
  - 先写下面两个测试：
    ```rust
    use std::collections::HashSet;
    use wjsm_backend_wasm::host_import_registry::{
        host_import_specs, HostImportGroup, HostImportKey,
    };

    #[test]
    fn registry_has_unique_names_and_keys() {
        let specs = host_import_specs();
        let mut names = HashSet::new();
        let mut builtin_keys = HashSet::new();
        let mut special_keys = HashSet::new();

        for spec in specs {
            assert!(names.insert(spec.name), "duplicate host import name: {}", spec.name);
            match spec.key {
                Some(HostImportKey::Builtin(builtin)) => {
                    assert!(builtin_keys.insert(builtin), "duplicate builtin key: {builtin:?}");
                }
                Some(HostImportKey::Special(special)) => {
                    assert!(special_keys.insert(special), "duplicate special key: {special:?}");
                }
                None => {}
            }
        }
    }

    #[test]
    fn array_prototype_group_is_explicit_not_range_based() {
        let specs = host_import_specs();
        let names: Vec<_> = specs
            .iter()
            .filter(|spec| spec.group == Some(HostImportGroup::ArrayPrototypeMethod))
            .map(|spec| spec.name)
            .collect();

        assert!(names.starts_with(&["arr_proto_push", "arr_proto_pop"]));
        assert!(names.ends_with(&["arr_proto_splice", "arr_proto_is_array"]));
        assert_eq!(names.len(), 27);
    }

    #[test]
    fn all_specs_have_valid_type_indices() {
        // 每个 spec 的 type_idx 必须指向合法的 func type；0 为非法占位
        let specs = host_import_specs();
        for spec in specs {
            assert!(
                spec.type_idx > 0,
                "spec '{}' has invalid type_idx 0", spec.name
            );
        }
    }
    ```
- [ ] **Verify RED**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry registry_has_unique_names_and_keys -- --exact
    ```
  - 预期：编译失败，因为 `host_import_registry` 模块和导出尚不存在。
- [ ] **Minimal code**
  - 新建 `crates/wjsm-backend-wasm/src/host_import_registry.rs`，定义以下核心类型：
    ```rust
    use wjsm_ir::Builtin;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) enum SpecialHostImport {
        StringConcat,
        StringConcatVa,
        GcCollect,
        ClosureCreate,
        ClosureGetFunc,
        ClosureGetEnv,
        NativeCall,
        NewTarget,
        NewTargetSet,
        GetBuiltinGlobal,
        CreateUnmappedArgumentsObject,
        CreateMappedArgumentsObject,
        ProxyTrapGet,
        ProxyTrapSet,
        ProxyTrapDelete,
        SymbolPropertyKey,
        ArrayFrom,
        ObjGetByIndex,
        TypedArraySetByIndex,
        ObjSpread,
    }

    /// AbortShadowStackOverflow / CreateException 是 Builtin 变体，走 builtin_func_indices，
    /// 不在此 SpecialHostImport 枚举内。

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) enum HostImportGroup {
        ArrayPrototypeMethod,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum HostImportKey {
        Builtin(Builtin),
        Special(SpecialHostImport),
    }

    #[derive(Debug, Clone, Copy)]
    pub struct HostImportSpec {
        pub name: &'static str,
        pub type_idx: u32,
        pub key: Option<HostImportKey>,
        pub group: Option<HostImportGroup>,
    }

    pub fn host_import_specs() -> &'static [HostImportSpec] {
        &HOST_IMPORT_SPECS
    }
    ```
  - 在同文件中把当前 `compiler_core.rs` 的每一条 `imports.import("env", name, EntityType::Function(type_idx))` 原样搬到 `HOST_IMPORT_SPECS`，顺序完全保持一致；对存在 `Builtin` 对应关系的项打 `HostImportKey::Builtin(...)`，对需要直接索引的项打 `HostImportKey::Special(...)`，Array prototype 27 项打 `HostImportGroup::ArrayPrototypeMethod`。
  - 在 `crates/wjsm-backend-wasm/src/lib.rs` 中追加：
    ```rust
    pub mod host_import_registry;
    ```
- [ ] **Verify GREEN**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry
    ```
  - 预期：新测试通过。
- [ ] **Commit**
  - 计划中的提交命令：
    ```bash
    git add crates/wjsm-backend-wasm/src/host_import_registry.rs crates/wjsm-backend-wasm/src/lib.rs crates/wjsm-backend-wasm/tests/host_import_registry.rs
    git commit -m "refactor: add host import registry owner"
    ```

---

### Task 2: 用 registry 接管 import section、导出名、count 与 lookup
**Files:**
- `crates/wjsm-backend-wasm/src/compiler_core.rs`
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-ir/src/builtin.rs`

**Why:** 让 `Compiler::new_with_data_base` 不再依赖 `HOST_IMPORT_NAMES` 和 `Builtin::import_name()` 反查。

**Impact/Compatibility:** 仍保持同样的 import 顺序与签名；只改生成路径。

**Repair Track:** 把“顺序、名字、类型、builtin lookup”集中由 registry 派生。

**Retirement Track:** 删除 `HOST_IMPORT_NAMES`；若 `Builtin::import_name()` / `ALL_BUILTINS` 仅剩旧用途，则一起删除。

**Verification:** `cargo test -p wjsm-backend-wasm --test host_import_registry compiler_registry_matches_expected_import_count -- --exact`

- [ ] **Write test**
  - 在 `crates/wjsm-backend-wasm/tests/host_import_registry.rs` 追加：
    ```rust
    use wjsm_backend_wasm::{compile, host_import_registry::host_import_specs};
    use wasmparser::{Parser, Payload};

    #[test]
    fn compiler_registry_matches_expected_import_count() {
        let module = wjsm_parser::parse_module(r#"console.log('hello');"#).expect("parse");
        let program = wjsm_semantic::lower_module(module, false).expect("lower");
        let wasm = compile(&program).expect("compile");

        let import_count = Parser::new(0)
            .parse_all(&wasm)
            .filter_map(|payload| match payload.expect("payload") {
                Payload::ImportSection(section) => Some(section.count()),
                _ => None,
            })
            .next()
            .expect("import section");

        assert_eq!(import_count as usize, host_import_specs().len());
    }
    ```
- [ ] **Verify RED**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry compiler_registry_matches_expected_import_count -- --exact
    ```
  - 预期：编译失败或测试失败，因为 compiler 仍未从 registry 派生 import section/count。
- [ ] **Minimal code**
  - 在 `compiler_core.rs`：
    - `use crate::host_import_registry::{host_import_specs, HostImportKey, SpecialHostImport};`
    - 用 `for spec in host_import_specs()` 替换全部 host-function `imports.import(...)` 手写行，生成：
      ```rust
      for spec in host_import_specs() {
          imports.import("env", spec.name, EntityType::Function(spec.type_idx));
      }
      ```
    - 构造 `builtin_func_indices` 与 `special_host_import_indices`：遍历 `host_import_specs().iter().enumerate()`，按 `spec.key` 分类填入 `HashMap`。
    - 导出 imported host function 名字时，遍历 `host_import_specs()`，不再遍历 `HOST_IMPORT_NAMES`。
    - `actual_import_count` 改为 `host_import_specs().len() as u32`。
  - 在 `lib.rs`：
    - 删除 `HOST_IMPORT_NAMES` 常量和其长度断言。
    - 给 `Compiler` 增加：
      ```rust
      special_host_import_indices: HashMap<host_import_registry::SpecialHostImport, u32>,
      ```
    - 新增字段后，执行 **Compiler 字段迁移**（见下表）。所有 imported host `*_func_idx` 字段改为从 `special_host_import_indices` 或 `builtin_func_indices` 查询；internal helper 字段保留不动。

      **删除**（imported host，迁移到 registry lookup）：
      `gc_collect_func_idx`, `closure_create_func_idx`, `closure_get_func_idx`, `closure_get_env_idx`, `native_call_func_idx`, `new_target_set_func_idx`, `obj_spread_func_idx`, `proxy_trap_get_func_idx`, `proxy_trap_set_func_idx`, `proxy_trap_delete_func_idx`, `obj_get_by_index_func_idx`, `typedarray_set_by_index_func_idx`

      **保留**（internal helper，不是 imported host）：
      `obj_new_func_idx`, `obj_get_func_idx`, `obj_set_func_idx`, `obj_delete_func_idx`, `to_int32_func_idx`, `arr_proto_table_base`, 所有 `*_global_idx` 字段
  - 在 `wjsm-ir/src/builtin.rs`：删除 `import_name()` 与 `ALL_BUILTINS`。此修复消除 `wjsm-ir`（零外部依赖纯 IR crate）携带 WASM backend 关注点的**层边界违反**。执行前 `cargo check -p wjsm-semantic -p wjsm-module` 二次确认无间接引用。
- [ ] **Verify GREEN**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry
    cargo check -p wjsm-ir
    ```
- [ ] **Commit**
  - 计划中的提交命令：
    ```bash
    git add crates/wjsm-backend-wasm/src/compiler_core.rs crates/wjsm-backend-wasm/src/lib.rs crates/wjsm-ir/src/builtin.rs crates/wjsm-backend-wasm/tests/host_import_registry.rs
    git commit -m "refactor: derive wasm imports from registry"
    ```

---

### Task 3: 清理 backend 所有 host import 魔数与区间假设
**Files:**
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- `crates/wjsm-backend-wasm/src/compiler_instructions.rs`
- `crates/wjsm-backend-wasm/src/compiler_control.rs`
- `crates/wjsm-backend-wasm/src/compiler_data.rs`
- `crates/wjsm-backend-wasm/src/compiler_module.rs`
- `crates/wjsm-backend-wasm/src/compiler_helpers.rs`
- `crates/wjsm-backend-wasm/src/compiler_array_helpers.rs`
- `crates/wjsm-backend-wasm/src/lib.rs`
**Why:** owner 已收敛后，必须把所有 call site 从数字索引切成命名 lookup，否则根因还在。

**Impact/Compatibility:** 行为不变；失败模式从“错误数字 call”变成“明确缺项报错”。

**Repair Track:** 修复 bug class，而不是继续给错误数字兜底。

**Retirement Track:** 删除 `gc_collect_func_idx = 22`、`closure_create_func_idx = 35`、`proxy_trap_get_func_idx = 320`、`for import_idx in 50u32..=76u32` 等旧知识。

**Verification:**
- `cargo check -p wjsm-backend-wasm`
- `cargo nextest run -E 'test(happy__hello) | test(happy__closure_counter) | test(happy__object_spread) | test(happy__new_target_constructor_context) | test(happy__proxy_traps_full) | test(happy__eval_scope_record) | test(happy__typedarray_full) | test(happy__throw_uncaught)'`

- [ ] **Write test**
  - 在 `crates/wjsm-backend-wasm/tests/host_import_registry.rs` 追加一个文本扫描测试，保证 backend 源码不再保留 imported host magic numbers。此测试是**过渡性护栏**，在 registry 稳定运行两个 feature cycle 后应删除：
    ```rust
    use std::fs;

    #[test]
    fn backend_source_no_longer_uses_host_import_magic_numbers() {
        let targets = [
            "src/compiler_builtins.rs",
            "src/compiler_instructions.rs",
            "src/compiler_control.rs",
            "src/compiler_data.rs",
            "src/compiler_module.rs",
            "src/compiler_core.rs",
            "src/compiler_helpers.rs",
            "src/compiler_array_helpers.rs",
        ];

        for target in targets {
            let text = fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), target))
                .expect("read source");
            for forbidden in [
                "Call(16)",
                "Call(17)",
                "unwrap_or(313)",
                "unwrap_or(95)",
                "unwrap_or(76)",
                "50u32..=76u32",
                "gc_collect_func_idx: 22",
                "obj_get_by_index_func_idx: 385",
                "typedarray_set_by_index_func_idx: 386",
            ] {
                assert!(!text.contains(forbidden), "{target} still contains {forbidden}");
            }
        }
    }
    ```
- [ ] **Verify RED**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry backend_source_no_longer_uses_host_import_magic_numbers -- --exact
    ```
  - 预期：失败，因为这些魔数当前仍存在。
- [ ] **Minimal code**
  - 在 `lib.rs` / `compiler_core.rs`：删除所有 imported host `*_func_idx` 固定数字初始化；改为从 `special_host_import_indices` 提取并存入字段，或直接在调用点查询。
  - 在 `compiler_instructions.rs`：
    - `Call(16)` 改成 `self.special_host_import_indices[&SpecialHostImport::StringConcat]`
    - `Call(17)` 改成 `self.special_host_import_indices[&SpecialHostImport::StringConcatVa]`
  - 在 `compiler_control.rs` / `compiler_data.rs` / `compiler_builtins.rs`：所有 `.unwrap_or(N)` 改成 `.context(...)` 或 `.expect(...)`，不再 fallback 到数字。
  - 在 `compiler_module.rs`：
    - `obj_spread` 改成 `SpecialHostImport::ObjSpread`
    - `50u32..=76u32` 改成遍历 `host_import_specs().iter().enumerate().filter(|(_, spec)| spec.group == Some(HostImportGroup::ArrayPrototypeMethod))`
  - 每改完一个文件，立刻跑 `cargo check -p wjsm-backend-wasm`，不要攒到最后一起爆。
  - 在 `compiler_helpers.rs` / `compiler_array_helpers.rs`：`self.gc_collect_func_idx`、`self.proxy_trap_get_func_idx`、`self.obj_get_by_index_func_idx`、`self.typedarray_set_by_index_func_idx` 全部改为从 `special_host_import_indices` 查询。
- [ ] **Verify GREEN**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry
    cargo nextest run -E 'test(happy__hello) | test(happy__closure_counter) | test(happy__object_spread) | test(happy__new_target_constructor_context) | test(happy__proxy_traps_full) | test(happy__eval_scope_record) | test(happy__typedarray_full) | test(happy__throw_uncaught)'
    ```
- [ ] **Commit**
  - 计划中的提交命令：
    ```bash
    git add crates/wjsm-backend-wasm/src/compiler_builtins.rs crates/wjsm-backend-wasm/src/compiler_instructions.rs crates/wjsm-backend-wasm/src/compiler_control.rs crates/wjsm-backend-wasm/src/compiler_data.rs crates/wjsm-backend-wasm/src/compiler_module.rs crates/wjsm-backend-wasm/src/compiler_helpers.rs crates/wjsm-backend-wasm/src/compiler_array_helpers.rs crates/wjsm-backend-wasm/src/lib.rs crates/wjsm-backend-wasm/tests/host_import_registry.rs
    git commit -m "refactor: remove host import index magic numbers"
    ```

---

### Task 4: 清理 runtime 编号 authority 注释并完成回归验证
**Files:**
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/host_imports/*.rs`

**Why:** runtime 编号注释虽然不参与执行，但会误导维护者继续把它们当真相源。

**Impact/Compatibility:** 纯注释治理；不改 runtime 行为。

**Repair Track:** 删除错误的“编号 authority”叙事。

**Retirement Track:** 用分组/函数名注释替代带编号注释，删除 `// Import N: ...` 与 `// index N` 注释模式。

**Verification:**
- `cargo check -p wjsm-runtime`
- `cargo nextest run -E 'test(happy__hello) | test(happy__proxy_traps_full) | test(happy__eval_scope_record) | test(happy__typedarray_full)'`

- [ ] **Write test**
  - 在 `crates/wjsm-backend-wasm/tests/host_import_registry.rs` 追加一个文本扫描测试。使用精确模式（`// Import <数字>:` 和 `// index <数字>`），避免误报普通变量名 `index`：
    ```rust
    #[test]
    fn runtime_source_no_longer_uses_numbered_import_comments() {
        let root = format!("{}/../wjsm-runtime/src", env!("CARGO_MANIFEST_DIR"));
        let files = [
            format!("{root}/lib.rs"),
            format!("{root}/host_imports/mod.rs"),
            format!("{root}/host_imports/core.rs"),
            format!("{root}/host_imports/array_object.rs"),
            format!("{root}/host_imports/primitive_core.rs"),
            format!("{root}/host_imports/timers_arrays.rs"),
            format!("{root}/host_imports/promise.rs"),
            format!("{root}/host_imports/promise_combinators.rs"),
            format!("{root}/host_imports/string_methods.rs"),
            format!("{root}/host_imports/async_fn.rs"),
            format!("{root}/host_imports/async_generator.rs"),
            format!("{root}/host_imports/misc.rs"),
            format!("{root}/host_imports/get_builtin_global_entry.rs"),
        ];

        let import_pat = regex::Regex::new(r"// Import \d+:").unwrap();
        let index_pat = regex::Regex::new(r"// index \d+").unwrap();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("read runtime file");
            assert!(!import_pat.is_match(&text), "still has numbered import comment: {file}");
            assert!(!index_pat.is_match(&text), "still has numbered index comment: {file}");
        }
    }
    ```
- [ ] **Verify RED**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry runtime_source_no_longer_uses_numbered_import_comments -- --exact
    ```
  - 预期：失败，runtime 还存在大量编号注释。另外测试依赖 `regex` crate，需先在 `crates/wjsm-backend-wasm/Cargo.toml` 的 `[dev-dependencies]` 添加 `regex = { workspace = true }`。
- [ ] **Minimal code**
  - 在 runtime 相关文件中，把 `// Import 321: ...` 改成 `// get_builtin_global` / `// new.target helpers` / `// Promise combinators` 这类分组或名字注释。
  - 不重排函数，不顺手改逻辑，不扩大到无关注释。
  - 优先清理 `search` 已命中的文件；改完后再执行一次全量 `search`，确认没有遗漏。
- [ ] **Verify GREEN**
  - 运行：
    ```bash
    cargo test -p wjsm-backend-wasm --test host_import_registry
    cargo check -p wjsm-runtime
    cargo nextest run -E 'test(happy__hello) | test(happy__proxy_traps_full) | test(happy__eval_scope_record) | test(happy__typedarray_full)'
    ```
- [ ] **Commit**
  - 计划中的提交命令：
    ```bash
    git add crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/host_imports crates/wjsm-backend-wasm/tests/host_import_registry.rs
    git commit -m "refactor: remove numbered host import comments"
    ```

---

## Risks

1. **Registry 首次搬运量大**：通过测试先锁唯一性、分组和 count，降低手抄漏项风险。
2. **把 internal helper 与 imported host 混淆**：严格只清理 imported host magic numbers；`obj_new_func_idx`、`obj_get_func_idx` 等内部 helper 继续保留现有机制。
3. **删掉 `Builtin::import_name()` 时误伤其他用途**：执行前 `cargo check -p wjsm-semantic -p wjsm-module` 确认无间接引用；搜索已确认唯一消费者在 `compiler_core.rs`。
4. **runtime 注释扫描误报**：已改用精确 regex 模式 `// Import \d+:` 和 `// index \d+`，不扫描普通变量名。
5. **`SpecialHostImport` 枚举搬运遗漏**：先在 Task 1 只添加已确认有直接索引需求的变体（计划中列出的 20 个），Task 3 清理各文件时按需追加；最终通过 `cargo check -p wjsm-backend-wasm` 编译失败来暴露遗漏项。

## Retirement

完成后必须确认以下旧路径已退休：
- `HOST_IMPORT_NAMES`
- `Builtin::import_name()` / `ALL_BUILTINS`（若已无引用）
- `Call(16)` / `Call(17)`
- `unwrap_or(313)` / `unwrap_or(95)` / `unwrap_or(76)` 等 import fallback
- `gc_collect_func_idx: 22` / `proxy_trap_get_func_idx: 320` 等 imported-host 固定值
- `50u32..=76u32`
- runtime `// Import N: ...` 与 `// index N` 编号注释
- `compiler_helpers.rs` 中 `self.gc_collect_func_idx` / `self.proxy_trap_get_func_idx` 等直接字段引用
- `compiler_array_helpers.rs` 中 `self.gc_collect_func_idx` / `self.obj_get_by_index_func_idx` / `self.typedarray_set_by_index_func_idx` 等直接字段引用

## Self-Review Checklist

- 每个 spec 要求都映射到任务：单一 owner、去魔数、去编号注释、回归验证。
- 无 TBD / TODO / “后续处理”。
- 验证命令都可直接复制执行。
- 任务边界按 owner 收敛，而不是按文件胡乱切分。
- 退役列表明确，避免“新 registry 上线但旧真相源还活着”。
