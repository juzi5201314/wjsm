//! 宿主侧表的槽位容器：稳定 u32 句柄 + 死槽回收复用。
//!
//! fetch / streams 等 Web 宿主侧表以下标充当句柄嵌入虚拟属性与可调用值，
//! 句柄必须在 owner 存活期间稳定；owner 死亡后槽位应被 GC sweep 释放并
//! 复用，保证长跑进程侧表长度有界。

use std::ops::{Index, IndexMut};

pub(crate) struct SlotTable<T> {
    slots: Vec<Option<T>>,
    free: Vec<u32>,
}

impl<T> Default for SlotTable<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> SlotTable<T> {
    /// 下一次 [`SlotTable::insert`] 将占用的句柄。仅在 peek 与 insert 之间
    /// 不发生其他插入/删除时可作为交叉引用的预分配句柄。
    pub(crate) fn peek_handle(&self) -> Option<u32> {
        match self.free.last() {
            Some(handle) => Some(*handle),
            None => u32::try_from(self.slots.len()).ok(),
        }
    }

    /// 插入并返回稳定句柄；优先复用空闲槽。句柄空间耗尽时返回 None。
    pub(crate) fn insert(&mut self, value: T) -> Option<u32> {
        if let Some(handle) = self.free.pop() {
            self.slots[handle as usize] = Some(value);
            return Some(handle);
        }
        let handle = u32::try_from(self.slots.len()).ok()?;
        self.slots.push(Some(value));
        Some(handle)
    }

    pub(crate) fn get(&self, handle: u32) -> Option<&T> {
        self.slots.get(handle as usize)?.as_ref()
    }

    pub(crate) fn get_mut(&mut self, handle: u32) -> Option<&mut T> {
        self.slots.get_mut(handle as usize)?.as_mut()
    }

    /// 释放槽位并将句柄放回空闲队列；死槽/越界返回 None。
    pub(crate) fn remove(&mut self, handle: u32) -> Option<T> {
        let value = self.slots.get_mut(handle as usize)?.take()?;
        self.free.push(handle);
        Some(value)
    }

    /// 遍历活槽，产出（句柄，值）。
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| Some((index as u32, slot.as_ref()?)))
    }

    /// 活槽数量（不含空闲槽）。
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }
}

impl<T> Index<u32> for SlotTable<T> {
    type Output = T;

    fn index(&self, handle: u32) -> &T {
        self.slots[handle as usize]
            .as_ref()
            .expect("slot handle must reference a live entry")
    }
}

impl<T> IndexMut<u32> for SlotTable<T> {
    fn index_mut(&mut self, handle: u32) -> &mut T {
        self.slots[handle as usize]
            .as_mut()
            .expect("slot handle must reference a live entry")
    }
}
