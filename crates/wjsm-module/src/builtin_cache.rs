//! builtin 段磁盘缓存：把 builtin 模块依赖闭包的 lower 产物序列化到
//! `${WJSM_CACHE_DIR}/builtin_ir/<key>.bin`，避免每次冷启动重复 lower。
//!
//! `emit_debug_checks` 与构建期生成的 [`BUILTIN_CACHE_ABI_HASH`] 共同决定。该指纹
//! 覆盖全部 builtin_js 源码及其 module/parser/semantic/IR lower 输入；这些输入任一
//! 改变都会自动切换缓存命名空间，并拒绝旧载荷，不再依赖人工维护版本号。
//!
//! # 段结构
//!
//! [`build_builtin_segment`] 以 frontier（用户模块直接引用的 builtin canonical 集合）
//! 为输入：构建 canonical builtin 闭包图（[`ModuleGraph::build_builtin_closure`]，
//! ModuleId 0..B 按 frontier 排序后确定分配），整体 lower 成单个 `Program`。段内
//! 元数据（导出映射、模块顶层作用域、作用域总数）由 wjsm-semantic 的
//! `lower_modules_with_debug_meta` 一并产出，随段序列化；hydration 时经
//! [`BuiltinSegmentCacheFile::to_semantic_segment`] 转为 `wjsm_semantic::BuiltinSegment`
//! 作为用户模块 lower 的种子，保证两段 id（ModuleId/FunctionId/ValueId/作用域）
//! 不重叠且 export 解析一致。

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use wjsm_ir::{FunctionId, ModuleId, Program};

use crate::builtin_modules;
use crate::bundler::module_metadata_for_node;
use crate::graph::ModuleGraph;
use crate::resolution_options::ResolutionOptions;
use crate::semantic::analyze_module_links;
use wjsm_semantic::ModuleLoweringInput;

use crate::builtin_modules::canonical_from_virtual_path;

include!(concat!(env!("OUT_DIR"), "/builtin_cache_abi_hash.rs"));

/// builtin 段中每个模块的布局记录（与 lower 时的 ModuleId / 作用域布局一致）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BuiltinModuleRecord {
    /// builtin canonical 名（不带 `node:` 前缀），如 `fs/promises`。
    pub canonical: String,
    /// 该模块 builtin_js 源码的 SHA-256（供 hydration 校验 / 追踪）。
    pub source_hash: [u8; 32],
    /// 该模块在段程序中的 ModuleId。
    pub module_id: u32,
    /// 该模块顶层作用域 ID（来自 `lower_modules_with_debug_meta` 的 module_scopes）。
    pub scope_id: usize,
}

/// 一个 builtin 依赖闭包的完整 lower 产物（磁盘缓存的载荷）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BuiltinSegmentCacheFile {
    /// 构建 builtin 段时的 [`BUILTIN_CACHE_ABI_HASH`]，用于拒绝旧载荷。
    pub cache_abi_hash: [u8; 32],
    /// 段内全部模块的布局记录（按 module_id 升序）。
    pub modules: Vec<BuiltinModuleRecord>,
    /// 段 IR：闭包内所有模块共同降级进入口函数（`$builtin_main`）的完整 `Program`。
    pub program: Program,
    /// 段 lower 结束时 lowerer 作用域总数（含 root）；hydration 用作用户 scope 基址。
    pub scope_count: u32,
    /// 段入口函数（由 `$module_main` 改名而来，见 [`rename_entry_function`]）。
    pub entry_function_id: FunctionId,
    /// `(module_id, 导出名) → IR 变量名`（来自 `lower_modules_with_debug_meta` 的 export_map）。
    pub export_map: Vec<((u32, String), String)>,
    /// `(module_id, 导出名列表)`：与 lowerer 内部 `module_export_names` 同源
    /// （`analyze_module_links` 的 `export_names`），按 module_id 升序。
    pub module_export_names: Vec<(u32, Vec<String>)>,
}

