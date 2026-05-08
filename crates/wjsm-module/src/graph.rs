// 模块依赖图：依赖图构建、循环检测、拓扑排序

use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use super::resolver::{ModuleResolver, ResolvedModule, ImportEntry, ExportEntry};
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
        queue.push_back(entry_id);
        
        while let Some(module_id) = queue.pop_front() {
            let module = resolver.get_module(module_id).unwrap();
            let imports = module.imports.clone();
            let path = module.path.clone();
            
            // 解析所有 import 依赖
            for import in &imports {
                let dep_id = resolver.resolve(&import.specifier, &path)?;
                queue.push_back(dep_id);
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
        

        // 检测循环依赖
        let graph = Self {
            modules,
            entry_id,
            dependencies,
        };
        
        graph.detect_cycles()?;
        
        Ok(graph)
    }
    /// 获取拓扑排序后的模块顺序
    pub fn topological_order(&self) -> Result<Vec<ModuleId>> {
        // Kahn's algorithm
        let mut in_degree: HashMap<ModuleId, usize> = HashMap::new();
        let mut reverse_edges: HashMap<ModuleId, Vec<ModuleId>> = HashMap::new();
        
        // 初始化入度
        for &id in self.modules.keys() {
            in_degree.insert(id, 0);
            reverse_edges.insert(id, Vec::new());
        }
        
        // 计算入度和反向边
        for (&from, deps) in &self.dependencies {
            for (to, _) in deps {
                *in_degree.get_mut(&from).unwrap() += 1;
                reverse_edges.get_mut(to).unwrap().push(from);
            }
        }
        
        // 找到所有入度为 0 的节点
        let mut queue: VecDeque<ModuleId> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| *id)
            .collect();
        
        let mut result = Vec::new();
        
        while let Some(id) = queue.pop_front() {
            result.push(id);
            
            // 减少依赖此模块的其他模块的入度
            for &dep_id in reverse_edges.get(&id).unwrap() {
                let degree = in_degree.get_mut(&dep_id).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(dep_id);
                }
            }
        }
        
        if result.len() != self.modules.len() {
            bail!("Cycle detected in module graph");
        }
        
        Ok(result)
    }
    
    /// 检测循环依赖
    fn detect_cycles(&self) -> Result<()> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        
        for &id in self.modules.keys() {
            if !visited.contains(&id) {
                self.detect_cycles_dfs(id, &mut visited, &mut rec_stack)?;
            }
        }
        
        Ok(())
    }
    
    fn detect_cycles_dfs(
        &self,
        id: ModuleId,
        visited: &mut HashSet<ModuleId>,
        rec_stack: &mut HashSet<ModuleId>,
    ) -> Result<()> {
        visited.insert(id);
        rec_stack.insert(id);
        
        if let Some(deps) = self.dependencies.get(&id) {
            for (dep_id, _) in deps {
                if !visited.contains(dep_id) {
                    self.detect_cycles_dfs(*dep_id, visited, rec_stack)?;
                } else if rec_stack.contains(dep_id) {
                    // 找到循环依赖
                    let from = self.modules.get(&id).unwrap();
                    let to = self.modules.get(dep_id).unwrap();
                    bail!(
                        "Circular dependency detected: {} -> {}",
                        from.path.display(),
                        to.path.display()
                    );
                }
            }
        }
        
        rec_stack.remove(&id);
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
