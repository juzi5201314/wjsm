//! memory64 V2 动态 JS 堆的唯一 host 访问入口。

use std::error::Error;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use crate::PropertyKey;
use crate::heap::{
    Allocation, AllocatorError, GrowableHeapMemory, HandleGeneration, HandleId, HandleTableError,
    HandleTableV2, HeapAddress, HeapMemoryError, ManagedHeap, ManagedHeapLayout, Nlab, ObjectRef,
    PageId, PageStats, RestoredHandleEntry,
};
use crate::shape::{PROTO_NULL_SENTINEL, ShapeProp, ShapeTable, ShapeTableSnapshot};
use crate::zgc::{
    BarrierRecord, HeaderLayout, HeapBarrier, LoadBarrierOutcome, RelocationDescriptor,
    color_stored_value, load_barrier,
};
use crate::StrView;
use wjsm_ir::{constants, value};

/// V2 dynamic heap 的唯一 host access owner；所有地址均为 memory64 byte offset。
pub struct HeapAccessV2<M: GrowableHeapMemory> {
    heap: Arc<ManagedHeap<M>>,
    layout: Arc<ManagedHeapLayout>,
    handles: Arc<HandleTableV2>,
    barrier: HeapBarrier<M>,
    /// 属性元数据（name_id / flags / 值槽下标）的唯一 owner；堆内只留紧凑值数组。
    shapes: ShapeTable,
    /// 属性/数组扩容与 mutator 共享同一 bump，避免每次 reserve 都吞掉一整页。
    nlab: Mutex<Nlab>,
    #[cfg(debug_assertions)]
    string_read_scope: AtomicUsize,
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


struct StringReadScope<'a, M: GrowableHeapMemory> {
    owner: &'a HeapAccessV2<M>,
}

impl<M: GrowableHeapMemory> Drop for StringReadScope<'_, M> {
    fn drop(&mut self) {
        self.owner.exit_string_read();
    }
}
impl<M: GrowableHeapMemory> HeapAccessV2<M> {
    /// 构造唯一 heap access owner；memory、allocator 与 handle table 共享同一 layout。
    pub fn with_handles(
        memory: M,
        layout: Arc<ManagedHeapLayout>,
        handles: Arc<HandleTableV2>,
        barrier: HeapBarrier<M>,
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
        if let HeapBarrier::Zgc(zgc) = &barrier
            && !std::ptr::eq(zgc.handles(), handles.as_ref())
        {
            return Err(HeapAccessV2Error::BarrierHandleTableMismatch);
        }
        let heap = Arc::new(
            ManagedHeap::with_epoch(memory, layout.as_ref().clone(), handles.epoch())
                .map_err(HeapAccessV2Error::Allocator)?,
        );
        Ok(Self {
            heap,
            layout,
            handles,
            barrier,
            shapes: ShapeTable::new(),
            nlab: Mutex::new(Nlab::new()),
            #[cfg(debug_assertions)]
            string_read_scope: AtomicUsize::new(0),
        })
    }