/// 计算 builtin 段缓存键：
/// `sha256(BUILTIN_CACHE_ABI_HASH ‖ u8(emit_debug_checks) ‖ 每个 canonical 名)`，
/// 十六进制输出（64 字符）。
///
/// [`BUILTIN_CACHE_ABI_HASH`] 覆盖全部 builtin_js 源码，因此也覆盖 frontier 的
/// 传递依赖；它同时覆盖 module/parser/semantic/IR 的 lower 输入与 Cargo.lock。
/// frontier 按 `BTreeSet` 排序迭代，键与元素顺序无关；`node:` 前缀先被归一化掉
/// （与 `builtin_modules::lookup` 的规则一致）。未知 canonical 返回错误，避免生成
/// 内容不确定的键。resolution options 与 root 有意不入键：builtin 源码自包含，只 import
/// 其它 `node:` builtin。
pub(crate) fn builtin_cache_key(
    frontier: &BTreeSet<String>,
    emit_debug_checks: bool,
) -> Result<String, anyhow::Error> {
    builtin_cache_key_with_abi_hash(frontier, emit_debug_checks, &BUILTIN_CACHE_ABI_HASH)
}

fn builtin_cache_key_with_abi_hash(
    frontier: &BTreeSet<String>,
    emit_debug_checks: bool,
    cache_abi_hash: &[u8; 32],
) -> Result<String, anyhow::Error> {
    let mut hasher = Sha256::new();
    hasher.update(b"wjsm-builtin-ir-cache-v1\0");
    hasher.update(cache_abi_hash);
    hasher.update([u8::from(emit_debug_checks)]);
    for specifier in frontier {
        let canonical = specifier.strip_prefix("node:").unwrap_or(specifier);
        if builtin_modules::source_for_canonical(canonical).is_none() {
            bail!("builtin cache key: 未知 builtin canonical {canonical:?}");
        }
        hash_cache_key_field(&mut hasher, canonical.as_bytes());
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hash_cache_key_field(hasher: &mut Sha256, field: &[u8]) {
    let len = u64::try_from(field.len()).expect("缓存键字段长度应可表示为 u64");
    hasher.update(len.to_le_bytes());
    hasher.update(field);
}

/// 从 `${dir}/<key>.bin` 读取并校验 builtin 段。任何失败（缺文件、反序列化错误、
/// ABI 指纹不匹配）都返回 `None`——调用方随后走 [`build_builtin_segment`] 重建。
///
/// 不做 `program.verify()` 门禁：部分 builtin 闭包（events/path/perf_hooks）在基线上
/// 就存在死块校验告警（block has instructions but terminator is unreachable），运行时
/// 与 native 编译均容忍；若把 verify 当命中条件，这些闭包的缓存永远不命中。
/// 段与 plain 路径同源（同一 lowerer），结构合法由 bincode 解码 + ABI 指纹保证。
pub(crate) fn load_builtin_segment(dir: &Path, key: &str) -> Option<BuiltinSegmentCacheFile> {
    if let Some(segment) = memory_get(key) {
        return Some(segment);
    }
    let path = dir.join(format!("{key}.bin"));
    let bytes = std::fs::read(path).ok()?;
    let (segment, _consumed): (BuiltinSegmentCacheFile, usize) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
    if segment.cache_abi_hash != BUILTIN_CACHE_ABI_HASH {
        return None;
    }
    memory_put(key, &segment);
    Some(segment)
}

pub(crate) fn load_memory_segment(key: &str) -> Option<BuiltinSegmentCacheFile> {
    memory_get(key)
}

pub(crate) fn remember_builtin_segment(key: &str, segment: &BuiltinSegmentCacheFile) {
    memory_put(key, segment);
}

static MEMORY_SEGMENTS: LazyLock<Mutex<HashMap<String, Arc<BuiltinSegmentCacheFile>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn memory_get(key: &str) -> Option<BuiltinSegmentCacheFile> {
    MEMORY_SEGMENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .map(|segment| (**segment).clone())
}

fn memory_put(key: &str, segment: &BuiltinSegmentCacheFile) {
    MEMORY_SEGMENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key.to_owned(), Arc::new(segment.clone()));
}

/// 原子写入 `${dir}/<key>.bin`：先写同目录临时文件再 rename，避免半截文件被读到。
pub(crate) fn store_builtin_segment(
    dir: &Path,
    key: &str,
    segment: &BuiltinSegmentCacheFile,
) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("创建 builtin 缓存目录 {}", dir.display()))?;
    let bytes = bincode::serde::encode_to_vec(segment, bincode::config::standard())
        .map_err(|error| anyhow!("bincode 序列化 builtin 段失败: {error}"))?;
    let final_path = dir.join(format!("{key}.bin"));
    let tmp_path = dir.join(format!(".{key}.{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&tmp_path, &bytes) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error)
            .with_context(|| format!("写入 builtin 段临时文件 {}", tmp_path.display()));
    }
    if let Err(error) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error)
            .with_context(|| format!("原子替换 builtin 段缓存 {}", final_path.display()));
    }
    Ok(())
}

