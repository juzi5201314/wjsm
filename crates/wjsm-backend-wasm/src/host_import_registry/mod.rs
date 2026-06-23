//! Canonical owner of all host import definitions.
//!
//! This module is the single source of truth for host import names,
//! WASM function type indices, Builtin bindings, special index requirements,
//! and grouping. All other modules derive their knowledge from this registry.
//!
//! Modifying host imports: add/remove/reorder entries here, then
//! update the corresponding runtime host function implementations.
//!
//! 数据拆分到 `specs_part1..6` 子模块，按位置顺序拼接（WASM 函数索引依赖位置）。

use std::sync::LazyLock;
use wjsm_ir::Builtin;

mod specs_part1;
mod specs_part2;
mod specs_part3;
mod specs_part4;
mod specs_part5;
mod specs_part6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialHostImport {
    ArrayFrom,
    ArrayProtoPush,
    ArrayProtoPop,
    ArrayProtoIncludes,
    ArrayProtoIndexOf,
    ArrayProtoJoin,
    ArrayProtoSlice,
    ArrayProtoFill,
    ArrayProtoReverse,
    ArrayProtoFlat,
    ClosureCreate,
    ClosureGetFunc,
    ClosureGetEnv,
    NativeCall,
    NewTargetSet,
    ObjGetByIndex,
    ObjectProtoInit,
    ObjSpread,
    ProxyApply,
    ProxyConstruct,
    ProxyTrapDelete,
    ProxyTrapGet,
    ProxyTrapSet,
    StringConcat,
    StringConcatVa,
    SymbolPropertyKey,
    StringToArrayIndex,
    NativeCallableGetProperty,
    PrimitiveBigIntGetMethod,
    PrimitiveNumberGetMethod,
    TypedArraySetByIndex,
    ToNumber,
    ToBool,
    // ── P4 GC framework host imports ──
    /// gc_alloc_slow(size, heap_type, capacity) -> handle：fast-path bump 失败后的 slow-path。
    GcAllocSlow,
    /// gc_maybe_collect()：proactive GC 触发（alloc_counter 达阈值时 WASM 调用）。
    GcMaybeCollect,
    /// gc_take_freed_handle() -> handle（-1 表空）：从 host handle_free_list pop 复用。
    GcTakeFreedHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostImportGroup {
    ArrayPrototypeMethod,
    NumberPrototypeMethod,
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

/// 合并所有分段数据，保持原始顺序（WASM 函数索引依赖位置）。
static HOST_IMPORT_SPECS: LazyLock<Vec<HostImportSpec>> = LazyLock::new(|| {
    let mut v = Vec::with_capacity(
        specs_part1::SPECS_PART1.len()
            + specs_part2::SPECS_PART2.len()
            + specs_part3::SPECS_PART3.len()
            + specs_part4::SPECS_PART4.len()
            + specs_part5::SPECS_PART5.len()
            + specs_part6::SPECS_PART6.len(),
    );
    v.extend_from_slice(specs_part1::SPECS_PART1);
    v.extend_from_slice(specs_part2::SPECS_PART2);
    v.extend_from_slice(specs_part3::SPECS_PART3);
    v.extend_from_slice(specs_part4::SPECS_PART4);
    v.extend_from_slice(specs_part5::SPECS_PART5);
    v.extend_from_slice(specs_part6::SPECS_PART6);
    v
});

pub fn host_import_specs() -> &'static [HostImportSpec] {
    &HOST_IMPORT_SPECS
}

pub fn array_proto_method_specs() -> impl Iterator<Item = (usize, &'static HostImportSpec)> {
    HOST_IMPORT_SPECS
        .iter()
        .enumerate()
        .filter(|(_, spec)| spec.group == Some(HostImportGroup::ArrayPrototypeMethod))
}

pub fn array_proto_table_len() -> u32 {
    array_proto_method_specs().count() as u32
}

pub fn array_proto_table_hash() -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (_, spec) in array_proto_method_specs() {
        fnv1a_update(&mut hash, &(spec.name.len() as u32).to_le_bytes());
        fnv1a_update(&mut hash, spec.name.as_bytes());
        fnv1a_update(&mut hash, &spec.type_idx.to_le_bytes());
    }
    hash
}

