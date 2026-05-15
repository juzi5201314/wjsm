#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Block,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexicalMode {
    Include,
    Exclude,
}

#[derive(Debug, Clone)]
pub(crate) struct VarInfo {
    pub(crate) kind: VarKind,
    pub(crate) initialised: bool,
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
        let mut arenas = Vec::new();
        arenas.push(root);
        Self { arenas, current: 0 }
    }

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

    pub(crate) fn current_scope_id(&self) -> usize {
        self.current
    }

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

    pub(crate) fn pop_scope(&mut self) {
        self.current = self.arenas[self.current]
            .parent
            .expect("cannot pop root scope");
    }

    pub(crate) fn declare(&mut self, name: &str, kind: VarKind, initialised: bool) -> Result<usize, String> {
        let target_idx = match kind {
            VarKind::Var => self.nearest_function_scope()?,
            VarKind::Let | VarKind::Const => self.current,
        };

        let scope = &mut self.arenas[target_idx];

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

        scope
            .variables
            .insert(name.to_string(), VarInfo { kind, initialised });
        Ok(scope.id)
    }

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

    pub(crate) fn visible_bindings(&self) -> Vec<(usize, String, VarKind)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(self.current);
        while let Some(scope_id) = cursor {
            let scope = &self.arenas[scope_id];
            let mut names: Vec<_> = scope.variables.keys().cloned().collect();
            names.sort();
            for name in names {
                if seen.insert(name.clone()) {
                    if let Some(info) = scope.variables.get(&name) {
                        if info.initialised {
                            result.push((scope.id, name, info.kind));
                        }
                    }
                }
            }
            cursor = scope.parent;
        }
        result
    }

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
}
