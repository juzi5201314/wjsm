//! 单堆多 realm：每 realm = intrinsics 句柄集 + global；共享 Store / obj_table / GC。
//!
//! 主 realm = `active_realms[0]`（惰性登记）。`execution_realm` 指示当前
//! 分配 / 构造 / 字面量 / eval 的 intrinsic 解析目标；默认 0。

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicU32, Ordering};

/// TypedArray 构造器种类数（与 `TypedArrayConstructorKind::COUNT` 对齐）。
pub const TYPEDARRAY_PROTO_COUNT: usize = 11;

/// 活跃 realm 默认上限（可用 `WJSM_VM_MAX_REALMS` 覆盖）。
pub const DEFAULT_MAX_REALMS: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealmId(pub u32);

/// 与 RuntimeState primordial / `roots.rs` 显式 root 对齐的 per-realm 句柄集。
#[derive(Debug, Clone, Copy)]
pub struct RealmIntrinsics {
    pub object_proto: i64,
    pub array_proto: i64,
    pub function_proto: i64,
    pub iterator_prototype: i64,
    pub generator_prototype: i64,
    pub async_iterator_prototype: i64,
    pub async_gen_prototype: i64,
    pub symbol_prototype: i64,
    pub promise_prototype: i64,
    pub regexp_prototype: i64,
    pub date_prototype: i64,
    pub error_proto: i64,
    pub type_error_proto: i64,
    pub range_error_proto: i64,
    pub reference_error_proto: i64,
    pub syntax_error_proto: i64,
    pub eval_error_proto: i64,
    pub uri_error_proto: i64,
    pub aggregate_error_proto: i64,
    pub buffer_prototype: i64,
    pub text_encoder_prototype: i64,
    pub text_decoder_prototype: i64,
    /// 按 `TypedArrayConstructorKind::index()` 顺序存放。
    pub typedarray_prototypes: [i64; TYPEDARRAY_PROTO_COUNT],
}

impl RealmIntrinsics {
    /// `value::encode_undefined()` 的常量折叠入口，供测试与 empty 初始化。
    pub const UNDEFINED: i64 = {
        // BOX_BASE | (TAG_UNDEFINED << 32)，与 value::encode_undefined 一致。
        // 避免 const 上下文直接调非 const fn。
        const BOX_BASE: u64 = 0x8000_0000_0000_0000 | 0x7FF0_0000_0000_0000 | 0x0008_0000_0000_0000;
        const TAG_UNDEFINED: u64 = 0x2;
        (BOX_BASE | (TAG_UNDEFINED << 32)) as i64
    };

    pub fn empty() -> Self {
        let u = Self::UNDEFINED;
        Self {
            object_proto: u,
            array_proto: u,
            function_proto: u,
            iterator_prototype: u,
            generator_prototype: u,
            async_iterator_prototype: u,
            async_gen_prototype: u,
            symbol_prototype: u,
            promise_prototype: u,
            regexp_prototype: u,
            date_prototype: u,
            error_proto: u,
            type_error_proto: u,
            range_error_proto: u,
            reference_error_proto: u,
            syntax_error_proto: u,
            eval_error_proto: u,
            uri_error_proto: u,
            aggregate_error_proto: u,
            buffer_prototype: u,
            text_encoder_prototype: u,
            text_decoder_prototype: u,
            typedarray_prototypes: [u; TYPEDARRAY_PROTO_COUNT],
        }
    }

