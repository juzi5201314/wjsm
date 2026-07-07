use std::collections::HashMap;
use std::path::PathBuf;

use wjsm_ir::value;

const RUNTIME_MODULE_ID_BASE: u32 = 1 << 30;

/// 运行时模块缓存使用的规范化 key。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RuntimeModuleKey {
    File(PathBuf),
    Json(PathBuf),
    Builtin(String),
    /// 初始 AOT bundle 的旧 ModuleId 兼容 key。
    PrecompiledModuleId(u32),
    /// 运行时编译 bundle 预留后的全局 ModuleId key。
    RuntimeModuleId(u32),
}

impl RuntimeModuleKey {
    fn can_delete_from_require_cache(&self) -> bool {
        matches!(self, Self::File(_) | Self::Json(_))
    }

    fn from_registered_module_id(module_id: u32) -> Self {
        if module_id >= RUNTIME_MODULE_ID_BASE {
            Self::RuntimeModuleId(module_id)
        } else {
            Self::PrecompiledModuleId(module_id)
        }
    }
}

fn require_cache_id_for_key(key: &RuntimeModuleKey) -> Option<String> {
    match key {
        RuntimeModuleKey::File(path) | RuntimeModuleKey::Json(path) => {
            Some(path.to_string_lossy().into_owned())
        }
        RuntimeModuleKey::Builtin(specifier) => Some(specifier.clone()),
        RuntimeModuleKey::PrecompiledModuleId(_) | RuntimeModuleKey::RuntimeModuleId(_) => None,
    }
}

fn require_cache_id_matches_key(cache_id: &str, key: &RuntimeModuleKey) -> bool {
    require_cache_id_for_key(key).as_deref() == Some(cache_id)
}

/// `require.cache` 上可观察的一条缓存记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRequireCacheEntry {
    pub id: String,
    pub module_object: i64,
}

/// registry 中单个模块的执行状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeModuleState {
    Loading {
        module_object: i64,
        exports_object: i64,
    },
    Loaded {
        module_object: i64,
        exports_object: i64,
        namespace_object: i64,
    },
    Errored {
        error_value: i64,
    },
}

