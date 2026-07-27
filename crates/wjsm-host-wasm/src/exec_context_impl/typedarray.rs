// ExecContext 方法片段：typedarray
macro_rules! exec_ctx_typedarray {
    () => {
    fn typedarray_resolve(&mut self, this: Value) -> Option<TypedArrayView> {
        let (buf, off, len, esize, kind, shared) =
            crate::host_imports::typedarray_new_methods::ta_resolve(self.caller, this)?;
        Some(TypedArrayView {
            buffer_handle: buf as u32,
            byte_offset: off as u32,
            length: len,
            element_size: esize,
            element_kind: kind,
            is_shared: shared,
        })
    }
    fn typedarray_read_elem(&mut self, view: &TypedArrayView, index: u32) -> Option<Value> {
        if view.is_shared {
            crate::host_imports::typedarray_new_methods::sab_read(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
            )
        } else {
            crate::host_imports::typedarray_new_methods::ta_read(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
            )
        }
    }
    fn typedarray_write_elem(&mut self, view: &TypedArrayView, index: u32, val: Value) {
        if view.is_shared {
            let _ = crate::host_imports::typedarray_new_methods::sab_write(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
                val,
            );
        } else {
            let _ = crate::host_imports::typedarray_new_methods::ta_write(
                self.caller,
                view.buffer_handle as usize,
                view.byte_offset as usize,
                view.element_size,
                view.element_kind,
                index,
                val,
            );
        }
    }
    fn typedarray_table_create(
        &mut self,
        buffer_handle: u32,
        buffer_object: Option<Value>,
        byte_offset: u32,
        length: u32,
        element_size: u8,
        element_kind: u8,
        is_shared: bool,
    ) -> u32 {
        let mut table = self
            .caller
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::TypedArrayEntry {
            buffer_handle,
            buffer_object,
            byte_offset,
            length,
            element_size,
            element_kind,
            is_shared,
        });
        handle
    }
    fn dataview_create(
        &mut self,
        buffer_handle: u32,
        buffer_object: Option<Value>,
        byte_offset: u32,
        byte_length: u32,
        is_shared: bool,
    ) -> Option<u32> {
        let mut table = self
            .caller
            .data()
            .dataview_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::DataViewEntry {
            buffer_handle,
            buffer_object,
            byte_offset,
            byte_length,
            is_shared,
        });
        Some(handle)
    }
    fn dataview_resolve(&mut self, handle: u32) -> Option<(u32, u32, u32, bool)> {
        let table = self
            .caller
            .data()
            .dataview_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let e = table.get(handle as usize)?;
        Some((e.buffer_handle, e.byte_offset, e.byte_length, e.is_shared))
    }
    fn array_push(&mut self, arr: Value, val: Value) -> Value {
        let handle = value::decode_handle(arr);
        match crate::push_v2_array_element(self.caller, handle, val as u64) {
            Ok(length) => value::encode_f64(length as f64),
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 Array.prototype.push: {error}"),
                );
                value::encode_undefined()
            }
        }
    }
    fn array_push_hole(&mut self, arr: Value) -> Value {
        let handle = value::decode_handle(arr);
        match crate::push_v2_array_element(self.caller, handle, value::encode_array_hole() as u64) {
            Ok(length) => value::encode_f64(length as f64),
            Err(error) => {
                crate::set_runtime_error(
                    self.caller.data(),
                    format!("V2 Array hole push: {error}"),
                );
                value::encode_undefined()
            }
        }
    }
    fn resolve_array(&mut self, arr: Value) -> bool {
        crate::resolve_array_ptr(self.caller, arr).is_some()
    }
    fn array_elem_at(&mut self, arr: Value, index: u32) -> Option<Value> {
        let handle = value::decode_handle(arr);
        if self
            .caller
            .data()
            .heap_access_v2()
            .resolve_handle(handle)
            .is_ok()
        {
            return self
                .caller
                .data()
                .heap_access_v2()
                .get_element(handle, index)
                .ok()
                .flatten()
                .map(|v| v as i64)
                .filter(|v| !value::is_array_hole(*v));
        }
        let ptr = crate::resolve_array_ptr(self.caller, arr)?;
        crate::read_array_elem(self.caller, ptr, index)
    }
    fn array_write_hole(&mut self, arr: Value, index: u32) {
        self.array_write_elem(arr, index, value::encode_array_hole());
    }
    fn array_ensure_capacity(&mut self, arr: Value, needed: u32) -> bool {
        let handle = value::decode_handle(arr);
        crate::ensure_v2_array_capacity(self.caller, handle, needed).is_ok()
    }
    fn array_species_create(&mut self, exemplar: Value, length: u32) -> Value {
        crate::runtime_host_helpers::array_species_create(self.caller, exemplar, length)
    }
    fn array_species_create_async<'c>(
        &'c mut self,
        exemplar: Value,
        length: u32,
    ) -> ExecFuture<'c, Value> {
        Box::pin(async move {
            crate::runtime_host_helpers::array_species_create_async(self.caller, exemplar, length)
                .await
        })
    }
    fn array_write_elem(&mut self, arr: Value, index: u32, value: Value) {
        let Some(env) = self.env() else {
            return;
        };
        crate::runtime_host_helpers::set_array_elem_with_env(
            self.caller,
            &env,
            arr,
            index as i32,
            value,
        );
    }
    fn array_read_length(&mut self, arr: Value) -> Option<u32> {
        let env = self.env()?;
        let ptr = crate::runtime_values::resolve_array_ptr_with_env(self.caller, &env, arr)?;
        crate::runtime_values::read_array_length_with_env(self.caller, &env, ptr)
    }
    fn array_read_elem(&mut self, arr: Value, index: u32) -> Option<Value> {
        let env = self.env()?;
        let ptr = crate::runtime_values::resolve_array_ptr_with_env(self.caller, &env, arr)?;
        crate::runtime_values::read_array_elem_with_env(self.caller, &env, ptr, index)
    }
    fn array_write_length(&mut self, arr: Value, len: u32) {
        let Some(env) = self.env() else {
            return;
        };
        let Some(ptr) = crate::runtime_values::resolve_array_ptr_with_env(self.caller, &env, arr)
        else {
            return;
        };
        crate::runtime_values::write_array_length_with_env(self.caller, &env, ptr, len);
    }
    };
}
