//! Support module ABI: imported env layout + helper exports + ABI hash.
//!
//! P2.2+P2.3 合并后的最终 ABI：
//! - 12 个 helper exports（obj_*/arr_*/elem_*/string_eq/to_int32/get_proto_from_ctor/
//!   wjsm_bootstrap_once/wjsm_init_function_props）
//! - support module 重新 export imported memory/table/globals，让从 support module
//!   发起的 host callback 仍能通过 `Caller::get_export` 读取同一份 WasmEnv
//! - 27 个 imported env globals：与 user wasm 的 27 个 global（索引 0..26）完全对齐，
//!   使 support module 的 global 索引与 user wasm 一致——helper body 移植时无需改索引
//! - support module 额外 import 它需要的 host 函数（gc_*/proxy_trap_*/native_call 等），
//!   通过 `env` namespace 引入，wasmtime Linker 已注册全部 host 函数实现
//! - SUPPORT_TABLE_RESERVED_LEN = 64：预留给后续 helper/table ABI，不由当前 support
//!   module 写 element section；用户 wasm 的 element section 从 table[0] 开始填充
//! - 任一字段变更必须导致 layout_hash 改变 → snapshot 失效 → cold rebuild

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use wjsm_ir::{
    HEAP_ALLOC_END_GLOBAL_NAME, HEAP_ALLOC_PTR_GLOBAL_NAME, HEAP_LIMIT_GLOBAL_NAME,
    HEAP_MEMORY_MAX_PAGES, HEAP_MEMORY_MIN_PAGES, HEAP_MEMORY_NAME, HEAP_OBJECT_START_GLOBAL_NAME,
};
pub const SUPPORT_MODULE_NAME: &str = "wjsm_support";
pub const ENV_MODULE_NAME: &str = "env";
pub const TABLE_IMPORT_NAME: &str = "__table";
pub const MEMORY_IMPORT_NAME: &str = "memory";
/// 独立影子栈线性内存（multi-memory index 1）。
pub const SHADOW_MEMORY_IMPORT_NAME: &str = "__shadow_memory";

/// Support module ABI 版本；任何不兼容改动必须 +1。
pub const SUPPORT_VERSION: u32 = 7;

/// Support module 在共享 table 起始保留的 slot 数：
/// 12 helper exports + 约 30 个 Array.prototype 方法 + 22 个 headroom = 64。
pub const SUPPORT_TABLE_RESERVED_LEN: u32 = 64;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum SupportGcFlavor {
    MarkSweep,
    G1,
    Zgc,
}

/// 当前随 build-time artifact 发布的 support flavor。
pub const AVAILABLE_SUPPORT_GC_FLAVORS: &[SupportGcFlavor] = &[
    SupportGcFlavor::MarkSweep,
    SupportGcFlavor::G1,
    SupportGcFlavor::Zgc,
];

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum SupportHeapAbi {
    LegacyMemory32,
    ManagedHeapV2Memory64,
}

pub const MANAGED_HEAP_V2_GLOBAL_IMPORTS: &[&str] = &[
    HEAP_ALLOC_PTR_GLOBAL_NAME,
    HEAP_ALLOC_END_GLOBAL_NAME,
    HEAP_OBJECT_START_GLOBAL_NAME,
    HEAP_LIMIT_GLOBAL_NAME,
];

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

