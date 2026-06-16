//! 可插拔 GC 框架（spec §6）。单一 canonical owner: 本模块组。
//!
//! 算法以 trait 抽象（`GcAlgorithm: Allocator + Marker + Sweeper`），
//! 默认实现 `MarkSweepCollector`（non-moving + segregated free list）。
//! 预留 generational/incremental/parallel 接入点（WriteBarrier/ReadBarrier/
//! HeapRegionManager/mark_step/sweep_step，默认 no-op）。
//!
//! 稳定性承诺见 spec 附录 D。关键不变量见 spec §18。
//!
//! # GC Safepoint 优化策略
//!
//! WASM 编译器在可能的 GC 点（safepoint）前需要 spill 活跃的对象指针到 shadow stack，
//! 以便 GC 扫描时能找到所有根。过度 spill 会导致代码膨胀（main.js: 31k→361k 行 WASM）。
//! 本实现采用三层正交优化策略减少不必要的 spill：
//!
//! ## Layer 1: ValueTy 固定点迭代
//!
//! **位置**: `wjsm-ir/src/value_ty.rs`
//!
//! 通过不动点迭代推断每个 SSA 值的类型（Scalar vs Handle），减少误判为 Handle 的标量值：
//! - **StoreVar→LoadVar 传播**: 若变量的所有 StoreVar 源值都是 Scalar，LoadVar 降级为 Scalar
//! - **Phi 折叠**: 若 Phi 的所有源值都是 Scalar，Phi 降级为 Scalar
//! - **Bug 修正**: DeleteProp/IsException 从误判的 Handle 修正为 Scalar（规范保证返回 bool）
//! - **安全保守**: EncodeException 保持 Handle（携带对象 handle，需 spill）
//!
//! 算法只能 Handle→Scalar（减少 spill），绝不反向。未被 StoreVar 的变量（函数参数、
//! 捕获变量）不降级。
//!
//! ## Layer 2: Spill Batch 优化
//!
//! **位置**: `wjsm-backend-wasm/src/compiler_instructions.rs`
//!
//! 将每个 spill 值的指令数从 7 条降到 3 条（55% 减少）：
//! - 使用 `i64.store offset=k*8` 的 immediate offset（无需逐次推进 sp）
//! - 批量推进 sp：`local.get spill_base; i32.const N*8; i32.add; global.set sp`
//! - 添加 `safepoint_sp_saved` 局部变量存储 spill_base
//!
//! 安全性：必须推进 sp 到 base+N*8 让 GC 扫到 spilled 值。
//!
//! ## Layer 3: Callee No-GC 分析
//!
//! **位置**: `wjsm-backend-wasm/src/compiler_gc_analysis.rs`
//!
//! 通过静态分析识别不触发 GC 的 callee，完全省略 safepoint spill：
//! - **Layer 3a**: IR Function 添加 `known_callee_vars` 字段（scope-qualified IR name → FunctionId）
//! - **Layer 3b**: 语义层填充 known_callee_vars（`wjsm-semantic/src/lowerer_function_decls.rs`）
//! - **Layer 3c**: 模块级 GcAnalysis 分析
//!   - `builtin_may_trigger_gc` 判定 builtin 是否可能触发 GC（与 `builtin_returns_scalar` 互补）
//!   - 构建 ValueId → LoadVar name 映射，精确追溯 Call 的 callee
//!   - 不动点迭代求传递闭包
//! - **Layer 3d**: 修改 Call 指令编译逻辑，条件执行 safepoint spill
//!   - SuperCall/ConstructCall 保守保持无条件 spill（构造调用几乎必分配）
//!
//! 安全性：unknown callee 一律保守 may-GC。只对单次赋值的函数声明变量建映射。
//!
//! ## 安全性原则
//!
//! 所有优化都遵循**保守原则**：宁可多 spill，绝不漏 spill。
//! - 算法只能 Handle→Scalar，绝不反向
//! - 未被 StoreVar 的变量不降级
//! - EncodeException 保持 Handle（TAG_EXCEPTION needs_root=true）
//! - Spill batch 必须推进 sp 让 GC 扫到 spilled 值
//! - Unknown callee 一律保守 may-GC
//! - SuperCall/ConstructCall 保守保持 spill（构造调用几乎必分配）
//!
//! ## 相关文件
//!
//! - `wjsm-ir/src/value_ty.rs`: Layer 1 固定点迭代
//! - `wjsm-backend-wasm/src/compiler_instructions.rs`: Layer 2 spill batch + Layer 3d 条件 spill
//! - `wjsm-backend-wasm/src/compiler_gc_analysis.rs`: Layer 3c 模块级分析
//! - `wjsm-semantic/src/lowerer_function_decls.rs`: Layer 3b 填充 known_callee_vars
//!
//! 详细设计见 plan.md。
pub mod api;
pub mod context;
pub mod mark_bitmap;
pub mod mark_sweep;
pub mod native_callable_refs;
pub mod roots;

pub use api::{
    Allocator, GcAlgorithm, GcContext, GcStats, Handle, HeapObjectQuery, HeapRegionManager,
    MarkProgress, Marker, ReadBarrier, RootProvider, Sweeper, Value, WriteBarrier,
};
pub use mark_bitmap::MarkBitmap;
pub use mark_sweep::MarkSweepCollector;
pub use mark_sweep::allocator::SegregatedFreeList;
