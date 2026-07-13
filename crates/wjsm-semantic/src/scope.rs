// 作用域树和作用域解析

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Block,
    Function,
    Module,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarKind {
    Var,
    Let,
    Const,
}

/// 控制预扫描时是否包含 let/const 声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexicalMode {
    /// 包含 let/const 声明（顶层作用域预扫描）。
    Include,
    /// 排除 let/const 声明（块级作用域嵌套扫描）。
    Exclude,
}

#[derive(Debug, Clone)]
pub(crate) struct VarInfo {
    pub(crate) kind: VarKind,
    /// `false` = in TDZ (declared lexically but not yet initialised).
    /// `true`  = initialised and ready for use.
    pub(crate) initialised: bool,
    /// `true` only for the implicit `arguments` binding created by emit_arguments_init.
    /// Used to distinguish implicit `arguments` from explicit `var`/`let`/`const arguments`.
    pub(crate) implicit_arguments: bool,
}

pub(crate) struct Scope {
    pub(crate) parent: Option<usize>,
    pub(crate) kind: ScopeKind,
    pub(crate) id: usize,
    pub(crate) variables: std::collections::HashMap<String, VarInfo>,
}

pub(crate) struct ScopeTree {
    pub(crate) arenas: Vec<Scope>,
    pub(crate) current: usize,
}

impl ScopeTree {
    pub(crate) fn new() -> Self {
        let root = Scope {
            parent: None,
            kind: ScopeKind::Function,
            id: 0,
            variables: std::collections::HashMap::new(),
        };
        let arenas = vec![root];
        Self { arenas, current: 0 }
    }

    /// Push a new child scope and enter it.
    pub(crate) fn push_scope(&mut self, kind: ScopeKind) {
        let idx = self.arenas.len();
        let scope = Scope {
            parent: Some(self.current),
            kind,
            id: idx,
            variables: std::collections::HashMap::new(),
        };
        self.arenas.push(scope);
        self.current = idx;
    }

    /// 获取当前 scope 的 id。
    pub(crate) fn current_scope_id(&self) -> usize {
        self.current
    }

    /// 重新进入一个已存在的作用域（用于多模块降级在 predeclare 与 lower 阶段间
    /// 重新激活某模块的顶层作用域，避免 push_scope 重复分配新作用域）。
    pub(crate) fn enter_scope(&mut self, id: usize) {
        debug_assert!(id < self.arenas.len(), "enter_scope: scope id 越界");
        self.current = id;
    }

    /// 返回指定 scope 所属的最近函数 scope。
    pub(crate) fn function_scope_for_scope(&self, mut scope_id: usize) -> usize {
        loop {
            let scope = &self.arenas[scope_id];
            if matches!(scope.kind, ScopeKind::Function) {
                return scope_id;
            }
            scope_id = scope
                .parent
                .expect("non-root scope must have a parent function scope");
        }
    }

    /// Pop the current scope, returning to its parent.
    pub(crate) fn pop_scope(&mut self) {
        self.current = self.arenas[self.current]
            .parent
            .expect("cannot pop root scope");
    }

    /// Declare a variable in the appropriate scope.
    ///
    /// - `let` / `const` → current (innermost) scope.
    /// - `var`          → nearest enclosing *function* scope.
    ///
    /// Returns `Err(message)` on redeclaration conflict (let/const in same scope).
    pub(crate) fn declare(
        &mut self,
        name: &str,
        kind: VarKind,
        initialised: bool,
    ) -> Result<usize, String> {
        let target_idx = match kind {
            VarKind::Var => self.nearest_var_scope()?,
            VarKind::Let | VarKind::Const => self.current,
        };

        let scope = &mut self.arenas[target_idx];

        // var redeclaration in the same scope is allowed (JS semantics).
        // let / const redeclaration in the same scope is an error.
        if let Some(existing) = scope.variables.get(name) {
            match (existing.kind, kind) {
                (VarKind::Var, VarKind::Var) => return Ok(scope.id),
                _ => {
                    return Err(format!(
                        "cannot redeclare identifier `{name}` in the same scope"
                    ));
                }
            }
        }

        scope.variables.insert(
            name.to_string(),
            VarInfo {
                kind,
                initialised,
                implicit_arguments: false,
            },
        );
        Ok(scope.id)
    }

