use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// `HostSideTable` 的内部存储：`entries` 用 `Option<T>` 表示 slot 占用/空洞，
/// `free_list` 记录可复用的空洞 handle，保证 reclaim 后的 slot 可复用。
pub(crate) struct HostSideTableInner<T> {
    pub(crate) entries: Vec<Option<T>>,
    free_list: Vec<u32>,
}

impl<T> HostSideTableInner<T> {
    pub(crate) fn get(&self, handle: usize) -> Option<&T> {
        self.entries.get(handle).and_then(|entry| entry.as_ref())
    }

    pub(crate) fn get_mut(&mut self, handle: usize) -> Option<&mut T> {
        self.entries
            .get_mut(handle)
            .and_then(|entry| entry.as_mut())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().filter_map(|entry| entry.as_ref())
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.iter_mut().filter_map(|entry| entry.as_mut())
    }
}

/// 与 GC 联动的 host 侧表。
///
/// JS wrapper 对象通过隐藏属性保存 side-table handle。GC sweep 之后，本表用
/// `obj_to_side` 记录的 wrapper→entry 关系作为 side-entry 可达性种子，再由
/// `runtime_gc::weak_refs` 中的流侧表图传播决定哪些 slot 可回收。
pub(crate) struct HostSideTable<T> {
    /// entries 和 free_list 共用同一个锁保证 alloc/reclaim 原子性。
    /// callers 通过 `.inner.lock()` 获取对 entries 的 &/&mut 访问。
    pub(crate) inner: Mutex<HostSideTableInner<T>>,
    /// obj_table handle → 本侧表的 handle。一个 entry 可以被多个 JS wrapper 指向，
    /// 因此只保存正向映射；sweep 后删除已释放 wrapper，再以剩余 values 作为种子。
    obj_to_side: Mutex<HashMap<u32, u32>>,
    /// 构造期或 Rust 侧异步持有的 side handles。pin 只保护 side-entry，真正 JS 值
    /// 仍需由各 entry 字段经 roots.rs 扫描。
    pinned: Mutex<HashSet<u32>>,
}

