//! AsyncLocalStorage 的 COW context frame（对齐 Node AsyncContextFrame）。

use std::collections::HashMap;
use std::sync::Arc;

/// ALS 实例键（host 侧分配的稳定 id）。
pub type AlsKey = u64;

/// Frame 标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub u64);

/// 不可变 frame：子 frame 共享 parent 映射，仅在写入时 clone。
#[derive(Debug, Clone, Default)]
pub(crate) struct ContextFrame {
    /// key → store value（NaN-box i64）
    map: HashMap<AlsKey, i64>,
}

impl ContextFrame {
    pub(crate) fn empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub(crate) fn get(&self, key: AlsKey) -> Option<i64> {
        self.map.get(&key).copied()
    }

    pub(crate) fn has(&self, key: AlsKey) -> bool {
        self.map.contains_key(&key)
    }

    /// 基于 self 派生新 frame，写入 key。
    pub(crate) fn child_with(&self, key: AlsKey, value: i64) -> Self {
        let mut map = self.map.clone();
        map.insert(key, value);
        Self { map }
    }

    /// 基于 self 派生新 frame，删除 key（disable）。
    pub(crate) fn child_without(&self, key: AlsKey) -> Self {
        let mut map = self.map.clone();
        map.remove(&key);
        Self { map }
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = i64> + '_ {
        self.map.values().copied()
    }
}

/// Frame 表：id → 共享 frame。
#[derive(Debug, Default)]
pub(crate) struct FrameTable {
    next_id: u64,
    frames: HashMap<FrameId, Arc<ContextFrame>>,
}

impl FrameTable {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            frames: HashMap::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub(crate) fn alloc(&mut self, frame: ContextFrame) -> FrameId {
        let id = FrameId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.frames.insert(id, Arc::new(frame));
        id
    }

    pub(crate) fn get(&self, id: FrameId) -> Option<Arc<ContextFrame>> {
        self.frames.get(&id).cloned()
    }

    pub(crate) fn get_ref(&self, id: FrameId) -> Option<&ContextFrame> {
        self.frames.get(&id).map(|a| a.as_ref())
    }

    pub(crate) fn remove(&mut self, id: FrameId) {
        self.frames.remove(&id);
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = i64> + '_ {
        self.frames.values().flat_map(|frame| frame.values())
    }
}
