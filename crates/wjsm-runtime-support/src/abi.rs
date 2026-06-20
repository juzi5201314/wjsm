//! Support module ABI: imported env layout + helper exports + ABI hash.
//!
//! P2.2+P2.3 合并后的最终 ABI：
//! - 12 个 helper exports（obj_*/arr_*/elem_*/string_eq/to_int32/get_proto_from_ctor/
//!   wjsm_bootstrap_once/wjsm_init_function_props）
//! - support module 重新 export imported memory/table/globals，让从 support module
//!   发起的 host callback 仍能通过 `Caller::get_export` 读取同一份 WasmEnv
//! - 19 个 imported env globals：与 user wasm 的 19 个 global（索引 0..18）完全对齐，
//!   使 support module 的 global 索引与 user wasm 一致——helper body 移植时无需改索引
//! - support module 额外 import 它需要的 host 函数（gc_*/proxy_trap_*/native_call 等），
//!   通过 `env` namespace 引入，wasmtime Linker 已注册全部 host 函数实现
//! - SUPPORT_TABLE_RESERVED_LEN = 64：预留给后续 helper/table ABI，不由当前 support
//!   module 写 element section；用户 wasm 的 element section 从 table[0] 开始填充
//! - 任一字段变更必须导致 layout_hash 改变 → snapshot 失效 → cold rebuild

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const SUPPORT_MODULE_NAME: &str = "wjsm_support";
pub const ENV_MODULE_NAME: &str = "env";
pub const TABLE_IMPORT_NAME: &str = "__table";
pub const MEMORY_IMPORT_NAME: &str = "memory";

/// Support module ABI 版本；任何不兼容改动必须 +1。
pub const SUPPORT_VERSION: u32 = 3;

/// Support module 在共享 table 起始保留的 slot 数：
/// 12 helper exports + 约 30 个 Array.prototype 方法 + 22 个 headroom = 64。
pub const SUPPORT_TABLE_RESERVED_LEN: u32 = 64;

/// Imported env global 的值类型。
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum GlobalValTy {
    I32,
    I64,
    F64,
}

/// Imported env global 的描述。
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct EnvGlobal {
    pub name: &'static str,
    pub ty: GlobalValTy,
    pub mutable: bool,
}

/// 19 个 env globals：与 user wasm 的全局索引 0..18 完全对齐。
/// 顺序与 user wasm compiler_module.rs 中的 global 定义顺序一致，
/// 使 support module import 的 global index 与 user wasm 一致。
/// 全部 mutable：P2.2 后 user wasm 在 bootstrap 中用 global.set 初始化。
pub const ENV_GLOBALS: &[EnvGlobal] = &[
    // idx 0: __func_props（已弃用，恒为 0；保留以对齐索引）
    EnvGlobal {
        name: "__func_props",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 1
    EnvGlobal {
        name: "__heap_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 2
    EnvGlobal {
        name: "__obj_table_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 3
    EnvGlobal {
        name: "__obj_table_count",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 4
    EnvGlobal {
        name: "__shadow_sp",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 5
    EnvGlobal {
        name: "__alloc_counter",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 6
    EnvGlobal {
        name: "__object_heap_start",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 7
    EnvGlobal {
        name: "__num_ir_functions",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 8
    EnvGlobal {
        name: "__shadow_stack_end",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 9
    EnvGlobal {
        name: "__array_proto_handle",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 10
    EnvGlobal {
        name: "__object_proto_handle",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 11
    EnvGlobal {
        name: "__eval_var_map_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 12
    EnvGlobal {
        name: "__eval_var_map_count",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 13
    EnvGlobal {
        name: "__bootstrap_done",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 14
    EnvGlobal {
        name: "__function_props_done",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 15
    EnvGlobal {
        name: "__function_props_base",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 16
    EnvGlobal {
        name: "__arr_proto_table_base",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 17
    EnvGlobal {
        name: "__arr_proto_table_len",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    // idx 18
    EnvGlobal {
        name: "__arr_proto_table_hash",
        ty: GlobalValTy::I64,
        mutable: true,
    },
];

/// 12 个 helper export 名字；用户 wasm 通过 `import "wjsm_support" "<name>"` 引用。
/// 顺序即 element section 在 [0..12) 的登记顺序，必须稳定。
pub const SUPPORT_EXPORTS: &[&str] = &[
    "obj_new",
    "obj_get",
    "obj_set",
    "obj_delete",
    "arr_new",
    "elem_get",
    "elem_set",
    "string_eq",
    "to_int32",
    "get_proto_from_ctor",
    "wjsm_bootstrap_once",
    "wjsm_init_function_props",
];

/// 计算 support module layout 的稳定 hash；任一 ABI 输入变化都会使之变化。
/// 输入：SUPPORT_VERSION + SUPPORT_TABLE_RESERVED_LEN + ENV_GLOBALS + SUPPORT_EXPORTS。
pub fn support_module_layout_hash() -> u64 {
    let mut h = DefaultHasher::new();
    SUPPORT_VERSION.hash(&mut h);
    SUPPORT_TABLE_RESERVED_LEN.hash(&mut h);
    for g in ENV_GLOBALS {
        g.hash(&mut h);
    }
    for e in SUPPORT_EXPORTS {
        e.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hash 必须确定性：同一份 ABI 定义下 hash 不变。
    #[test]
    fn support_module_layout_hash_is_deterministic() {
        let a = support_module_layout_hash();
        let b = support_module_layout_hash();
        assert_eq!(a, b, "support_module_layout_hash 必须是确定性的");
    }

    /// 12 helper exports 数量锁死；增删 export 必须显式更新此处。
    #[test]
    fn support_exports_count_locked() {
        assert_eq!(
            SUPPORT_EXPORTS.len(),
            12,
            "SUPPORT_EXPORTS 数量改变必须同步更新 ABI 测试"
        );
    }

    /// 19 env globals 数量锁死（与 user wasm 全局索引 0..18 对齐）。
    #[test]
    fn env_globals_count_locked() {
        assert_eq!(
            ENV_GLOBALS.len(),
            19,
            "ENV_GLOBALS 数量改变必须同步更新 ABI 测试"
        );
    }

    /// 保留 64 slot：12 helper + ~30 Array.prototype + headroom，避免 user element 段冲撞。
    #[test]
    fn support_table_reserved_covers_helpers_and_array_proto() {
        assert!(
            SUPPORT_TABLE_RESERVED_LEN as usize >= SUPPORT_EXPORTS.len() + 30,
            "SUPPORT_TABLE_RESERVED_LEN 必须容纳 helpers + ~30 Array.prototype 方法"
        );
    }
}
