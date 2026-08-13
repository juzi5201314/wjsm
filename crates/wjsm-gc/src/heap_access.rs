//! memory64 V2 动态 JS 堆的唯一 host 访问入口。

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::heap::{
    GrowableHeapMemory, HandleGeneration, HandleId, HandleTableError, HandleTableV2, HeapAddress,
    HeapMemoryError, ManagedHeapLayout, RestoredHandleEntry,
};
use crate::shape::{PROTO_NULL_SENTINEL, ShapeProp, ShapeTable, ShapeTableSnapshot};
use wjsm_ir::{constants, value};

/// 相邻两次惰性合并之间允许新增的空闲区间数量。
/// GC 清扫逐对象释放时，每次 `release_region` 只 push，累计达到该值才排序归并一次；
/// 单次排序成本 O((baseline + BATCH) log)，清扫总成本 O(n log BATCH) 而非 O(n² log n)。
const FREE_REGION_MERGE_BATCH: usize = 1024;

/// V2 dynamic heap 的唯一 host access owner；所有地址均为 memory64 byte offset。
pub struct HeapAccessV2<M: GrowableHeapMemory> {
    memory: M,
    layout: Arc<ManagedHeapLayout>,
    handles: Arc<HandleTableV2>,
    next_object: AtomicU64,
    free_regions: Mutex<Vec<(u64, u64)>>,
    /// 上次惰性合并时的空闲区数量（排序基线）。`free_regions` 长度超过
    /// `baseline + FREE_REGION_MERGE_BATCH` 时才重新排序合并，把 GC 清扫批量
    /// 释放的每次 `release_region` 摊还为 O(1)。
    merged_free_region_count: AtomicU64,
    /// 属性元数据（name_id / flags / 值槽下标）的唯一 owner；堆内只留紧凑值数组。
    shapes: ShapeTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeapAccessV2Property {
    pub flags: u32,
    pub value: u64,
    pub getter: u64,
    pub setter: u64,
}

/// Collector 对唯一 heap owner 的窄借用；不拥有 memory、layout 或 handle table。
pub struct CollectorHeapCapability<'a, M: GrowableHeapMemory> {
    heap: &'a HeapAccessV2<M>,
}

impl<M: GrowableHeapMemory> HeapAccessV2<M> {
    /// 构造唯一 heap access owner；memory 与 handle table 必须共享完全相同的 layout。
    pub fn with_handles(
        memory: M,
        layout: Arc<ManagedHeapLayout>,
        handles: Arc<HandleTableV2>,
    ) -> Result<Self, HeapAccessV2Error> {
        if handles.layout() != layout.as_ref()
            || memory.logical_base() != layout.object_heap_base()
            || memory.maximum_byte_len() != layout.object_heap_end()
        {
            return Err(HeapAccessV2Error::LayoutMismatch {
                memory_base: memory.logical_base(),
                memory_end: memory.maximum_byte_len(),
                object_heap_base: layout.object_heap_base(),
                object_heap_end: layout.object_heap_end(),
            });
        }
        let next_object = layout.object_heap_base();
        Ok(Self {
            memory,
            layout,
            handles,
            next_object: AtomicU64::new(next_object),
            free_regions: Mutex::new(Vec::new()),
            merged_free_region_count: AtomicU64::new(0),
            shapes: ShapeTable::new(),
        })
    }