/// 构建一个 builtin 依赖闭包段。
///
/// 语义：以 `frontier`（builtin canonical 集合，可带 `node:` 前缀）为多入口，构建
/// canonical 闭包图（[`ModuleGraph::build_builtin_closure`]，ModuleId 0..B 按 frontier
/// 排序后确定分配），用 [`wjsm_semantic::lower_modules_with_debug_meta`] 整体 lower 闭包
/// 成单个 `Program`——闭包内共享依赖只 lower 一次。段内元数据（export_map、模块顶层
/// 作用域、作用域总数）直接来自 lowerer，随段序列化供 hydration 复用。
pub(crate) fn build_builtin_segment(
    frontier: &BTreeSet<String>,
    root: &Path,
    options: &ResolutionOptions,
    emit_debug_checks: bool,
) -> Result<BuiltinSegmentCacheFile> {
    if frontier.is_empty() {
        bail!("builtin 段 frontier 不能为空");
    }

    // 1) canonical builtin 闭包图（多入口，id 分配确定）。
    let graph = ModuleGraph::build_builtin_closure(frontier, root, options.clone())
        .with_context(|| format!("构建 builtin 闭包图 {frontier:?}"))?;

    // 2) 拓扑序 + 链接分析（与 bundler::lower_bundle 的用户路径一致）。
    let (order, cycles) = graph
        .topological_order()
        .with_context(|| format!("builtin 闭包拓扑排序 {frontier:?}"))?;
    let _ = cycles;
    let link =
        analyze_module_links(&graph).with_context(|| format!("分析 builtin 链接 {frontier:?}"))?;

    // 3) 整体 lower 闭包，拿 LoweringMetadata（export_map / module_scopes / scope_count）。
    let mut modules = Vec::new();
    for &id in &order {
        let node = graph
            .get_module(id)
            .with_context(|| format!("builtin 闭包图缺少模块 {id:?}"))?;
        modules.push(ModuleLoweringInput {
            id: node.id,
            ast: node.ast.clone(),
            metadata: module_metadata_for_node(node)?,
            source: Some(std::sync::Arc::<str>::from(node.source.as_str())),
        });
    }
    let (mut program, metadata) = wjsm_semantic::lower_modules_with_debug_meta(
        modules,
        &link.import_map,
        &link.dynamic_import_targets,
        &link.export_names,
        &link.dynamic_import_specifiers,
        &link.re_export_map,
        emit_debug_checks,
    )
    .with_context(|| format!("lower builtin 闭包段 {frontier:?}"))?;

    // 4) 入口函数：多模块共同降级进 $module_main（wjsm_ir::MODULE_ENTRY_IR_NAME），
    //    改名为 $builtin_main 避免与用户段入口同名冲突。
    let entry_function_id = locate_entry_function(&program)?;
    rename_entry_function(&mut program, entry_function_id)?;

    // 5) 模块布局记录：按 module_id 升序（all_module_ids 来自 HashMap，迭代序不确定）。
    let mut module_ids: Vec<_> = graph.all_module_ids().collect();
    module_ids.sort_by_key(|id| id.0);
    let mut modules_record = Vec::with_capacity(module_ids.len());
    for &id in &module_ids {
        let node = graph
            .get_module(id)
            .with_context(|| format!("builtin 图缺少模块 {id:?}"))?;
        let canonical = canonical_from_virtual_path(&node.path).with_context(|| {
            format!(
                "builtin 图节点 {} 不是 builtin 虚拟路径: {}",
                id.0,
                node.path.display()
            )
        })?;
        let source = builtin_modules::source_for_canonical(canonical)
            .with_context(|| format!("builtin 源码缺失: {canonical:?}"))?;
        modules_record.push(BuiltinModuleRecord {
            canonical: canonical.to_string(),
            source_hash: Sha256::digest(source.as_bytes()).into(),
            module_id: id.0,
            scope_id: metadata.module_scopes.get(&id).copied().unwrap_or(0),
        });
    }

    // 6) 导出映射：lowerer 内部 export_map 与 module_export_names 序列化。
    let mut export_map: Vec<((u32, String), String)> = metadata
        .export_map
        .iter()
        .map(|((id, name), ir_name)| ((id.0, name.clone()), ir_name.clone()))
        .collect();
    export_map.sort_by(|a, b| a.0.cmp(&b.0));
    let mut module_export_names: Vec<(u32, Vec<String>)> = link
        .export_names
        .iter()
        .map(|(id, names)| (id.0, names.iter().cloned().collect()))
        .collect();
    module_export_names.sort_by_key(|(id, _)| *id);

    let scope_count = u32::try_from(metadata.scope_count)
        .with_context(|| format!("builtin 段作用域总数 {} 超出 u32", metadata.scope_count))?;

    Ok(BuiltinSegmentCacheFile {
        cache_abi_hash: BUILTIN_CACHE_ABI_HASH,
        modules: modules_record,
        program,
        scope_count,
        entry_function_id,
        export_map,
        module_export_names,
    })
}

