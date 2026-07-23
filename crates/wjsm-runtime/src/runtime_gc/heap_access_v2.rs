//! memory64 V2 动态 JS 堆的唯一 host 访问入口。

use std::error::Error;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::heap::{HandleGeneration, HandleState, HeapAddress, HeapMemoryError, SharedHeapMemory};
use wjsm_ir::{constants, value};

const PROTO_NULL_SENTINEL: u32 = 0xFFFF_FFFF;

/// V2 dynamic heap 的唯一 host access owner；所有地址均为 memory64 byte offset。
pub struct HeapAccessV2 {
    memory: SharedHeapMemory,
    next_object: AtomicU64,
    heap_limit: u64,
    free_regions: Mutex<Vec<(u64, u64)>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeapAccessV2Property {
    pub flags: u32,
    pub value: u64,
    pub getter: u64,
    pub setter: u64,
}
impl HeapAccessV2 {
    pub fn new(memory: SharedHeapMemory) -> Self {
        let heap_limit = memory.maximum_byte_len();
        Self::with_heap_limit(memory, heap_limit)
    }

    /// 使用显式逻辑堆上限（`object_start + max_heap_size`），可小于 shared memory64 的页对齐 maximum。
    pub fn with_heap_limit(memory: SharedHeapMemory, heap_limit: u64) -> Self {
        let next_object = crate::heap::HANDLE_REGION_BYTES + 64 * 1024;
        let heap_limit = heap_limit.max(next_object).min(memory.maximum_byte_len());
        Self {
            memory,
            next_object: AtomicU64::new(next_object),
            heap_limit,
            free_regions: Mutex::new(Vec::new()),
        }
    }