    pub fn collector_capability(&self) -> CollectorHeapCapability<'_, M> {
        CollectorHeapCapability { heap: self }
    }

    pub fn reserve_nlab(&self, minimum_bytes: u64) -> Result<(u64, u64), HeapAccessV2Error> {
        let minimum_bytes = align_reservation(minimum_bytes)?;
        self.reserve_region(minimum_bytes, minimum_bytes.max(64 * 1024))
    }

    fn reserve_exact(&self, bytes: u64) -> Result<u64, HeapAccessV2Error> {
        let bytes = align_reservation(bytes)?;
        self.reserve_region(bytes, bytes).map(|(start, _)| start)
    }

    fn reserve_region(
        &self,
        minimum_bytes: u64,
        preferred_bytes: u64,
    ) -> Result<(u64, u64), HeapAccessV2Error> {
        if let Some(region) = self.take_free_region(minimum_bytes) {
            return Ok(region);
        }
        loop {
            let start = self.next_object.load(Ordering::Acquire);
            let remaining = self.layout.object_heap_end().saturating_sub(start);
            if minimum_bytes > remaining {
                return Err(HeapAccessV2Error::HeapExhausted {
                    requested: minimum_bytes,
                    heap_limit: self.layout.object_heap_end(),
                });
            }
            let reservation = preferred_bytes.min(remaining).max(minimum_bytes);
            let end = start + reservation;
            if self
                .next_object
                .compare_exchange(start, end, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            self.memory
                .grow_to(end)
                .map_err(HeapAccessV2Error::VirtualMemoryGrow)?;
            return Ok((start, end));
        }
    }
    fn take_free_region(&self, minimum_bytes: u64) -> Option<(u64, u64)> {
        let mut regions = self
            .free_regions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // 惰性合并：只在列表相对上次合并明显增长时排序归并一次。GC 清扫逐对象
        // 释放时若每次 `release_region` 都全表排序，纯对象分配循环会在清扫大堆时
        // 退化为 O(n² log n)（见 release_region 注释）；这里把排序成本摊还到 NLAB
        // 补给（take）路径，清扫本身只剩 O(n) 次 push。
        let baseline = self.merged_free_region_count.load(Ordering::Relaxed) as usize;
        if regions.len() >= baseline.saturating_add(FREE_REGION_MERGE_BATCH) {
            self.merged_free_region_count
                .store(regions.len() as u64, Ordering::Relaxed);
            Self::merge_regions(&mut regions);
        }
        let index = regions
            .iter()
            .position(|(start, end)| end.saturating_sub(*start) >= minimum_bytes)?;
        let (start, end) = regions.remove(index);
        let allocation_end = start + minimum_bytes;
        if allocation_end < end {
            // 余量追加在尾部；首次适配搜索不依赖顺序，下次合并时统一归并。
            regions.push((allocation_end, end));
        }
        Some((start, allocation_end))
    }

    fn release_region(&self, start: u64, bytes: u64) {
        let Some(end) = start.checked_add(bytes) else {
            return;
        };
        let mut regions = self
            .free_regions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        regions.push((start, end));
        // 批量释放（GC 清扫逐对象 retire_handle / 重定位逐对象 relocate）时按批次
        // 排序归并：每累计 FREE_REGION_MERGE_BATCH 个新区间才合并一次，单次成本
        // O((baseline + BATCH) log)，总成本 O(n log BATCH)，避免每次 push 都
        // O(n log n) 全表排序。
        let baseline = self.merged_free_region_count.load(Ordering::Relaxed) as usize;
        if regions.len() >= baseline.saturating_add(FREE_REGION_MERGE_BATCH) {
            self.merged_free_region_count
                .store(regions.len() as u64, Ordering::Relaxed);
            Self::merge_regions(&mut regions);
        }
    }

    /// 按起始地址排序并合并相邻/重叠空闲区。
    fn merge_regions(regions: &mut Vec<(u64, u64)>) {
        regions.sort_unstable_by_key(|(start, _)| *start);
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(regions.len());
        for (start, end) in regions.drain(..) {
            if let Some((_, previous_end)) = merged.last_mut()
                && start <= *previous_end
            {
                *previous_end = (*previous_end).max(end);
            } else {
                merged.push((start, end));
            }
        }
        *regions = merged;
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_regions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|(start, end)| end.saturating_sub(*start))
            .sum()
    }

    pub fn used_bytes(&self) -> u64 {
        self.next_object
            .load(Ordering::Acquire)
            .saturating_sub(self.layout.object_heap_base())
    }
    pub fn heap_limit_bytes(&self) -> u64 {
        self.layout.object_heap_end()
    }

    /// V2 对象堆起点；来自唯一共享 layout。
    pub fn object_heap_base(&self) -> u64 {
        self.layout.object_heap_base()
    }

    /// 当前 bump 游标（已分配对象区终点）。
    pub fn next_object_cursor(&self) -> u64 {
        self.next_object.load(Ordering::Acquire)
    }

    /// 捕获 `[object_heap_base, next_object)` 连续对象字节，供 startup snapshot。
    pub fn capture_object_region(&self) -> Result<Vec<u8>, HeapAccessV2Error> {
        let base = self.object_heap_base();
        let end = self.next_object_cursor().max(base);
        let len = end - base;
        self.memory
            .copy_to(HeapAddress::new(base), len)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 恢复对象区字节并推进 bump 游标。
    pub fn restore_object_region(&self, bytes: &[u8]) -> Result<(), HeapAccessV2Error> {
        let base = self.object_heap_base();
        let end = base
            .checked_add(bytes.len() as u64)
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        self.memory
            .grow_to(end)
            .map_err(HeapAccessV2Error::VirtualMemoryGrow)?;
        self.memory
            .copy_from(HeapAddress::new(base), bytes)
            .map_err(HeapAccessV2Error::Memory)?;
        self.next_object.store(end, Ordering::Release);
        Ok(())
    }

    pub fn allocate_handle(&self) -> Result<u32, HeapAccessV2Error> {
        self.handles
            .allocate_handle()
            .map(HandleId::get)
            .map_err(HeapAccessV2Error::HandleTable)
    }
    pub fn allocate_mirrored_handle(&self, expected: u32) -> Result<u32, HeapAccessV2Error> {
        if u64::from(expected) < self.handles.allocated_count() {
            return Ok(expected);
        }
        loop {
            let actual = self.allocate_handle()?;
            if actual == expected {
                return Ok(actual);
            }
            if actual > expected {
                return Err(HeapAccessV2Error::HandleMirrorMismatch { expected, actual });
            }
        }
    }

    pub fn capture_handles(&self) -> Result<(Vec<RestoredHandleEntry>, u64), HeapAccessV2Error> {
        let entries = self
            .handles
            .snapshot_entries()
            .map_err(HeapAccessV2Error::HandleTable)?;
        Ok((entries, self.handles.allocated_count()))
    }

    pub fn restore_handles(
        &self,
        entries: &[RestoredHandleEntry],
        next_handle: u64,
    ) -> Result<(), HeapAccessV2Error> {
        self.handles
            .restore_snapshot(entries, next_handle)
            .map_err(HeapAccessV2Error::HandleTable)
    }

    /// 发布新对象：header 写 proto / value_capacity / 空 shape，并登记 handle entry。
    /// `capacity` 是**值槽**容量（8 字节/槽），不是属性数。
    pub fn publish_object(
        &self,
        handle: u32,
        object: u64,
        prototype: u32,
        capacity: u32,
    ) -> Result<(), HeapAccessV2Error> {
        if object < self.object_heap_base() || object & 7 != 0 || object >> 48 != 0 {
            return Err(HeapAccessV2Error::InvalidObjectAddress { object });
        }
        let mut header = [0_u8; constants::HEAP_OBJECT_HEADER_SIZE as usize];
        header[constants::HEAP_OBJECT_PROTO_OFFSET as usize..][..4]
            .copy_from_slice(&prototype.to_le_bytes());
        header[constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as usize..][..4]
            .copy_from_slice(&capacity.to_le_bytes());
        header[constants::HEAP_OBJECT_SHAPE_ID_OFFSET as usize..][..4]
            .copy_from_slice(&ShapeTable::empty_shape().to_le_bytes());
        self.memory
            .copy_from(HeapAddress::new(object), &header)
            .map_err(HeapAccessV2Error::Memory)?;
        self.shapes.note_prototype(prototype);
        self.handles
            .publish(HandleId::new(handle), object, HandleGeneration::Young)
            .map_err(HeapAccessV2Error::HandleTable)
    }

    /// 宿主侧隐藏类表；IC 回填与属性枚举都经它。
    pub fn shapes(&self) -> &ShapeTable {
        &self.shapes
    }

    /// 整张 ShapeTable 的 name_id 并集，供宿主 string intern 表回收钉扎。
    pub fn property_name_ids(&self) -> std::collections::HashSet<u32> {
        self.shapes.all_name_ids()
    }

    /// 导出 ShapeTable（startup snapshot / realm 克隆）。
    /// 堆字节与 shape 表必须成对捕获，否则恢复后 shape_id 指向错误的属性结构。
    pub fn export_shapes(&self) -> ShapeTableSnapshot {
        self.shapes.export()
    }

    /// 恢复 ShapeTable，与 `restore_object_region` 成对使用。
    pub fn import_shapes(&self, snapshot: ShapeTableSnapshot) {
        self.shapes.import(snapshot);
    }

    /// 读对象当前 shape_id。数组没有 shape（`+12` 是 capacity），调用方须先判类型。
    pub fn shape_id_at(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        self.memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as u64,
            ))
            .map(|word| (word >> 32) as u32)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 读 handle 的 shape_id。
    pub fn shape_id(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.shape_id_at(object)
    }

    pub fn set_prototype(&self, handle: u32, prototype: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        self.shapes.note_prototype(prototype);
        // 换 proto 会改变整条链的解析结果：接收者自身的 IC 靠缓存的 expected_proto
        // 比较失效，但以本对象为原型的下游对象必须整体重新预热。
        self.shapes.invalidate_if_prototype(handle);
        self.memory
            .store_word(
                HeapAddress::new(object),
                (header & !u64::from(u32::MAX)) | u64::from(prototype),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn publish_array(
        &self,
        handle: u32,
        object: u64,
        prototype: u32,
        capacity: u32,
    ) -> Result<(), HeapAccessV2Error> {
        self.publish_object(handle, object, prototype, capacity)?;
        let mut type_word = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        type_word |= u64::from(wjsm_ir::HEAP_TYPE_ARRAY) << 32;
        self.memory
            .store_word(HeapAddress::new(object), type_word)
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64),
                u64::from(capacity) << 32,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        Ok(())
    }

    /// 读数组 ElementsKind（对象头 `+5` 的 pad 首字节）。
    ///
    /// 头字节 `+0..8` 是一个 word：低 32 位 proto、`+4` heap_type、`+5` kind。
    /// 故 kind = `(header >> 40) & 0xFF`。新分配的数组 header 全零 ⇒ PACKED。
    pub fn array_kind(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.array_kind_at(object)
    }

    pub fn array_kind_at(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok(((header >> (constants::HEAP_ARRAY_KIND_OFFSET * 8)) & 0xFF) as u32)
    }

    /// 单向升级数组 ElementsKind；已是更高等级时不降级。
    ///
    /// 升级只会让元素读**更保守**（退回宿主完整语义），因此永不破坏正确性；
    /// 降级则需要全扫描证明「不含洞且无异质索引属性」，不值得。
    pub fn raise_array_kind(&self, handle: u32, kind: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        let shift = constants::HEAP_ARRAY_KIND_OFFSET * 8;
        let current = ((header >> shift) & 0xFF) as u32;
        if current >= kind {
            return Ok(());
        }
        let cleared = header & !(0xFF_u64 << shift);
        self.memory
            .store_word(
                HeapAddress::new(object),
                cleared | (u64::from(kind) << shift),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn get_element(&self, handle: u32, index: u32) -> Result<Option<u64>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        let length = shape as u32;
        if index >= length {
            return Ok(None);
        }
        let address = array_element_address(object, index)?;
        self.memory
            .load_word(HeapAddress::new(address))
            .map(Some)
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn set_element(
        &self,
        handle: u32,
        index: u32,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape_address = object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64;
        let shape = self
            .memory
            .load_word(HeapAddress::new(shape_address))
            .map_err(HeapAccessV2Error::Memory)?;
        let length = shape as u32;
        let capacity = (shape >> 32) as u32;
        if index >= capacity {
            self.grow_array_capacity(handle, index.saturating_add(1))?;
            return self.set_element(handle, index, value);
        }
        let address = array_element_address(object, index)?;
        self.memory
            .store_word(HeapAddress::new(address), value)
            .map_err(HeapAccessV2Error::Memory)?;
        if index >= length {
            // 跨越式写入（`a[5] = v` 而 length 只有 1）必须把 `[length, index)`
            // 填成洞哨兵：这些槽自分配起是 0，而 `0u64` 解码为 `+0.0`，
            // 不填就会让 `a[2]` 读出 0 而非 undefined。
            if index > length {
                for gap in length..index {
                    self.memory
                        .store_word(
                            HeapAddress::new(array_element_address(object, gap)?),
                            value::encode_array_hole() as u64,
                        )
                        .map_err(HeapAccessV2Error::Memory)?;
                }
                // 产生了洞 → 元素读必须落宿主（洞按缺失属性查原型链）。
                self.raise_array_kind(handle, constants::ARRAY_KIND_HOLEY)?;
            }
            self.memory
                .store_word(
                    HeapAddress::new(shape_address),
                    u64::from(index + 1) | (u64::from(capacity) << 32),
                )
                .map_err(HeapAccessV2Error::Memory)?;
        }
        Ok(())
    }

    pub fn array_length(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
            ))
            .map(|shape| shape as u32)
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn set_array_length(&self, handle: u32, length: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape_address = object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64;
        let shape = self
            .memory
            .load_word(HeapAddress::new(shape_address))
            .map_err(HeapAccessV2Error::Memory)?;
        let capacity = (shape >> 32) as u32;
        if length > capacity {
            return Err(HeapAccessV2Error::ElementCapacityExceeded {
                handle,
                index: length.saturating_sub(1),
                capacity,
            });
        }
        let old_length = shape as u32;
        for index in length..old_length {
            self.set_element(handle, index, value::encode_array_hole() as u64)?;
        }
        self.memory
            .store_word(
                HeapAddress::new(shape_address),
                u64::from(length) | (u64::from(capacity) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn array_shape(&self, handle: u32) -> Result<(u32, u32), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok((shape as u32, (shape >> 32) as u32))
    }

    /// 把数组元素容量扩到至少 `needed` 槽，并通过 handle table 原子发布新地址。
    pub fn grow_array_capacity(&self, handle: u32, needed: u32) -> Result<(), HeapAccessV2Error> {
        let (_, capacity) = self.array_shape(handle)?;
        if needed <= capacity {
            return Ok(());
        }
        let new_capacity = capacity.saturating_mul(2).max(4).max(needed);
        if new_capacity <= capacity {
            return Err(HeapAccessV2Error::AddressOverflow);
        }
        let bytes = object_payload_bytes(new_capacity)?;
        let destination = self.reserve_exact(bytes)?;
        self.relocate_array(handle, destination, new_capacity)
    }

    pub fn relocate_array(
        &self,
        handle: u32,
        new_object: u64,
        new_capacity: u32,
    ) -> Result<(), HeapAccessV2Error> {
        if new_object < self.object_heap_base() || new_object & 7 != 0 || new_object >> 48 != 0 {
            return Err(HeapAccessV2Error::InvalidObjectAddress { object: new_object });
        }
        let old_object = self.resolve_handle(handle)?;
        let header = self
            .memory
            .load_word(HeapAddress::new(old_object))
            .map_err(HeapAccessV2Error::Memory)?;
        let (length, old_capacity) = self.array_shape(handle)?;
        if new_capacity < length || new_capacity <= old_capacity {
            return Err(HeapAccessV2Error::ElementCapacityExceeded {
                handle,
                index: length,
                capacity: new_capacity,
            });
        }
        self.memory
            .store_word(HeapAddress::new(new_object), header)
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(new_object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64),
                u64::from(length) | (u64::from(new_capacity) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        for index in 0..length {
            let value = self
                .memory
                .load_word(HeapAddress::new(array_element_address(old_object, index)?))
                .map_err(HeapAccessV2Error::Memory)?;
            self.memory
                .store_word(
                    HeapAddress::new(array_element_address(new_object, index)?),
                    value,
                )
                .map_err(HeapAccessV2Error::Memory)?;
        }
        self.handles
            .begin_relocation(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles
            .complete_relocation(HandleId::new(handle), new_object)
            .map_err(HeapAccessV2Error::HandleTable)?;
        let old_bytes = u64::from(old_capacity)
            .checked_mul(u64::from(constants::HEAP_ARRAY_ELEMENT_SIZE))
            .and_then(|payload| payload.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE)))
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        self.handles.quarantine_allocation(old_object, old_bytes);
        Ok(())
    }

    pub fn push_element(&self, handle: u32, value: u64) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        let length = shape as u32;
        self.set_element(handle, length, value)?;
        Ok(length + 1)
    }

    /// 删除自有属性：对象退化为字典 shape，被删属性的值槽清零。
    /// 值槽不回收——其余属性的下标必须保持稳定，否则已发射的 IC 会读错槽。
    pub fn delete_property(&self, handle: u32, key: u32) -> Result<bool, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let shape_id = self.shape_id_at(object)?;
        let Some((dictionary_id, (index, span))) = self.shapes.remove_prop(shape_id, key) else {
            return Ok(true);
        };
        self.write_shape_id(object, dictionary_id)?;
        self.shapes.invalidate_if_prototype(handle);
        for offset in 0..span {
            self.memory
                .store_word(
                    HeapAddress::new(value_slot_address(object, index + offset)?),
                    0,
                )
                .map_err(HeapAccessV2Error::Memory)?;
        }
        Ok(true)
    }

    pub fn resolve_handle(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.handles
            .resolve(HandleId::new(handle))
            .map(|entry| entry.address())
            .ok_or(HeapAccessV2Error::UnresolvedHandle { handle })
    }

    pub fn handle_generation(&self, handle: u32) -> Option<HandleGeneration> {
        self.handles
            .resolve(HandleId::new(handle))
            .map(|entry| entry.generation())
    }
    pub fn register_epoch_participant(&self) -> crate::heap::EpochParticipant {
        self.handles.register_participant()
    }

    /// 推进 grace period，并同时回收 handle slot 与对应 object allocation。
    pub fn advance_epoch_and_reclaim(&self) -> (usize, usize) {
        self.handles.advance_epoch();
        let allocations = self.handles.take_reclaimable_allocations();
        let allocation_count = allocations.len();
        for (start, bytes) in allocations {
            self.release_region(start, bytes);
        }
        let handle_count = self.handles.reclaim_quarantine();
        (handle_count, allocation_count)
    }

    pub fn promote_to_old(&self, handle: u32) -> Result<(), HeapAccessV2Error> {
        if self.handle_generation(handle) == Some(HandleGeneration::Old) {
            return Ok(());
        }
        self.handles
            .promote(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)
    }

    fn live_handles(&self) -> Vec<u32> {
        let count = self.handles.allocated_count().min(u64::from(u32::MAX) + 1);
        (0..count as u32)
            .filter(|handle| self.resolve_handle(*handle).is_ok())
            .collect()
    }

    pub fn object_references(&self, handle: u32) -> Result<Vec<i64>, HeapAccessV2Error> {
        let mut references = Vec::new();
        let prototype = self.prototype(handle)?;
        if prototype != PROTO_NULL_SENTINEL && prototype != handle {
            if prototype & 0x8000_0000 != 0 {
                references.push(value::encode_proxy_handle(prototype & 0x7FFF_FFFF));
            } else {
                references.push(value::encode_object_handle(prototype));
            }
        }
        if self.object_type(handle)? == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
            let (length, _) = self.array_shape(handle)?;
            for index in 0..length {
                if let Some(element) = self.get_element(handle, index)? {
                    references.push(element as i64);
                }
            }
        } else {
            for (key, _) in self.own_property_slots(handle)? {
                if let Some(property) = self.get_property_slot(handle, key)? {
                    references.extend([
                        property.value as i64,
                        property.getter as i64,
                        property.setter as i64,
                    ]);
                }
            }
        }
        Ok(references)
    }

    pub fn retire_handle(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let bytes = self.object_size(handle)?;
        self.handles
            .retire(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles.quarantine_allocation(object, bytes);
        Ok(bytes)
    }

    /// Collector 在 stop-the-world/relocation capability 内搬迁对象。
    fn relocate_object(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let bytes = self.object_size(handle)?;
        let destination = self.reserve_exact(bytes)?;
        let contents = self
            .memory
            .copy_to(HeapAddress::new(object), bytes)
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .copy_from(HeapAddress::new(destination), &contents)
            .map_err(HeapAccessV2Error::Memory)?;
        self.handles
            .begin_relocation(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles
            .complete_relocation(HandleId::new(handle), destination)
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles.quarantine_allocation(object, bytes);
        Ok(bytes)
    }

    /// 对象与数组的字节数公式统一为 `header + capacity * 8`——两者 payload 同构。
    fn object_size(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let capacity = if self.object_at_is_array(object)? {
            self.array_shape(handle)?.1
        } else {
            self.value_capacity(object)?
        };
        object_payload_bytes(capacity)
    }

    /// 读对象 heap_type。
    ///
    /// `heap_type` 是 `+4` 处的**单字节**；`+5` 起是 pad（数组 ElementsKind 占用）。
    /// 因此必须显式掩掉高位，不能整取 `header >> 32` 的 u32——那会把 kind 字节
    /// 混进类型值（HEAP_TYPE_ARRAY 会读成 0x101）。
    pub fn object_type(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.object_type_at(object)
    }

    pub fn object_type_at(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok(header_heap_type(header))
    }

    /// 数组的 offset 8/12 是 length/元素容量，与对象属性头（capacity/shape_id）
    /// 布局别名；own 属性槽操作绝不能作用于数组对象——数组命名属性由宿主
    /// `ArrayNamedPropsStore` 侧表承载（与 V1 support 模块语义一致）。
    fn object_at_is_array(&self, object: u64) -> Result<bool, HeapAccessV2Error> {
        Ok(self.object_type_at(object)? == u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
    }

    /// 覆写对象 header 中的 heap type 标记（如 HEAP_TYPE_ARGUMENTS）。
    pub fn set_object_type(&self, handle: u32, object_type: u8) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(object),
                (header & u64::from(u32::MAX)) | (u64::from(object_type) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn prototype(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        Ok(self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)? as u32)
    }

    pub fn get_property(&self, handle: u32, key: u32) -> Result<Option<u64>, HeapAccessV2Error> {
        Ok(self
            .get_property_slot(handle, key)?
            .map(|property| property.value))
    }

    /// 读自有属性槽：shape 查 name_id → 值槽下标，再按数据/accessor 取值。
    pub fn get_property_slot(
        &self,
        handle: u32,
        key: u32,
    ) -> Result<Option<HeapAccessV2Property>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(None);
        }
        let shape_id = self.shape_id_at(object)?;
        let Some(prop) = self.shapes.lookup(shape_id, key) else {
            return Ok(None);
        };
        self.read_prop(object, &prop).map(Some)
    }

    /// 自有属性的 `(name_id, flags)` 列表，按插入序（即 `Object.keys` 顺序）。
    pub fn own_property_slots(&self, handle: u32) -> Result<Vec<(u32, u32)>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(Vec::new());
        }
        let shape_id = self.shape_id_at(object)?;
        Ok(self
            .shapes
            .props(shape_id)
            .into_iter()
            .map(|prop| (prop.name_id, prop.flags))
            .collect())
    }

    /// 收紧已存在属性的 flags（seal/freeze 等描述符路径）。属性不存在则无操作。
    pub fn update_property_flags(
        &self,
        handle: u32,
        key: u32,
        flags: u32,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(());
        }
        let shape_id = self.shape_id_at(object)?;
        let Some(transition) = self.shapes.update_flags(shape_id, key, flags) else {
            return Ok(());
        };
        // flags 收紧不改属性种类时下标不变，无需扩容；改变种类时按 transition 处理。
        self.apply_transition(handle, object, shape_id, transition)
            .map(|_| ())
    }

    pub fn get_property_slot_on_proto_chain(
        &self,
        handle: u32,
        key: u32,
    ) -> Result<Option<HeapAccessV2Property>, HeapAccessV2Error> {
        let mut current = handle;
        loop {
            // 高位标记的 proxy handle 不能 resolve 为 V2 heap 地址；
            // 交给上层 host 走 Proxy [[Get]] trap。
            if current & 0x8000_0000 != 0 {
                return Ok(None);
            }
            let object = self.resolve_handle(current)?;
            let header = self
                .memory
                .load_word(HeapAddress::new(object))
                .map_err(HeapAccessV2Error::Memory)?;
            let object_type = header_heap_type(header);
            if object_type != u32::from(wjsm_ir::HEAP_TYPE_ARRAY)
                && let Some(property) = self.get_property_slot(current, key)?
            {
                return Ok(Some(property));
            }
            let prototype = header as u32;
            if prototype == PROTO_NULL_SENTINEL || prototype == current {
                return Ok(None);
            }
            // 下一环是 Proxy：停止并返回 None，由 host 继续 proxy 路径。
            if prototype & 0x8000_0000 != 0 {
                return Err(HeapAccessV2Error::ProxyPrototype { handle: prototype });
            }
            current = prototype;
        }
    }

    pub fn define_accessor_property(
        &self,
        handle: u32,
        key: u32,
        getter: u64,
        setter: u64,
    ) -> Result<(), HeapAccessV2Error> {
        self.define_accessor_property_with_flags(
            handle,
            key,
            getter,
            setter,
            (constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE) as u32,
        )
    }

    pub fn define_accessor_property_with_flags(
        &self,
        handle: u32,
        key: u32,
        getter: u64,
        setter: u64,
        flags: u32,
    ) -> Result<(), HeapAccessV2Error> {
        self.define_property_slot(
            handle,
            key,
            flags | constants::FLAG_IS_ACCESSOR as u32,
            value::encode_undefined() as u64,
            getter,
            setter,
        )
    }

    pub fn define_data_property(
        &self,
        handle: u32,
        key: u32,
        property_value: u64,
        flags: u32,
    ) -> Result<(), HeapAccessV2Error> {
        self.define_property_slot(
            handle,
            key,
            flags,
            property_value,
            value::encode_undefined() as u64,
            value::encode_undefined() as u64,
        )
    }

    pub fn get_property_on_proto_chain(
        &self,
        handle: u32,
        key: u32,
    ) -> Result<Option<u64>, HeapAccessV2Error> {
        Ok(self
            .get_property_slot_on_proto_chain(handle, key)?
            .map(|property| property.value))
    }

    /// 写自有属性：命中现有 shape 槽则原地覆写，否则按默认数据属性 flags 定义。
    pub fn set_property(&self, handle: u32, key: u32, value: u64) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let shape_id = self.shape_id_at(object)?;
        if let Some(prop) = self.shapes.lookup(shape_id, key)
            && !prop.is_accessor()
        {
            return self.store_value_slot(object, prop.index, value);
        }
        self.define_property_slot(
            handle,
            key,
            (constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE)
                as u32,
            value,
            value::encode_undefined() as u64,
            value::encode_undefined() as u64,
        )
    }

    fn define_property_slot(
        &self,
        handle: u32,
        key: u32,
        flags: u32,
        property_value: u64,
        getter: u64,
        setter: u64,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let shape_id = self.shape_id_at(object)?;
        let transition = self.shapes.transition_add(shape_id, key, flags);
        // transition 可能触发扩容 relocate，对象地址随之变化。
        let object = self.apply_transition(handle, object, shape_id, transition)?;
        if flags & constants::FLAG_IS_ACCESSOR as u32 != 0 {
            self.store_value_slot(object, transition.index, getter)?;
            self.store_value_slot(object, transition.index + 1, setter)
        } else {
            self.store_value_slot(object, transition.index, property_value)
        }
    }

    /// 落地一次 shape 变化：按需扩容值数组、清理被弃用的旧槽、写入新 shape_id、
    /// 使以本对象为原型的 IC 失效。返回（可能因扩容而改变的）对象地址。
    fn apply_transition(
        &self,
        handle: u32,
        object: u64,
        old_shape_id: u32,
        transition: crate::shape::ShapeTransition,
    ) -> Result<u64, HeapAccessV2Error> {
        let mut object = object;
        if self.value_capacity(object)? < transition.slot_count {
            self.grow_value_capacity(handle, object, transition.slot_count)?;
            object = self.resolve_handle(handle)?;
        }
        if let Some((index, span)) = transition.abandoned {
            // 弃用槽必须清零：残留句柄会让 GC 误留对象存活。
            for offset in 0..span {
                self.store_value_slot(object, index + offset, 0)?;
            }
        }
        if transition.shape_id != old_shape_id {
            self.write_shape_id(object, transition.shape_id)?;
            self.shapes.invalidate_if_prototype(handle);
        }
        Ok(object)
    }

    fn write_shape_id(&self, object: u64, shape_id: u32) -> Result<(), HeapAccessV2Error> {
        let address = object + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as u64;
        let word = self
            .memory
            .load_word(HeapAddress::new(address))
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(address),
                (word & u64::from(u32::MAX)) | (u64::from(shape_id) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 值槽容量（8 字节/槽）。
    fn value_capacity(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        self.memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as u64,
            ))
            .map(|word| word as u32)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 把值数组容量扩到至少 `needed` 槽（分配新区、整块搬运、更新 handle entry）。
    pub fn grow_object_capacity(&self, handle: u32, needed: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        if needed <= self.value_capacity(object)? {
            return Ok(());
        }
        self.grow_value_capacity(handle, object, needed)
    }

    fn grow_value_capacity(
        &self,
        handle: u32,
        object: u64,
        needed: u32,
    ) -> Result<(), HeapAccessV2Error> {
        let capacity = self.value_capacity(object)?;
        let new_capacity = capacity.saturating_mul(2).max(4).max(needed);
        if new_capacity <= capacity {
            return Err(HeapAccessV2Error::AddressOverflow);
        }
        let old_bytes = object_payload_bytes(capacity)?;
        let new_bytes = object_payload_bytes(new_capacity)?;
        let destination = self.reserve_exact(new_bytes)?;
        let contents = self
            .memory
            .copy_to(HeapAddress::new(object), old_bytes)
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .copy_from(HeapAddress::new(destination), &contents)
            .map_err(HeapAccessV2Error::Memory)?;
        for index in capacity..new_capacity {
            self.store_value_slot(destination, index, 0)?;
        }
        let capacity_address = destination + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as u64;
        let word = self
            .memory
            .load_word(HeapAddress::new(capacity_address))
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(capacity_address),
                (word & !u64::from(u32::MAX)) | u64::from(new_capacity),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.handles
            .begin_relocation(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles
            .complete_relocation(HandleId::new(handle), destination)
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles.quarantine_allocation(object, old_bytes);
        Ok(())
    }

    fn read_prop(
        &self,
        object: u64,
        prop: &ShapeProp,
    ) -> Result<HeapAccessV2Property, HeapAccessV2Error> {
        let undefined = value::encode_undefined() as u64;
        if prop.is_accessor() {
            return Ok(HeapAccessV2Property {
                flags: prop.flags,
                value: undefined,
                getter: self.load_value_slot(object, prop.getter_index())?,
                setter: self.load_value_slot(object, prop.setter_index())?,
            });
        }
        Ok(HeapAccessV2Property {
            flags: prop.flags,
            value: self.load_value_slot(object, prop.index)?,
            getter: undefined,
            setter: undefined,
        })
    }

    fn load_value_slot(&self, object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
        self.memory
            .load_word(HeapAddress::new(value_slot_address(object, index)?))
            .map_err(HeapAccessV2Error::Memory)
    }

    fn store_value_slot(
        &self,
        object: u64,
        index: u32,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        self.memory
            .store_word(HeapAddress::new(value_slot_address(object, index)?), value)
            .map_err(HeapAccessV2Error::Memory)
    }
}

impl<M: GrowableHeapMemory> CollectorHeapCapability<'_, M> {
    pub fn live_handles(&self) -> Vec<u32> {
        self.heap.live_handles()
    }

    pub fn generation(&self, handle: u32) -> Option<HandleGeneration> {
        self.heap.handle_generation(handle)
    }

    pub fn object_size(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.heap.object_size(handle)
    }

    pub fn promote(&self, handle: u32) -> Result<(), HeapAccessV2Error> {
        self.heap.promote_to_old(handle)
    }

    pub fn relocate(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.heap.relocate_object(handle)
    }

    pub fn retire(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.heap.retire_handle(handle)
    }

    pub fn advance_epoch_and_reclaim(&self) -> (usize, usize) {
        self.heap.advance_epoch_and_reclaim()
    }

    pub fn free_bytes(&self) -> u64 {
        self.heap.free_bytes()
    }

    pub fn used_bytes(&self) -> u64 {
        self.heap.used_bytes()
    }
}