/// 在段程序中定位合成入口函数 `$module_main`。
fn locate_entry_function(program: &Program) -> Result<FunctionId, anyhow::Error> {
    let index = program
        .functions()
        .iter()
        .position(|function| function.name() == wjsm_ir::MODULE_ENTRY_IR_NAME)
        .with_context(|| format!("builtin 段缺少入口函数 {}", wjsm_ir::MODULE_ENTRY_IR_NAME))?;
    let index =
        u32::try_from(index).with_context(|| format!("builtin 段函数表索引 {index} 超出 u32"))?;
    Ok(FunctionId(index))
}

/// 把段入口函数 `$module_main` 改名为 `$builtin_main`，避免 hydration 合并段时与
/// 用户段入口函数同名。
fn rename_entry_function(program: &mut Program, entry_id: FunctionId) -> Result<(), anyhow::Error> {
    let entry = program
        .function_mut(entry_id)
        .with_context(|| format!("builtin 段缺少入口函数 {entry_id:?}"))?;
    if entry.name() != wjsm_ir::MODULE_ENTRY_IR_NAME {
        bail!(
            "builtin 段入口函数名异常: {:?}（期望 {:?}）",
            entry.name(),
            wjsm_ir::MODULE_ENTRY_IR_NAME
        );
    }
    entry.set_name("$builtin_main");
    Ok(())
}

