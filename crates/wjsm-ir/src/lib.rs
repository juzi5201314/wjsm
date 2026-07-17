pub mod builtin;
pub mod constants;
pub mod types;
pub mod value;
mod verify;

pub use builtin::Builtin;
use std::fmt::{self, Write};
pub use types::*;
pub use verify::IrVerificationError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIdOffsetError {
    module_id: ModuleId,
    offset: u32,
}

impl ModuleIdOffsetError {
    fn new(module_id: ModuleId, offset: u32) -> Self {
        Self { module_id, offset }
    }
}

impl fmt::Display for ModuleIdOffsetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "module id {} cannot be offset by {} without overflowing u32",
            self.module_id, self.offset
        )
    }
}

impl std::error::Error for ModuleIdOffsetError {}

pub fn offset_module_id(module_id: ModuleId, offset: u32) -> Result<ModuleId, ModuleIdOffsetError> {
    module_id
        .0
        .checked_add(offset)
        .map(ModuleId)
        .ok_or_else(|| ModuleIdOffsetError::new(module_id, offset))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Module {
    constants: Vec<Constant>,
    functions: Vec<Function>,
    script_mode: bool,
    /// 源文件路径（用于运行时错误堆栈映射）。
    source_file: Option<String>,
}

pub type Program = Module;

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_constant(&mut self, constant: Constant) -> ConstantId {
        let id = ConstantId(self.constants.len() as u32);
        self.constants.push(constant);
        id
    }

    pub fn push_function(&mut self, function: Function) -> FunctionId {
        let id = FunctionId(self.functions.len() as u32);
        self.functions.push(function);
        id
    }

    pub fn constants(&self) -> &[Constant] {
        &self.constants
    }

    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    pub fn function_mut(&mut self, id: FunctionId) -> Option<&mut Function> {
        self.functions.get_mut(id.0 as usize)
    }

    pub fn script_mode(&self) -> bool {
        self.script_mode
    }

    pub fn source_file(&self) -> Option<&str> {
        self.source_file.as_deref()
    }

    pub fn set_source_file(&mut self, file: impl Into<String>) {
        self.source_file = Some(file.into());
    }

    pub fn offset_module_ids(&mut self, offset: u32) -> Result<(), ModuleIdOffsetError> {
        if offset == 0 {
            return Ok(());
        }

        // 先完整校验，再写回，避免溢出时留下半迁移 IR。
        for constant in &self.constants {
            if let Constant::ModuleId(module_id) = constant {
                offset_module_id(*module_id, offset)?;
            }
        }

        for constant in &mut self.constants {
            if let Constant::ModuleId(module_id) = constant {
                *module_id = offset_module_id(*module_id, offset)?;
            }
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), IrVerificationError> {
        verify::verify_module(self)
    }
    pub fn dump_text(&self) -> String {
        let mut out = String::from("module {\n");

        if self.constants.is_empty() {
            out.push_str("  constants: []\n");
        } else {
            out.push_str("  constants:\n");
            for (index, constant) in self.constants.iter().enumerate() {
                let _ = writeln!(out, "    c{index} = {constant}");
            }
        }

        out.push('\n');

        for (index, function) in self.functions.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            function.dump_into(&mut out);
        }

        out.push_str("}\n");
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    FunctionRef(FunctionId),
    /// 运行时原生可调用对象；当前用于全局 eval 被作为值读取时。
    NativeCallableEval,
    /// BigInt 字面量（十进制字符串）
    BigInt(String),
    /// RegExp 字面量（pattern 和 flags）
    RegExp {
        pattern: String,
        flags: String,
    },
    /// AOT 解析的模块 ID（用于动态 import）
    ModuleId(ModuleId),
}

impl fmt::Display for Constant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => write!(formatter, "number({value})"),
            Self::String(value) => write!(formatter, "string({value:?})"),
            Self::Bool(value) => write!(formatter, "bool({value})"),
            Self::Null => formatter.write_str("null"),
            Self::Undefined => formatter.write_str("undefined"),
            Self::FunctionRef(id) => write!(formatter, "functionref(@{id})"),
            Self::NativeCallableEval => formatter.write_str("native_callable(eval)"),
            Self::BigInt(value) => write!(formatter, "bigint({value})"),
            Self::RegExp { pattern, flags } => {
                write!(formatter, "regex(/{pattern}/{flags})")
            }
            Self::ModuleId(id) => write!(formatter, "moduleid({id})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_module_ids_rewrites_only_module_id_constants() {
        let mut module = Module::new();
        module.add_constant(Constant::ModuleId(ModuleId(1)));
        module.add_constant(Constant::String("keep".to_string()));
        module.add_constant(Constant::ModuleId(ModuleId(3)));

        module
            .offset_module_ids(10)
            .expect("module ids should offset");

        assert_eq!(
            module.constants(),
            &[
                Constant::ModuleId(ModuleId(11)),
                Constant::String("keep".to_string()),
                Constant::ModuleId(ModuleId(13)),
            ]
        );
    }

    #[test]
    fn offset_module_ids_overflow_leaves_constants_unchanged() {
        let mut module = Module::new();
        module.add_constant(Constant::ModuleId(ModuleId(1)));
        module.add_constant(Constant::ModuleId(ModuleId(u32::MAX)));

        let error = module
            .offset_module_ids(1)
            .expect_err("overflow should be reported");

        assert_eq!(
            error.to_string(),
            "module id mod4294967295 cannot be offset by 1 without overflowing u32"
        );
        assert_eq!(
            module.constants(),
            &[
                Constant::ModuleId(ModuleId(1)),
                Constant::ModuleId(ModuleId(u32::MAX)),
            ]
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeObject {
    /// 实例方法/构造器的 [[HomeObject]] 是构造器的 prototype 对象。
    Prototype(FunctionId),
    /// 静态方法/静态块的 [[HomeObject]] 是构造器函数对象本身。
    Constructor(FunctionId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    name: String,
    params: Vec<String>,
    entry: BasicBlockId,
    blocks: Vec<BasicBlock>,
    /// 函数体是否包含 direct eval。后端据此降低局部变量优化强度。
    has_eval: bool,
    /// 该函数捕获的外层变量名列表（闭包用）。
    /// 语义层逃逸分析后填入，后端用于 env 对象的属性名。
    captured_names: Vec<String>,
    /// 该函数内 LoadVar 读到"已知函数声明/闭包"的变量名→FunctionId。
    /// 语义层填充（仅对单次赋值的函数声明变量），后端用于 callee no-GC 分析（Layer 3）。
    /// key = scope-qualified IR name（如 "$0.foo"），value = 被调用函数的 FunctionId。
    /// 空表示该函数不调用任何已知函数声明（保守：后端对未知 callee 当 may-GC）。
    known_callee_vars: std::collections::HashMap<String, FunctionId>,
    /// 方法的 [[HomeObject]]，用于实现 super 属性访问。
    /// 普通函数为 None；箭头函数可继承外层方法的 home object。
    pub home_object: Option<HomeObject>,
    /// 该函数是否需要 prototype 属性（普通函数声明/表达式 = true；箭头/方法/类构造器 = false）。
    /// 后端 init_function_props 据此决定是否创建 prototype 对象。
    needs_prototype: bool,
    /// 函数声明的 JS 源码位置（1-indexed line:col）。
    /// 语义层从 SWC span 填入，后端编码到 WASM custom section 供运行时错误映射。
    source_span: Option<SourceSpan>,
}
impl Function {
    pub fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            entry,
            blocks: Vec::new(),
            has_eval: false,
            captured_names: Vec::new(),
            known_callee_vars: std::collections::HashMap::new(),
            home_object: None,
            needs_prototype: false,
            source_span: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn params(&self) -> &[String] {
        &self.params
    }

    pub fn set_params(&mut self, params: Vec<String>) {
        self.params = params;
    }

    pub fn has_eval(&self) -> bool {
        self.has_eval
    }

    pub fn set_has_eval(&mut self, has_eval: bool) {
        self.has_eval = has_eval;
    }

    pub fn captured_names(&self) -> &[String] {
        &self.captured_names
    }

    pub fn set_captured_names(&mut self, names: Vec<String>) {
        self.captured_names = names;
    }

    pub fn source_span(&self) -> Option<SourceSpan> {
        self.source_span
    }

    pub fn set_source_span(&mut self, span: SourceSpan) {
        self.source_span = Some(span);
    }

    pub fn needs_prototype(&self) -> bool {
        self.needs_prototype
    }

    pub fn set_needs_prototype(&mut self, v: bool) {
        self.needs_prototype = v;
    }

    /// 返回该函数调用的"已知函数声明"变量名→FunctionId 映射（Layer 3 callee 分析）。
    pub fn known_callee_vars(&self) -> &std::collections::HashMap<String, FunctionId> {
        &self.known_callee_vars
    }

    /// 记录一个 callee 变量（scope-qualified IR name）→ FunctionId 映射（Layer 3）。
    /// 仅对单次赋值的函数声明安全（function 声明 hoisted 且语义不可重赋）。
    pub fn record_known_callee(&mut self, ir_name: String, function_id: FunctionId) {
        self.known_callee_vars.insert(ir_name, function_id);
    }

    pub fn entry(&self) -> BasicBlockId {
        self.entry
    }

    pub fn push_block(&mut self, block: BasicBlock) {
        self.blocks.push(block);
    }

    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    pub fn blocks_mut(&mut self) -> &mut [BasicBlock] {
        &mut self.blocks
    }

    /// O(1) 通过 id 获取 block 引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 FunctionBuilder::new_block 保证），
    /// 使用直接索引访问而非 iter().find()，将 O(n) 降为 O(1)。
    pub fn block_by_id(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    /// O(1) 通过 id 获取 block 可变引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 FunctionBuilder::new_block 保证），
    /// 使用直接索引访问而非 iter().find()，将 O(n) 降为 O(1)。
    pub fn block_by_id_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id.0 as usize)
    }

    fn dump_into(&self, out: &mut String) {
        let _ = write!(out, "  fn @{}", self.name);
        if let Some(home) = self.home_object {
            match home {
                HomeObject::Prototype(id) => {
                    let _ = write!(out, " [home_object=@{}.prototype]", id.0);
                }
                HomeObject::Constructor(id) => {
                    let _ = write!(out, " [home_object=@{}]", id.0);
                }
            }
        }
        if self.has_eval {
            let _ = write!(out, " [has_eval]");
        }
        if self.needs_prototype {
            let _ = write!(out, " [needs_prototype]");
        }
        if !self.captured_names.is_empty() {
            let _ = write!(out, " [captures: ");
            for (i, name) in self.captured_names.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                let _ = write!(out, "{name}");
            }
            let _ = write!(out, "]");
        }
        if self.params.is_empty() {
            let _ = writeln!(out, " [entry={}]:", self.entry);
        } else {
            let _ = write!(out, " [params: ");
            for (i, param) in self.params.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                let _ = write!(out, "{param}");
            }
            let _ = writeln!(out, "] [entry={}]:", self.entry);
        }

        for block in &self.blocks {
            let _ = writeln!(out, "    {}:", block.id);

            for instruction in &block.instructions {
                let _ = writeln!(out, "      {instruction}");
            }

            let _ = writeln!(out, "      {}", block.terminator);
        }
    }

    /// 输出单个函数的 IR 文本（不含 `module {` 包裹和常量块）。
    pub fn dump_text(&self) -> String {
        let mut s = String::new();
        self.dump_into(&mut s);
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BasicBlock {
    id: BasicBlockId,
    instructions: Vec<Instruction>,
    terminator: Terminator,
}

impl BasicBlock {
    pub fn new(id: BasicBlockId) -> Self {
        Self {
            id,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        }
    }

    pub fn new_with_terminator(id: BasicBlockId, terminator: Terminator) -> Self {
        Self {
            id,
            instructions: Vec::new(),
            terminator,
        }
    }

    pub fn id(&self) -> BasicBlockId {
        self.id
    }

    pub fn push_instruction(&mut self, instruction: Instruction) {
        self.instructions.push(instruction);
    }

    pub fn instructions(&self) -> &[Instruction] {
        &self.instructions
    }

    pub fn instructions_mut(&mut self) -> &mut Vec<Instruction> {
        &mut self.instructions
    }

    pub fn terminator(&self) -> &Terminator {
        &self.terminator
    }

    pub fn set_terminator(&mut self, terminator: Terminator) {
        self.terminator = terminator;
    }

    pub fn terminator_mut(&mut self) -> &mut Terminator {
        &mut self.terminator
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    Const {
        dest: ValueId,
        constant: ConstantId,
    },
    Binary {
        dest: ValueId,
        op: BinaryOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Unary {
        dest: ValueId,
        op: UnaryOp,
        value: ValueId,
    },
    Compare {
        dest: ValueId,
        op: CompareOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Phi {
        dest: ValueId,
        sources: Vec<PhiSource>,
    },
    CallBuiltin {
        dest: Option<ValueId>,
        builtin: Builtin,
        args: Vec<ValueId>,
    },
    StringConcatVa {
        dest: ValueId,
        parts: Vec<ValueId>,
    },
    LoadVar {
        dest: ValueId,
        name: String,
    },
    StoreVar {
        name: String,
        value: ValueId,
    },
    Call {
        dest: Option<ValueId>,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    /// 调用当前派生类的 super 构造器；保留当前 new.target。
    SuperCall {
        dest: Option<ValueId>,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
        forward_args: bool,
    },
    ConstructCall {
        dest: Option<ValueId>,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    NewObject {
        dest: ValueId,
        capacity: u32,
    },
    GetProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    SetProp {
        object: ValueId,
        key: ValueId,
        value: ValueId,
    },
    /// 删除对象属性，返回布尔值表示是否成功删除
    DeleteProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    /// 直接设置对象的 __proto__ 槽位（offset 0），用于原型链构建。
    SetProto {
        object: ValueId,
        value: ValueId,
    },
    /// 创建 TAG_ARRAY 数组对象
    NewArray {
        dest: ValueId,
        capacity: u32,
    },
    /// 按数字索引读取数组元素
    GetElem {
        dest: ValueId,
        object: ValueId,
        index: ValueId,
    },
    /// 按数字索引写入数组元素
    SetElem {
        object: ValueId,
        index: ValueId,
        value: ValueId,
    },
    /// 可选链属性访问：object?.key，object 为 null/undefined 时返回 undefined
    OptionalGetProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    /// 可选链索引访问：object?.[expr]
    OptionalGetElem {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    /// 可选链调用：callee?.(...args)，callee 为 null/undefined 时返回 undefined
    OptionalCall {
        dest: ValueId,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    /// 对象 spread：将 source 的 own enumerable 属性复制到 dest
    ObjectSpread {
        dest: ValueId,
        source: ValueId,
    },
    /// 获取 super 属性基对象：实例方法为 Base.prototype，静态方法为 Base 构造器。
    GetSuperBase {
        dest: ValueId,
    },
    /// 获取派生构造器的 super 构造器。
    GetSuperConstructor {
        dest: ValueId,
    },
    NewPromise {
        dest: ValueId,
    },
    PromiseResolve {
        promise: ValueId,
        value: ValueId,
    },
    PromiseReject {
        promise: ValueId,
        reason: ValueId,
    },
    Suspend {
        promise: ValueId,
        state: u32,
    },
    GeneratorSuspend {
        result: ValueId,
        state: u32,
    },
    CollectRestArgs {
        dest: ValueId,
        skip: u32,
    },
    /// 检查值是否为 TAG_EXCEPTION，将 dest 设为布尔值（true=是异常）
    IsException {
        dest: ValueId,
        value: ValueId,
    },
    /// 将错误对象编码为 TAG_EXCEPTION（用于函数返回异常）
    EncodeException {
        dest: ValueId,
        value: ValueId,
    },
    /// 将 TAG_EXCEPTION 解码为原始对象（用于重新抛出）
    ExceptionToObject {
        dest: ValueId,
        value: ValueId,
    },
    /// 调试检查点：源码行/列位置，供 inspector 单步与断点使用。无 dest、无 uses。
    DebugCheck {
        line: u32,
        col: u32,
    },
}

impl fmt::Display for Instruction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Const { dest, constant } => write!(formatter, "{dest} = const {constant}"),
            Self::Binary { dest, op, lhs, rhs } => {
                write!(formatter, "{dest} = {op} {lhs}, {rhs}")
            }
            Self::Unary { dest, op, value } => {
                write!(formatter, "{dest} = {op} {value}")
            }
            Self::Compare { dest, op, lhs, rhs } => {
                write!(formatter, "{dest} = {op} {lhs}, {rhs}")
            }
            Self::Phi { dest, sources } => {
                write!(formatter, "{dest} = phi [")?;
                for (index, source) in sources.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "({}, {})", source.predecessor, source.value)?;
                }
                formatter.write_char(']')
            }
            Self::CallBuiltin {
                dest,
                builtin,
                args,
            } => {
                if let Some(dest) = dest {
                    write!(formatter, "{dest} = ")?;
                }

                write!(formatter, "call builtin.{builtin}(")?;
                for (index, arg) in args.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{arg}")?;
                }
                formatter.write_char(')')
            }
            Self::StringConcatVa { dest, parts } => {
                write!(formatter, "{dest} = string_concat_va [")?;
                for (index, part) in parts.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{part}")?;
                }
                formatter.write_char(']')
            }
            Self::LoadVar { dest, name } => {
                write!(formatter, "{dest} = load var {name}")
            }
            Self::StoreVar { name, value } => {
                write!(formatter, "store var {name}, {value}")
            }
            Self::Call {
                dest,
                callee,
                this_val,
                args,
            } => {
                if let Some(dest) = dest {
                    write!(formatter, "{dest} = ")?;
                }
                write!(formatter, "call {callee}, this={this_val}")?;
                if !args.is_empty() {
                    formatter.write_str(", args=[")?;
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(formatter, "{arg}")?;
                    }
                    formatter.write_char(']')?;
                }
                Ok(())
            }
            Self::SuperCall {
                dest,
                callee,
                this_val,
                args,
                forward_args,
            } => {
                if let Some(dest) = dest {
                    write!(formatter, "{dest} = ")?;
                }
                write!(formatter, "super_call {callee}, this={this_val}")?;
                if *forward_args {
                    formatter.write_str(", forward_args")?;
                } else if !args.is_empty() {
                    formatter.write_str(", args=[")?;
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(formatter, "{arg}")?;
                    }
                    formatter.write_char(']')?;
                }
                Ok(())
            }
            Self::ConstructCall {
                dest,
                callee,
                this_val,
                args,
            } => {
                if let Some(dest) = dest {
                    write!(formatter, "{dest} = ")?;
                }
                write!(formatter, "construct_call {callee}, this={this_val}")?;
                if !args.is_empty() {
                    formatter.write_str(", args=[")?;
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(formatter, "{arg}")?;
                    }
                    formatter.write_char(']')?;
                }
                Ok(())
            }
            Self::NewObject { dest, capacity } => {
                write!(formatter, "{dest} = new_object(capacity={capacity})")
            }
            Self::GetProp { dest, object, key } => {
                write!(formatter, "{dest} = get_prop {object}, {key}")
            }
            Self::SetProp { object, key, value } => {
                write!(formatter, "set_prop {object}, {key}, {value}")
            }
            Self::DeleteProp { dest, object, key } => {
                write!(formatter, "{dest} = delete_prop {object}, {key}")
            }
            Self::SetProto { object, value } => {
                write!(formatter, "set_proto {object}, {value}")
            }
            Self::NewArray { dest, capacity } => {
                write!(formatter, "{dest} = new_array(capacity={capacity})")
            }
            Self::GetElem {
                dest,
                object,
                index,
            } => {
                write!(formatter, "{dest} = get_elem {object}, {index}")
            }
            Self::SetElem {
                object,
                index,
                value,
            } => {
                write!(formatter, "set_elem {object}, {index}, {value}")
            }
            Self::OptionalGetProp { dest, object, key } => {
                write!(formatter, "{dest} = optional_get_prop {object}, {key}")
            }
            Self::OptionalGetElem { dest, object, key } => {
                write!(formatter, "{dest} = optional_get_elem {object}, {key}")
            }
            Self::OptionalCall {
                dest,
                callee,
                this_val,
                args,
            } => {
                write!(
                    formatter,
                    "{dest} = optional_call {callee}, this={this_val}"
                )?;
                if !args.is_empty() {
                    formatter.write_str(", args=[")?;
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(formatter, "{arg}")?;
                    }
                    formatter.write_char(']')?;
                }
                Ok(())
            }
            Self::ObjectSpread { dest, source } => {
                write!(formatter, "{dest} = object_spread {source}")
            }
            Self::GetSuperBase { dest } => {
                write!(formatter, "{dest} = get_super_base")
            }
            Self::GetSuperConstructor { dest } => {
                write!(formatter, "{dest} = get_super_constructor")
            }
            Self::NewPromise { dest } => write!(formatter, "{dest} = new_promise"),
            Self::PromiseResolve { promise, value } => {
                write!(formatter, "promise_resolve {promise}, {value}")
            }
            Self::PromiseReject { promise, reason } => {
                write!(formatter, "promise_reject {promise}, {reason}")
            }
            Self::Suspend { promise, state } => {
                write!(formatter, "suspend {promise}, state={state}")
            }
            Self::GeneratorSuspend { result, state } => {
                write!(formatter, "generator_suspend {result}, state={state}")
            }
            Self::CollectRestArgs { dest, skip } => {
                write!(formatter, "{dest} = collect_rest_args skip={skip}")
            }
            Self::IsException { dest, value } => {
                write!(formatter, "{dest} = is_exception {value}")
            }
            Self::EncodeException { dest, value } => {
                write!(formatter, "{dest} = encode_exception {value}")
            }
            Self::ExceptionToObject { dest, value } => {
                write!(formatter, "{dest} = exception_to_object {value}")
            }
            Self::DebugCheck { line, col } => {
                write!(formatter, "debug_check line={line} col={col}")
            }
        }
    }
}