    /// Mark a variable as initialised (exit TDZ).
    pub(crate) fn mark_initialised(&mut self, name: &str) -> Result<(), String> {
        let mut cursor = self.current;
        loop {
            let scope = &mut self.arenas[cursor];
            if let Some(info) = scope.variables.get_mut(name) {
                info.initialised = true;
                return Ok(());
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// Look up a variable by name. Returns `(scope_id, VarKind)` if found.
    pub(crate) fn lookup(&self, name: &str) -> Result<(usize, VarKind), String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                if !info.initialised {
                    return Err(format!("cannot access `{name}` before initialisation"));
                }
                return Ok((scope.id, info.kind));
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// Return all lexically visible bindings, including uninitialized (TDZ) ones.
    /// Returns (scope_id, name, kind, is_initialised).
    pub(crate) fn visible_bindings_all(&self) -> Vec<(usize, String, VarKind, bool)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &self.arenas[scope_id];
            let mut names: Vec<_> = scope.variables.keys().cloned().collect();
            names.sort();
            for name in names {
                if seen.insert(name.clone())
                    && let Some(info) = scope.variables.get(&name)
                {
                    result.push((scope.id, name.clone(), info.kind, info.initialised));
                }
            }
            cursor = scope.parent;
        }
        result
    }

    /// Resolve a variable's scope id without checking TDZ.
    pub(crate) fn resolve_scope_id(&self, name: &str) -> Result<usize, String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if scope.variables.contains_key(name) {
                return Ok(scope.id);
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// 赋值时查找变量：组合 `check_mutable` 和 `lookup`，
    /// 一次 scope chain 遍历完成 const 检查 + TDZ 检查。
    ///
    /// # 性能优化
    /// `lower_assign` 原本先调 `check_mutable` 再调 `lookup`，
    /// 分别遍历 scope chain 各一次。合并为一次遍历减少冗余的 HashMap 查找，
    /// 在深层嵌套作用域中有最多约 50% 的查找节省。
    pub(crate) fn lookup_for_assign(&self, name: &str) -> Result<(usize, VarKind), String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                if matches!(info.kind, VarKind::Const) {
                    return Err(format!(
                        "cannot reassign a const-declared variable `{name}`"
                    ));
                }
                if !info.initialised {
                    return Err(format!("cannot access `{name}` before initialisation"));
                }
                return Ok((scope.id, info.kind));
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }
    /// Check that this variable is not `const` before reassignment.
    /// 注意：`lower_assign` 现在使用 `lookup_for_assign` 在一次遍历中同时完成
    /// const 检查和 scope 解析，此方法保留以供未来使用。
    #[allow(dead_code)]
    pub(crate) fn check_mutable(&self, name: &str) -> Result<(), String> {
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                if matches!(info.kind, VarKind::Const) {
                    return Err(format!(
                        "cannot reassign a const-declared variable `{name}`"
                    ));
                }
                return Ok(());
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return Err(format!("undeclared identifier `{name}`")),
            }
        }
    }

    /// 返回最近的 var 声明作用域。模块顶层与函数体都拥有独立 var 环境。
    fn nearest_var_scope(&self) -> Result<usize, String> {
        let mut cursor = self.current;
        loop {
            if matches!(
                self.arenas[cursor].kind,
                ScopeKind::Function | ScopeKind::Module
            ) {
                return Ok(cursor);
            }
            cursor = self.arenas[cursor]
                .parent
                .ok_or_else(|| "root must be a var scope".to_string())?;
        }
    }

    pub(crate) fn nearest_function_scope(&self) -> Result<usize, String> {
        let mut cursor = self.current;
        loop {
            if matches!(self.arenas[cursor].kind, ScopeKind::Function) {
                return Ok(cursor);
            }
            cursor = self.arenas[cursor]
                .parent
                .ok_or_else(|| "root must be function scope".to_string())?;
        }
    }

    /// True when the current function scope already has a binding named `arguments` (e.g. parameter).
    pub(crate) fn current_function_has_param_arguments(&self) -> bool {
        let Ok(scope_id) = self.nearest_function_scope() else {
            return false;
        };
        self.arenas[scope_id].variables.contains_key("arguments")
    }

    /// Mark an existing variable as implicit `arguments`.
    pub(crate) fn set_implicit_arguments(&mut self, name: &str) -> Result<(), String> {
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &mut self.arenas[scope_id];
            if let Some(info) = scope.variables.get_mut(name) {
                info.implicit_arguments = true;
                return Ok(());
            }
            cursor = scope.parent;
        }
        Err(format!("undeclared identifier `{name}`"))
    }
}