    /// GC / 克隆 BFS 用的全部根句柄（i64 NaN-box）。
    pub fn iter_roots(&self) -> impl Iterator<Item = i64> + '_ {
        [
            self.object_proto,
            self.array_proto,
            self.function_proto,
            self.iterator_prototype,
            self.generator_prototype,
            self.async_iterator_prototype,
            self.async_gen_prototype,
            self.symbol_prototype,
            self.promise_prototype,
            self.regexp_prototype,
            self.date_prototype,
            self.error_proto,
            self.type_error_proto,
            self.range_error_proto,
            self.reference_error_proto,
            self.syntax_error_proto,
            self.eval_error_proto,
            self.uri_error_proto,
            self.aggregate_error_proto,
            self.buffer_prototype,
            self.text_encoder_prototype,
            self.text_decoder_prototype,
        ]
        .into_iter()
        .chain(self.typedarray_prototypes.iter().copied())
    }

    pub fn try_map_values<E>(&self, mut map: impl FnMut(i64) -> Result<i64, E>) -> Result<Self, E> {
        let mut typedarray_prototypes = [Self::UNDEFINED; TYPEDARRAY_PROTO_COUNT];
        for (target, value) in typedarray_prototypes
            .iter_mut()
            .zip(self.typedarray_prototypes)
        {
            *target = map(value)?;
        }
        Ok(Self {
            object_proto: map(self.object_proto)?,
            array_proto: map(self.array_proto)?,
            function_proto: map(self.function_proto)?,
            iterator_prototype: map(self.iterator_prototype)?,
            generator_prototype: map(self.generator_prototype)?,
            async_iterator_prototype: map(self.async_iterator_prototype)?,
            async_gen_prototype: map(self.async_gen_prototype)?,
            symbol_prototype: map(self.symbol_prototype)?,
            promise_prototype: map(self.promise_prototype)?,
            regexp_prototype: map(self.regexp_prototype)?,
            date_prototype: map(self.date_prototype)?,
            error_proto: map(self.error_proto)?,
            type_error_proto: map(self.type_error_proto)?,
            range_error_proto: map(self.range_error_proto)?,
            reference_error_proto: map(self.reference_error_proto)?,
            syntax_error_proto: map(self.syntax_error_proto)?,
            eval_error_proto: map(self.eval_error_proto)?,
            uri_error_proto: map(self.uri_error_proto)?,
            aggregate_error_proto: map(self.aggregate_error_proto)?,
            buffer_prototype: map(self.buffer_prototype)?,
            text_encoder_prototype: map(self.text_encoder_prototype)?,
            text_decoder_prototype: map(self.text_decoder_prototype)?,
            typedarray_prototypes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::value;

    #[test]
    fn realm_undefined_matches_value_encode() {
        assert_eq!(RealmIntrinsics::UNDEFINED, value::encode_undefined());
    }

    #[test]
    fn main_realm_intrinsics_from_state_wires_fields() {
        let err = crate::runtime_heap::ErrorPrototypes {
            error: 1,
            type_error: 2,
            range_error: 3,
            syntax_error: 4,
            reference_error: 5,
            uri_error: 6,
            eval_error: 7,
            aggregate_error: 8,
        };
        let ta = [9_i64; TYPEDARRAY_PROTO_COUNT];
        let intr = main_realm_intrinsics_from_state(
            10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, err, ta,
        );
        assert_eq!(intr.object_proto, 10);
        assert_eq!(intr.array_proto, 11);
        assert_eq!(intr.function_proto, 18);
        assert_eq!(intr.aggregate_error_proto, 8);
        assert_eq!(intr.typedarray_prototypes[0], 9);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CodeGenFlags {
    /// false → 该 realm 内 eval / Function 抛 EvalError（不拦截 runIn* / Script）
    pub strings: bool,
    pub wasm: bool,
}

impl Default for CodeGenFlags {
    fn default() -> Self {
        Self {
            strings: true,
            wasm: true,
        }
    }
}

/// `createContext` / `runInNewContext` 的 `microtaskMode`。
///
/// Node 默认在 run 结束时不排空本轮 microtask；`afterEvaluate` 则 drain 到稳态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicrotaskMode {
    #[default]
    Default,
    AfterEvaluate,
}

#[derive(Debug, Clone)]
pub struct Realm {
    pub id: RealmId,
    pub global_object: i64,
    pub intrinsics: RealmIntrinsics,
    pub code_generation: CodeGenFlags,
    pub microtask_mode: MicrotaskMode,
}

impl Realm {
    pub fn new(id: RealmId, global_object: i64, intrinsics: RealmIntrinsics) -> Self {
        Self {
            id,
            global_object,
            intrinsics,
            code_generation: CodeGenFlags::default(),
            microtask_mode: MicrotaskMode::default(),
        }
    }
}

/// 保存 / 恢复 `execution_realm` 槽，支持嵌套（栈式）。
///
/// 供 RuntimeState 与单测共用；panic 路径也会 restore。
pub fn with_execution_realm_slot<R>(
    slot: &AtomicU32,
    realm_id: RealmId,
    f: impl FnOnce() -> R,
) -> R {
    let prev = slot.swap(realm_id.0, Ordering::Relaxed);
    let result = catch_unwind(AssertUnwindSafe(f));
    slot.store(prev, Ordering::Relaxed);
    match result {
        Ok(v) => v,
        Err(payload) => resume_unwind(payload),
    }
}

/// 执行帧：保存/恢复 `execution_realm` + `__array_proto_handle` / `__object_proto_handle`。
///
/// compiled eval 与 support `arr_new` 读父模块同一 mutable global；进入非 0 realm 时
/// 必须 swap 为该 realm 的 intrinsic handle，退出时嵌套安全地 restore。
///
/// 正常路径必定 restore。若 `f` panic，global 可能残留（与多数 host 回调一致）；
/// vm 入口应避免在帧内 panic。
pub(crate) fn with_execution_realm_frame<C, R>(
    ctx: &mut C,
    env: &crate::wasm_env::WasmEnv,
    realm_id: RealmId,
    f: impl FnOnce(&mut C) -> R,
) -> R
where
    C: wasmtime::AsContextMut<Data = crate::RuntimeState>,
{
    use wasmtime::{AsContext, Val};

    let prev_realm = ctx
        .as_context()
        .data()
        .execution_realm
        .swap(realm_id.0, Ordering::Relaxed);
    let prev_array = env.array_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    let prev_object = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);

    if let Some((target_array, target_object)) = resolve_proto_global_handles(ctx, realm_id) {
        let _ = env
            .array_proto_handle
            .set(&mut *ctx, Val::I32(target_array));
        let _ = env
            .object_proto_handle
            .set(&mut *ctx, Val::I32(target_object));
    }

    let result = f(ctx);

    let _ = env.array_proto_handle.set(&mut *ctx, Val::I32(prev_array));
    let _ = env
        .object_proto_handle
        .set(&mut *ctx, Val::I32(prev_object));
    ctx.as_context()
        .data()
        .execution_realm
        .store(prev_realm, Ordering::Relaxed);

    result
}

/// 解析目标 realm 的 array/object proto 裸 handle。
/// 返回 `None` 表示保持当前 global（主 realm 未登记时）。
fn resolve_proto_global_handles<C>(ctx: &mut C, realm_id: RealmId) -> Option<(i32, i32)>
where
    C: wasmtime::AsContextMut<Data = crate::RuntimeState>,
{
    use wasmtime::AsContext;

    let realms = ctx
        .as_context()
        .data()
        .active_realms
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let r = if realm_id.0 == 0 {
        realms.first()
    } else {
        realms.iter().find(|r| r.id == realm_id)
    }?;
    let arr = nanbox_to_proto_i32(r.intrinsics.array_proto);
    let obj = nanbox_to_proto_i32(r.intrinsics.object_proto);
    if arr < 0 || obj < 0 {
        return None;
    }
    Some((arr, obj))
}

fn nanbox_to_proto_i32(raw: i64) -> i32 {
    use wjsm_ir::value;
    if value::is_object(raw) || value::is_array(raw) {
        value::decode_object_handle(raw) as i32
    } else {
        -1
    }
}

/// 读取 `WJSM_VM_MAX_REALMS`（默认 1024）。
pub fn max_realms_limit() -> u32 {
    std::env::var("WJSM_VM_MAX_REALMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_REALMS)
}

/// GC 后回收 sandbox 不可达的 realm（Task 4.2）。
///
/// - realm 0 恒留
/// - 其它仅当 `global_object` handle 仍 live（`is_live`）
/// - 同步清理 `contextified` side table
pub(crate) fn reclaim_dead_realms(
    state: &crate::RuntimeState,
    mut is_live: impl FnMut(u32) -> bool,
) {
    use wjsm_ir::value;

    let mut live_ids = std::collections::HashSet::new();
    {
        let mut realms = state
            .active_realms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        realms.retain(|r| {
            if r.id.0 == 0 {
                live_ids.insert(r.id);
                return true;
            }
            let live = if value::is_object(r.global_object) || value::is_array(r.global_object) {
                is_live(value::decode_object_handle(r.global_object))
            } else {
                false
            };
            if live {
                live_ids.insert(r.id);
            }
            live
        });
    }
    {
        let mut table = state.contextified.lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|handle, realm_id| {
            if realm_id.0 == 0 {
                return true;
            }
            // handle 本身仍 live 且 realm 仍在表中
            is_live(*handle) && live_ids.contains(realm_id)
        });
    }
}

/// 从主 realm 的 RuntimeState 字段装配 intrinsics（object/array proto 由调用方从 WASM global 填入）。
/// Phase 1 克隆入口使用；当前阶段由单测覆盖。
#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn main_realm_intrinsics_from_state(
    object_proto: i64,
    array_proto: i64,
    iterator_prototype: i64,
    generator_prototype: i64,
    async_iterator_prototype: i64,
    async_gen_prototype: i64,
    symbol_prototype: i64,
    promise_prototype: i64,
    function_prototype: i64,
    regexp_prototype: i64,
    date_prototype: i64,
    buffer_prototype: i64,
    text_encoder_prototype: i64,
    text_decoder_prototype: i64,
    error: crate::runtime_heap::ErrorPrototypes,
    typedarray_prototypes: [i64; TYPEDARRAY_PROTO_COUNT],
) -> RealmIntrinsics {
    RealmIntrinsics {
        object_proto,
        array_proto,
        function_proto: function_prototype,
        iterator_prototype,
        generator_prototype,
        async_iterator_prototype,
        async_gen_prototype,
        symbol_prototype,
        promise_prototype,
        regexp_prototype,
        date_prototype,
        error_proto: error.error,
        type_error_proto: error.type_error,
        range_error_proto: error.range_error,
        reference_error_proto: error.reference_error,
        syntax_error_proto: error.syntax_error,
        eval_error_proto: error.eval_error,
        uri_error_proto: error.uri_error,
        aggregate_error_proto: error.aggregate_error,
        buffer_prototype,
        text_encoder_prototype,
        text_decoder_prototype,
        typedarray_prototypes,
    }
}