// BinaryOp, UnaryOp, CompareOp → types.rs

// Terminator, SwitchCaseTarget, PhiSource → types.rs

// ConstantId, FunctionId, BasicBlockId, ValueId, ModuleId → types.rs

/// 合成模块顶层入口的 IR 函数名（与用户声明的 `main` 区分，避免 wasm 入口约定冲突）。
pub const MODULE_ENTRY_IR_NAME: &str = "$module_main";

/// 是否为编译器合成的模块入口函数（非用户 `function main()`）。
pub fn is_module_entry_ir_function(name: &str) -> bool {
    name == MODULE_ENTRY_IR_NAME
}

/// Import 绑定信息（用于模块系统）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    /// 源模块 ID
    pub source_module: ModuleId,
    /// 导入的名称列表：(local_name, imported_name)
    /// - `import { x } from './foo'` → ("x", "x")
    /// - `import { y as z } from './foo'` → ("z", "y")
    /// - `import * as ns from './foo'` → ("ns", "*")
    /// - `import defaultExport from './foo'` → ("defaultExport", "default")
    pub names: Vec<(String, String)>,
    /// 模块说明符（如 './foo'），用于动态 import 的 specifier 查找
    pub specifier: String,
}
/// 模块重导出说明（`export … from`），供 lower 阶段填充 `export_map`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReExportBinding {
    /// 被重导出的源模块 ID
    pub source_module: ModuleId,
    /// `export { local as exported } from` 的 local；`export *` 时为 None
    pub local_name: Option<String>,
    /// 当前模块对外导出名；`export *` 时为 None（表示复制源模块全部导出）
    pub exported_name: Option<String>,
}