fn align_reservation(bytes: u64) -> Result<u64, HeapAccessV2Error> {
    bytes
        .checked_add(7)
        .map(|bytes| bytes & !7)
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

/// 从 header word 提取 heap_type。
///
/// header 首 8 字节是一个 word：低 32 位 proto handle、`+4` heap_type（**单字节**）、
/// `+5..8` pad（数组 ElementsKind 占 `+5`）。因此类型必须按字节掩码提取——
/// 整取 `header >> 32` 会把 kind 字节混进类型值。
fn header_heap_type(header: u64) -> u32 {
    ((header >> (constants::HEAP_OBJECT_TYPE_OFFSET * 8)) & 0xFF) as u32
}

/// 值槽地址：`object + 16 + index * 8`，与数组元素同一套公式。
pub fn value_slot_address(object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
    object
        .checked_add(constants::HEAP_OBJECT_HEADER_SIZE as u64)
        .and_then(|base| {
            base.checked_add(u64::from(index) * u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        })
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

/// 对象/数组字节数：`16 + capacity * 8`。
pub fn object_payload_bytes(capacity: u32) -> Result<u64, HeapAccessV2Error> {
    u64::from(capacity)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .and_then(|slots| slots.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE)))
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

/// 数组元素地址；与对象值槽同构，故直接复用同一公式。
fn array_element_address(object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
    value_slot_address(object, index)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapAccessV2Error {
    AddressOverflow,
    ElementCapacityExceeded {
        handle: u32,
        index: u32,
        capacity: u32,
    },
    HandleTable(HandleTableError),
    HandleMirrorMismatch {
        expected: u32,
        actual: u32,
    },
    LayoutMismatch {
        memory_base: u64,
        memory_end: u64,
        object_heap_base: u64,
        object_heap_end: u64,
    },
    HeapExhausted {
        requested: u64,
        heap_limit: u64,
    },
    InvalidObjectAddress {
        object: u64,
    },
    Memory(HeapMemoryError),
    PropertyCapacityExceeded {
        handle: u32,
        capacity: u32,
    },
    VirtualMemoryGrow(String),
    UnresolvedHandle {
        handle: u32,
    },
    /// 原型链下一环是高位标记的 Proxy handle，需 host 走 trap。
    ProxyPrototype {
        handle: u32,
    },
    /// 数组对象没有属性槽（offset 8/12 与 length/元素容量别名）；
    /// 命名属性必须经宿主 `ArrayNamedPropsStore` 侧表。
    ArrayPropertySlots {
        handle: u32,
    },
}

impl fmt::Display for HeapAccessV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressOverflow => formatter.write_str("V2 heap address overflows u64"),
            Self::ElementCapacityExceeded {
                handle,
                index,
                capacity,
            } => write!(
                formatter,
                "V2 array handle {handle} index {index} exceeds capacity {capacity}"
            ),
            Self::HandleTable(error) => error.fmt(formatter),
            Self::HandleMirrorMismatch { expected, actual } => write!(
                formatter,
                "handle mirror expected {expected}, HandleTableV2 allocated {actual}"
            ),
            Self::LayoutMismatch {
                memory_base,
                memory_end,
                object_heap_base,
                object_heap_end,
            } => write!(
                formatter,
                "heap memory range {memory_base:#x}..{memory_end:#x} does not match managed layout {object_heap_base:#x}..{object_heap_end:#x}"
            ),
            Self::HeapExhausted {
                requested,
                heap_limit,
            } => write!(
                formatter,
                "V2 heap cannot reserve {requested} bytes below limit {heap_limit:#x}"
            ),
            Self::InvalidObjectAddress { object } => {
                write!(formatter, "invalid V2 object address {object:#x}")
            }
            Self::Memory(error) => error.fmt(formatter),
            Self::PropertyCapacityExceeded { handle, capacity } => {
                write!(
                    formatter,
                    "V2 object handle {handle} has property capacity {capacity}"
                )
            }
            Self::VirtualMemoryGrow(error) => {
                write!(formatter, "unable to grow V2 shared memory64: {error}")
            }
            Self::UnresolvedHandle { handle } => write!(formatter, "unresolved V2 handle {handle}"),
            Self::ProxyPrototype { handle } => {
                write!(formatter, "proxy prototype handle {handle:#x}")
            }
            Self::ArrayPropertySlots { handle } => write!(
                formatter,
                "V2 array handle {handle} has no property slots; named props live in the host side table"
            ),
        }
    }
}

impl Error for HeapAccessV2Error {}
