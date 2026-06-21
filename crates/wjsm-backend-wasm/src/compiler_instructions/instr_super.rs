use super::*;

impl Compiler {
    pub(crate) fn compile_object_spread(&mut self, dest: &ValueId, source: &ValueId) -> Result<()> {
        self.emit(WasmInstruction::LocalGet(self.local_idx(dest.0)));
        self.emit(WasmInstruction::LocalGet(self.local_idx(source.0)));
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ObjSpread],
        ));
        Ok(())
    }

    /// 编译 GetSuperBase：按当前函数的 [[HomeObject]] 计算 super base。
    /// 类方法使用编译期 home metadata；对象字面量/动态 eval 通过 env.home 传入 home object。
    pub(crate) fn compile_get_super_base(&mut self, dest: &ValueId) -> Result<()> {
        match self.current_home_object {
            Some(HomeObject::Prototype(constructor_id)) => {
                let constructor = self.encode_function_ref_id(constructor_id);
                let prototype_key = self.ensure_string_ptr_const("prototype");
                self.emit(WasmInstruction::I64Const(constructor));
                self.emit(WasmInstruction::I32Const(prototype_key as i32));
                self.emit(WasmInstruction::Call(self.obj_get_func_idx));
            }
            Some(HomeObject::Constructor(constructor_id)) => {
                let constructor = self.encode_function_ref_id(constructor_id);
                self.emit(WasmInstruction::I64Const(constructor));
            }
            None => {
                self.emit(WasmInstruction::LocalGet(0));
                let home_key = self.ensure_string_ptr_const("home");
                self.emit(WasmInstruction::I32Const(home_key as i32));
                self.emit(WasmInstruction::Call(self.obj_get_func_idx));
            }
        }

        self.emit(WasmInstruction::Call(
            self.builtin_func_indices[&Builtin::ObjectGetPrototypeOf],
        ));
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        Ok(())
    }

    pub(crate) fn compile_get_super_constructor(&mut self, dest: &ValueId) -> Result<()> {
        if let Some(function_id) = self.current_function_id {
            let constructor = self.encode_function_ref_id(function_id);
            self.emit(WasmInstruction::I64Const(constructor));
            self.emit(WasmInstruction::Call(
                self.builtin_func_indices[&Builtin::ObjectGetPrototypeOf],
            ));
        } else {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        Ok(())
    }
}