/// CJS `require()` 查询 registry 后的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeModuleRequireResult {
    Missing,
    Exports(i64),
    LoadedModule {
        module_object: i64,
        exports_object: i64,
    },
    Errored(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeModuleImportResult {
    Missing,
    Namespace(i64),
    Errored(i64),
}

/// 运行时模块状态的唯一 owner。
#[derive(Clone, Debug)]
pub struct RuntimeModuleRegistry {
    by_key: HashMap<RuntimeModuleKey, RuntimeModuleState>,
    by_module_id: HashMap<u32, RuntimeModuleKey>,
    next_runtime_module_id: u32,
}

impl Default for RuntimeModuleRegistry {
    fn default() -> Self {
        Self {
            by_key: HashMap::new(),
            by_module_id: HashMap::new(),
            next_runtime_module_id: RUNTIME_MODULE_ID_BASE,
        }
    }
}

impl RuntimeModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve_runtime_module_id_range(&mut self, count: u32) -> Option<u32> {
        let base = self.next_runtime_module_id;
        self.next_runtime_module_id = self.next_runtime_module_id.checked_add(count)?;
        Some(base)
    }

    pub fn begin_loading(
        &mut self,
        key: RuntimeModuleKey,
        module_id: Option<u32>,
        module_object: i64,
        exports_object: i64,
    ) {
        self.remember_module_id(module_id, &key);
        self.by_key.insert(
            key,
            RuntimeModuleState::Loading {
                module_object,
                exports_object,
            },
        );
    }

    pub fn finish_loaded(
        &mut self,
        key: RuntimeModuleKey,
        module_id: Option<u32>,
        module_object: i64,
        exports_object: i64,
        namespace_object: i64,
    ) {
        self.remove_module_id_key_if_different(module_id, &key);
        self.remember_module_id(module_id, &key);
        self.by_key.insert(
            key,
            RuntimeModuleState::Loaded {
                module_object,
                exports_object,
                namespace_object,
            },
        );
    }

    pub fn finish_errored(
        &mut self,
        key: RuntimeModuleKey,
        module_id: Option<u32>,
        error_value: i64,
    ) {
        self.remove_module_id_key_if_different(module_id, &key);
        self.remember_module_id(module_id, &key);
        self.by_key
            .insert(key, RuntimeModuleState::Errored { error_value });
    }

    pub fn register_static_namespace(&mut self, module_id: u32, namespace_object: i64) {
        self.finish_loaded(
            RuntimeModuleKey::from_registered_module_id(module_id),
            Some(module_id),
            value::encode_undefined(),
            namespace_object,
            namespace_object,
        );
    }

    pub fn get_for_require(&self, key: &RuntimeModuleKey) -> RuntimeModuleRequireResult {
        match self.by_key.get(key) {
            Some(RuntimeModuleState::Loading { exports_object, .. }) => {
                RuntimeModuleRequireResult::Exports(*exports_object)
            }
            Some(RuntimeModuleState::Loaded {
                module_object,
                exports_object,
                ..
            }) => RuntimeModuleRequireResult::LoadedModule {
                module_object: *module_object,
                exports_object: *exports_object,
            },
            Some(RuntimeModuleState::Errored { error_value }) => {
                RuntimeModuleRequireResult::Errored(*error_value)
            }
            None => RuntimeModuleRequireResult::Missing,
        }
    }

    pub(crate) fn is_loading(&self, key: &RuntimeModuleKey) -> bool {
        matches!(self.by_key.get(key), Some(RuntimeModuleState::Loading { .. }))
    }

    pub(crate) fn loading_module(&self, key: &RuntimeModuleKey) -> Option<(i64, i64)> {
        match self.by_key.get(key) {
            Some(RuntimeModuleState::Loading {
                module_object,
                exports_object,
            }) => Some((*module_object, *exports_object)),
            _ => None,
        }
    }

    pub(crate) fn get_for_import(&self, key: &RuntimeModuleKey) -> RuntimeModuleImportResult {
        match self.by_key.get(key) {
            Some(RuntimeModuleState::Loaded {
                namespace_object, ..
            }) => RuntimeModuleImportResult::Namespace(*namespace_object),
            Some(RuntimeModuleState::Errored { error_value }) => {
                RuntimeModuleImportResult::Errored(*error_value)
            }
            _ => RuntimeModuleImportResult::Missing,
        }
    }

    fn require_cache_entry(
        key: &RuntimeModuleKey,
        state: &RuntimeModuleState,
    ) -> Option<RuntimeRequireCacheEntry> {
        let id = require_cache_id_for_key(key)?;
        let module_object = match state {
            RuntimeModuleState::Loading { module_object, .. }
            | RuntimeModuleState::Loaded { module_object, .. } => *module_object,
            RuntimeModuleState::Errored { .. } => return None,
        };
        Some(RuntimeRequireCacheEntry { id, module_object })
    }

    pub fn require_cache_entry_by_id(&self, cache_id: &str) -> Option<RuntimeRequireCacheEntry> {
        self.by_key.iter().find_map(|(key, state)| {
            require_cache_id_matches_key(cache_id, key)
                .then(|| Self::require_cache_entry(key, state))
                .flatten()
        })
    }

    pub fn require_cache_entries(&self) -> Vec<RuntimeRequireCacheEntry> {
        let mut entries = self
            .by_key
            .iter()
            .filter_map(|(key, state)| Self::require_cache_entry(key, state))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        entries
    }

    pub fn delete_cache_entry_by_id(&mut self, cache_id: &str) -> bool {
        let Some(key) = self
            .by_key
            .keys()
            .find(|key| require_cache_id_matches_key(cache_id, key))
            .cloned()
        else {
            return true;
        };
        self.delete_cache_entry(&key)
    }

    pub fn get_namespace_by_module_id(&self, module_id: u32) -> Option<i64> {
        let key = self.by_module_id.get(&module_id)?;
        match self.by_key.get(key) {
            Some(RuntimeModuleState::Loaded {
                namespace_object, ..
            }) => Some(*namespace_object),
            _ => None,
        }
    }

    pub fn delete_cache_entry(&mut self, key: &RuntimeModuleKey) -> bool {
        match self.by_key.get(key) {
            Some(RuntimeModuleState::Loading { .. }) => false,
            Some(_) if key.can_delete_from_require_cache() => {
                self.by_key.remove(key);
                self.by_module_id.retain(|_, mapped_key| mapped_key != key);
                true
            }
            _ => true,
        }
    }

    pub fn roots(&self) -> Vec<i64> {
        let mut roots = Vec::new();
        for state in self.by_key.values() {
            match state {
                RuntimeModuleState::Loading {
                    module_object,
                    exports_object,
                } => {
                    roots.push(*module_object);
                    roots.push(*exports_object);
                }
                RuntimeModuleState::Loaded {
                    module_object,
                    exports_object,
                    namespace_object,
                } => {
                    roots.push(*module_object);
                    roots.push(*exports_object);
                    roots.push(*namespace_object);
                }
                RuntimeModuleState::Errored { error_value } => roots.push(*error_value),
            }
        }
        roots
    }

    fn remove_module_id_key_if_different(
        &mut self,
        module_id: Option<u32>,
        key: &RuntimeModuleKey,
    ) {
        let Some(module_id) = module_id else {
            return;
        };
        let module_id_key = RuntimeModuleKey::from_registered_module_id(module_id);
        if &module_id_key != key {
            self.by_key.remove(&module_id_key);
        }
    }

    fn remember_module_id(&mut self, module_id: Option<u32>, key: &RuntimeModuleKey) {
        if let Some(module_id) = module_id {
            self.by_module_id.insert(module_id, key.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        RuntimeModuleImportResult, RuntimeModuleKey, RuntimeModuleRegistry,
        RuntimeModuleRequireResult,
    };
    use wjsm_ir::value;

    fn obj(handle: u32) -> i64 {
        value::encode_object_handle(handle)
    }

    fn file_key(name: &str) -> RuntimeModuleKey {
        RuntimeModuleKey::File(PathBuf::from(name))
    }

    fn json_key(name: &str) -> RuntimeModuleKey {
        RuntimeModuleKey::Json(PathBuf::from(name))
    }

    #[test]
    fn module_registry_returns_loaded_namespace_by_module_id() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/main.js");
        let namespace = obj(12);

        registry.finish_loaded(key, Some(7), obj(10), obj(11), namespace);

        assert_eq!(registry.get_namespace_by_module_id(7), Some(namespace));
    }

    #[test]
    fn module_registry_returns_loading_exports_for_circular_require() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/circular.js");
        let module = obj(20);
        let exports = obj(21);

        registry.begin_loading(key.clone(), Some(8), module, exports);

        assert_eq!(
            registry.get_for_require(&key),
            RuntimeModuleRequireResult::Exports(exports)
        );
        assert_eq!(registry.loading_module(&key), Some((module, exports)));
    }

    #[test]
    fn module_registry_returns_loaded_module_owner_for_live_exports() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/loaded.js");
        let module = obj(30);
        let initial_exports = obj(31);

        registry.finish_loaded(key.clone(), Some(9), module, initial_exports, obj(32));

        assert_eq!(
            registry.get_for_require(&key),
            RuntimeModuleRequireResult::LoadedModule {
                module_object: module,
                exports_object: initial_exports,
            }
        );
    }

    #[test]
    fn module_registry_delete_loaded_file_entry() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/delete-me.js");

        registry.finish_loaded(key.clone(), Some(9), obj(30), obj(31), obj(32));

        assert!(registry.delete_cache_entry(&key));
        assert_eq!(
            registry.get_for_require(&key),
            RuntimeModuleRequireResult::Missing
        );
        assert_eq!(registry.get_namespace_by_module_id(9), None);
    }

    #[test]
    fn module_registry_delete_loaded_json_entry() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = json_key("/project/data.json");

        registry.finish_loaded(key.clone(), None, obj(33), obj(34), obj(34));

        assert!(registry.delete_cache_entry(&key));
        assert_eq!(
            registry.get_for_require(&key),
            RuntimeModuleRequireResult::Missing
        );
    }

    #[test]
    fn module_registry_delete_errored_file_entry() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/broken.js");

        registry.finish_errored(key.clone(), Some(12), obj(35));

        assert!(registry.delete_cache_entry(&key));
        assert_eq!(
            registry.get_for_require(&key),
            RuntimeModuleRequireResult::Missing
        );
        assert_eq!(registry.get_namespace_by_module_id(12), None);
    }

    #[test]
    fn module_registry_refuses_delete_loading_entry() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/loading.js");
        let exports = obj(41);

        registry.begin_loading(key.clone(), Some(10), obj(40), exports);

        assert!(!registry.delete_cache_entry(&key));
        assert_eq!(
            registry.get_for_require(&key),
            RuntimeModuleRequireResult::Exports(exports)
        );
    }

    #[test]
    fn module_registry_errored_entry_is_rooted() {
        let mut registry = RuntimeModuleRegistry::new();
        let key = file_key("/project/broken.js");
        let error = obj(50);

        registry.finish_errored(key, Some(11), error);

        assert_eq!(registry.roots(), vec![error]);
    }

    #[test]
    fn module_registry_reserves_disjoint_runtime_module_id_ranges() {
        let mut registry = RuntimeModuleRegistry::new();

        let first = registry
            .reserve_runtime_module_id_range(2)
            .expect("first range should reserve");
        let second = registry
            .reserve_runtime_module_id_range(3)
            .expect("second range should reserve");

        assert_eq!(first, super::RUNTIME_MODULE_ID_BASE);
        assert_eq!(second, super::RUNTIME_MODULE_ID_BASE + 2);
    }

    #[test]
    fn module_registry_keeps_runtime_module_ids_out_of_precompiled_keyspace() {
        let mut registry = RuntimeModuleRegistry::new();
        let runtime_base = registry
            .reserve_runtime_module_id_range(2)
            .expect("runtime range should reserve");
        let static_namespace = obj(60);
        let runtime_namespace = obj(61);

        registry.register_static_namespace(1, static_namespace);
        registry.register_static_namespace(runtime_base + 1, runtime_namespace);

        assert_eq!(registry.get_namespace_by_module_id(1), Some(static_namespace));
        assert_eq!(
            registry.get_namespace_by_module_id(runtime_base + 1),
            Some(runtime_namespace)
        );
        assert!(registry
            .by_key
            .contains_key(&RuntimeModuleKey::PrecompiledModuleId(1)));
        assert!(registry
            .by_key
            .contains_key(&RuntimeModuleKey::RuntimeModuleId(runtime_base + 1)));
    }

    #[test]
    fn module_registry_promotes_runtime_entry_id_to_canonical_key() {
        let mut registry = RuntimeModuleRegistry::new();
        let runtime_base = registry
            .reserve_runtime_module_id_range(1)
            .expect("runtime range should reserve");
        let key = file_key("/project/runtime-entry.mjs");
        let namespace = obj(70);

        registry.register_static_namespace(runtime_base, namespace);
        registry.finish_loaded(
            key.clone(),
            Some(runtime_base),
            obj(71),
            obj(72),
            namespace,
        );

        assert_eq!(registry.get_namespace_by_module_id(runtime_base), Some(namespace));
        assert!(registry.by_key.contains_key(&key));
        assert!(!registry
            .by_key
            .contains_key(&RuntimeModuleKey::RuntimeModuleId(runtime_base)));
    }

    #[test]
    fn module_registry_promotes_runtime_entry_id_to_canonical_errored_key() {
        let mut registry = RuntimeModuleRegistry::new();
        let runtime_base = registry
            .reserve_runtime_module_id_range(1)
            .expect("runtime range should reserve");
        let key = file_key("/project/runtime-throw.mjs");
        let namespace = obj(80);
        let error = obj(81);

        registry.register_static_namespace(runtime_base, namespace);
        registry.finish_errored(key.clone(), Some(runtime_base), error);

        assert_eq!(registry.get_namespace_by_module_id(runtime_base), None);
        assert_eq!(
            registry.get_for_import(&RuntimeModuleKey::RuntimeModuleId(runtime_base)),
            RuntimeModuleImportResult::Missing
        );
        assert!(registry.by_key.contains_key(&key));
        assert!(!registry
            .by_key
            .contains_key(&RuntimeModuleKey::RuntimeModuleId(runtime_base)));
        assert_eq!(registry.roots(), vec![error]);
    }
}