// ── Shadow Stack Constants ──────────────────────────────────────────────
// 影子栈位于独立 WASM 线性内存 `env.__shadow_memory`（memory index 1）。
// 主内存不再预留影子区；冷启动只提交 INITIAL，按需 grow 到 soft max。

/// 影子栈初始容量（1 页 = 64KiB = 8192 个 i64 槽位）。
pub const SHADOW_STACK_INITIAL_SIZE: u32 = 64 * 1024;
/// 向后兼容别名：表示冷启动初始容量，而非硬上限。
pub const SHADOW_STACK_SIZE: u32 = SHADOW_STACK_INITIAL_SIZE;
/// 默认软上限（16MiB）。超过时 ensure 返回失败并写入 RangeError。
pub const SHADOW_STACK_DEFAULT_MAX_SIZE: u32 = 16 * 1024 * 1024;
/// 影子内存在 multi-memory 模块中的 index（`env.memory`=0，`env.__shadow_memory`=1）。
pub const SHADOW_MEMORY_INDEX: u32 = 1;
/// 影子内存 import/export 名。
pub const SHADOW_MEMORY_NAME: &str = "__shadow_memory";

/// V2 shared dynamic heap 在 multi-memory 模块中的 index。
pub const HEAP_MEMORY_INDEX: u32 = 2;
/// V2 shared dynamic heap import/export 名。
pub const HEAP_MEMORY_NAME: &str = "__heap_memory";
/// V2 dynamic heap 的固定虚拟 reserve 大小（32 GiB）。
pub const HEAP_MEMORY_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// WebAssembly 64 KiB page 计数。
pub const HEAP_MEMORY_PAGES: u64 = HEAP_MEMORY_BYTES / (64 * 1024);
/// V2 NLAB fast-path 当前分配 cursor（i64 byte address）。
pub const HEAP_ALLOC_PTR_GLOBAL_NAME: &str = "__heap_alloc_ptr";
/// V2 NLAB fast-path 当前 buffer 末端（i64 byte address）。
pub const HEAP_ALLOC_END_GLOBAL_NAME: &str = "__heap_alloc_end";
/// V2 动态对象区起点（i64 byte address）。
pub const HEAP_OBJECT_START_GLOBAL_NAME: &str = "__heap_object_start";
/// V2 动态对象区上限（i64 byte address）。
pub const HEAP_LIMIT_GLOBAL_NAME: &str = "__heap_limit_v2";