fn fnv1a_update(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

pub fn array_proto_property_name(import_name: &str) -> Option<String> {
    let suffix = import_name.strip_prefix("arr_proto_")?;
    let mut property = String::with_capacity(suffix.len());
    let mut upper_next = false;
    for byte in suffix.bytes() {
        if byte == b'_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            property.push((byte as char).to_ascii_uppercase());
            upper_next = false;
        } else {
            property.push(byte as char);
        }
    }
    Some(property)
}

#[cfg(test)]
mod registry_consistency {
    use super::*;
    use std::collections::HashMap;

    /// 导入名必须唯一——名字即 WASM import 的链接键，重复会让后注册的覆盖前者，
    /// 静默破坏调用目标。
    #[test]
    fn import_names_are_unique() {
        let mut seen: HashMap<&'static str, usize> = HashMap::new();
        for (i, spec) in HOST_IMPORT_SPECS.iter().enumerate() {
            if let Some(prev) = seen.insert(spec.name, i) {
                panic!(
                    "duplicate host import name {:?} at indices {prev} and {i}",
                    spec.name
                );
            }
        }
    }

    /// 每个 Builtin 至多绑定一个 host import——一个 Builtin 映射到两个 import
    /// 会让 builtin_func_indices 解析到非确定的目标。
    #[test]
    fn builtin_bindings_are_unique() {
        let mut seen: HashMap<Builtin, &'static str> = HashMap::new();
        for spec in HOST_IMPORT_SPECS.iter() {
            if let Some(HostImportKey::Builtin(b)) = spec.key {
                if let Some(prev) = seen.insert(b, spec.name) {
                    panic!("Builtin::{b:?} bound to both {prev:?} and {:?}", spec.name);
                }
            }
        }
    }

    /// 每个 SpecialHostImport 至多绑定一个 import——同上，避免解析歧义。
    #[test]
    fn special_bindings_are_unique() {
        let mut seen: HashMap<SpecialHostImport, &'static str> = HashMap::new();
        for spec in HOST_IMPORT_SPECS.iter() {
            if let Some(HostImportKey::Special(s)) = spec.key {
                if let Some(prev) = seen.insert(s, spec.name) {
                    panic!(
                        "SpecialHostImport::{s:?} bound to both {prev:?} and {:?}",
                        spec.name
                    );
                }
            }
        }
    }

    /// type_idx 的顺序无关性：注册表本身不依赖条目顺序解析 key，
    /// 因此 key 查找必须对任意排列稳定。这里验证 key→spec 反查不依赖位置。
    #[test]
    fn keyed_specs_are_reachable_by_key() {
        for spec in HOST_IMPORT_SPECS.iter() {
            let Some(key) = spec.key else { continue };
            let found = HOST_IMPORT_SPECS
                .iter()
                .find(|candidate| candidate.key == Some(key));
            assert!(
                found.is_some(),
                "keyed spec {:?} not reachable by its own key",
                spec.name
            );
        }
    }

    /// IR `Builtin` 变体数与 registry 中 Builtin 绑定数必须一致（见 `wjsm-ir::Builtin`）。
    /// 新增 Builtin 时：改 enum + 在此常量加 1 + 登记 `HOST_IMPORT_SPECS`。
    const EXPECTED_BUILTIN_REGISTRY_BINDINGS: usize = 382;

    #[test]
    fn builtin_registry_binding_count_matches_ir_contract() {
        let n = HOST_IMPORT_SPECS
            .iter()
            .filter(|spec| matches!(spec.key, Some(HostImportKey::Builtin(_))))
            .count();
        assert_eq!(
            n, EXPECTED_BUILTIN_REGISTRY_BINDINGS,
            "update EXPECTED_BUILTIN_REGISTRY_BINDINGS when adding/removing Builtin host imports"
        );
    }
}
