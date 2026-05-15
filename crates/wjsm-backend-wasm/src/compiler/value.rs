use wasm_encoder::{BlockType, Instruction as WasmInstruction, MemArg, ValType};
use wjsm_ir::value;

use super::state::Compiler;

impl Compiler {
    pub(crate) fn emit_is_nullish_i32(&mut self, val_id: u32) {
        let val_local = self.local_idx(val_id);
        let box_base = value::BOX_BASE as i64;

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::I32Or);

        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::End);
    }

    // ── Truthiness helpers ───────────────────────────────────────────────────

    /// Emit WASM instructions that convert a NaN-boxed i64 value to an i32 boolean
    /// (1 = truthy, 0 = falsy).
    ///
    /// This is the unified truthiness check for all control flow conditions.
    pub(crate) fn emit_to_bool_i32(&mut self, val_id: u32) {
        let val_local = self.local_idx(val_id);
        // Strategy:
        // 1. Check if it's undefined (TAG_UNDEFINED) → falsy
        // 2. Check if it's null (TAG_NULL) → falsy
        // 3. Check if it's bool (TAG_BOOL) → decode payload bit
        // 4. Check if it's f64 (no tag) → check 0.0 and NaN
        // 5. Otherwise (string, handle) → truthy
        //
        // Implementation using a series of nested if/else:

        let box_base = value::BOX_BASE as i64;

        // Check if the value is NaN-boxed (has BOX_BASE pattern)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Eq);

        // If NaN-boxed, check the tag
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // NaN-boxed path: check tag
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);

        // Check TAG_UNDEFINED (0x2)
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // undefined is falsy
        self.emit(WasmInstruction::Else);

        // Check TAG_NULL (0x3)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // null is falsy
        self.emit(WasmInstruction::Else);

        // Check TAG_BOOL (0x4)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_BOOL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // Bool: extract payload bit (val & 1)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(1));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Else);
        // Check TAG_STRING (0x1): load first byte from memory to detect empty string
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0xF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_STRING as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // 运行时字符串句柄不对应线性内存指针；当前运行时只会产生非空字符串。
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(
            (value::STRING_RUNTIME_HANDLE_FLAG << 32) as i64,
        ));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(
            (value::STRING_RUNTIME_HANDLE_FLAG << 32) as i64,
        ));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::Else);
        // 编译期字符串：提取低 32 位作为内存指针，读取首字节
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // 如果首字节 == 0（nul-terminated 空串）则 falsy，否则 truthy
        self.emit(WasmInstruction::I32Eqz);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // 空串 falsy
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(1)); // 非空串 truthy
        self.emit(WasmInstruction::End); // end empty string check
        self.emit(WasmInstruction::End); // end runtime string check
        self.emit(WasmInstruction::Else);
        // Other NaN-boxed types (handle, etc.) → truthy
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::End); // end TAG_STRING check

        self.emit(WasmInstruction::End); // end TAG_BOOL check

        self.emit(WasmInstruction::End); // end TAG_NULL check

        self.emit(WasmInstruction::End); // end TAG_UNDEFINED check

        self.emit(WasmInstruction::Else);
        // Not NaN-boxed → it's a raw f64
        // Check for +0, -0, and NaN
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Const(0.0.into()));
        self.emit(WasmInstruction::F64Eq);
        // If equal to 0.0, it's falsy (+0 or -0)
        // Also need to check NaN (NaN != NaN, so NaN is falsy too)
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // 0 is falsy
        self.emit(WasmInstruction::Else);
        // Check for NaN: x != x
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Ne);
        // f64.ne returns 1 if NaN (since NaN != NaN)
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // NaN is falsy
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(1)); // non-zero number is truthy
        self.emit(WasmInstruction::End); // end NaN check
        self.emit(WasmInstruction::End); // end == 0 check

        self.emit(WasmInstruction::End); // end NaN-boxed check
    }

    // ── Local management ────────────────────────────────────────────────────

}