    pub fn collector_capability(&self) -> CollectorHeapCapability<'_, M> {
        CollectorHeapCapability { heap: self }
    }

    fn enter_string_read(&self) {
        #[cfg(debug_assertions)]
        self.string_read_scope.fetch_add(1, Ordering::SeqCst);
    }

    fn exit_string_read(&self) {
        #[cfg(debug_assertions)]
        self.string_read_scope.fetch_sub(1, Ordering::SeqCst);
    }

    fn string_read_scope(&self) -> StringReadScope<'_, M> {
        self.enter_string_read();
        StringReadScope { owner: self }
    }

    fn assert_not_in_string_read(&self) {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.string_read_scope.load(Ordering::SeqCst),
            0,
            "with_string_* 闭包内禁止 JS 堆分配"
        );
    }

    pub const fn barrier(&self) -> &HeapBarrier<M> {
        &self.barrier
    }

    pub fn epoch(&self) -> Arc<crate::heap::HeapEpoch> {
        self.heap.allocator().epoch()
    }

    /// 从 mutator 私有 NLAB 分配；命中快链不获取 allocator lock。
    pub fn allocate(&self, nlab: &mut Nlab, bytes: u64) -> Result<Allocation, HeapAccessV2Error> {
        self.assert_not_in_string_read();
        let allocation = self.heap.allocate(nlab, bytes).map_err(|error| {
            if matches!(error, AllocatorError::OutOfPages { .. }) {
                HeapAccessV2Error::HeapExhausted {
                    requested: bytes,
                    heap_limit: self.layout.object_heap_end(),
                }
            } else {
                HeapAccessV2Error::Allocator(error)
            }
        })?;
        let end = allocation
            .object()
            .offset()
            .checked_add(allocation.bytes())
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        self.heap
            .memory()
            .grow_to(end)
            .map_err(HeapAccessV2Error::VirtualMemoryGrow)?;
        Ok(allocation)
    }

    fn reserve_exact(&self, bytes: u64) -> Result<u64, HeapAccessV2Error> {
        let mut nlab = self.nlab.lock();
        self.allocate(&mut nlab, bytes)
            .map(|allocation| allocation.object().offset())
    }

    pub fn reset_nlab(&self) {
        self.nlab.lock().reset();
    }

    pub fn free_bytes(&self) -> u64 {
        self.heap.allocator().free_bytes()
    }

    pub fn free_pages(&self) -> u32 {
        self.heap.allocator().free_pages()
    }

    pub fn used_bytes(&self) -> u64 {
        self.heap.allocator().allocated_bytes()
    }
    pub fn heap_limit_bytes(&self) -> u64 {
        self.layout.object_heap_end()
    }

    /// V2 对象堆起点；来自唯一共享 layout。
    pub fn object_heap_base(&self) -> u64 {
        self.layout.object_heap_base()
    }

    /// 捕获已提交 object heap 字节；稳定 handle 决定恢复时的真实对象集合。
    pub fn capture_object_region(&self) -> Result<Vec<u8>, HeapAccessV2Error> {
        let base = self.object_heap_base();
        let end = self.heap.byte_len().max(base);
        self.heap
            .copy_to(HeapAddress::new(base), end - base)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 恢复 object heap 原始字节；page/object metadata 随稳定 handle 单独重建。
    pub fn restore_object_region(&self, bytes: &[u8]) -> Result<(), HeapAccessV2Error> {
        let base = self.object_heap_base();
        let end = base
            .checked_add(bytes.len() as u64)
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        self.heap
            .memory()
            .grow_to(end)
            .map_err(HeapAccessV2Error::VirtualMemoryGrow)?;
        self.heap
            .copy_from(HeapAddress::new(base), bytes)
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn allocate_handle(&self) -> Result<u32, HeapAccessV2Error> {
        self.assert_not_in_string_read();
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

    pub fn restore_page_metadata(
        &self,
        entries: &[RestoredHandleEntry],
    ) -> Result<(), HeapAccessV2Error> {
        for entry in entries {
            let actual = self.object_handle_at(entry.address)?;
            if actual != entry.handle.get() {
                return Err(HeapAccessV2Error::RestoredGcWordHandleMismatch {
                    object: entry.address,
                    expected: entry.handle.get(),
                    actual,
                });
            }
            let bytes = self.object_size(entry.handle.get())?;
            self.heap
                .allocator()
                .restore_object(ObjectRef::new(entry.address), bytes)
                .map_err(HeapAccessV2Error::Allocator)?;
        }
        Ok(())
    }

    pub fn page_stats(&self) -> Vec<PageStats> {
        self.heap.allocator().page_stats()
    }

    pub fn handles_in_page(&self, page: PageId) -> Result<Vec<u32>, HeapAccessV2Error> {
        self.heap
            .allocator()
            .objects_in_page(page)
            .map(|object| self.object_handle_at(object.offset()))
            .collect()
    }

    pub fn generation_bytes(&self) -> Result<(u64, u64), HeapAccessV2Error> {
        let mut young = 0_u64;
        let mut old = 0_u64;
        for page in self.page_stats() {
            for handle in self.handles_in_page(page.page)? {
                let bytes = self.object_size(handle)?;
                match self.handle_generation(handle) {
                    Some(HandleGeneration::Young) => young = young.saturating_add(bytes),
                    Some(HandleGeneration::Old) => old = old.saturating_add(bytes),
                    None => {}
                }
            }
        }
        Ok((young, old))
    }

    pub fn set_object_age(&self, handle: u32, age: u8) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let address = HeapAddress::new(object + u64::from(constants::HEAP_OBJECT_GC_WORD_OFFSET));
        let current = self
            .heap
            .memory()
            .load_word(address)
            .map_err(HeapAccessV2Error::Memory)?;
        let age_bits = u64::from(age) << constants::HEAP_GC_AGE_SHIFT;
        self.heap
            .memory()
            .store_word(address, (current & !constants::HEAP_GC_AGE_MASK) | age_bits)
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn prepare_relocation(
        &self,
        handles: &[u32],
        reserve_pages: u32,
    ) -> Result<Vec<Arc<RelocationDescriptor>>, HeapAccessV2Error> {
        let HeapBarrier::Zgc(barrier) = &self.barrier else {
            return Err(HeapAccessV2Error::RelocationAssist(
                "relocation requires the ZGC barrier".into(),
            ));
        };
        let mut nlab = self
            .heap
            .allocator()
            .reserve_relocation(reserve_pages)
            .map_err(HeapAccessV2Error::Allocator)?;
        let mut descriptors = Vec::with_capacity(handles.len());
        for handle in handles {
            let source = self.resolve_handle(*handle)?;
            let size = self.object_size(*handle)?;
            let generation = self
                .handle_generation(*handle)
                .ok_or(HeapAccessV2Error::UnresolvedHandle { handle: *handle })?;
            let allocation = self
                .heap
                .allocator()
                .allocate_relocation(&mut nlab, size)
                .map_err(HeapAccessV2Error::Allocator)?;
            let destination = allocation.object().offset();
            let end = destination
                .checked_add(allocation.bytes())
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .memory()
                .grow_to(end)
                .map_err(HeapAccessV2Error::VirtualMemoryGrow)?;
            let descriptor = barrier
                .relocator()
                .install_descriptor(RelocationDescriptor::new(
                    HandleId::new(*handle),
                    source,
                    destination,
                    size,
                    generation,
                    self.string_layout_for(*handle)?,
                ));
            descriptors.push(descriptor);
        }
        self.heap
            .allocator()
            .finish_relocation(nlab)
            .map_err(HeapAccessV2Error::Allocator)?;
        Ok(descriptors)
    }

    pub fn relocate_descriptor(
        &self,
        descriptor: &RelocationDescriptor,
        worker_id: u64,
    ) -> Result<bool, HeapAccessV2Error> {
        let HeapBarrier::Zgc(barrier) = &self.barrier else {
            return Err(HeapAccessV2Error::RelocationAssist(
                "relocation requires the ZGC barrier".into(),
            ));
        };
        let copied = barrier
            .relocator()
            .copy_with_ownership(&self.handles, self.heap.memory(), descriptor, worker_id)
            .map_err(HeapAccessV2Error::RelocationAssist)?;
        if copied {
            self.heap
                .allocator()
                .transfer_mark(
                    ObjectRef::new(descriptor.source),
                    ObjectRef::new(descriptor.destination),
                    descriptor.size,
                    descriptor.generation,
                )
                .map_err(HeapAccessV2Error::Allocator)?;
        }
        if copied
            && self
                .heap
                .allocator()
                .forget_object_if_present(ObjectRef::new(descriptor.source), descriptor.size)
                .map_err(HeapAccessV2Error::Allocator)?
        {
            self.heap
                .allocator()
                .quarantine_allocation(ObjectRef::new(descriptor.source), descriptor.size);
        }
        Ok(copied)
    }

    pub fn try_mark_handle(&self, handle: u32) -> Result<bool, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let generation = self
            .handle_generation(handle)
            .ok_or(HeapAccessV2Error::UnresolvedHandle { handle })?;
        self.heap
            .allocator()
            .try_mark(
                ObjectRef::new(object),
                self.object_size(handle)?,
                generation,
            )
            .map_err(HeapAccessV2Error::Allocator)
    }

    pub fn is_marked_handle(
        &self,
        handle: u32,
        generation: HandleGeneration,
    ) -> Result<bool, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.heap
            .allocator()
            .is_marked(ObjectRef::new(object), generation)
            .map_err(HeapAccessV2Error::Allocator)
    }

    pub fn clear_marks(&self, generation: HandleGeneration) {
        self.heap.allocator().clear_marks(generation);
    }

    pub fn load_reference_slot(&self, slot_addr: u64) -> Result<i64, HeapAccessV2Error> {
        self.heap
            .memory()
            .load_word(HeapAddress::new(slot_addr))
            .map(|word| word as i64)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 发布新对象：header 写 proto / value_capacity / 空 shape，并登记 handle entry。
    /// 调用方随后通过 `set_prototype` 完成原型绑定；该步骤也会使复用的 prototype
    /// handle 对应的原型链 IC 世代失效。`capacity` 是**值槽**容量（8 字节/槽），不是属性数。
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
        let gc_word = u64::from(handle) & constants::HEAP_GC_HANDLE_MASK;
        header[constants::HEAP_OBJECT_GC_WORD_OFFSET as usize..][..8]
            .copy_from_slice(&gc_word.to_le_bytes());
        self.heap
            .memory()
            .copy_from(HeapAddress::new(object), &header)
            .map_err(HeapAccessV2Error::Memory)?;
        self.shapes.note_prototype(prototype);
        self.handles
            .publish(HandleId::new(handle), object, HandleGeneration::Young)
            .map_err(HeapAccessV2Error::HandleTable)?;
        if let HeapBarrier::Zgc(barrier) = &self.barrier
            && barrier.epoch().young_marking
        {
            self.heap
                .allocator()
                .try_mark(
                    ObjectRef::new(object),
                    object_payload_bytes(capacity)?,
                    HandleGeneration::Young,
                )
                .map_err(HeapAccessV2Error::Allocator)?;
        }
        Ok(())
    }

    pub fn gc_word_at(&self, object: u64) -> Result<u64, HeapAccessV2Error> {
        self.heap
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_OBJECT_GC_WORD_OFFSET),
            ))
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn object_handle_at(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        Ok((self.gc_word_at(object)? & constants::HEAP_GC_HANDLE_MASK) as u32)
    }

    pub fn object_age_at(&self, object: u64) -> Result<u8, HeapAccessV2Error> {
        Ok(((self.gc_word_at(object)? & constants::HEAP_GC_AGE_MASK)
            >> constants::HEAP_GC_AGE_SHIFT) as u8)
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
        self.heap
            .memory()
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

    /// handle region 基址；generated code 经 vmctx 的 `handle_table_base` 用它把
    /// 句柄下标换算成 8 字节 entry 地址。region 生命周期与 handle table 相同。
    pub fn handle_table_base(&self) -> *mut u8 {
        self.handles.region_base()
    }

    /// 对象地址的「逻辑 → 虚拟」偏移（`virtual_base - logical_base`）；generated
    /// code 的属性快链用它把 handle entry 里的逻辑对象地址换算成真实映射地址。
    /// TestHeapMemory 的 virtual_base 即 logical_base，故测试下该值为 0。
    pub fn object_address_delta(&self) -> i64 {
        let virtual_base = self.heap.memory().virtual_base() as i64;
        let logical_base =
            i64::try_from(self.heap.memory().logical_base()).expect("logical heap base fits i64");
        virtual_base - logical_base
    }

    /// 返回自有数据属性的 `(shape_id, value_index)`；accessor / 字典 shape /
    /// 数组 / 缺失属性一律返回 `None`（快路径 miss，由 miss handler 决定回填）。
    pub fn own_data_property_index(
        &self,
        handle: u32,
        key: PropertyKey,
    ) -> Result<Option<(u32, u32)>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(None);
        }
        let shape_id = self.shape_id_at(object)?;
        if self.shapes.is_dictionary(shape_id) {
            return Ok(None);
        }
        let Some(prop) = self.shapes.lookup(shape_id, key.get()) else {
            return Ok(None);
        };

        if prop.is_accessor() {
            return Ok(None);
        }
        Ok(Some((shape_id, prop.index)))
    }

    pub fn set_prototype(&self, handle: u32, prototype: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let header = self
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        if let HeapBarrier::Zgc(barrier) = &self.barrier {
            let epoch = barrier.epoch();
            let old_prototype = header as u32;
            if (epoch.young_marking || epoch.old_marking) && old_prototype != PROTO_NULL_SENTINEL {
                barrier
                    .record(BarrierRecord::Satb(value::encode_object_handle(
                        old_prototype,
                    )))
                    .map_err(|_| HeapAccessV2Error::BarrierBufferFull)?;
            }
            if (epoch.young_marking || epoch.old_marking) && prototype != PROTO_NULL_SENTINEL {
                barrier
                    .record(BarrierRecord::Mark(value::encode_object_handle(prototype)))
                    .map_err(|_| HeapAccessV2Error::BarrierBufferFull)?;
            }
            if self.handle_generation(handle) == Some(HandleGeneration::Old) {
                barrier
                    .record(BarrierRecord::RememberedObject(HandleId::new(handle)))
                    .map_err(|_| HeapAccessV2Error::BarrierBufferFull)?;
            }
        }
        self.shapes.note_prototype(prototype);
        // 换 proto 会改变整条链的解析结果：接收者自身的 IC 靠缓存的 expected_proto
        // 比较失效，但以本对象为原型的下游对象必须整体重新预热。
        self.shapes.invalidate_if_prototype(handle);
        self.heap
            .memory()
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
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        type_word |= u64::from(wjsm_ir::HEAP_TYPE_ARRAY) << 32;
        self.heap
            .memory()
            .store_word(HeapAddress::new(object), type_word)
            .map_err(HeapAccessV2Error::Memory)?;
        self.heap
            .memory()
            .store_word(
                HeapAddress::new(object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64),
                u64::from(capacity) << 32,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        Ok(())
    }

    /// 发布字符串对象：写 32 字节字符串头（proto / type / repr / flags / length /
    /// capacity / gc_word），payload 保持调用方已写入的字节不变。
    ///
    /// `capacity` 是 **payload 字节数**（按 8 对齐）：Flat/Builder 为原始数据缓冲
    /// 大小，Cons/Slice 为固定子引用区大小（`HEAP_STRING_CONS_PAYLOAD_SIZE` /
    /// `HEAP_STRING_SLICE_PAYLOAD_SIZE`）。hash 字段初始为 0（未计算），惰性由
    /// [`HeapAccessV2::string_content_hash`] 填充。
    #[allow(clippy::too_many_arguments)]
    pub fn publish_string(
        &self,
        handle: u32,
        object: u64,
        prototype: u32,
        repr: u8,
        flags: u8,
        length: u32,
        capacity: u32,
    ) -> Result<(), HeapAccessV2Error> {
        if object < self.object_heap_base() || object & 7 != 0 || object >> 48 != 0 {
            return Err(HeapAccessV2Error::InvalidObjectAddress { object });
        }
        if capacity & 7 != 0 {
            return Err(HeapAccessV2Error::InvalidStringCapacity { capacity });
        }
        if !matches!(
            repr,
            constants::STRING_REPR_LATIN1_FLAT
                | constants::STRING_REPR_UTF16_FLAT
                | constants::STRING_REPR_CONS
                | constants::STRING_REPR_SLICE
                | constants::STRING_REPR_BUILDER
        ) {
            return Err(HeapAccessV2Error::InvalidStringRepr { repr });
        }
        let mut header = [0_u8; constants::HEAP_STRING_HEADER_SIZE as usize];
        header[constants::HEAP_OBJECT_PROTO_OFFSET as usize..][..4]
            .copy_from_slice(&prototype.to_le_bytes());
        header[constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_STRING;
        header[constants::HEAP_STRING_REPR_OFFSET as usize] = repr;
        header[constants::HEAP_STRING_FLAGS_OFFSET as usize] = flags;
        header[constants::HEAP_STRING_LENGTH_OFFSET as usize..][..4]
            .copy_from_slice(&length.to_le_bytes());
        header[constants::HEAP_STRING_CAPACITY_OFFSET as usize..][..4]
            .copy_from_slice(&capacity.to_le_bytes());
        let gc_word = u64::from(handle) & constants::HEAP_GC_HANDLE_MASK;
        header[constants::HEAP_OBJECT_GC_WORD_OFFSET as usize..][..8]
            .copy_from_slice(&gc_word.to_le_bytes());
        // +24 hash 与 +28 pad 保持零：hash 未计算，pad 恒零。
        self.heap
            .memory()
            .copy_from(HeapAddress::new(object), &header)
            .map_err(HeapAccessV2Error::Memory)?;
        self.shapes.note_prototype(prototype);
        self.handles
            .publish(HandleId::new(handle), object, HandleGeneration::Young)
            .map_err(HeapAccessV2Error::HandleTable)?;
        if let HeapBarrier::Zgc(barrier) = &self.barrier
            && barrier.epoch().young_marking
        {
            self.heap
                .allocator()
                .try_mark(
                    ObjectRef::new(object),
                    string_payload_bytes(capacity)?,
                    HandleGeneration::Young,
                )
                .map_err(HeapAccessV2Error::Allocator)?;
        }
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
            .heap
            .memory()
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
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        let shift = constants::HEAP_ARRAY_KIND_OFFSET * 8;
        let current = ((header >> shift) & 0xFF) as u32;
        if current >= kind {
            return Ok(());
        }
        let cleared = header & !(0xFF_u64 << shift);
        self.heap
            .memory()
            .store_word(
                HeapAddress::new(object),
                cleared | (u64::from(kind) << shift),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn get_element(&self, handle: u32, index: u32) -> Result<Option<u64>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        let length = shape as u32;
        let capacity = (shape >> 32) as u32;
        // length 可以超过容量；未分配槽是隐式 hole，不能去读越界地址。
        if index >= length || index >= capacity {
            return Ok(None);
        }
        let address = array_element_address(object, index)?;
        self.heap
            .memory()
            .load_word(HeapAddress::new(address))
            .map(|stored| Some(value::strip_gc_color(stored as i64) as u64))
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn set_element(
        &self,
        handle: u32,
        index: u32,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .heap
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_ARRAY_LENGTH_OFFSET),
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        let length = shape as u32;
        let capacity = (shape >> 32) as u32;
        if index >= capacity {
            self.grow_array_capacity(handle, index.saturating_add(1))?;
            // 写入 length 以内、尚未分配的槽：前后仍是隐式 hole。
            if index < length {
                self.raise_array_kind(handle, constants::ARRAY_KIND_HOLEY)?;
            }
            return self.set_element(handle, index, value);
        }
        if index > length {
            for gap in length..index {
                let current = self.resolve_handle(handle)?;
                self.store_reference(
                    handle,
                    array_element_address(current, gap)?,
                    value::encode_array_hole() as u64,
                )?;
            }
            self.raise_array_kind(handle, constants::ARRAY_KIND_HOLEY)?;
        }
        let current = self.resolve_handle(handle)?;
        self.store_reference(handle, array_element_address(current, index)?, value)?;
        if index >= length {
            let current = self.resolve_handle(handle)?;
            self.heap
                .store_word(
                    HeapAddress::new(current + u64::from(constants::HEAP_ARRAY_LENGTH_OFFSET)),
                    u64::from(index + 1) | (u64::from(capacity) << 32),
                )
                .map_err(HeapAccessV2Error::Memory)?;
        }
        Ok(())
    }

    pub fn array_length(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.heap
            .memory()
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
            .heap
            .memory()
            .load_word(HeapAddress::new(shape_address))
            .map_err(HeapAccessV2Error::Memory)?;
        let capacity = (shape >> 32) as u32;
        let old_length = shape as u32;
        // length 可以超过已分配容量：超出部分是隐式 hole，不必立刻扩容。
        let fill_end = old_length.min(capacity);
        if length < fill_end {
            for index in length..fill_end {
                self.set_element(handle, index, value::encode_array_hole() as u64)?;
            }
        }
        self.heap
            .memory()
            .store_word(
                HeapAddress::new(shape_address),
                u64::from(length) | (u64::from(capacity) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn array_shape(&self, handle: u32) -> Result<(u32, u32), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .heap
            .memory()
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
        let (length, old_capacity) = self.array_shape(handle)?;
        // length 可以超过容量（隐式 hole）；扩容只要求新容量严格大于旧容量。
        if new_capacity <= old_capacity {
            return Err(HeapAccessV2Error::ElementCapacityExceeded {
                handle,
                index: length,
                capacity: new_capacity,
            });
        }
        let old_bytes = object_payload_bytes(old_capacity)?;
        let new_bytes = object_payload_bytes(new_capacity)?;
        let generation = self
            .handle_generation(handle)
            .ok_or(HeapAccessV2Error::UnresolvedHandle { handle })?;
        self.heap
            .copy_atomic_words(
                HeapAddress::new(old_object),
                HeapAddress::new(new_object),
                old_bytes,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        let hole = value::encode_array_hole() as u64;
        for index in old_capacity..new_capacity {
            self.heap
                .store_word(
                    HeapAddress::new(array_element_address(new_object, index)?),
                    hole,
                )
                .map_err(HeapAccessV2Error::Memory)?;
        }
        self.heap
            .store_word(
                HeapAddress::new(new_object + u64::from(constants::HEAP_ARRAY_LENGTH_OFFSET)),
                u64::from(length) | (u64::from(new_capacity) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.handles
            .begin_relocation(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles
            .complete_relocation(HandleId::new(handle), new_object)
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.heap
            .allocator()
            .transfer_mark(
                ObjectRef::new(old_object),
                ObjectRef::new(new_object),
                new_bytes,
                generation,
            )
            .map_err(HeapAccessV2Error::Allocator)?;
        if self
            .heap
            .allocator()
            .forget_object_if_present(ObjectRef::new(old_object), old_bytes)
            .map_err(HeapAccessV2Error::Allocator)?
        {
            self.heap
                .allocator()
                .quarantine_allocation(ObjectRef::new(old_object), old_bytes);
        }
        Ok(())
    }

    pub fn push_element(&self, handle: u32, value: u64) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let shape = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        let length = shape as u32;
        self.set_element(handle, length, value)?;
        Ok(length + 1)
    }

    // ── 字符串存取器 ─────────────────────────────────────────────────────────
    /// 读字符串 repr（header `+5` 单字节）。
    pub fn string_repr(&self, handle: u32) -> Result<u8, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.string_repr_at(object)
    }

    fn string_repr_at(&self, object: u64) -> Result<u8, HeapAccessV2Error> {
        // repr 在 header `+5` 单字节，load_word 要求 8 对齐，从 `+0` 整 word 移位取字节。
        self.heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map(|word| ((word >> (constants::HEAP_STRING_REPR_OFFSET * 8)) & 0xFF) as u8)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 读字符串 flags（header `+6` 单字节）。
    pub fn string_flags(&self, handle: u32) -> Result<u8, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map(|word| ((word >> (constants::HEAP_STRING_FLAGS_OFFSET * 8)) & 0xFF) as u8)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 原子覆写 flags 字节（`set` 置位 / `clear` 清零；同一字节同时给定时 set 优先）。
    /// 与其他 header 字段一样走搬迁同步，ZGC 下不会写丢。
    pub fn update_string_flags(
        &self,
        handle: u32,
        set: u8,
        clear: u8,
    ) -> Result<(), HeapAccessV2Error> {
        let owner = self.resolve_handle(handle)?;
        let word = self
            .heap
            .memory()
            .load_word(HeapAddress::new(owner))
            .map_err(HeapAccessV2Error::Memory)?;
        let shift = constants::HEAP_STRING_FLAGS_OFFSET * 8;
        let current = ((word >> shift) & 0xFF) as u8;
        let next = (current | set) & !clear;
        let stored = (word & !(0xFF_u64 << shift)) | (u64::from(next) << shift);
        self.store_header_word(handle, 0, stored)
    }

    /// 读字符串码元数（header `+8`）。
    pub fn string_length(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.string_length_at(object)
    }

    fn string_length_at(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        self.heap
            .memory()
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_STRING_LENGTH_OFFSET),
            ))
            .map(|word| word as u32)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 覆写字符串 length（Builder 追加后推进）；length 与 capacity 共享 `+8` 的
    /// word（低 32 位 length、高 32 位 capacity），写入必须保留 capacity。
    pub fn set_string_length(&self, handle: u32, length: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let word = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_STRING_LENGTH_OFFSET),
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        let packed = (word & !u64::from(u32::MAX)) | u64::from(length);
        self.store_header_word(handle, constants::HEAP_STRING_LENGTH_OFFSET, packed)
    }

    /// 覆写字符串 repr 字节；Builder 完成后冻结为 UTF-16 Flat。
    pub fn set_string_repr(&self, handle: u32, repr: u8) -> Result<(), HeapAccessV2Error> {
        if !matches!(
            repr,
            constants::STRING_REPR_LATIN1_FLAT
                | constants::STRING_REPR_UTF16_FLAT
                | constants::STRING_REPR_BUILDER
        ) {
            return Err(HeapAccessV2Error::InvalidStringRepr { repr });
        }
        let object = self.resolve_handle(handle)?;
        let word = self
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        let shift = constants::HEAP_STRING_REPR_OFFSET * 8;
        self.store_header_word(
            handle,
            0,
            (word & !(0xFF_u64 << shift)) | (u64::from(repr) << shift),
        )
    }

    /// 读 payload 字节容量（header `+12`；Cons/Slice 为固定子引用区大小）。
    pub fn string_capacity(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.string_capacity_at(object)
    }

    fn string_capacity_at(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        // capacity 在 `+12`（非 8 对齐），与 length 共享 `+8` 的 word 高 32 位。
        self.heap
            .memory()
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_STRING_LENGTH_OFFSET),
            ))
            .map(|word| (word >> 32) as u32)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 读已缓存的内容哈希（header `+24`）；0 表示尚未计算，不触发计算。
    pub fn string_hash(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        self.heap
            .memory()
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_STRING_HASH_OFFSET),
            ))
            .map(|word| word as u32)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 读取扁平字符串的字节视图；闭包内不得触发 JS 堆分配。
    pub fn with_string_bytes<R>(
        &self,
        handle: u32,
        f: impl FnOnce(StrView<'_>) -> R,
    ) -> Result<R, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let repr = self.string_repr_at(object)?;
        let length = usize::try_from(self.string_length_at(object)?)
            .map_err(|_| HeapAccessV2Error::AddressOverflow)?;
        let byte_length = match repr {
            constants::STRING_REPR_LATIN1_FLAT => length,
            constants::STRING_REPR_UTF16_FLAT | constants::STRING_REPR_BUILDER => length
                .checked_mul(2)
                .ok_or(HeapAccessV2Error::AddressOverflow)?,
            constants::STRING_REPR_CONS | constants::STRING_REPR_SLICE => {
                return Err(HeapAccessV2Error::StringFlattenRequired { handle });
            }
            _ => return Err(HeapAccessV2Error::InvalidStringRepr { repr }),
        };
        let payload = self.string_payload_address(object)?;
        let address = HeapAddress::new(payload);
        if self.direct_string_access_safe()
            && let Some(bytes) = self.heap.memory().try_bytes(address, byte_length as u64)
        {
            let _scope = self.string_read_scope();
            return match repr {
                constants::STRING_REPR_LATIN1_FLAT => Ok(f(StrView::Latin1(&bytes[..length]))),
                constants::STRING_REPR_UTF16_FLAT | constants::STRING_REPR_BUILDER => {
                    // SAFETY: payload 起点由 8 字节对齐的对象地址加 32 字节得到，满足
                    // u16 对齐；byte_length 已由 length * 2 精确校验，区间在堆内。
                    let units =
                        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), length) };
                    Ok(f(StrView::Utf16(units)))
                }
                _ => unreachable!("repr validated above"),
            };
        }

        let bytes = self
            .heap
            .memory()
            .copy_to(address, u64::try_from(byte_length).expect("usize always fits u64"))
            .map_err(HeapAccessV2Error::Memory)?;
        let _scope = self.string_read_scope();
        match repr {
            constants::STRING_REPR_LATIN1_FLAT => Ok(f(StrView::Latin1(&bytes[..length]))),
            constants::STRING_REPR_UTF16_FLAT | constants::STRING_REPR_BUILDER => {
                let (pairs, remainder) = bytes.as_chunks::<2>();
                debug_assert!(remainder.is_empty());
                let units = pairs
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect::<Vec<_>>();
                Ok(f(StrView::Utf16(&units)))
            }
            _ => unreachable!("repr validated above"),
        }
    }

    /// 读取字符串 UTF-16 码元；Latin-1 在读取作用域内展开为 owned 码元。
    pub fn with_string_units<R>(
        &self,
        handle: u32,
        f: impl FnOnce(&[u16]) -> R,
    ) -> Result<R, HeapAccessV2Error> {
        self.with_string_bytes(handle, |view| {
            if let Some(units) = view.as_utf16() {
                f(units)
            } else {
                let units = view.to_utf16();
                f(&units)
            }
        })
    }

    fn direct_string_access_safe(&self) -> bool {
        match &self.barrier {
            HeapBarrier::Disabled => true,
            HeapBarrier::Zgc(barrier) => {
                let epoch = barrier.epoch();
                !epoch.young_marking
                    && !epoch.old_marking
                    && barrier.access_epoch().is_multiple_of(2)
            }
        }
    }

    /// 读整个 payload（`capacity` 字节）；测试与宿主迁移期读取原始字节用。
    pub fn read_string_payload(&self, handle: u32) -> Result<Vec<u8>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let capacity = self.string_capacity_at(object)?;
        self.heap
            .memory()
            .copy_to(
                HeapAddress::new(self.string_payload_address(object)?),
                u64::from(capacity),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 写 payload 区间 `[offset, offset + bytes.len())`（Flat/Builder 的原始字节）。
    pub fn write_string_payload(
        &self,
        handle: u32,
        offset: u32,
        bytes: &[u8],
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let capacity = self.string_capacity_at(object)?;
        if offset
            .checked_add(bytes.len() as u32)
            .is_none_or(|end| end > capacity)
        {
            return Err(HeapAccessV2Error::StringPayloadOverflow {
                offset,
                len: bytes.len() as u32,
                capacity,
            });
        }
        let payload = self.string_payload_address(object)?;
        self.heap
            .memory()
            .copy_from(HeapAddress::new(payload + u64::from(offset)), bytes)
            .map_err(HeapAccessV2Error::Memory)
    }

    fn string_payload_address(&self, object: u64) -> Result<u64, HeapAccessV2Error> {
        object
            .checked_add(u64::from(constants::HEAP_STRING_PAYLOAD_OFFSET))
            .ok_or(HeapAccessV2Error::AddressOverflow)
    }

    /// 把子引用写进 Cons 节点：两个子句柄各占一个独立 8 字节槽，都走
    /// `store_reference`（写屏障 + remset），与数组元素同规则。
    pub fn set_cons_children(
        &self,
        handle: u32,
        left: u32,
        right: u32,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.string_repr_at(object)? != constants::STRING_REPR_CONS {
            return Err(HeapAccessV2Error::NotAStringRepr {
                handle,
                repr: self.string_repr_at(object)?,
                expected: constants::STRING_REPR_CONS,
            });
        }
        let left_slot = self.string_payload_address(object)?
            + u64::from(constants::HEAP_STRING_CONS_LEFT_OFFSET);
        self.store_reference(handle, left_slot, value::encode_object_handle(left) as u64)?;
        // 首次写入可能经 ZGC assist 触发搬迁，第二次写入前重新解析地址。
        let object = self.resolve_handle(handle)?;
        let right_slot = self.string_payload_address(object)?
            + u64::from(constants::HEAP_STRING_CONS_RIGHT_OFFSET);
        self.store_reference(handle, right_slot, value::encode_object_handle(right) as u64)
    }

    /// 读 Cons 节点两个子句柄；非 Cons 返回 `None`。
    pub fn cons_children(&self, handle: u32) -> Result<Option<(u32, u32)>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.string_repr_at(object)? != constants::STRING_REPR_CONS {
            return Ok(None);
        }
        let left = self.load_string_child(object, constants::HEAP_STRING_CONS_LEFT_OFFSET)?;
        let right = self.load_string_child(object, constants::HEAP_STRING_CONS_RIGHT_OFFSET)?;
        Ok(Some((left, right)))
    }

    /// 写 Slice 节点：base 句柄走 `store_reference`；start/end 打包进一个 word
    /// （低 32 位 start、高 32 位 end），两者都非引用，走搬迁同步的整 word 写。
    pub fn set_slice_parts(
        &self,
        handle: u32,
        base: u32,
        start: u32,
        end: u32,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.string_repr_at(object)? != constants::STRING_REPR_SLICE {
            return Err(HeapAccessV2Error::NotAStringRepr {
                handle,
                repr: self.string_repr_at(object)?,
                expected: constants::STRING_REPR_SLICE,
            });
        }
        let base_slot = self.string_payload_address(object)?
            + u64::from(constants::HEAP_STRING_SLICE_BASE_OFFSET);
        self.store_reference(handle, base_slot, value::encode_object_handle(base) as u64)?;
        self.store_header_word(
            handle,
            constants::HEAP_STRING_PAYLOAD_OFFSET + constants::HEAP_STRING_SLICE_RANGE_OFFSET,
            u64::from(start) | (u64::from(end) << 32),
        )
    }

    /// 读 Slice 节点 `(base, start, end)`；非 Slice 返回 `None`。
    pub fn slice_parts(&self, handle: u32) -> Result<Option<(u32, u32, u32)>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.string_repr_at(object)? != constants::STRING_REPR_SLICE {
            return Ok(None);
        }
        let base = self.load_string_child(object, constants::HEAP_STRING_SLICE_BASE_OFFSET)?;
        let range = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                self.string_payload_address(object)?
                    + u64::from(constants::HEAP_STRING_SLICE_RANGE_OFFSET),
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok(Some((base, range as u32, (range >> 32) as u32)))
    }

    /// 读字符串节点 payload 区的子引用句柄；存储值经 GC 上色，读取时剥离颜色。
    fn load_string_child(&self, object: u64, payload_offset: u32) -> Result<u32, HeapAccessV2Error> {
        let stored = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                self.string_payload_address(object)? + u64::from(payload_offset),
            ))
            .map_err(HeapAccessV2Error::Memory)? as i64;
        Ok(value::strip_gc_color(stored) as u32)
    }

    /// 覆写 header/载荷区的单 word 字段；ZGC 下同步写入搬迁目的地与最终地址，
    /// 与 `write_shape_id` 同一套搬迁一致性协议。
    fn store_header_word(&self, handle: u32, offset: u32, value: u64) -> Result<(), HeapAccessV2Error> {
        let owner = self.resolve_handle(handle)?;
        let address = owner
            .checked_add(u64::from(offset))
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        self.heap
            .memory()
            .store_word(HeapAddress::new(address), value)
            .map_err(HeapAccessV2Error::Memory)?;
        if let HeapBarrier::Zgc(barrier) = &self.barrier
            && let Some(descriptor) = barrier.relocator().descriptor(HandleId::new(handle))
            && descriptor.source == owner
        {
            let destination_slot = descriptor
                .destination
                .checked_add(u64::from(offset))
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .memory()
                .store_word(HeapAddress::new(destination_slot), value)
                .map_err(HeapAccessV2Error::Memory)?;
        }
        let final_owner = self.resolve_handle(handle)?;
        if final_owner != owner {
            let final_slot = final_owner
                .checked_add(u64::from(offset))
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .memory()
                .store_word(HeapAddress::new(final_slot), value)
                .map_err(HeapAccessV2Error::Memory)?;
        }
        Ok(())
    }

    // ── 字符串增长 / 搬迁 ────────────────────────────────────────────────────
    /// 把字符串 payload 容量扩到至少 `needed` 字节（Flat/Builder 的原始数据缓冲；
    /// Cons/Slice 的子引用区大小固定，调用方不应扩容）。通过 handle table 原子发布新地址。
    pub fn grow_string_capacity(&self, handle: u32, needed: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let repr = self.string_repr_at(object)?;
        let capacity = self.string_capacity_at(object)?;
        if needed <= capacity {
            return Ok(());
        }
        match repr {
            constants::STRING_REPR_CONS | constants::STRING_REPR_SLICE => {
                return Err(HeapAccessV2Error::FixedSizeStringPayload { handle });
            }
            _ => {}
        }
        let grown = capacity.saturating_mul(2).max(8).max(needed);
        let new_capacity = (grown + 7) & !7;
        if new_capacity <= capacity {
            return Err(HeapAccessV2Error::AddressOverflow);
        }
        self.relocate_string(handle, new_capacity)
    }

    fn relocate_string(&self, handle: u32, new_capacity: u32) -> Result<(), HeapAccessV2Error> {
        let old_object = self.resolve_handle(handle)?;
        let old_capacity = self.string_capacity_at(old_object)?;
        if new_capacity <= old_capacity {
            return Err(HeapAccessV2Error::StringCapacityExceeded {
                handle,
                capacity: new_capacity,
            });
        }
        let old_bytes = string_payload_bytes(old_capacity)?;
        let new_bytes = string_payload_bytes(new_capacity)?;
        let generation = self
            .handle_generation(handle)
            .ok_or(HeapAccessV2Error::UnresolvedHandle { handle })?;
        let destination = self.reserve_exact(new_bytes)?;
        self.heap
            .memory()
            .copy_atomic_words(
                HeapAddress::new(old_object),
                HeapAddress::new(destination),
                old_bytes,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        // 新扩容区清零（header 与 payload 都是 8 对齐，按 word 写零）。
        let mut offset = old_bytes;
        while offset < new_bytes {
            self.heap
                .memory()
                .store_word(HeapAddress::new(destination + offset), 0)
                .map_err(HeapAccessV2Error::Memory)?;
            offset += 8;
        }
        // 更新新对象的 capacity（与 length 共享 `+8` word 的高 32 位；
        // `+12` 非 8 对齐不能单独写，且不能覆盖 `+16` gc_word）。
        let shape_word = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                destination + u64::from(constants::HEAP_STRING_LENGTH_OFFSET),
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        self.heap
            .memory()
            .store_word(
                HeapAddress::new(destination + u64::from(constants::HEAP_STRING_LENGTH_OFFSET)),
                (shape_word & u64::from(u32::MAX)) | (u64::from(new_capacity) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.handles
            .begin_relocation(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles
            .complete_relocation(HandleId::new(handle), destination)
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.heap
            .allocator()
            .transfer_mark(
                ObjectRef::new(old_object),
                ObjectRef::new(destination),
                new_bytes,
                generation,
            )
            .map_err(HeapAccessV2Error::Allocator)?;
        if self
            .heap
            .allocator()
            .forget_object_if_present(ObjectRef::new(old_object), old_bytes)
            .map_err(HeapAccessV2Error::Allocator)?
        {
            self.heap
                .allocator()
                .quarantine_allocation(ObjectRef::new(old_object), old_bytes);
        }
        Ok(())
    }

    // ── 字符串内容哈希 ────────────────────────────────────────────────────────
    /// 惰性计算并缓存内容哈希；语义与宿主 `RuntimeString::content_hash` 一致
    /// （进程级随机种子抗碰撞构造、0 表示未计算、真实哈希归一化到非 0）。
    ///
    /// 只支持扁平载荷（Latin1/Utf16 Flat 与 Builder）；Cons/Slice 需先扁平化
    /// （宿主 `with_string_units` 作用域，2.2 落地）后哈希。
    pub fn string_content_hash(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let cached = self
            .heap
            .memory()
            .load_word(HeapAddress::new(
                object + u64::from(constants::HEAP_STRING_HASH_OFFSET),
            ))
            .map_err(HeapAccessV2Error::Memory)? as u32;
        if cached != 0 {
            return Ok(cached);
        }
        let repr = self.string_repr_at(object)?;
        let length = self.string_length_at(object)? as usize;
        let payload = self.string_payload_address(object)?;
        let hash = match repr {
            constants::STRING_REPR_LATIN1_FLAT => {
                let bytes = self
                    .heap
                    .memory()
                    .copy_to(HeapAddress::new(payload), length as u64)
                    .map_err(HeapAccessV2Error::Memory)?;
                compute_string_hash(length, |index| u16::from(bytes[index]))
            }
            constants::STRING_REPR_UTF16_FLAT | constants::STRING_REPR_BUILDER => {
                let bytes = self
                    .heap
                    .memory()
                    .copy_to(HeapAddress::new(payload), (length as u64) * 2)
                    .map_err(HeapAccessV2Error::Memory)?;
                compute_string_hash(length, |index| {
                    u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]])
                })
            }
            _ => {
                return Err(HeapAccessV2Error::StringHashRequiresFlatten { handle });
            }
        };
        self.store_header_word(handle, constants::HEAP_STRING_HASH_OFFSET, u64::from(hash))?;
        Ok(hash)
    }

    /// ZGC 重定位按对象类型选择 header 布局：字符串的 `+24` hash 在发布后仍可能
    /// 被 mutator 惰性写入，必须以 MutableAtomicWord 参与搬迁同步。
    fn string_layout_for(&self, handle: u32) -> Result<HeaderLayout, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_type_at(object)? == u32::from(wjsm_ir::HEAP_TYPE_STRING) {
            Ok(HeaderLayout::STRING)
        } else {
            Ok(HeaderLayout::OBJECT)
        }
    }

    /// 删除自有属性：对象退化为字典 shape，被删属性的值槽清零。
    /// 值槽不回收——其余属性的下标必须保持稳定，否则已发射的 IC 会读错槽。
    pub fn delete_property(
        &self,
        handle: u32,
        key: PropertyKey,
    ) -> Result<bool, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let shape_id = self.shape_id_at(object)?;
        let Some((dictionary_id, (index, span))) = self.shapes.remove_prop(shape_id, key.get())
        else {
            return Ok(true);
        };
        self.write_shape_id(handle, dictionary_id)?;
        self.shapes.invalidate_if_prototype(handle);
        for offset in 0..span {
            self.store_value_slot(handle, object, index + offset, 0)?;
        }
        Ok(true)
    }

    pub fn resolve_handle(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let handle_id = HandleId::new(handle);
        match &self.barrier {
            HeapBarrier::Disabled => self
                .handles
                .resolve(handle_id)
                .map(|entry| entry.address())
                .ok_or(HeapAccessV2Error::UnresolvedHandle { handle }),
            HeapBarrier::Zgc(barrier) => match load_barrier(&self.handles, handle_id) {
                LoadBarrierOutcome::Stable { address, .. } => Ok(address),
                LoadBarrierOutcome::Relocating { .. } => {
                    let participant = self.epoch().register();
                    participant.enter();
                    barrier
                        .relocator()
                        .assist(&self.handles, self.heap.memory(), handle_id)
                        .map_err(HeapAccessV2Error::RelocationAssist)
                }

                LoadBarrierOutcome::Invalid => Err(HeapAccessV2Error::UnresolvedHandle { handle }),
            },
        }
    }
    pub fn store_reference(
        &self,
        owner_handle: u32,
        slot_addr: u64,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        let owner = self.resolve_handle(owner_handle)?;
        let owner_generation =
            self.handle_generation(owner_handle)
                .ok_or(HeapAccessV2Error::UnresolvedHandle {
                    handle: owner_handle,
                })?;
        let target_generation = (value::is_handle_backed_reference(value as i64))
            .then(|| self.handle_generation(value::decode_handle(value as i64)))
            .flatten();
        let mut stored = value;
        if let HeapBarrier::Zgc(barrier) = &self.barrier {
            let epoch = barrier.epoch();
            let old = self
                .heap
                .load_word(HeapAddress::new(slot_addr))
                .map_err(HeapAccessV2Error::Memory)? as i64;
            if (epoch.young_marking || epoch.old_marking) && value::is_handle_backed_reference(old)
            {
                barrier
                    .record(BarrierRecord::Satb(value::strip_gc_color(old)))
                    .map_err(|_| HeapAccessV2Error::BarrierBufferFull)?;
            }
            if (epoch.young_marking || epoch.old_marking)
                && self
                    .heap
                    .allocator()
                    .is_marked(ObjectRef::new(owner), owner_generation)
                    .unwrap_or(false)
                && value::is_handle_backed_reference(value as i64)
            {
                barrier
                    .record(BarrierRecord::Mark(value::strip_gc_color(value as i64)))
                    .map_err(|_| HeapAccessV2Error::BarrierBufferFull)?;
            }
            if owner_generation == HandleGeneration::Old
                && target_generation == Some(HandleGeneration::Young)
            {
                barrier
                    .record(BarrierRecord::RememberedSlot { slot_addr })
                    .map_err(|_| HeapAccessV2Error::BarrierBufferFull)?;
            }
            stored = color_stored_value(epoch, value as i64) as u64;
        }
        self.heap
            .store_word(HeapAddress::new(slot_addr), stored)
            .map_err(HeapAccessV2Error::Memory)?;

        if let HeapBarrier::Zgc(barrier) = &self.barrier
            && let Some(descriptor) = barrier.relocator().descriptor(HandleId::new(owner_handle))
            && descriptor.source == owner
        {
            let offset = slot_addr
                .checked_sub(owner)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            let destination_slot = descriptor
                .destination
                .checked_add(offset)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .store_word(HeapAddress::new(destination_slot), stored)
                .map_err(HeapAccessV2Error::Memory)?;
        }

        let final_owner = self.resolve_handle(owner_handle)?;
        if final_owner != owner {
            let offset = slot_addr
                .checked_sub(owner)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            let final_slot = final_owner
                .checked_add(offset)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .store_word(HeapAddress::new(final_slot), stored)
                .map_err(HeapAccessV2Error::Memory)?;
        }
        Ok(())
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
    pub fn advance_epoch_and_reclaim(&self) -> Result<(usize, usize), HeapAccessV2Error> {
        self.handles.advance_epoch();
        let allocations = self.heap.allocator().take_reclaimable_allocations();
        let allocation_count = allocations.len();
        for (start, bytes) in allocations {
            self.heap
                .allocator()
                .reclaim_quarantined_object(ObjectRef::new(start), bytes)
                .map_err(HeapAccessV2Error::Allocator)?;
        }
        let handle_count = self.handles.reclaim_quarantine();
        Ok((handle_count, allocation_count))
    }

    pub fn finish_relocation_epoch(&self) -> (usize, usize) {
        if let HeapBarrier::Zgc(barrier) = &self.barrier {
            barrier.relocator().epoch_reclaim(&self.handles);
        } else {
            self.handles.advance_epoch();
        }
        let allocations = self.heap.allocator().take_reclaimable_allocations();
        let allocation_count = allocations.len();
        for (start, bytes) in allocations {
            self.heap
                .allocator()
                .reclaim_quarantined_object(ObjectRef::new(start), bytes)
                .expect("quarantined allocation must belong to the managed heap");
        }
        (self.handles.reclaim_quarantine(), allocation_count)
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

    pub fn scan_references(
        &self,
        handle: u32,
        mut visitor: impl FnMut(i64),
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let header = self
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        let prototype = header as u32;
        if prototype != PROTO_NULL_SENTINEL && prototype != handle {
            if prototype & 0x8000_0000 != 0 {
                visitor(value::encode_proxy_handle(prototype & 0x7FFF_FFFF));
            } else {
                visitor(value::encode_object_handle(prototype));
            }
        }
        // 字符串对象：payload 是原始字节或子引用句柄，不是值槽数组。
        if header_heap_type(header) == u32::from(wjsm_ir::HEAP_TYPE_STRING) {
            let repr = self.string_repr_at(object)?;
            match repr {
                // Cons / Slice 的子引用是 handle 编码（低 32 位句柄），
                // 与数组元素同规则；Flat / Builder 的 payload 无引用。
                constants::STRING_REPR_CONS => {
                    let left = self.load_string_child(object, constants::HEAP_STRING_CONS_LEFT_OFFSET)?;
                    let right = self
                        .load_string_child(object, constants::HEAP_STRING_CONS_RIGHT_OFFSET)?;
                    visitor(value::encode_object_handle(left));
                    visitor(value::encode_object_handle(right));
                }
                constants::STRING_REPR_SLICE => {
                    let base = self
                        .load_string_child(object, constants::HEAP_STRING_SLICE_BASE_OFFSET)?;
                    visitor(value::encode_object_handle(base));
                }
                _ => {}
            }
            return Ok(());
        }
        let capacity = if header_heap_type(header) == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
            self.heap
                .memory()
                .load_word(HeapAddress::new(
                    object + constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
                ))
                .map(|word| (word >> 32) as u32)
                .map_err(HeapAccessV2Error::Memory)?
        } else {
            self.value_capacity(object)?
        };
        for index in 0..capacity {
            let encoded = self
                .heap
                .memory()
                .load_word(HeapAddress::new(value_slot_address(object, index)?))
                .map_err(HeapAccessV2Error::Memory)? as i64;
            visitor(encoded);
        }
        Ok(())
    }

    pub fn object_references(&self, handle: u32) -> Result<Vec<i64>, HeapAccessV2Error> {
        let mut references = Vec::new();
        self.scan_references(handle, |encoded| references.push(encoded))?;
        Ok(references)
    }

    pub fn retire_handle(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let bytes = self.object_size(handle)?;
        self.handles
            .retire(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.heap
            .allocator()
            .quarantine_allocation(ObjectRef::new(object), bytes);
        Ok(bytes)
    }

    /// Collector 在 stop-the-world/relocation capability 内搬迁对象。
    fn relocate_object(&self, nlab: &mut Nlab, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let bytes = self.object_size(handle)?;
        let generation = self
            .handle_generation(handle)
            .ok_or(HeapAccessV2Error::UnresolvedHandle { handle })?;
        let destination = self.allocate(nlab, bytes)?.object().offset();
        self.heap
            .memory()
            .copy_atomic_words(
                HeapAddress::new(object),
                HeapAddress::new(destination),
                bytes,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.handles
            .begin_relocation(HandleId::new(handle))
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.handles
            .complete_relocation(HandleId::new(handle), destination)
            .map_err(HeapAccessV2Error::HandleTable)?;
        self.heap
            .allocator()
            .transfer_mark(
                ObjectRef::new(object),
                ObjectRef::new(destination),
                bytes,
                generation,
            )
            .map_err(HeapAccessV2Error::Allocator)?;
        if self
            .heap
            .allocator()
            .forget_object_if_present(ObjectRef::new(object), bytes)
            .map_err(HeapAccessV2Error::Allocator)?
        {
            self.heap
                .allocator()
                .quarantine_allocation(ObjectRef::new(object), bytes);
        }
        Ok(bytes)
    }

    /// 对象与数组的字节数公式统一为 `header + capacity * 8`——两者 payload 同构；
    /// 字符串为 `string header + payload 字节数`（Cons/Slice 的 capacity 是固定
    /// 子引用区大小），三种类型都按 `object_type_at` 分派。
    pub fn object_size(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let object_type = self.object_type_at(object)?;
        if object_type == u32::from(wjsm_ir::HEAP_TYPE_STRING) {
            return string_payload_bytes(self.string_capacity_at(object)?);
        }
        let capacity = if object_type == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
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
            .heap
            .memory()
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
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        self.heap
            .memory()
            .store_word(
                HeapAddress::new(object),
                (header & u64::from(u32::MAX)) | (u64::from(object_type) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn prototype(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        Ok(self
            .heap
            .memory()
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)? as u32)
    }

    pub fn get_property(
        &self,
        handle: u32,
        key: PropertyKey,
    ) -> Result<Option<u64>, HeapAccessV2Error> {
        Ok(self
            .get_property_slot(handle, key)?
            .map(|property| property.value))
    }

    /// 读自有属性槽：shape 查 name_id → 值槽下标，再按数据/accessor 取值。
    pub fn get_property_slot(
        &self,
        handle: u32,
        key: PropertyKey,
    ) -> Result<Option<HeapAccessV2Property>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(None);
        }
        let shape_id = self.shape_id_at(object)?;
        let Some(prop) = self.shapes.lookup(shape_id, key.get()) else {
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
        key: PropertyKey,
        flags: u32,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(());
        }
        let shape_id = self.shape_id_at(object)?;
        let Some(transition) = self.shapes.update_flags(shape_id, key.get(), flags) else {
            return Ok(());
        };
        // flags 收紧不改属性种类时下标不变，无需扩容；改变种类时按 transition 处理。
        self.apply_transition(handle, object, shape_id, transition)
            .map(|_| ())
    }

    pub fn get_property_slot_on_proto_chain(
        &self,
        handle: u32,
        key: PropertyKey,
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
                .heap
                .memory()
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

    /// 属性 IC 回填专用：沿原型链查找首个可放入 CLIF 快路径的属性，并返回
    /// `(holder_handle, value_slot_index, property)`。`value_slot_index` 对数据
    /// 属性是值槽下标，对 accessor 是 getter 槽下标（与 IC 槽的 value_index
    /// 语义一致）。只有普通对象（`HEAP_TYPE_OBJECT`）且 shape 非字典时才可安全
    /// 地从 holder 值槽直接 load；数组/函数等旁挂属性表、proxy 原型、字典
    /// shape 一律返回 `None`（由宿主退化 MEGAMORPHIC）。
    pub fn get_property_slot_on_proto_chain_for_ic(
        &self,
        handle: u32,
        key: PropertyKey,
    ) -> Result<Option<(u32, u32, HeapAccessV2Property)>, HeapAccessV2Error> {
        let mut current = handle;
        loop {
            // 高位标记的 proxy handle 不能 resolve 为 V2 heap 地址。
            if current & 0x8000_0000 != 0 {
                return Ok(None);
            }
            let object = self.resolve_handle(current)?;
            let header = self
                .heap
                .memory()
                .load_word(HeapAddress::new(object))
                .map_err(HeapAccessV2Error::Memory)?;
            // 只接受普通对象：数组/函数/promise 等的命名属性在宿主旁挂表中，
            // 直接读值槽会读到错误数据。
            if self.object_type_at(object)? != u32::from(wjsm_ir::HEAP_TYPE_OBJECT) {
                return Ok(None);
            }
            let shape_id = self.shape_id_at(object)?;
            if self.shapes.is_dictionary(shape_id) {
                return Ok(None);
            }
            if let Some(property) = self.shapes.lookup(shape_id, key.get()) {
                let value_slot_index = property.index;
                return self
                    .read_prop(object, &property)
                    .map(|property| Some((current, value_slot_index, property)));
            }
            let prototype = header as u32;
            if prototype == PROTO_NULL_SENTINEL || prototype == current {
                return Ok(None);
            }
            if prototype & 0x8000_0000 != 0 {
                return Ok(None);
            }
            current = prototype;
        }
    }

    pub fn define_accessor_property(
        &self,
        handle: u32,
        key: PropertyKey,
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
        key: PropertyKey,
        getter: u64,
        setter: u64,
        flags: u32,
    ) -> Result<(), HeapAccessV2Error> {
        self.define_property_slot(
            handle,
            key.get(),
            flags | constants::FLAG_IS_ACCESSOR as u32,
            value::encode_undefined() as u64,
            getter,
            setter,
        )
    }

    pub fn define_data_property(
        &self,
        handle: u32,
        key: PropertyKey,
        property_value: u64,
        flags: u32,
    ) -> Result<(), HeapAccessV2Error> {
        self.define_property_slot(
            handle,
            key.get(),
            flags,
            property_value,
            value::encode_undefined() as u64,
            value::encode_undefined() as u64,
        )
    }

    pub fn get_property_on_proto_chain(
        &self,
        handle: u32,
        key: PropertyKey,
    ) -> Result<Option<u64>, HeapAccessV2Error> {
        Ok(self
            .get_property_slot_on_proto_chain(handle, key)?
            .map(|property| property.value))
    }

    /// 写自有属性：命中现有 shape 槽则原地覆写，否则按默认数据属性 flags 定义。
    pub fn set_property(
        &self,
        handle: u32,
        key: PropertyKey,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let shape_id = self.shape_id_at(object)?;
        if let Some(prop) = self.shapes.lookup(shape_id, key.get())
            && !prop.is_accessor()
        {
            return self.store_value_slot(handle, object, prop.index, value);
        }
        self.define_property_slot(
            handle,
            key.get(),
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
            self.store_value_slot(handle, object, transition.index, getter)?;
            self.store_value_slot(handle, object, transition.index + 1, setter)
        } else {
            self.store_value_slot(handle, object, transition.index, property_value)
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
                self.store_value_slot(handle, object, index + offset, 0)?;
            }
        }
        if transition.shape_id != old_shape_id {
            self.write_shape_id(handle, transition.shape_id)?;
            self.shapes.invalidate_if_prototype(handle);
        }
        Ok(object)
    }

    fn write_shape_id(&self, handle: u32, shape_id: u32) -> Result<(), HeapAccessV2Error> {
        let owner = self.resolve_handle(handle)?;
        let address = owner + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as u64;
        let word = self
            .heap
            .memory()
            .load_word(HeapAddress::new(address))
            .map_err(HeapAccessV2Error::Memory)?;
        let stored = (word & u64::from(u32::MAX)) | (u64::from(shape_id) << 32);
        self.heap
            .memory()
            .store_word(HeapAddress::new(address), stored)
            .map_err(HeapAccessV2Error::Memory)?;
        if let HeapBarrier::Zgc(barrier) = &self.barrier
            && let Some(descriptor) = barrier.relocator().descriptor(HandleId::new(handle))
            && descriptor.source == owner
        {
            let offset = address
                .checked_sub(owner)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            let destination = descriptor
                .destination
                .checked_add(offset)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .memory()
                .store_word(HeapAddress::new(destination), stored)
                .map_err(HeapAccessV2Error::Memory)?;
        }
        let final_owner = self.resolve_handle(handle)?;
        if final_owner != owner {
            let offset = address
                .checked_sub(owner)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.heap
                .memory()
                .store_word(HeapAddress::new(final_owner + offset), stored)
                .map_err(HeapAccessV2Error::Memory)?;
        }
        Ok(())
    }
    /// 值槽容量（8 字节/槽）。
    fn value_capacity(&self, object: u64) -> Result<u32, HeapAccessV2Error> {
        self.heap
            .memory()
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
        let generation = self
            .handle_generation(handle)
            .ok_or(HeapAccessV2Error::UnresolvedHandle { handle })?;
        let destination = self.reserve_exact(new_bytes)?;
        self.heap
            .memory()
            .copy_atomic_words(
                HeapAddress::new(object),
                HeapAddress::new(destination),
                old_bytes,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        for index in capacity..new_capacity {
            self.store_unpublished_value_slot(destination, index, 0)?;
        }
        let capacity_address = destination + constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET as u64;
        let word = self
            .heap
            .memory()
            .load_word(HeapAddress::new(capacity_address))
            .map_err(HeapAccessV2Error::Memory)?;
        self.heap
            .memory()
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
        self.heap
            .allocator()
            .transfer_mark(
                ObjectRef::new(object),
                ObjectRef::new(destination),
                new_bytes,
                generation,
            )
            .map_err(HeapAccessV2Error::Allocator)?;
        if self
            .heap
            .allocator()
            .forget_object_if_present(ObjectRef::new(object), old_bytes)
            .map_err(HeapAccessV2Error::Allocator)?
        {
            self.heap
                .allocator()
                .quarantine_allocation(ObjectRef::new(object), old_bytes);
        }
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
        self.heap
            .memory()
            .load_word(HeapAddress::new(value_slot_address(object, index)?))
            .map(|stored| value::strip_gc_color(stored as i64) as u64)
            .map_err(HeapAccessV2Error::Memory)
    }

    fn store_value_slot(
        &self,
        handle: u32,
        object: u64,
        index: u32,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        self.store_reference(handle, value_slot_address(object, index)?, value)
    }

    fn store_unpublished_value_slot(
        &self,
        object: u64,
        index: u32,
        value: u64,
    ) -> Result<(), HeapAccessV2Error> {
        self.heap
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
    pub fn scan_references(
        &self,
        handle: u32,
        visitor: impl FnMut(i64),
    ) -> Result<(), HeapAccessV2Error> {
        self.heap.scan_references(handle, visitor)
    }
    pub fn generation_bytes(&self) -> Result<(u64, u64), HeapAccessV2Error> {
        self.heap.generation_bytes()
    }

    pub fn promote(&self, handle: u32) -> Result<(), HeapAccessV2Error> {
        self.heap.promote_to_old(handle)
    }

    pub fn relocate(&self, nlab: &mut Nlab, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.heap.relocate_object(nlab, handle)
    }

    pub fn retire(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.heap.retire_handle(handle)
    }

    pub fn advance_epoch_and_reclaim(&self) -> Result<(usize, usize), HeapAccessV2Error> {
        self.heap.advance_epoch_and_reclaim()
    }

    pub fn free_bytes(&self) -> u64 {
        self.heap.free_bytes()
    }

    pub fn used_bytes(&self) -> u64 {
        self.heap.used_bytes()
    }
}

/// 从 header word 提取 heap_type。
///
/// header 首 8 字节是一个 word：低 32 位 proto handle、`+4` heap_type（**单字节**）、
/// `+5..8` pad（数组 ElementsKind 占 `+5`）。因此类型必须按字节掩码提取——
/// 整取 `header >> 32` 会把 kind 字节混进类型值。
fn header_heap_type(header: u64) -> u32 {
    ((header >> (constants::HEAP_OBJECT_TYPE_OFFSET * 8)) & 0xFF) as u32
}

/// 值槽地址：`object + 24 + index * 8`，与数组元素同一套公式。
pub fn value_slot_address(object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
    object
        .checked_add(constants::HEAP_OBJECT_HEADER_SIZE as u64)
        .and_then(|base| {
            base.checked_add(u64::from(index) * u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        })
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

/// 对象/数组字节数：`24 + capacity * 8`。
pub fn object_payload_bytes(capacity: u32) -> Result<u64, HeapAccessV2Error> {
    u64::from(capacity)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .and_then(|slots| slots.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE)))
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

/// 字符串字节数：`32 + capacity`；Cons/Slice 的 capacity 是固定子引用区大小。
pub fn string_payload_bytes(capacity: u32) -> Result<u64, HeapAccessV2Error> {
    u64::from(capacity)
        .checked_add(u64::from(constants::HEAP_STRING_HEADER_SIZE))
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

/// 进程级哈希种子：与宿主 `wjsm-host::runtime_string` 同一来源（`RandomState`），
/// 使内容哈希不可被外部构造碰撞。字符串归入 ManagedHeap 后哈希由本层唯一计算，
/// 种子语义必须与宿主保持同构，迁移期才不会出现同内容不同哈希。
fn string_hash_seed() -> u32 {
    static SEED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = std::hash::RandomState::new().build_hasher();
        hasher.write_u8(0);
        (hasher.finish() as u32) | 1
    })
}

/// UTF-16 码元序列内容哈希；算法与宿主 `RuntimeString` 的 `compute_hash` 完全一致
/// （每 8 码元两组打包进双累加器，murmur 风格收尾），保证堆内字符串与宿主侧
/// 旧表示的哈希值语义等价。按码元下标读取（Latin-1 逐字节、UTF-16 逐字），
/// 闭包在编译期内联；哈希惰性只算一次，非热路径。
fn compute_string_hash(len: usize, mut read_unit: impl FnMut(usize) -> u16) -> u32 {
    const K1: u64 = 0xff51_afd7_ed55_8ccd;
    const K2: u64 = 0xc4ce_b9fe_1a85_ec53;

    let mut left = u64::from(string_hash_seed()) ^ (len as u64).wrapping_mul(K1);
    let mut right = K2;
    let full_chunks = len / 8;
    for chunk in 0..full_chunks {
        let base = chunk * 8;
        let mut low = 0_u64;
        let mut high = 0_u64;
        for j in 0..4 {
            low |= u64::from(read_unit(base + j)) << (j * 16);
        }
        for j in 0..4 {
            high |= u64::from(read_unit(base + 4 + j)) << (j * 16);
        }
        left = (left ^ low).wrapping_mul(K1).rotate_left(31);
        right = (right ^ high).wrapping_mul(K2).rotate_left(29);
    }
    let mut tail = 0_u64;
    let mut tail_units = 0;
    for index in (full_chunks * 8)..len {
        tail |= u64::from(read_unit(index)) << ((tail_units % 4) * 16);
        tail_units += 1;
        if tail_units % 4 == 0 {
            left = (left ^ tail).wrapping_mul(K1).rotate_left(31);
            tail = 0;
        }
    }
    right ^= tail;

    let mut mixed = left ^ right.wrapping_mul(K1);
    mixed ^= mixed >> 33;
    mixed = mixed.wrapping_mul(K2);
    mixed ^= mixed >> 29;
    let hash = (mixed as u32) ^ ((mixed >> 32) as u32);
    if hash == 0 { 1 } else { hash }
}

/// 数组元素地址；与对象值槽同构，故直接复用同一公式。
fn array_element_address(object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
    value_slot_address(object, index)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapAccessV2Error {
    AddressOverflow,
    BarrierHandleTableMismatch,
    BarrierBufferFull,
    Allocator(AllocatorError),
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
    RestoredGcWordHandleMismatch {
        object: u64,
        expected: u32,
        actual: u32,
    },
    RelocationAssist(String),
    /// 原型链下一环是高位标记的 Proxy handle，需 host 走 trap。
    ProxyPrototype {
        handle: u32,
    },
    /// 数组对象没有属性槽（offset 8/12 与 length/元素容量别名）；
    /// 命名属性必须经宿主 `ArrayNamedPropsStore` 侧表。
    ArrayPropertySlots {
        handle: u32,
    },
    /// payload 容量未按 8 对齐（publish_string 的前置条件）。
    InvalidStringCapacity {
        capacity: u32,
    },
    /// 未知字符串 repr 编码（header `+5`）。
    InvalidStringRepr {
        repr: u8,
    },
    /// 写 payload 越界（offset + len > capacity）。
    StringPayloadOverflow {
        offset: u32,
        len: u32,
        capacity: u32,
    },
    /// 操作与字符串 repr 不符（如对 Flat 调 set_cons_children）。
    NotAStringRepr {
        handle: u32,
        repr: u8,
        expected: u8,
    },
    /// Cons/Slice 的子引用区大小固定，不支持扩容。
    FixedSizeStringPayload {
        handle: u32,
    },
    /// 扩容目标容量不大于当前容量。
    StringCapacityExceeded {
        handle: u32,
        capacity: u32,
    },
    /// 内容哈希需要扁平载荷；Cons/Slice 须先经宿主作用域扁平化。
    StringHashRequiresFlatten {
        handle: u32,
    },
    /// Cons/Slice 不是扁平载荷，读取前必须先展平。
    StringFlattenRequired {
        handle: u32,
    },
}

impl fmt::Display for HeapAccessV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressOverflow => formatter.write_str("V2 heap address overflows u64"),
            Self::BarrierHandleTableMismatch => {
                formatter.write_str("ZGC barrier and HeapAccessV2 use different handle tables")
            }
            Self::Allocator(error) => error.fmt(formatter),
            Self::ElementCapacityExceeded {
                handle,
                index,
                capacity,
            } => write!(
                formatter,
                "V2 array handle {handle} index {index} exceeds capacity {capacity}"
            ),
            Self::BarrierBufferFull => formatter.write_str("ZGC barrier buffer is full"),
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
            Self::RestoredGcWordHandleMismatch {
                object,
                expected,
                actual,
            } => write!(
                formatter,
                "restored object {object:#x} GC word has handle {actual}, expected {expected}"
            ),
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
            Self::RelocationAssist(error) => {
                write!(formatter, "ZGC relocation assist failed: {error}")
            }
            Self::ProxyPrototype { handle } => {
                write!(formatter, "proxy prototype handle {handle:#x}")
            }
            Self::ArrayPropertySlots { handle } => write!(
                formatter,
                "V2 array handle {handle} has no property slots; named props live in the host side table"
            ),
            Self::InvalidStringCapacity { capacity } => write!(
                formatter,
                "string payload capacity {capacity} is not 8-byte aligned"
            ),
            Self::InvalidStringRepr { repr } => {
                write!(formatter, "unknown string repr encoding {repr}")
            }
            Self::StringPayloadOverflow {
                offset,
                len,
                capacity,
            } => write!(
                formatter,
                "string payload write offset {offset} + {len} bytes exceeds capacity {capacity}"
            ),
            Self::NotAStringRepr {
                handle,
                repr,
                expected,
            } => write!(
                formatter,
                "string handle {handle} has repr {repr}, expected {expected}"
            ),
            Self::FixedSizeStringPayload { handle } => write!(
                formatter,
                "string handle {handle} has a fixed-size payload (Cons/Slice) and cannot grow"
            ),
            Self::StringCapacityExceeded { handle, capacity } => write!(
                formatter,
                "string handle {handle} cannot shrink to capacity {capacity}"
            ),
            Self::StringHashRequiresFlatten { handle } => write!(
                formatter,
                "string handle {handle} must be flattened before hashing (Cons/Slice)"
            ),
            Self::StringFlattenRequired { handle } => write!(
                formatter,
                "string handle {handle} must be flattened before reading (Cons/Slice)"
            ),
        }
    }
}

impl Error for HeapAccessV2Error {}