impl BuiltinSegmentCacheFile {
    /// 把序列化形式的段转换为 wjsm-semantic 的 hydration 种子。
    ///
    /// 合并时 wjsm-semantic 保持 builtin 段函数原序追加到用户段之前，因此
    /// `entry_function_id` 在合并后的 Program 中仍指向 `$builtin_main`。
    pub(crate) fn to_semantic_segment(&self) -> wjsm_semantic::BuiltinSegment {
        use std::collections::{BTreeSet, HashMap};
        wjsm_semantic::BuiltinSegment {
            program: self.program.clone(),
            scope_count: self.scope_count,
            entry_function_id: self.entry_function_id,
            export_map: self
                .export_map
                .iter()
                .map(|((id, name), ir_name)| ((ModuleId(*id), name.clone()), ir_name.clone()))
                .collect::<HashMap<_, _>>(),
            module_export_names: self
                .module_export_names
                .iter()
                .map(|(id, names)| {
                    (
                        ModuleId(*id),
                        names.iter().cloned().collect::<BTreeSet<_>>(),
                    )
                })
                .collect::<HashMap<_, _>>(),
            module_scopes: self
                .modules
                .iter()
                .map(|record| (ModuleId(record.module_id), record.scope_id))
                .collect::<HashMap<_, _>>(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontier(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn cache_key_is_deterministic_order_free_and_debug_sensitive() {
        let a = builtin_cache_key(&frontier(&["fs", "path"]), false).unwrap();
        let b = builtin_cache_key(&frontier(&["path", "fs"]), false).unwrap();
        assert_eq!(a, b);
        assert_ne!(
            a,
            builtin_cache_key(&frontier(&["fs", "path"]), true).unwrap()
        );
        assert_eq!(a.len(), 64, "sha256 十六进制长度");
    }

    #[test]
    fn cache_key_changes_with_abi_fingerprint() {
        let frontier = frontier(&["fs", "path"]);
        let current =
            builtin_cache_key_with_abi_hash(&frontier, false, &BUILTIN_CACHE_ABI_HASH).unwrap();
        let mut changed_abi_hash = BUILTIN_CACHE_ABI_HASH;
        changed_abi_hash[0] ^= 1;
        let changed = builtin_cache_key_with_abi_hash(&frontier, false, &changed_abi_hash).unwrap();

        assert_ne!(current, changed);
        assert_eq!(builtin_cache_key(&frontier, false).unwrap(), current);
    }

    #[test]
    fn cache_key_normalizes_node_prefix() {
        assert_eq!(
            builtin_cache_key(&frontier(&["fs"]), false).unwrap(),
            builtin_cache_key(&frontier(&["node:fs"]), false).unwrap()
        );
    }

    #[test]
    fn cache_key_rejects_unknown_canonical() {
        assert!(builtin_cache_key(&frontier(&["not_a_builtin"]), false).is_err());
    }

    #[test]
    fn store_load_roundtrip_and_abi_gate() {
        // 最小合法 Program：单个空入口函数（verify 可过）。
        let mut program = Program::new();
        let mut function =
            wjsm_ir::Function::new(wjsm_ir::MODULE_ENTRY_IR_NAME, wjsm_ir::BasicBlockId(0));
        function.push_block(wjsm_ir::BasicBlock::new_with_terminator(
            wjsm_ir::BasicBlockId(0),
            wjsm_ir::Terminator::Return { value: None },
        ));
        let entry_function_id = program.push_function(function);
        assert_eq!(entry_function_id, FunctionId(0));

        let segment = BuiltinSegmentCacheFile {
            cache_abi_hash: BUILTIN_CACHE_ABI_HASH,
            modules: vec![BuiltinModuleRecord {
                canonical: "fs".to_string(),
                source_hash: [7u8; 32],
                module_id: 0,
                scope_id: 0,
            }],
            program,
            scope_count: 0,
            entry_function_id,
            export_map: vec![((0, "readFile".to_string()), "$0.readFile".to_string())],
            module_export_names: vec![(0, vec!["readFile".to_string()])],
        };

        let dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("module")
            .join(format!("builtin-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        store_builtin_segment(&dir, "testkey", &segment).unwrap();
        let loaded = load_builtin_segment(&dir, "testkey").expect("roundtrip 应命中");
        assert_eq!(loaded, segment);

        // ABI 指纹不匹配 → 视为过期，返回 None。
        let mut stale = segment.clone();
        stale.cache_abi_hash[0] ^= 1;
        store_builtin_segment(&dir, "stale", &stale).unwrap();
        assert!(load_builtin_segment(&dir, "stale").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_segment_for_real_builtin_closure() {
        // assert 通过 `node:util` 依赖 util —— 真实依赖闭包：assert + util
        // 两个模块共享一次 lower，合并进同一个段程序。
        let segment = build_builtin_segment(
            &frontier(&["assert"]),
            Path::new("."),
            &ResolutionOptions::default(),
            false,
        )
        .expect("lower assert 闭包应成功");
        assert_eq!(segment.cache_abi_hash, BUILTIN_CACHE_ABI_HASH);

        let canonicals: Vec<&str> = segment
            .modules
            .iter()
            .map(|record| record.canonical.as_str())
            .collect();
        assert!(
            canonicals.contains(&"assert"),
            "闭包含 assert: {canonicals:?}"
        );
        assert!(
            canonicals.contains(&"util"),
            "assert 依赖 util: {canonicals:?}"
        );
        // 入口模块排在最前（module_id 升序）。
        assert_eq!(segment.modules[0].canonical, "assert");

        // 入口函数已改名为 $builtin_main（避免与用户段 $module_main 冲突），
        // 且段程序通过 IR 校验。
        let index = usize::try_from(segment.entry_function_id.0).expect("u32 索引在 usize 内");
        assert_eq!(segment.program.functions()[index].name(), "$builtin_main");
        assert!(segment.program.verify().is_ok());
        assert!(!segment.module_export_names.is_empty());
        // LoweringMetadata 已接入：作用域总数与导出映射不再是占位。
        assert!(segment.scope_count > 0, "scope_count 应从 lowerer 产出");
        assert!(
            segment.modules.iter().all(|record| record.scope_id > 0),
            "模块顶层作用域应从 lowerer 产出: {:?}",
            segment.modules
        );
        assert!(
            !segment.export_map.is_empty(),
            "export_map 应从 lowerer 产出"
        );

        // 段可序列化落盘并读回。
        let dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("module")
            .join(format!("builtin-cache-closure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        store_builtin_segment(&dir, "assert", &segment).unwrap();
        let loaded = load_builtin_segment(&dir, "assert").expect("闭包段 roundtrip 应命中");
        // Program 常量含 NaN（assert/util 源码），f64 的 PartialEq 认为 NaN != NaN，
        // 不能整体比较；改为逐字段 + 结构不变量。
        assert_eq!(loaded.cache_abi_hash, segment.cache_abi_hash);
        assert_eq!(loaded.modules, segment.modules);
        assert_eq!(loaded.scope_count, segment.scope_count);
        assert_eq!(loaded.entry_function_id, segment.entry_function_id);
        assert_eq!(loaded.export_map, segment.export_map);
        assert_eq!(loaded.module_export_names, segment.module_export_names);
        assert_eq!(
            loaded.program.functions().len(),
            segment.program.functions().len()
        );
        assert_eq!(
            loaded.program.constants().len(),
            segment.program.constants().len()
        );
        assert!(loaded.program.verify().is_ok());
        let _ = std::fs::remove_dir_all(&dir);

        // 多入口闭包：frontier 元素不需要落在单一入口的闭包内。
        let multi = build_builtin_segment(
            &frontier(&["assert", "fs"]),
            Path::new("."),
            &ResolutionOptions::default(),
            false,
        )
        .expect("多入口 builtin 闭包应成功");
        let canonicals: Vec<&str> = multi
            .modules
            .iter()
            .map(|record| record.canonical.as_str())
            .collect();
        assert!(
            canonicals.contains(&"assert"),
            "闭包含 assert: {canonicals:?}"
        );
        assert!(canonicals.contains(&"fs"), "闭包含 fs: {canonicals:?}");
        // 入口按 frontier 排序确定分配：assert 在 fs 前。
        assert_eq!(multi.modules[0].canonical, "assert");
        assert!(multi.program.verify().is_ok());
        let multi_index = usize::try_from(multi.entry_function_id.0).expect("u32 索引在 usize 内");
        assert_eq!(
            multi.program.functions()[multi_index].name(),
            "$builtin_main"
        );

        // to_semantic_segment：字段齐全且 id 对应。
        let semantic = multi.to_semantic_segment();
        assert_eq!(semantic.scope_count, multi.scope_count);
        assert_eq!(semantic.entry_function_id, multi.entry_function_id);
        assert_eq!(
            semantic.program.functions().len(),
            multi.program.functions().len()
        );
        for record in &multi.modules {
            let module_id = ModuleId(record.module_id);
            assert_eq!(
                semantic.module_scopes.get(&module_id).copied(),
                Some(record.scope_id)
            );
        }
        assert_eq!(
            semantic.export_map.len(),
            multi.export_map.len(),
            "export_map 转换不应丢条目"
        );
    }
}