/// 27 个 env globals：与 user wasm 的全局索引 0..26 完全对齐。
/// 顺序与 user wasm compiler_module.rs 中的 global 定义顺序一致，
/// 使 support module import 的 global index 与 user wasm 一致。
/// 全部 mutable：user wasm 在 bootstrap 中用 global.set 初始化。
pub const ENV_GLOBALS: &[EnvGlobal] = &[
    EnvGlobal {
        name: "__func_props",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__heap_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__obj_table_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__obj_table_count",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__shadow_sp",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__object_heap_start",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__num_ir_functions",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__shadow_stack_end",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__array_proto_handle",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__object_proto_handle",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__eval_var_map_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__eval_var_map_count",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__bootstrap_done",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__function_props_done",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__function_props_base",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__arr_proto_table_base",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__arr_proto_table_len",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__arr_proto_table_hash",
        ty: GlobalValTy::I64,
        mutable: true,
    },
    EnvGlobal {
        name: "__heap_limit",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__alloc_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__alloc_end",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__gc_alloc_bytes",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__gc_trigger_bytes",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__gc_phase",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__good_color",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__barrier_buf_ptr",
        ty: GlobalValTy::I32,
        mutable: true,
    },
    EnvGlobal {
        name: "__barrier_buf_end",
        ty: GlobalValTy::I32,
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

/// 计算 support ABI union 的稳定 hash；任一已发布 flavor 的 ABI 输入变化都会使之变化。
/// 输入：SUPPORT_VERSION + SUPPORT_TABLE_RESERVED_LEN + AVAILABLE_SUPPORT_GC_FLAVORS + ENV_GLOBALS + SUPPORT_EXPORTS。
pub fn support_abi_union_hash() -> u64 {
    let mut h = DefaultHasher::new();
    SUPPORT_VERSION.hash(&mut h);
    SUPPORT_TABLE_RESERVED_LEN.hash(&mut h);
    MEMORY_IMPORT_NAME.hash(&mut h);
    SHADOW_MEMORY_IMPORT_NAME.hash(&mut h);
    for flavor in AVAILABLE_SUPPORT_GC_FLAVORS {
        flavor.hash(&mut h);
    }
    for g in ENV_GLOBALS {
        g.hash(&mut h);
    }
    for e in SUPPORT_EXPORTS {
        e.hash(&mut h);
    }
    h.finish()
}

/// 计算 managed-heap V2 support artifact 的独立 ABI hash。
///
/// 它刻意不改变 active V1 `support_abi_union_hash()`，因此 V2 artifact 的 ABI
/// 演进不会伪装成 V1 snapshot format 变更。
pub fn managed_heap_v2_support_abi_hash() -> u64 {
    let mut h = DefaultHasher::new();
    support_abi_union_hash().hash(&mut h);
    SupportHeapAbi::ManagedHeapV2Memory64.hash(&mut h);
    HEAP_MEMORY_NAME.hash(&mut h);
    HEAP_MEMORY_MIN_PAGES.hash(&mut h);
    HEAP_MEMORY_MAX_PAGES.hash(&mut h);
    for global in MANAGED_HEAP_V2_GLOBAL_IMPORTS {
        global.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hash 必须确定性：同一份 ABI 定义下 hash 不变。
    #[test]
    fn support_abi_union_hash_is_deterministic() {
        let a = support_abi_union_hash();
        let b = support_abi_union_hash();
        assert_eq!(a, b, "support_abi_union_hash 必须是确定性的");
    }

    #[test]
    fn managed_heap_v2_support_abi_hash_is_deterministic_and_distinct() {
        let hash = managed_heap_v2_support_abi_hash();
        assert_eq!(hash, managed_heap_v2_support_abi_hash());
        assert_ne!(hash, support_abi_union_hash());
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

    /// 27 env globals 数量锁死（与 user wasm 全局索引 0..26 对齐）。
    #[test]
    fn env_globals_count_locked() {
        assert_eq!(
            ENV_GLOBALS.len(),
            27,
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

    /// 跨 crate 一致性：abi.rs 和 backend-wasm support_module.rs 的
    /// SUPPORT_TABLE_RESERVED_LEN 必须相同，否则 embedded snapshot ABI hash 失配。
    #[test]
    fn support_table_reserved_matches_backend() {
        assert_eq!(
            SUPPORT_TABLE_RESERVED_LEN,
            wjsm_backend_wasm::support_module::SUPPORT_TABLE_RESERVED_LEN,
            "abi::SUPPORT_TABLE_RESERVED_LEN 与 backend support_module::SUPPORT_TABLE_RESERVED_LEN 不一致"
        );
    }

    #[test]
    fn available_support_gc_flavors_count_locked() {
        assert_eq!(
            AVAILABLE_SUPPORT_GC_FLAVORS,
            &[
                SupportGcFlavor::MarkSweep,
                SupportGcFlavor::G1,
                SupportGcFlavor::Zgc,
            ]
        );
    }
}