impl<T> HostSideTable<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HostSideTableInner {
                entries: Vec::new(),
                free_list: Vec::new(),
            }),
            obj_to_side: Mutex::new(HashMap::new()),
            pinned: Mutex::new(HashSet::new()),
        }
    }

    /// 分配新条目并默认 pin 住。调用方在 entry 被 JS wrapper 绑定或接入另一条
    /// 已可达 side-entry 边之后，必须调用 `unpin`。
    pub fn alloc(&self, entry: T) -> u32 {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let handle = if let Some(h) = inner.free_list.pop() {
            h
        } else {
            let h = inner.entries.len() as u32;
            inner.entries.push(None);
            h
        };
        inner.entries[handle as usize] = Some(entry);
        drop(inner);
        self.pin(handle);
        handle
    }

    /// 将一个 JS wrapper 对象绑定到 side-table entry。`obj_handle` 必须是 obj_table
    /// 下标，不是 heap pointer。
    pub fn bind_obj_handle(&self, obj_handle: u32, side_handle: u32) {
        self.obj_to_side
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(obj_handle, side_handle);
        self.unpin(side_handle);
    }

    /// 通过精确 wrapper object handle 查询 side-table handle；不沿原型链读取属性。
    pub fn side_handle_for_obj(&self, obj_handle: u32) -> Option<u32> {
        self.obj_to_side
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&obj_handle)
            .copied()
    }

    pub fn object_bindings(&self) -> Vec<(u32, u32)> {
        self.obj_to_side
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(&object, &side)| (object, side))
            .collect()
    }

    pub fn pin(&self, side_handle: u32) {
        self.pinned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(side_handle);
    }

    pub fn unpin(&self, side_handle: u32) {
        self.pinned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&side_handle);
    }

    /// sweep 后删除已释放 wrapper，并返回仍由活 wrapper 或 pin 直接保持的 side handles。
    pub fn direct_roots_after_pruning(&self, freed_objs: &HashSet<u32>) -> HashSet<u32> {
        let mut roots = self
            .pinned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut obj_to_side = self.obj_to_side.lock().unwrap_or_else(|e| e.into_inner());
        obj_to_side.retain(|obj_handle, _| !freed_objs.contains(obj_handle));
        roots.extend(obj_to_side.values().copied());
        roots
    }

    /// 按 side-entry reachability 回收不可达 slot。调用方负责先完成表间传播。
    pub fn reclaim_unreachable(&self, reachable: &HashSet<u32>) {
        drop(self.reclaim_unreachable_entries(reachable));
    }

    /// 与 `reclaim_unreachable` 相同，但把被回收的 entry 交给 owner 做资源注销。
    pub fn reclaim_unreachable_entries(&self, reachable: &HashSet<u32>) -> Vec<(u32, T)> {
        let pinned = self
            .pinned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut reclaimed = Vec::new();
        for (idx, entry) in inner.entries.iter_mut().enumerate() {
            let handle = idx as u32;
            if !reachable.contains(&handle)
                && !pinned.contains(&handle)
                && let Some(entry) = entry.take()
            {
                reclaimed.push((handle, entry));
            }
        }
        inner
            .free_list
            .extend(reclaimed.iter().map(|(handle, _)| *handle));
        drop(inner);

        if !reclaimed.is_empty() {
            let reclaimed_handles: HashSet<u32> =
                reclaimed.iter().map(|(handle, _)| *handle).collect();
            self.obj_to_side
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, side_handle| !reclaimed_handles.contains(side_handle));
            self.pinned
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|side_handle| !reclaimed_handles.contains(side_handle));
        }
        reclaimed
    }

    /// 活跃条目数（不含 tombstone）。
    pub fn active_count(&self) -> usize {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.iter().filter(|e| e.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.active_count() == 0
    }
}

impl<T> Default for HostSideTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_get() {
        let table = HostSideTable::<String>::new();
        let h0 = table.alloc("a".into());
        let h1 = table.alloc("b".into());
        let inner = table.inner.lock().unwrap();
        assert_eq!(inner.entries[h0 as usize].as_deref(), Some("a"));
        assert_eq!(inner.entries[h1 as usize].as_deref(), Some("b"));
    }

    #[test]
    fn reclaim_unreachable_reuses_slot() {
        let table = HostSideTable::<String>::new();
        let h0 = table.alloc("x".into());
        table.unpin(h0);
        table.reclaim_unreachable(&HashSet::new());
        {
            let inner = table.inner.lock().unwrap();
            assert!(inner.entries[h0 as usize].is_none());
        }
        let h1 = table.alloc("y".into());
        assert_eq!(h0, h1);
    }

    #[test]
    fn pinned_entry_survives_reclaim() {
        let table = HostSideTable::<String>::new();
        let h = table.alloc("x".into());
        table.reclaim_unreachable(&HashSet::new());
        assert_eq!(table.active_count(), 1);
        table.unpin(h);
        table.reclaim_unreachable(&HashSet::new());
        assert!(table.is_empty());
    }

    #[test]
    fn live_wrapper_seed_survives_freed_wrapper_prunes() {
        let table = HostSideTable::<String>::new();
        let h0 = table.alloc("a".into());
        let h1 = table.alloc("b".into());
        table.bind_obj_handle(100, h0);
        table.bind_obj_handle(200, h1);
        let roots = table.direct_roots_after_pruning(&HashSet::from([100]));
        assert!(!roots.contains(&h0));
        assert!(roots.contains(&h1));
        table.reclaim_unreachable(&roots);
        let inner = table.inner.lock().unwrap();
        assert!(inner.entries[h0 as usize].is_none());
        assert_eq!(inner.entries[h1 as usize].as_deref(), Some("b"));
    }
}
