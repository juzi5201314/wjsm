// 作用域树和作用域解析

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Block,
    Function,
    Module,
    /// with 语句的对象环境记录：解析穿越该作用域的标识符须先按对象属性
    /// 动态分派（见 `lowerer_with`）。variables 仅存放持有 with 对象的
    /// 合成绑定，不会含用户标识符。
    With,
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
    /// `true` 仅用于具名函数表达式自身名字绑定（§15.2.5 步骤 4
    /// CreateImmutableBinding(name, false)）：写入按运行时语义分流——
    /// 非严格代码静默忽略、严格代码 TypeError，而非 const 的编译期拒绝。
    pub(crate) fn_expr_name: bool,
    /// `true` 仅用于类自身名字绑定（ClassDefinitionEvaluation 步骤 3
    /// CreateImmutableBinding(classBinding, true)）：写入按运行时语义分流——
    /// TDZ 内 ReferenceError、初始化后 TypeError，而非 const 的编译期拒绝。
    pub(crate) class_self_name: bool,
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

    /// 以 `count` 个作用域为基址创建作用域树（builtin 段 hydration 用）。
    ///
    /// id 0 是真实根作用域；id 1..count 是占位作用域（parent=0、空变量表），
    /// 仅用于保持作用域 id 连续性——builtin 段 lower 时的作用域 id 必须与合并后
    /// 用户 lowerer 的作用域 id 一致（IR 变量名 `${scope_id}.{name}` 是跨函数协议）。
    /// 之后 `push_scope` 从 `count` 开始分配新作用域。
    pub(crate) fn with_base_scope_count(count: usize) -> Self {
        let mut tree = Self::new();
        for _ in 1..count {
            tree.arenas.push(Scope {
                parent: Some(0),
                kind: ScopeKind::Module,
                id: tree.arenas.len(),
                variables: std::collections::HashMap::new(),
            });
        }
        tree
    }

    /// 当前作用域总数（含 root；builtin 段缓存用，作为用户 lowerer 的 scope 基址）。
    pub(crate) fn scope_count(&self) -> usize {
        self.arenas.len()
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
                fn_expr_name: false,
                class_self_name: false,
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

    /// 按 scope id 精确设置绑定初始化状态（退出/恢复 TDZ）。
    /// 用于类名这类「延迟执行的函数体可见、立即执行的类求值代码仍为 TDZ」的绑定；
    /// 必须按 scope_id 定位，避免嵌套同名绑定遮蔽时误改。
    pub(crate) fn set_initialised(
        &mut self,
        scope_id: usize,
        name: &str,
        initialised: bool,
    ) -> Result<(), String> {
        let info = self
            .arenas
            .get_mut(scope_id)
            .and_then(|scope| scope.variables.get_mut(name))
            .ok_or_else(|| format!("undeclared identifier `{name}`"))?;
        info.initialised = initialised;
        Ok(())
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
    /// with 作用域的合成对象绑定不属于用户可见词法绑定，跳过（eval 桥接经
    /// `ScopeRecordAddWithLayer` 单独传递 with 链）。
    pub(crate) fn visible_bindings_all(&self) -> Vec<(usize, String, VarKind, bool)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &self.arenas[scope_id];
            if !matches!(scope.kind, ScopeKind::With) {
                let mut names: Vec<_> = scope.variables.keys().cloned().collect();
                names.sort();
                for name in names {
                    if seen.insert(name.clone())
                        && let Some(info) = scope.variables.get(&name)
                    {
                        result.push((scope.id, name.clone(), info.kind, info.initialised));
                    }
                }
            }
            cursor = scope.parent;
        }
        result
    }

    /// 解析标识符时统计穿越的 with 作用域。
    ///
    /// 返回 `(命中绑定的 (scope_id, kind)（未命中为 None）, 命中前穿越的
    /// With 作用域 id 列表（由内到外）)`。命中绑定处于 TDZ 时仍视为命中——
    /// with 分派只关心词法遮蔽结构，TDZ 诊断由静态回退路径自行处理。
    pub(crate) fn with_scopes_crossed(&self, name: &str) -> (Option<(usize, VarKind)>, Vec<usize>) {
        let mut crossed = Vec::new();
        let mut cursor = self.current;
        loop {
            let scope = &self.arenas[cursor];
            if let Some(info) = scope.variables.get(name) {
                return (Some((scope.id, info.kind)), crossed);
            }
            if matches!(scope.kind, ScopeKind::With) {
                crossed.push(scope.id);
            }
            match scope.parent {
                Some(parent) => cursor = parent,
                None => return (None, crossed),
            }
        }
    }

    /// 判断 `ancestor` 是否为 `descendant` 的祖先（不含自身相等）。
    /// 供 direct eval 的 with 层 inner_names 计算判定「绑定声明于该 with
    /// 层内侧」。
    pub(crate) fn is_strict_ancestor(&self, ancestor: usize, descendant: usize) -> bool {
        let mut cursor = self.arenas[descendant].parent;
        while let Some(id) = cursor {
            if id == ancestor {
                return true;
            }
            cursor = self.arenas[id].parent;
        }
        false
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

    /// 赋值时查找变量：组合 const 检查与作用域解析，
    /// 一次 scope chain 遍历完成 const 检查 + TDZ 检查。
    ///
    /// # 性能优化
    /// `lower_assign` 原本先做独立 const 检查再查找绑定，
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

    /// 将 `(scope_id, name)` 处的绑定标记为具名函数表达式自身名字绑定。
    pub(crate) fn set_fn_expr_name(&mut self, scope_id: usize, name: &str) -> Result<(), String> {
        let info = self
            .arenas
            .get_mut(scope_id)
            .and_then(|scope| scope.variables.get_mut(name))
            .ok_or_else(|| format!("undeclared identifier `{name}`"))?;
        info.fn_expr_name = true;
        Ok(())
    }

    /// `(scope_id, name)` 处的绑定是否为具名函数表达式自身名字绑定。
    pub(crate) fn is_fn_expr_name(&self, scope_id: usize, name: &str) -> bool {
        self.arenas
            .get(scope_id)
            .and_then(|scope| scope.variables.get(name))
            .is_some_and(|info| info.fn_expr_name)
    }

    /// 将 `(scope_id, name)` 处的绑定标记为类自身名字绑定（classEnv 的
    /// CreateImmutableBinding(classBinding, true)）。
    pub(crate) fn set_class_self_name(
        &mut self,
        scope_id: usize,
        name: &str,
    ) -> Result<(), String> {
        let info = self
            .arenas
            .get_mut(scope_id)
            .and_then(|scope| scope.variables.get_mut(name))
            .ok_or_else(|| format!("undeclared identifier `{name}`"))?;
        info.class_self_name = true;
        Ok(())
    }

    /// `(scope_id, name)` 处的绑定是否为类自身名字绑定。
    pub(crate) fn is_class_self_name(&self, scope_id: usize, name: &str) -> bool {
        self.arenas
            .get(scope_id)
            .and_then(|scope| scope.variables.get(name))
            .is_some_and(|info| info.class_self_name)
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
