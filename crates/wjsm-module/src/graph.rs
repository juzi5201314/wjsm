// 模块依赖图：依赖图构建、循环检测、拓扑排序

use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use super::resolver::{ModuleResolver, ImportEntry, ExportEntry};
pub use super::resolver::ModuleId;


/// 模块依赖图
pub struct ModuleGraph {
    /// 所有模块节点
    modules: HashMap<ModuleId, GraphNode>,
    /// 入口模块 ID
    entry_id: ModuleId,
    /// 依赖关系：module_id → 它依赖的模块 ID 列表
    dependencies: HashMap<ModuleId, Vec<(ModuleId, ImportEntry)>>,
}

/// 图节点
#[derive(Debug)]
pub struct GraphNode {
    pub id: ModuleId,
    pub path: std::path::PathBuf,
    pub source: String,
    pub ast: swc_core::ecma::ast::Module,
    pub imports: Vec<(ModuleId, ImportEntry)>,
    pub exports: Vec<ExportEntry>,
}

impl ModuleGraph {
    /// 从入口模块构建依赖图
    pub fn build(entry: &str, root: &Path) -> Result<Self> {
        let mut resolver = ModuleResolver::new(root);
        
        // 解析入口模块（构造完整的文件路径作为 parent）
        let entry_path = root.join(entry);
        let entry_id = resolver.resolve(entry, &entry_path)?;

        
        // BFS 遍历所有依赖
        let mut queue = VecDeque::new();
        let mut discovered = HashSet::new();
        queue.push_back(entry_id);
        discovered.insert(entry_id);
        
        while let Some(module_id) = queue.pop_front() {
            let module = resolver.get_module(module_id).unwrap();
            let imports = module.imports.clone();
            let path = module.path.clone();
            
            // 解析所有 import 依赖
            for import in &imports {
                let dep_id = resolver.resolve(&import.specifier, &path)?;
                if discovered.insert(dep_id) {
                    queue.push_back(dep_id);
                }
            }

        }
        
        // 构建图结构
        let mut modules = HashMap::new();
        let mut dependencies = HashMap::new();
        
        for module in resolver.all_modules() {
            let id = module.id;
            let path = module.path.clone();
            let source = module.source.clone();
            let ast = module.ast.clone();
            let exports = module.exports.clone();
            
            // 构建依赖列表
            let mut imports_with_ids = Vec::new();
            for import in &module.imports {
                let dep_path = ModuleResolver::resolve_path(&import.specifier, &module.path)?;
                if let Some(dep_id) = resolver.get_id_by_path(&dep_path) {
                    imports_with_ids.push((dep_id, import.clone()));
                }
            }
            
            dependencies.insert(id, imports_with_ids.clone());
            
            let node = GraphNode {
                id,
                path,
                source,
                ast,
                imports: imports_with_ids,
                exports,
            };
            
            modules.insert(id, node);
        }
        
        let graph = Self {
            modules,
            entry_id,
            dependencies,
        };

        Ok(graph)
    }
    /// 获取拓扑排序后的模块顺序
    pub fn topological_order(&self) -> Result<Vec<ModuleId>> {
        // 使用 DFS 后序遍历生成“依赖优先”的执行顺序。
        // 遇到正在访问中的节点（回边）时直接忽略，允许基础循环依赖场景继续执行。
        let mut visit_state = HashMap::new();
        let mut result = Vec::with_capacity(self.modules.len());

        // 先从入口开始，保证入口可达子图优先。
        self.visit_module(self.entry_id, &mut visit_state, &mut result)?;

        // 处理与入口不连通的模块（理论上 build 后一般不会出现），按路径排序保证稳定性。
        let mut remaining: Vec<ModuleId> = self.modules.keys().copied().collect();
        remaining.sort_by_key(|id| {
            self.modules
                .get(id)
                .map(|node| node.path.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        for id in remaining {
            self.visit_module(id, &mut visit_state, &mut result)?;
        }

        Ok(result)
    }

    fn visit_module(
        &self,
        id: ModuleId,
        visit_state: &mut HashMap<ModuleId, VisitState>,
        result: &mut Vec<ModuleId>,
    ) -> Result<()> {
        match visit_state.get(&id) {
            Some(VisitState::Visited) => return Ok(()),
            Some(VisitState::Visiting) => return Ok(()),
            None => {}
        }

        visit_state.insert(id, VisitState::Visiting);

        if let Some(deps) = self.dependencies.get(&id) {
            for (dep_id, _) in deps {
                self.visit_module(*dep_id, visit_state, result)?;
            }
        }

        visit_state.insert(id, VisitState::Visited);
        result.push(id);
        Ok(())
    }
    
    /// 获取模块节点
    pub fn get_module(&self, id: ModuleId) -> Option<&GraphNode> {
        self.modules.get(&id)
    }
    
    /// 获取所有模块 ID
    pub fn all_module_ids(&self) -> impl Iterator<Item = ModuleId> + '_ {
        self.modules.keys().copied()
    }
    
    /// 获取入口模块 ID
    pub fn entry_id(&self) -> ModuleId {
        self.entry_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn topological_order_is_dependency_first() {
        let root = create_temp_project("order_dependency_first");
        write_file(
            &root,
            "main.js",
            "import { a } from './a.js';\nimport { b } from './b.js';\nconsole.log(a, b);\n",
        );
        write_file(&root, "a.js", "import { c } from './c.js';\nexport const a = c + 1;\n");
        write_file(&root, "b.js", "import { c } from './c.js';\nexport const b = c + 2;\n");
        write_file(&root, "c.js", "export const c = 1;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let order = graph.topological_order().expect("order should be computable");

        let c_path = root.join("c.js").canonicalize().expect("path should exist");
        let a_path = root.join("a.js").canonicalize().expect("path should exist");
        let b_path = root.join("b.js").canonicalize().expect("path should exist");
        let main_path = root.join("main.js").canonicalize().expect("path should exist");

        let c_pos = position_by_path(&graph, &order, &c_path);
        let a_pos = position_by_path(&graph, &order, &a_path);
        let b_pos = position_by_path(&graph, &order, &b_path);
        let main_pos = position_by_path(&graph, &order, &main_path);

        assert!(c_pos < a_pos, "c.js should execute before a.js");
        assert!(c_pos < b_pos, "c.js should execute before b.js");
        assert!(a_pos < main_pos, "a.js should execute before main.js");
        assert!(b_pos < main_pos, "b.js should execute before main.js");
    }

    #[test]
    fn shared_module_is_loaded_once() {
        let root = create_temp_project("cache_once");
        write_file(
            &root,
            "main.js",
            "import { a } from './a.js';\nimport { b } from './b.js';\nconsole.log(a, b);\n",
        );
        write_file(
            &root,
            "a.js",
            "import { shared } from './shared.js';\nexport const a = shared + 1;\n",
        );
        write_file(
            &root,
            "b.js",
            "import { shared } from './shared.js';\nexport const b = shared + 2;\n",
        );
        write_file(&root, "shared.js", "export const shared = 1;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let shared_path = root
            .join("shared.js")
            .canonicalize()
            .expect("path should exist");

        let shared_count = graph
            .all_module_ids()
            .filter(|id| {
                graph.get_module(*id).map(|node| node.path.as_path()) == Some(shared_path.as_path())
            })
            .count();
        assert_eq!(shared_count, 1, "shared.js should only be loaded once");

        let order = graph.topological_order().expect("order should be computable");
        let unique_count = order.iter().copied().collect::<HashSet<_>>().len();
        assert_eq!(order.len(), unique_count, "execution order should not duplicate modules");
    }

    #[test]
    fn basic_cycle_has_predictable_order() {
        let root = create_temp_project("basic_cycle");
        write_file(
            &root,
            "main.js",
            "import { a } from './a.js';\nconsole.log(a);\n",
        );
        write_file(&root, "a.js", "import { b } from './b.js';\nexport const a = b + 1;\n");
        write_file(&root, "b.js", "import { a } from './a.js';\nexport const b = 1;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let order = graph.topological_order().expect("cycle should still be orderable");

        let names: Vec<String> = order
            .iter()
            .map(|id| {
                graph.get_module(*id)
                    .and_then(|node| node.path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .expect("module file name should exist")
            })
            .collect();

        assert_eq!(names, vec!["b.js", "a.js", "main.js"]);
    }

    fn position_by_path(graph: &ModuleGraph, order: &[ModuleId], path: &Path) -> usize {
        order
            .iter()
            .position(|id| {
                graph.get_module(*id).map(|node| node.path.as_path()) == Some(path)
            })
            .expect("module should appear in order")
    }

    fn create_temp_project(case: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "wjsm_module_graph_{case}_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("temp project dir should be creatable");
        dir
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir should be created");
        }
        std::fs::write(path, content).expect("fixture file should be writable");
    }
}