// ── Well-Known Symbol 索引 ─────────────────────────────────────────────
/// Well-known symbol 索引常量，semantic 和 runtime 共享。
pub mod wk_symbol {
    pub const ITERATOR: u32 = 0;
    pub const SPECIES: u32 = 1;
    pub const TO_STRING_TAG: u32 = 2;
    pub const ASYNC_ITERATOR: u32 = 3;
    pub const HAS_INSTANCE: u32 = 4;
    pub const TO_PRIMITIVE: u32 = 5;
    pub const DISPOSE: u32 = 6;
    pub const MATCH: u32 = 7;
    pub const ASYNC_DISPOSE: u32 = 8;
    pub const IS_CONCAT_SPREADABLE: u32 = 9;
    pub const MATCH_ALL: u32 = 10;
    pub const REPLACE: u32 = 11;
    pub const SEARCH: u32 = 12;
    pub const SPLIT: u32 = 13;
    pub const UNSCOPABLES: u32 = 14;
}

// ── Heap type tags ──────────────────────────────────────────────────────
/// 0x00 = object (HEAP_TYPE_OBJECT)
pub const HEAP_TYPE_OBJECT: u8 = 0x00;
/// 0x01 = array (HEAP_TYPE_ARRAY)
pub const HEAP_TYPE_ARRAY: u8 = 0x01;
pub const HEAP_TYPE_PROMISE: u8 = 0x02;
pub const HEAP_TYPE_CONTINUATION: u8 = 0x03;
pub const HEAP_TYPE_ASYNC_GENERATOR: u8 = 0x04;
pub const HEAP_TYPE_ARGUMENTS: u8 = 0x05;
pub const HEAP_TYPE_MODULE_NAMESPACE: u8 = 0x08;
