use crate::{MapEntry, RuntimeState, SetEntry};

impl RuntimeState {
    pub(crate) fn alloc_map_entry(&self) -> u32 {
        let mut table = self.map_table.lock().unwrap_or_else(|e| e.into_inner());
        let mut free = self
            .map_free_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while let Some(handle) = free.pop() {
            if let Some(entry) = table.get_mut(handle as usize)
                && entry.owner.is_none()
            {
                *entry = MapEntry::new_unowned();
                return handle;
            }
        }
        let handle = table.len() as u32;
        table.push(MapEntry::new_unowned());
        handle
    }

    pub(crate) fn alloc_set_entry(&self) -> u32 {
        let mut table = self.set_table.lock().unwrap_or_else(|e| e.into_inner());
        let mut free = self
            .set_free_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while let Some(handle) = free.pop() {
            if let Some(entry) = table.get_mut(handle as usize)
                && entry.owner.is_none()
            {
                *entry = SetEntry::new_unowned();
                return handle;
            }
        }
        let handle = table.len() as u32;
        table.push(SetEntry::new_unowned());
        handle
    }

    pub(crate) fn bind_map_entry_owner(&self, side_handle: u32, owner_handle: u32) {
        let mut table = self.map_table.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(side_handle as usize) {
            entry.owner = Some(owner_handle);
        }
    }

    pub(crate) fn bind_set_entry_owner(&self, side_handle: u32, owner_handle: u32) {
        let mut table = self.set_table.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(side_handle as usize) {
            entry.owner = Some(owner_handle);
        }
    }

    pub(crate) fn release_unowned_map_entry(&self, side_handle: u32) {
        let mut table = self.map_table.lock().unwrap_or_else(|e| e.into_inner());
        let mut free = self
            .map_free_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(side_handle as usize)
            && entry.owner.is_none()
        {
            entry.clear_for_reuse();
            if !free.contains(&side_handle) {
                free.push(side_handle);
            }
        }
    }

    pub(crate) fn release_unowned_set_entry(&self, side_handle: u32) {
        let mut table = self.set_table.lock().unwrap_or_else(|e| e.into_inner());
        let mut free = self
            .set_free_slots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(side_handle as usize)
            && entry.owner.is_none()
        {
            entry.clear_for_reuse();
            if !free.contains(&side_handle) {
                free.push(side_handle);
            }
        }
    }

    pub(crate) fn reclaim_unmarked_collection_entries(
        &self,
        mut is_marked: impl FnMut(u32) -> bool,
    ) {
        {
            let mut table = self.map_table.lock().unwrap_or_else(|e| e.into_inner());
            let mut free = self
                .map_free_slots
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (idx, entry) in table.iter_mut().enumerate() {
                if let Some(owner) = entry.owner
                    && !is_marked(owner)
                {
                    entry.clear_for_reuse();
                    free.push(idx as u32);
                }
            }
        }
        {
            let mut table = self.set_table.lock().unwrap_or_else(|e| e.into_inner());
            let mut free = self
                .set_free_slots
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (idx, entry) in table.iter_mut().enumerate() {
                if let Some(owner) = entry.owner
                    && !is_marked(owner)
                {
                    entry.clear_for_reuse();
                    free.push(idx as u32);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{RuntimeState, value};

    #[test]
    fn owner_backed_collection_entries_reclaim_and_reuse_slots() {
        let state = RuntimeState::new();
        let map_key = value::encode_object_handle(100);
        let map_value = value::encode_object_handle(101);
        let set_value = value::encode_object_handle(102);

        for _ in 0..128 {
            let map_handle = state.alloc_map_entry();
            assert_eq!(map_handle, 0);
            state.bind_map_entry_owner(map_handle, 10);
            {
                let mut table = state.map_table.lock().unwrap_or_else(|e| e.into_inner());
                let entry = &mut table[map_handle as usize];
                entry.keys.push(map_key);
                entry.values.push(map_value);
            }
            let set_handle = state.alloc_set_entry();
            assert_eq!(set_handle, 0);
            state.bind_set_entry_owner(set_handle, 20);
            {
                let mut table = state.set_table.lock().unwrap_or_else(|e| e.into_inner());
                table[set_handle as usize].values.push(set_value);
            }

            state.reclaim_unmarked_collection_entries(|_| false);
        }

        {
            let table = state.map_table.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(table.len(), 1);
            assert_eq!(table[0].owner, None);
            assert!(table[0].keys.is_empty());
            assert!(table[0].values.is_empty());
        }
        {
            let table = state.set_table.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(table.len(), 1);
            assert_eq!(table[0].owner, None);
            assert!(table[0].values.is_empty());
        }

        let live_map = state.alloc_map_entry();
        state.bind_map_entry_owner(live_map, 10);
        {
            let mut table = state.map_table.lock().unwrap_or_else(|e| e.into_inner());
            table[live_map as usize].values.push(map_value);
        }
        let live_set = state.alloc_set_entry();
        state.bind_set_entry_owner(live_set, 20);
        {
            let mut table = state.set_table.lock().unwrap_or_else(|e| e.into_inner());
            table[live_set as usize].values.push(set_value);
        }
        state.reclaim_unmarked_collection_entries(|h| h == 10 || h == 20);

        assert_eq!(
            state.map_table.lock().unwrap_or_else(|e| e.into_inner())[live_map as usize].owner,
            Some(10)
        );
        assert_eq!(
            state.set_table.lock().unwrap_or_else(|e| e.into_inner())[live_set as usize].owner,
            Some(20)
        );
    }
}
