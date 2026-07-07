use std::collections::HashMap;
use std::path::PathBuf;

use wjsm_ir::value;

/// 运行时模块缓存使用的规范化 key。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RuntimeModuleKey {
    File(PathBuf),
    Json(PathBuf),
    Builtin(String),
    /// 过渡 key：承接现有 `register_module_namespace(module_id)` 静态快路径。
    ///
    /// 后续 host import 全部拿到 File/Json/Builtin key 后，应停止新写入该分支。
    PrecompiledModuleId(u32),
}

impl RuntimeModuleKey {
    fn can_delete_from_require_cache(&self) -> bool {
        matches!(self, Self::File(_) | Self::Json(_))
    }
}

fn require_cache_id_for_key(key: &RuntimeModuleKey) -> Option<String> {
    match key {
        RuntimeModuleKey::File(path) | RuntimeModuleKey::Json(path) => {
            Some(path.to_string_lossy().into_owned())
        }
        RuntimeModuleKey::Builtin(specifier) => Some(specifier.clone()),
        RuntimeModuleKey::PrecompiledModuleId(_) => None,
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
#[derive(Clone, Debug, Default)]
pub struct RuntimeModuleRegistry {
    by_key: HashMap<RuntimeModuleKey, RuntimeModuleState>,
    by_module_id: HashMap<u32, RuntimeModuleKey>,
}

impl RuntimeModuleRegistry {
    pub fn new() -> Self {
        Self::default()
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
        self.remember_module_id(module_id, &key);
        self.by_key
            .insert(key, RuntimeModuleState::Errored { error_value });
    }

    pub fn register_static_namespace(&mut self, module_id: u32, namespace_object: i64) {
        self.finish_loaded(
            RuntimeModuleKey::PrecompiledModuleId(module_id),
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

    fn remember_module_id(&mut self, module_id: Option<u32>, key: &RuntimeModuleKey) {
        if let Some(module_id) = module_id {
            self.by_module_id.insert(module_id, key.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{RuntimeModuleKey, RuntimeModuleRegistry, RuntimeModuleRequireResult};
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
}
