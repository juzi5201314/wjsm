//! 主 realm pristine 可达图克隆 → 新 realm handle 区。
//!
//! 禁止整段 immortal memcpy / 二次 snapshot restore；只对可达闭包中的对象逐个
//! 分配 dynamic 槽并复制后做 ObjectHandleMapPolicy 重映射。


use anyhow::Result;
use wasmtime::AsContextMut;
use wjsm_ir::value;

use crate::RuntimeState;
use crate::realm::{Realm, RealmIntrinsics, main_realm_intrinsics_from_state};
use crate::wasm_env::WasmEnv;

/// 从主 realm 的 WASM global + RuntimeState 字段装配 intrinsics。
pub(crate) fn main_realm_intrinsics_from_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) -> RealmIntrinsics {
    let object_proto = {
        let h = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
        if h < 0 {
            value::encode_undefined()
        } else {
            value::encode_object_handle(h as u32)
        }
    };
    let array_proto = {
        let h = env.array_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
        if h < 0 {
            value::encode_undefined()
        } else {
            value::encode_object_handle(h as u32)
        }
    };
    let st = ctx.as_context().data();
    main_realm_intrinsics_from_state(
        object_proto,
        array_proto,
        st.iterator_prototype,
        st.generator_prototype,
        st.async_iterator_prototype,
        st.async_gen_prototype,
        st.symbol_prototype,
        st.promise_prototype,
        st.function_prototype,
        st.regexp_prototype,
        st.date_prototype,
        st.buffer_prototype,
        st.text_encoder_prototype,
        st.text_decoder_prototype,
        st.error_prototypes,
        st.typedarray_prototypes,
    )
}

/// 惰性登记主 realm 为 `active_realms[0]`。






pub(crate) fn clone_pristine_realm<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    sandbox_global: i64,
) -> Result<Realm> {
    crate::realm_clone_v2::clone_pristine_realm_v2(ctx, env, sandbox_global)
}





/// 测试探针结果。
#[derive(Debug, Clone)]
pub struct RealmCloneProbe {
    pub main_array_proto_handle: u32,
    pub clone_array_proto_handle: u32,
    pub main_object_proto_handle: u32,
    pub clone_object_proto_handle: u32,
    /// clone.array_proto 的 [[Prototype]] handle
    pub clone_array_proto_of: u32,
    pub realm_id: u32,
    pub closure_size: usize,
    /// 闭包内每个对象的子 handle 均在闭包内（无悬挂堆引用）
    pub closure_closed: bool,
    /// 全部 RealmIntrinsics 根（有效 object/array）均落在闭包内
    pub roots_covered: bool,
}

/// 执行帧探针：enter 克隆 realm 后 WASM global 是否切到新 array/object proto，exit 是否恢复。
#[derive(Debug, Clone)]
pub struct ExecutionRealmFrameProbe {
    pub main_array: i32,
    pub main_object: i32,
    pub inside_array: i32,
    pub inside_object: i32,
    pub after_array: i32,
    pub after_object: i32,
    pub inside_execution_realm: u32,
    pub after_execution_realm: u32,
}


/// 在克隆 realm 执行帧内分配 `[]` 同源数组，对照 proto handle。
///
/// 解释器 `eval_array_lit` 与 compiled `arr_new` / `ArrayConstructor` 均经
/// `alloc_array_with_env` 读 `__array_proto_handle`；帧 swap 后三者同源。
#[derive(Debug, Clone)]
pub struct EvalRealmArrayProbe {
    pub realm_array_proto: u32,
    pub result_proto: u32,
    pub main_array_proto: u32,
}