    pub fn reserve_nlab(&self, minimum_bytes: u64) -> Result<(u64, u64), HeapAccessV2Error> {
        let minimum_bytes = minimum_bytes
            .checked_add(7)
            .map(|bytes| bytes & !7)
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        if let Some(region) = self.take_free_region(minimum_bytes) {
            return Ok(region);
        }
        // 优先预留至少 64KiB，但绝不超过 remaining（小 max_heap_size 时必须能精确 OOM）。
        let preferred_bytes = minimum_bytes.max(64 * 1024);
        loop {
            let start = self.next_object.load(Ordering::Acquire);
            let remaining = self.heap_limit.saturating_sub(start);
            if minimum_bytes > remaining {
                return Err(HeapAccessV2Error::HeapExhausted {
                    requested: minimum_bytes,
                    heap_limit: self.heap_limit,
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
        let index = regions
            .iter()
            .position(|(start, end)| end.saturating_sub(*start) >= minimum_bytes)?;
        let (start, end) = regions.remove(index);
        let allocation_end = start + minimum_bytes;
        if allocation_end < end {
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
            .saturating_sub(crate::heap::HANDLE_REGION_BYTES + 64 * 1024)
    }
    pub fn heap_limit_bytes(&self) -> u64 {
        self.heap_limit
    }

    pub fn publish_object(
        &self,
        handle: u32,
        object: u64,
        prototype: u32,
        capacity: u32,
    ) -> Result<(), HeapAccessV2Error> {
        if object < crate::heap::HANDLE_REGION_BYTES || object & 7 != 0 || object >> 48 != 0 {
            return Err(HeapAccessV2Error::InvalidObjectAddress { object });
        }
        let mut header = [0_u8; constants::HEAP_OBJECT_HEADER_SIZE as usize];
        header[constants::HEAP_OBJECT_PROTO_OFFSET as usize..][..4]
            .copy_from_slice(&prototype.to_le_bytes());
        header[constants::HEAP_OBJECT_CAPACITY_OFFSET as usize..][..4]
            .copy_from_slice(&capacity.to_le_bytes());
        self.memory
            .copy_from(HeapAddress::new(object), &header)
            .map_err(HeapAccessV2Error::Memory)?;
        let entry = (object << 16) | u64::from(crate::heap::HandleState::StableYoung as u16);
        self.memory
            .store_word(HeapAddress::new(u64::from(handle) * 8), entry)
            .map_err(HeapAccessV2Error::Memory)
    }

    pub fn set_prototype(&self, handle: u32, prototype: u32) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
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
            return Err(HeapAccessV2Error::ElementCapacityExceeded {
                handle,
                index,
                capacity,
            });
        }
        let address = array_element_address(object, index)?;
        self.memory
            .store_word(HeapAddress::new(address), value)
            .map_err(HeapAccessV2Error::Memory)?;
        if index >= length {
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

    pub fn relocate_array(
        &self,
        handle: u32,
        new_object: u64,
        new_capacity: u32,
    ) -> Result<(), HeapAccessV2Error> {
        if new_object < crate::heap::HANDLE_REGION_BYTES
            || new_object & 7 != 0
            || new_object >> 48 != 0
        {
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
        let old_entry = self
            .memory
            .load_word(HeapAddress::new(u64::from(handle) * 8))
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(u64::from(handle) * 8),
                (new_object << 16) | (old_entry & u64::from(u16::MAX)),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.release_region(
            old_object,
            u64::from(old_capacity)
                .checked_mul(u64::from(constants::HEAP_ARRAY_ELEMENT_SIZE))
                .and_then(|payload| {
                    payload.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
                })
                .ok_or(HeapAccessV2Error::AddressOverflow)?,
        );
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

    pub fn delete_property(&self, handle: u32, key: u32) -> Result<bool, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let (capacity, count) = self.property_shape(object)?;
        for index in 0..count.min(capacity) {
            let slot = property_slot_address(object, index)?;
            let name = self
                .memory
                .load_word(HeapAddress::new(slot))
                .map_err(HeapAccessV2Error::Memory)? as u32;
            if name == key {
                self.memory
                    .store_word(HeapAddress::new(slot), 0)
                    .map_err(HeapAccessV2Error::Memory)?;
                return Ok(true);
            }
        }
        Ok(true)
    }

    pub fn resolve_handle(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        let entry = self
            .memory
            .load_word(HeapAddress::new(u64::from(handle) * 8))
            .map_err(HeapAccessV2Error::Memory)?;
        let state = (entry & u16::MAX as u64) as u16;
        if state == crate::heap::HandleState::Free as u16
            || state == crate::heap::HandleState::Retired as u16
        {
            return Err(HeapAccessV2Error::UnresolvedHandle { handle });
        }
        Ok(entry >> 16)
    }

    /// 读取 handle entry 的世代（Free/Retired 返回 None）。
    pub fn handle_generation(&self, handle: u32) -> Option<HandleGeneration> {
        let entry = self
            .memory
            .load_word(HeapAddress::new(u64::from(handle) * 8))
            .ok()?;
        let state = HandleState::from_raw((entry & u16::MAX as u64) as u16)?;
        state.generation()
    }

    /// 将 StableYoung 晋升为 StableOld（失败时保留原状态）。
    pub fn promote_to_old(&self, handle: u32) -> Result<(), HeapAccessV2Error> {
        let entry = self
            .memory
            .load_word(HeapAddress::new(u64::from(handle) * 8))
            .map_err(HeapAccessV2Error::Memory)?;
        let state = (entry & u16::MAX as u64) as u16;
        if state != HandleState::StableYoung as u16 {
            return Ok(());
        }
        let object = entry >> 16;
        let next = (object << 16) | u64::from(HandleState::StableOld as u16);
        self.memory
            .store_word(HeapAddress::new(u64::from(handle) * 8), next)
            .map_err(HeapAccessV2Error::Memory)
    }

    /// 供 active ZGC 构图使用的对象字节数。
    pub fn object_size_public(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        self.object_size(handle)
    }
    pub fn live_handles(&self, count: u32) -> Vec<u32> {
        (0..count)
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
        self.memory
            .store_word(HeapAddress::new(u64::from(handle) * 8), 0)
            .map_err(HeapAccessV2Error::Memory)?;
        self.release_region(object, bytes);
        Ok(bytes)
    }

    fn object_size(&self, handle: u32) -> Result<u64, HeapAccessV2Error> {
        if self.object_type(handle)? == u32::from(wjsm_ir::HEAP_TYPE_ARRAY) {
            let (_, capacity) = self.array_shape(handle)?;
            return u64::from(capacity)
                .checked_mul(u64::from(constants::HEAP_ARRAY_ELEMENT_SIZE))
                .and_then(|payload| {
                    payload.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE))
                })
                .ok_or(HeapAccessV2Error::AddressOverflow);
        }
        let object = self.resolve_handle(handle)?;
        let (capacity, _) = self.property_shape(object)?;
        object_property_bytes(capacity)
    }

    pub fn object_type(&self, handle: u32) -> Result<u32, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        Ok((self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?
            >> 32) as u32)
    }

    /// 数组的 offset 8/12 是 length/元素容量，与对象属性头（capacity/count）
    /// 布局别名；own 属性槽操作绝不能作用于数组对象——数组命名属性由宿主
    /// `ArrayNamedPropsStore` 侧表承载（与 V1 support 模块语义一致）。
    fn object_at_is_array(&self, object: u64) -> Result<bool, HeapAccessV2Error> {
        let header = self
            .memory
            .load_word(HeapAddress::new(object))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok((header >> 32) as u32 == u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
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

    pub fn get_property_slot(
        &self,
        handle: u32,
        key: u32,
    ) -> Result<Option<HeapAccessV2Property>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(None);
        }
        let (capacity, count) = self.property_shape(object)?;
        for index in 0..count.min(capacity) {
            let slot = property_slot_address(object, index)?;
            let name_and_flags = self
                .memory
                .load_word(HeapAddress::new(slot))
                .map_err(HeapAccessV2Error::Memory)?;
            if name_and_flags as u32 == key {
                return self.read_property_slot(slot).map(Some);
            }
        }
        Ok(None)
    }

    pub fn own_property_slots(&self, handle: u32) -> Result<Vec<(u32, u32)>, HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Ok(Vec::new());
        }
        let (capacity, count) = self.property_shape(object)?;
        let mut slots = Vec::with_capacity(count.min(capacity) as usize);
        for index in 0..count.min(capacity) {
            let slot = property_slot_address(object, index)?;
            let name_and_flags = self
                .memory
                .load_word(HeapAddress::new(slot))
                .map_err(HeapAccessV2Error::Memory)?;
            let key = name_and_flags as u32;
            if key != 0 {
                slots.push((key, (name_and_flags >> 32) as u32));
            }
        }
        Ok(slots)
    }
    /// 覆写已存在属性槽的 flags（seal/freeze 等描述符收紧路径）。
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
        let (capacity, count) = self.property_shape(object)?;
        for index in 0..count.min(capacity) {
            let slot = property_slot_address(object, index)?;
            let name = self
                .memory
                .load_word(HeapAddress::new(slot))
                .map_err(HeapAccessV2Error::Memory)? as u32;
            if name == key {
                return self
                    .memory
                    .store_word(
                        HeapAddress::new(slot),
                        u64::from(key) | (u64::from(flags) << 32),
                    )
                    .map_err(HeapAccessV2Error::Memory);
            }
        }
        Ok(())
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
            let object_type = (header >> 32) as u32;
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

    pub fn set_property(&self, handle: u32, key: u32, value: u64) -> Result<(), HeapAccessV2Error> {
        let object = self.resolve_handle(handle)?;
        if self.object_at_is_array(object)? {
            return Err(HeapAccessV2Error::ArrayPropertySlots { handle });
        }
        let (capacity, count) = self.property_shape(object)?;
        for index in 0..count.min(capacity) {
            let slot = property_slot_address(object, index)?;
            let name = self
                .memory
                .load_word(HeapAddress::new(slot))
                .map_err(HeapAccessV2Error::Memory)? as u32;
            if name == key {
                return self.store_property_value(slot, value);
            }
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
        let (capacity, count) = self.property_shape(object)?;
        for index in 0..count.min(capacity) {
            let slot = property_slot_address(object, index)?;
            let name = self
                .memory
                .load_word(HeapAddress::new(slot))
                .map_err(HeapAccessV2Error::Memory)? as u32;
            if name == key {
                return self.write_property_slot(slot, key, flags, property_value, getter, setter);
            }
        }
        if count == capacity {
            self.grow_object_property_capacity(handle, object, capacity, count)?;
            return self.define_property_slot(handle, key, flags, property_value, getter, setter);
        }
        let slot = property_slot_address(object, count)?;
        self.write_property_slot(slot, key, flags, property_value, getter, setter)?;
        self.memory
            .store_word(
                HeapAddress::new(object + constants::HEAP_OBJECT_CAPACITY_OFFSET as u64),
                u64::from(capacity) | (u64::from(count + 1) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    fn property_shape(&self, object: u64) -> Result<(u32, u32), HeapAccessV2Error> {
        let shape = self
            .memory
            .load_word(HeapAddress::new(
                object + constants::HEAP_OBJECT_CAPACITY_OFFSET as u64,
            ))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok((shape as u32, (shape >> 32) as u32))
    }

    fn grow_object_property_capacity(
        &self,
        handle: u32,
        object: u64,
        capacity: u32,
        count: u32,
    ) -> Result<(), HeapAccessV2Error> {
        let minimum = count
            .checked_add(1)
            .ok_or(HeapAccessV2Error::AddressOverflow)?;
        let new_capacity = capacity.saturating_mul(2).max(4).max(minimum);
        if new_capacity == capacity {
            return Err(HeapAccessV2Error::AddressOverflow);
        }
        let old_bytes = object_property_bytes(capacity)?;
        let new_bytes = object_property_bytes(new_capacity)?;
        let (destination, _) = self.reserve_nlab(new_bytes)?;
        let contents = self
            .memory
            .copy_to(HeapAddress::new(object), old_bytes)
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .copy_from(HeapAddress::new(destination), &contents)
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(destination + constants::HEAP_OBJECT_CAPACITY_OFFSET as u64),
                u64::from(new_capacity) | (u64::from(count) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        let entry_address = u64::from(handle) * 8;
        let entry = self
            .memory
            .load_word(HeapAddress::new(entry_address))
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(entry_address),
                (destination << 16) | (entry & 0xFFFF),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.release_region(object, old_bytes);
        Ok(())
    }

    fn read_property_slot(&self, slot: u64) -> Result<HeapAccessV2Property, HeapAccessV2Error> {
        let name_and_flags = self
            .memory
            .load_word(HeapAddress::new(slot))
            .map_err(HeapAccessV2Error::Memory)?;
        Ok(HeapAccessV2Property {
            flags: (name_and_flags >> 32) as u32,
            value: self
                .memory
                .load_word(HeapAddress::new(
                    slot + constants::PROP_SLOT_VALUE_OFFSET as u64,
                ))
                .map_err(HeapAccessV2Error::Memory)?,
            getter: self
                .memory
                .load_word(HeapAddress::new(
                    slot + constants::PROP_SLOT_GETTER_OFFSET as u64,
                ))
                .map_err(HeapAccessV2Error::Memory)?,
            setter: self
                .memory
                .load_word(HeapAddress::new(
                    slot + constants::PROP_SLOT_SETTER_OFFSET as u64,
                ))
                .map_err(HeapAccessV2Error::Memory)?,
        })
    }

    fn write_property_slot(
        &self,
        slot: u64,
        key: u32,
        flags: u32,
        property_value: u64,
        getter: u64,
        setter: u64,
    ) -> Result<(), HeapAccessV2Error> {
        self.memory
            .store_word(
                HeapAddress::new(slot),
                u64::from(key) | (u64::from(flags) << 32),
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.store_property_value(slot, property_value)?;
        self.memory
            .store_word(
                HeapAddress::new(slot + constants::PROP_SLOT_GETTER_OFFSET as u64),
                getter,
            )
            .map_err(HeapAccessV2Error::Memory)?;
        self.memory
            .store_word(
                HeapAddress::new(slot + constants::PROP_SLOT_SETTER_OFFSET as u64),
                setter,
            )
            .map_err(HeapAccessV2Error::Memory)
    }

    fn store_property_value(&self, slot: u64, value: u64) -> Result<(), HeapAccessV2Error> {
        self.memory
            .store_word(
                HeapAddress::new(slot + constants::PROP_SLOT_VALUE_OFFSET as u64),
                value,
            )
            .map_err(HeapAccessV2Error::Memory)
    }
}

fn property_slot_address(object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
    object
        .checked_add(constants::HEAP_OBJECT_HEADER_SIZE as u64)
        .and_then(|base| base.checked_add(u64::from(index) * constants::PROP_SLOT_SIZE as u64))
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

fn object_property_bytes(capacity: u32) -> Result<u64, HeapAccessV2Error> {
    u64::from(capacity)
        .checked_mul(u64::from(constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE))
        .and_then(|slots| slots.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE)))
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

fn array_element_address(object: u64, index: u32) -> Result<u64, HeapAccessV2Error> {
    object
        .checked_add(constants::HEAP_OBJECT_HEADER_SIZE as u64)
        .and_then(|base| base.checked_add(u64::from(index) * 8))
        .ok_or(HeapAccessV2Error::AddressOverflow)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapAccessV2Error {
    AddressOverflow,
    ElementCapacityExceeded {
        handle: u32,
        index: u32,
        capacity: u32,
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
