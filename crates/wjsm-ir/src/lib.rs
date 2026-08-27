pub mod builtin;
pub mod cfg;
pub mod constants;
pub mod string_hash;
pub mod typed_cfg;
pub mod types;
pub mod value;
pub mod value_class;
pub mod variable_ssa;
mod verify;

pub use builtin::Builtin;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write};
pub use types::*;
pub use verify::IrVerificationError;

/// Eval/VM 模块入口通过 native environment 参数接收的 scope bridge slot。
pub const EVAL_SCOPE_ENV_PARAM: &str = "$eval_env";

/// 必须走共享 host 槽表的变量：模块级、跨函数协议槽、eval 桥。
pub fn is_host_shared_variable(name: &str) -> bool {
    matches!(name, "$this" | "$env" | EVAL_SCOPE_ENV_PARAM)
        || name.starts_with("$0.")
        || name.starts_with("$sroa.")
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Module {
    constants: Vec<Constant>,
    /// 与 `constants` 下标对齐的字符串烘焙元数据；仅 `Constant::String` 槽位有值。
    /// 编译期由 [`Module::add_constant`] 即时计算，制品解码则直接填充 wire 里的
    /// 烘焙值——运行时（install 期发布）因此零哈希、零表示转换。
    #[serde(default)]
    string_constant_metas: Vec<Option<StringConstantMeta>>,
    functions: Vec<Function>,
    script_mode: bool,
    /// 源文件路径（用于运行时错误堆栈映射）。
    source_file: Option<String>,
}

pub type Program = Module;

/// 字符串常量的编译期烘焙元数据（4.1 常量段）。
///
/// 哈希与表示选择在编译期一次算定并随制品分发；install 期发布照此直写堆头，
/// 不再经 UTF-8 → UTF-16 转换与内容哈希。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringConstantMeta {
    /// 内容哈希（与 `wjsm_ir::string_hash` 权威实现同函数同种子，已归一化非 0）。
    pub hash: u32,
    /// true = payload 是 Latin-1 单字节载荷；false = UTF-16LE 码元序列。
    pub latin1: bool,
    /// 扁平载荷：Latin-1 每码元 1 字节，UTF-16 每码元 2 字节（小端）。
    pub payload: Vec<u8>,
}

impl StringConstantMeta {
    /// 从 UTF-8 文本构造：全 Latin-1 即选单字节载荷；哈希跨表示同值。
    pub fn from_text(text: &str) -> Self {
        let units: Vec<u16> = text.encode_utf16().collect();
        let latin1 = units.iter().all(|unit| *unit <= u8::MAX as u16);
        let payload = if latin1 {
            units.iter().map(|unit| *unit as u8).collect()
        } else {
            units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
        };
        Self {
            hash: string_hash::content_hash_units(&units),
            latin1,
            payload,
        }
    }

    /// UTF-16 码元长度。
    pub fn unit_len(&self) -> u32 {
        let bytes = u32::try_from(self.payload.len()).unwrap_or(u32::MAX);
        if self.latin1 { bytes } else { bytes / 2 }
    }
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_constant(&mut self, constant: Constant) -> ConstantId {
        let meta = match &constant {
            Constant::String(text) => Some(StringConstantMeta::from_text(text)),
            _ => None,
        };
        self.add_constant_with_meta(constant, meta)
    }

    /// 制品解码专用：wire 已携带烘焙元数据，直接填充不再重算。
    pub fn add_constant_with_meta(
        &mut self,
        constant: Constant,
        meta: Option<StringConstantMeta>,
    ) -> ConstantId {
        let id = ConstantId(self.constants.len() as u32);
        self.constants.push(constant);
        self.string_constant_metas.push(meta);
        debug_assert_eq!(self.constants.len(), self.string_constant_metas.len());
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

    /// 取字符串常量的烘焙元数据；非字符串槽位或元数据缺失（serde 兼容回退）为 None，
    /// 调用方需自行从文本重算。
    pub fn string_constant_meta(&self, id: ConstantId) -> Option<&StringConstantMeta> {
        self.string_constant_metas.get(id.0 as usize)?.as_ref()
    }

    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// 将另一个 Program 的全部常量与函数按原顺序追加到本 Module 末尾
    /// （builtin 段 hydration 用）。
    ///
    /// 追加后本模块的常量/函数索引保持对方段内偏移不变（段函数在前、用户函数在后），
    /// 因此段内 `Constant::FunctionRef` 引用在合并后的模块中依然有效。
    pub fn append_builtin(&mut self, other: &Program) {
        self.constants.extend(other.constants.iter().cloned());
        self.string_constant_metas
            .extend(other.string_constant_metas.iter().cloned());
        self.functions.extend(other.functions.iter().cloned());
    }

    /// 定位 `$builtin_main`。没有则 `None`（无 builtin 段 / TLA 回退整包）。
    pub fn builtin_entry_function_id(&self) -> Option<FunctionId> {
        self.functions
            .iter()
            .position(|function| is_builtin_entry_ir_function(function.name()))
            .and_then(|index| u32::try_from(index).ok())
            .map(FunctionId)
    }

    /// 切出两段独立 Program，供分别 codegen。
    ///
    /// 前置：`self.functions()` 前 `split` 个函数是 builtin 段（`split` =
    /// `builtin_entry_function_id().0 + 1`，因为 `$builtin_main` 是段内最后一个函数，
    /// 由 `build_builtin_segment` 的 `finalize` 保证）。
    ///
    /// - builtin 段：函数 `[0, split)`、常量全量拷贝（段内 FunctionRef / 字符串常量下标不变）。
    /// - 用户段：函数 `[split, len)`。用户函数体内的 `Constant::FunctionRef(id)`：
    ///   - `id.0 >= split`：改写成 `FunctionId(id.0 - split)`，因为用户 image 的
    ///     `wjsm_function_{i}` 从 0 编号；
    ///   - `id.0 < split`：改写成 `FunctionId(user_count + id.0)`，让跨 image 引用
    ///     落在 `function_index >= user_function_count`，runtime 再映射回 builtin image。
    ///     用户段常量表仍是合并 Program 的全量常量拷贝，这样 `MaterializeString` 等
    ///     下标不用重映射。
    pub fn split_builtin_segment(&self) -> Option<(Program, Program)> {
        let entry_id = self.builtin_entry_function_id()?;
        let entry_index = usize::try_from(entry_id.0).ok()?;
        if self
            .functions
            .get(entry_index)
            .is_none_or(|function| !is_builtin_entry_ir_function(function.name()))
        {
            return None;
        }
        let split = entry_index.checked_add(1)?;
        if split > self.functions.len() {
            return None;
        }
        let split_id = entry_id.0.checked_add(1)?;

        let builtin_used = max_constant_id_in(&self.functions[..split]);
        let builtin_constants = match builtin_used {
            Some(max) => self.constants[..=max].to_vec(),
            None => Vec::new(),
        };
        let builtin_metas = match builtin_used {
            Some(max) => self.string_constant_metas[..=max].to_vec(),
            None => Vec::new(),
        };
        let builtin = Program {
            constants: builtin_constants,
            string_constant_metas: builtin_metas,
            functions: self.functions[..split].to_vec(),
            script_mode: self.script_mode,
            source_file: None,
        };
        let user_count = u32::try_from(self.functions.len() - split).ok()?;
        let user = Program {
            constants: self
                .constants
                .iter()
                .cloned()
                .map(|constant| remap_user_function_ref(constant, split_id, user_count))
                .collect(),
            string_constant_metas: self.string_constant_metas.clone(),
            functions: self.functions[split..].to_vec(),
            script_mode: self.script_mode,
            source_file: self.source_file.clone(),
        };
        Some((builtin, user))
    }

    pub fn function_mut(&mut self, id: FunctionId) -> Option<&mut Function> {
        self.functions.get_mut(id.0 as usize)
    }

    pub fn script_mode(&self) -> bool {
        self.script_mode
    }
    pub fn set_script_mode(&mut self, script_mode: bool) {
        self.script_mode = script_mode;
    }

    pub fn source_file(&self) -> Option<&str> {
        self.source_file.as_deref()
    }

    pub fn set_source_file(&mut self, file: impl Into<String>) {
        self.source_file = Some(file.into());
    }
    pub fn clear_source_file(&mut self) {
        self.source_file = None;
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

    /// 函数 `function` 可以提升到 generated SSA 的局部变量。
    ///
    /// 跨函数只读/只写的共享槽（例如 builtin 写入、用户读取的 `$1.answer`）
    /// 必须留在 host 槽表。因内联而在多个函数里各自 StoreVar 的同名局部
    /// 可以分别提升，互不影响。`$sroa.*` 是标量替换留下的跨函数字段槽，
    /// 多名使用者必须共享同一 host 槽，不能按函数各提升一份。
    ///
    /// `$builtin_main` 的 StoreVar 是段接口：builtin image 按 Program digest
    /// 缓存、与用户段分开编译，用户 import 的 live binding 只会 LoadVar
    /// 这些名字。入口函数自身不提升；它写过的名字在本段其它函数里也不提升。
    pub fn frame_local_variable_names<'a>(&'a self, function: &'a Function) -> BTreeSet<&'a str> {
        self.functions
            .iter()
            .position(|candidate| std::ptr::eq(candidate, function))
            .and_then(|index| {
                self.frame_local_variable_names_by_function()
                    .into_iter()
                    .nth(index)
            })
            .unwrap_or_default()
    }

    /// 一次扫完整包，按函数下标给出可提升的局部名。
    ///
    /// 后端 / host 按函数查询时必须走这份表，避免对每个函数再扫一遍整包。
    pub fn frame_local_variable_names_by_function(&self) -> Vec<BTreeSet<&str>> {
        let published = self.builtin_entry_published_names();
        let mut readers: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        let mut owners: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (index, function) in self.functions.iter().enumerate() {
            for name in function.used_variable_names() {
                readers.entry(name).or_default().push(index);
            }
            for name in function.owned_variable_names() {
                owners.entry(name).or_default().push(index);
            }
        }
        self.functions
            .iter()
            .enumerate()
            .map(|(index, function)| {
                if is_builtin_entry_ir_function(function.name()) {
                    return BTreeSet::new();
                }
                function
                    .frame_local_candidate_names()
                    .into_iter()
                    .filter(|name| {
                        !published.contains(name)
                            && !(name.starts_with("$sroa.")
                                && readers.get(name).is_some_and(|users| users.len() > 1))
                            && readers.get(name).is_none_or(|users| {
                                users.iter().all(|user| {
                                    *user == index
                                        || owners
                                            .get(name)
                                            .is_some_and(|owned| owned.contains(user))
                                })
                            })
                    })
                    .collect()
            })
            .collect()
    }

    /// `$builtin_main` 写入的绑定，对用户段和缓存复用都是共享槽。
    fn builtin_entry_published_names(&self) -> BTreeSet<&str> {
        self.functions
            .iter()
            .filter(|function| is_builtin_entry_ir_function(function.name()))
            .flat_map(Function::stored_variable_names)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Constant {
    Number(f64),
    String(String),
    /// 编译期 split 产生的不可变数组模板；运行时必须 clone 成独立数组。
    ArrayTemplate(Vec<ConstantId>),
    /// 静态 SSO 键对象字面量模板；install 期烘焙 shape transition。
    ObjectTemplate {
        keys: Vec<u64>,
    },
    Bool(bool),
    Null,
    Undefined,
    /// TDZ 未初始化哨兵（`value::encode_uninitialized()`）；仅由闭包环境快照
    /// 在被前向引用的 let/const/class 绑定声明执行前写入，永不暴露给用户代码。
    Uninitialized,
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
            Self::ArrayTemplate(elements) => write!(formatter, "array_template({elements:?})"),
            Self::ObjectTemplate { keys } => write!(formatter, "object_template({keys:?})"),
            Self::Bool(value) => write!(formatter, "bool({value})"),
            Self::Null => formatter.write_str("null"),
            Self::Undefined => formatter.write_str("undefined"),
            Self::Uninitialized => formatter.write_str("uninitialized"),
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

    /// 序列化 → 反序列化 → 相等：验证 Program 可作为磁盘缓存的载体。
    #[test]
    fn program_serde_roundtrip() {
        let mut module = Module::new();
        module.set_source_file("test.js");

        let num = module.add_constant(Constant::Number(1.5));
        let text = module.add_constant(Constant::String("hello".to_string()));
        let flag = module.add_constant(Constant::Bool(true));
        let regex = module.add_constant(Constant::RegExp {
            pattern: "^a+$".to_string(),
            flags: "gi".to_string(),
        });
        let bigint = module.add_constant(Constant::BigInt("12345678901234567890".to_string()));
        let module_id = module.add_constant(Constant::ModuleId(ModuleId(7)));

        let mut function = Function::new("main".to_string(), BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: num,
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: text,
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: regex,
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(3),
            constant: bigint,
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(4),
            constant: module_id,
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(13),
            constant: flag,
        });
        entry.push_instruction(Instruction::Binary {
            dest: ValueId(5),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(0),
        });
        entry.push_instruction(Instruction::Unary {
            dest: ValueId(6),
            op: UnaryOp::Not,
            value: ValueId(4),
        });
        entry.push_instruction(Instruction::Compare {
            dest: ValueId(7),
            op: CompareOp::StrictEq,
            lhs: ValueId(0),
            rhs: ValueId(5),
        });
        entry.push_instruction(Instruction::Phi {
            dest: ValueId(8),
            sources: vec![PhiSource {
                predecessor: BasicBlockId(0),
                value: ValueId(5),
            }],
        });
        entry.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(9)),
            builtin: Builtin::ConsoleLog,
            args: vec![ValueId(1)],
        });
        entry.push_instruction(Instruction::StringConcatVa {
            dest: ValueId(10),
            parts: vec![ValueId(1), ValueId(1)],
        });
        entry.push_instruction(Instruction::LoadVar {
            dest: ValueId(11),
            name: "x".to_string(),
        });
        entry.push_instruction(Instruction::StoreVar {
            name: "y".to_string(),
            value: ValueId(11),
        });
        entry.push_instruction(Instruction::GetProp {
            dest: ValueId(12),
            object: ValueId(1),
            key: ValueId(1),
        });
        entry.push_instruction(Instruction::SetElem {
            dest: ValueId(13),
            object: ValueId(12),
            index: ValueId(0),
            value: ValueId(11),
            strict: false,
        });
        entry.push_instruction(Instruction::SetProto {
            object: ValueId(12),
            value: ValueId(4),
        });
        entry.push_instruction(Instruction::DebugCheck { line: 3, col: 7 });
        entry.set_terminator(Terminator::Branch {
            condition: ValueId(8),
            true_block: BasicBlockId(0),
            false_block: BasicBlockId(1),
        });
        function.push_block(entry);

        let mut exit = BasicBlock::new(BasicBlockId(1));
        exit.set_terminator(Terminator::Return {
            value: Some(ValueId(0)),
        });
        function.push_block(exit);
        module.push_function(function);

        let json = serde_json::to_string(&module).expect("module 应可序列化为 JSON");
        let restored: Program = serde_json::from_str(&json).expect("JSON 应可反序列化为 Program");
        assert_eq!(module, restored);
    }

    fn empty_function(name: &str) -> Function {
        let mut function = Function::new(name, BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Terminator::Return { value: None });
        function.push_block(entry);
        function
    }

    #[test]
    fn frame_local_variable_names_exclude_shared_and_eval_bindings() {
        let mut function = Function::new("work", BasicBlockId(0));
        function.set_params(vec!["$1.$env".into(), "$1.$this".into(), "$1.n".into()]);
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::StoreVar {
            name: "$1.s".into(),
            value: ValueId(0),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.s".into(),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(2),
            name: "$1.n".into(),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$0.N".into(),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(4),
            name: "$this".into(),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        function.push_block(block);
        assert_eq!(
            function
                .frame_local_variable_names()
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["$1.n", "$1.s"]
        );

        function.set_has_eval(true);
        assert!(function.frame_local_variable_names().is_empty());
        function.set_has_eval(false);
        function.set_captured_names(vec!["$1.s".into()]);
        assert_eq!(
            function
                .frame_local_variable_names()
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["$1.n"]
        );
    }

    #[test]
    fn program_frame_locals_keep_cross_function_shared_slots_on_host() {
        let mut writer = Function::new("writer", BasicBlockId(0));
        let mut write_block = BasicBlock::new(BasicBlockId(0));
        write_block.push_instruction(Instruction::StoreVar {
            name: "$1.answer".into(),
            value: ValueId(0),
        });
        write_block.set_terminator(Terminator::Return { value: None });
        writer.push_block(write_block);

        let mut reader = Function::new("reader", BasicBlockId(0));
        let mut read_block = BasicBlock::new(BasicBlockId(0));
        read_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$1.answer".into(),
        });
        read_block.push_instruction(Instruction::StoreVar {
            name: "$1.local".into(),
            value: ValueId(0),
        });
        read_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.local".into(),
        });
        read_block.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });

        reader.push_block(read_block);

        let mut program = Module::new();
        program.push_function(writer);
        program.push_function(reader);
        let writer = &program.functions()[0];
        let reader = &program.functions()[1];
        assert!(program.frame_local_variable_names(writer).is_empty());
        assert_eq!(
            program
                .frame_local_variable_names(reader)
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["$1.local"]
        );
    }

    fn store_and_load(name: &str) -> Function {
        let mut function = Function::new(name, BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::StoreVar {
            name: "$1.s".into(),
            value: ValueId(0),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.s".into(),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        function.push_block(block);
        function
    }

    #[test]
    fn independently_owned_inlined_locals_can_promote() {
        let mut program = Module::new();
        program.push_function(store_and_load("work"));
        program.push_function(store_and_load("$module_main"));
        for function in program.functions() {
            assert_eq!(
                program
                    .frame_local_variable_names(function)
                    .into_iter()
                    .collect::<Vec<_>>(),
                vec!["$1.s"]
            );
        }
    }

    #[test]
    fn builtin_entry_published_slots_stay_on_host() {
        let mut entry = Function::new(BUILTIN_ENTRY_IR_NAME, BasicBlockId(0));
        let mut entry_block = BasicBlock::new(BasicBlockId(0));
        entry_block.push_instruction(Instruction::StoreVar {
            name: "$1.createHash".into(),
            value: ValueId(0),
        });
        entry_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.createHash".into(),
        });
        entry_block.push_instruction(Instruction::StoreVar {
            name: "$1.scratch".into(),
            value: ValueId(1),
        });
        entry_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(2),
            name: "$1.scratch".into(),
        });
        entry_block.set_terminator(Terminator::Return { value: None });
        entry.push_block(entry_block);

        let mut helper = Function::new("createHash", BasicBlockId(0));
        let mut helper_block = BasicBlock::new(BasicBlockId(0));
        helper_block.push_instruction(Instruction::StoreVar {
            name: "$1.createHash".into(),
            value: ValueId(0),
        });
        helper_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.createHash".into(),
        });
        helper_block.push_instruction(Instruction::StoreVar {
            name: "$2.n".into(),
            value: ValueId(1),
        });
        helper_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(2),
            name: "$2.n".into(),
        });
        helper_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        helper.push_block(helper_block);

        let mut program = Module::new();
        program.push_function(helper);
        program.push_function(entry);
        let helper = &program.functions()[0];
        let entry = &program.functions()[1];
        assert!(program.frame_local_variable_names(entry).is_empty());
        assert_eq!(
            program
                .frame_local_variable_names(helper)
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["$2.n"]
        );
    }

    #[test]
    fn split_builtin_segment_remaps_user_function_refs() {
        let mut program = Module::new();
        program.set_script_mode(true);
        program.set_source_file("user.js");
        program.add_constant(Constant::FunctionRef(FunctionId(0)));
        program.add_constant(Constant::FunctionRef(FunctionId(2)));
        program.add_constant(Constant::String("keep".into()));
        program.push_function(empty_function("builtin_helper"));
        program.push_function(empty_function(BUILTIN_ENTRY_IR_NAME));
        program.push_function(empty_function(MODULE_ENTRY_IR_NAME));

        let (builtin, user) = program
            .split_builtin_segment()
            .expect("含 $builtin_main 的合并 Program 必须能切段");

        assert_eq!(builtin.functions().len(), 2);
        assert_eq!(builtin.functions()[0].name(), "builtin_helper");
        assert_eq!(builtin.functions()[1].name(), BUILTIN_ENTRY_IR_NAME);
        assert!(builtin.source_file().is_none());
        assert!(builtin.script_mode());
        assert!(builtin.constants().is_empty());

        assert_eq!(user.functions().len(), 1);
        assert_eq!(user.functions()[0].name(), MODULE_ENTRY_IR_NAME);
        assert_eq!(user.source_file(), Some("user.js"));
        assert!(user.script_mode());
        assert_eq!(
            user.constants(),
            &[
                Constant::FunctionRef(FunctionId(1)),
                Constant::FunctionRef(FunctionId(0)),
                Constant::String("keep".into()),
            ]
        );
    }

    #[test]
    fn split_builtin_segment_returns_none_without_entry() {
        let mut program = Module::new();
        program.push_function(empty_function(MODULE_ENTRY_IR_NAME));
        assert!(program.split_builtin_segment().is_none());
        assert!(program.builtin_entry_function_id().is_none());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HomeObject {
    /// 实例方法/构造器的 [[HomeObject]] 是构造器的 prototype 对象。
    Prototype(FunctionId),
    /// 静态方法/静态块的 [[HomeObject]] 是构造器函数对象本身。
    Constructor(FunctionId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// 语义层从 SWC span 填入，后端编码到 native image metadata 供运行时错误映射。
    source_span: Option<SourceSpan>,
    /// 该函数是否可直接调用（函数体不依赖 env/this/new.target，且无 eval）。
    /// 由语义层 direct_call pass 计算，后端据此对调用点发射直接 `call`（跳过动态分派）。
    direct_callable: bool,
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
            direct_callable: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 重命名函数（builtin 段缓存用：把段内 `$module_main` 改名为 `$builtin_main`，
    /// 避免与用户入口 `$module_main` 在后端 entry 识别时冲突）。
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
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

    /// 只属于本函数栈帧、不必写入共享 host 槽表的局部变量名。
    ///
    /// 排除模块级 `$0.*`、捕获名、eval 可见绑定以及 `$this` / `$env` /
    /// `$eval_env` 这类跨函数或跨 image 协议槽。跨函数共享槽还要再经
    /// [`Module::frame_local_variable_names`] 过滤。
    /// `$builtin_main` 的写入是段接口，本函数一律不提升。
    pub fn frame_local_candidate_names(&self) -> BTreeSet<&str> {
        if self.has_eval || is_builtin_entry_ir_function(&self.name) {
            return BTreeSet::new();
        }
        let captured: BTreeSet<&str> = self.captured_names.iter().map(String::as_str).collect();
        let mut loaded = BTreeSet::new();
        let mut stored = BTreeSet::new();
        for param in &self.params {
            if !is_host_shared_variable(param) && !captured.contains(param.as_str()) {
                stored.insert(param.as_str());
            }
        }
        for block in &self.blocks {
            for instruction in block.instructions() {
                match instruction {
                    Instruction::LoadVar { name, .. } => {
                        loaded.insert(name.as_str());
                    }
                    Instruction::StoreVar { name, .. } => {
                        stored.insert(name.as_str());
                    }
                    _ => {}
                }
            }
        }
        loaded
            .intersection(&stored)
            .copied()
            .filter(|name| !is_host_shared_variable(name) && !captured.contains(name))
            .collect()
    }

    pub fn frame_local_variable_names(&self) -> BTreeSet<&str> {
        self.frame_local_candidate_names()
    }

    fn stored_variable_names(&self) -> BTreeSet<&str> {
        let mut names = BTreeSet::new();
        for param in &self.params {
            names.insert(param.as_str());
        }
        for block in &self.blocks {
            for instruction in block.instructions() {
                if let Instruction::StoreVar { name, .. } = instruction {
                    names.insert(name.as_str());
                }
            }
        }
        names
    }

    fn owned_variable_names(&self) -> BTreeSet<&str> {
        self.stored_variable_names()
    }

    fn used_variable_names(&self) -> BTreeSet<&str> {
        let mut names = self.owned_variable_names();
        for block in &self.blocks {
            for instruction in block.instructions() {
                if let Instruction::LoadVar { name, .. } = instruction {
                    names.insert(name.as_str());
                }
            }
        }
        names
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
    pub fn clear_source_span(&mut self) {
        self.source_span = None;
    }

    pub fn needs_prototype(&self) -> bool {
        self.needs_prototype
    }

    pub fn set_needs_prototype(&mut self, v: bool) {
        self.needs_prototype = v;
    }

    /// 该函数是否可直接调用（由 semantic direct_call pass 计算）。
    pub fn direct_callable(&self) -> bool {
        self.direct_callable
    }

    pub fn set_direct_callable(&mut self, v: bool) {
        self.direct_callable = v;
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
        if self.direct_callable {
            let _ = write!(out, " [direct_callable]");
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        dest: ValueId,
        object: ValueId,
        key: ValueId,
        value: ValueId,
        /// 该赋值点是否处于严格模式代码：基元接收者上写入失败时
        /// strict 抛 TypeError，sloppy 静默 no-op（PutValue 步骤 6.c）。
        #[serde(default)]
        strict: bool,
    },
    /// 以 own data property 语义创建对象属性，不触发原型链 setter。
    /// 成功时返回 object，失败时返回 TAG_EXCEPTION。
    CreateDataProperty {
        dest: ValueId,
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
    /// 从编译期常量模板克隆一个新的可变数组对象。
    CloneArrayTemplate {
        dest: ValueId,
        template: ConstantId,
    },
    /// 以静态 SSO 键模板初始化对象：单次 TLAB bump + 直写 value slot。
    InitObjectLiteral {
        dest: ValueId,
        template: ConstantId,
        values: Vec<ValueId>,
    },
    /// 按数字索引读取数组元素
    GetElem {
        dest: ValueId,
        object: ValueId,
        index: ValueId,
    },
    /// 按数字索引写入数组元素
    SetElem {
        dest: ValueId,
        object: ValueId,
        index: ValueId,
        value: ValueId,
        /// 与 [`Instruction::SetProp`] 的 `strict` 含义一致。
        #[serde(default)]
        strict: bool,
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
    /// 元素 shape 守卫（licm elem-guard 外提专用，仅出现在循环 pre-header）：
    /// 宿主一次性校验 `array` 当前是无洞的普通 packed 数组、全部元素都是
    /// shape 等于 `template` 烘焙 shape 的普通对象、且元素各值槽均非对象
    /// （消除循环内 ToPrimitive 触发用户代码的可能）。产出布尔值；false 时
    /// 同循环的 Guarded 指令逐次退回通用路径，语义不变。
    ElemShapeGuard {
        dest: ValueId,
        array: ValueId,
        template: ConstantId,
    },
    /// 语义与 [`Instruction::GetElem`] 完全一致；唯一区别是宿主 miss 路径
    /// 可能执行用户代码（原型链上的索引 accessor / ToPropertyKey 回调），
    /// 进入宿主前后端必须先把 `guard` 的变量置 false，使同循环全部
    /// GetPropGuarded 快路径失效（true→false 单向闩锁，只关闭优化）。
    GetElemGuarded {
        dest: ValueId,
        object: ValueId,
        index: ValueId,
        guard: ValueId,
    },
    /// 语义与 [`Instruction::GetProp`] 完全一致；`guard` 为 true 时 receiver
    /// 已被 [`Instruction::ElemShapeGuard`] 证明持有 `template` 的烘焙 shape，
    /// 后端跳过逐迭代 shape 检查，直接按模板槽偏移读取；其余情况先把
    /// `guard` 置 false 再走完整 GetProp IC。
    GetPropGuarded {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
        guard: ValueId,
        template: ConstantId,
    },
    /// 可选链调用：callee?.(...args)，callee 为 null/undefined 时返回 undefined
    OptionalCall {
        dest: ValueId,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    /// 对象 spread：将 source 的 own enumerable 属性复制到 object。
    /// dest 接收运行时结果——成功为 object 本身，属性读取/getter 抛错时为
    /// TAG_EXCEPTION，由 lowering 插桩检查以按 CopyDataProperties 传播异常。
    ObjectSpread {
        dest: ValueId,
        object: ValueId,
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
    /// 推测内联防卫：callee 底层函数 == function 时为 boxed true，否则 boxed false。
    /// 失配时调用点必须回退动态调用（语义由内联 pass 的回退分支保证）。
    GuardSameFunction {
        dest: ValueId,
        callee: ValueId,
        function: FunctionId,
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
            Self::SetProp {
                dest,
                object,
                key,
                value,
                strict,
            } => {
                write!(formatter, "{dest} = set_prop {object}, {key}, {value}")?;
                if *strict {
                    formatter.write_str(", strict")?;
                }
                Ok(())
            }
            Self::CreateDataProperty {
                dest,
                object,
                key,
                value,
            } => {
                write!(
                    formatter,
                    "{dest} = create_data_property {object}, {key}, {value}"
                )
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
            Self::CloneArrayTemplate { dest, template } => {
                write!(formatter, "{dest} = clone_array_template({template})")
            }
            Self::InitObjectLiteral {
                dest,
                template,
                values,
            } => {
                write!(formatter, "{dest} = init_object_literal({template}")?;
                if !values.is_empty() {
                    formatter.write_str(", values=[")?;
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(formatter, "{value}")?;
                    }
                    formatter.write_char(']')?;
                }
                formatter.write_char(')')
            }
            Self::GetElem {
                dest,
                object,
                index,
            } => {
                write!(formatter, "{dest} = get_elem {object}, {index}")
            }
            Self::SetElem {
                dest,
                object,
                index,
                value,
                strict,
            } => {
                write!(formatter, "{dest} = set_elem {object}, {index}, {value}")?;
                if *strict {
                    formatter.write_str(", strict")?;
                }
                Ok(())
            }
            Self::OptionalGetProp { dest, object, key } => {
                write!(formatter, "{dest} = optional_get_prop {object}, {key}")
            }
            Self::OptionalGetElem { dest, object, key } => {
                write!(formatter, "{dest} = optional_get_elem {object}, {key}")
            }
            Self::ElemShapeGuard {
                dest,
                array,
                template,
            } => {
                write!(formatter, "{dest} = elem_shape_guard {array}, {template}")
            }
            Self::GetElemGuarded {
                dest,
                object,
                index,
                guard,
            } => {
                write!(
                    formatter,
                    "{dest} = get_elem_guarded {object}, {index}, guard={guard}"
                )
            }
            Self::GetPropGuarded {
                dest,
                object,
                key,
                guard,
                template,
            } => {
                write!(
                    formatter,
                    "{dest} = get_prop_guarded {object}, {key}, guard={guard}, template={template}"
                )
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
            Self::ObjectSpread {
                dest,
                object,
                source,
            } => {
                write!(formatter, "{dest} = object_spread {source} into {object}")
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
            Self::GuardSameFunction {
                dest,
                callee,
                function,
            } => write!(
                formatter,
                "{dest} = guard_same_function {callee}, @{function}",
                function = function.0
            ),
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

impl Instruction {
    /// 重映射指令内全部 ValueId 操作数（含 Phi source 值）。
    ///
    /// builtin 段入口函数 inline 进用户 `$module_main` 时用：两段各自的 ValueId
    /// 都从 0 编号，拼接前必须把段内 ValueId 加上用户函数的偏移。
    pub fn remap_values(&mut self, f: &mut impl FnMut(ValueId) -> ValueId) {
        match self {
            Self::Const { dest, .. } => *dest = f(*dest),
            Self::Binary { dest, lhs, rhs, .. } => {
                *dest = f(*dest);
                *lhs = f(*lhs);
                *rhs = f(*rhs);
            }
            Self::Unary { dest, value, .. } => {
                *dest = f(*dest);
                *value = f(*value);
            }
            Self::Compare { dest, lhs, rhs, .. } => {
                *dest = f(*dest);
                *lhs = f(*lhs);
                *rhs = f(*rhs);
            }
            Self::Phi { dest, sources } => {
                *dest = f(*dest);
                for source in sources {
                    source.value = f(source.value);
                }
            }
            Self::CallBuiltin { dest, args, .. } => {
                if let Some(dest) = dest {
                    *dest = f(*dest);
                }
                for arg in args {
                    *arg = f(*arg);
                }
            }
            Self::StringConcatVa { dest, parts } => {
                *dest = f(*dest);
                for part in parts {
                    *part = f(*part);
                }
            }
            Self::LoadVar { dest, .. } => *dest = f(*dest),
            Self::StoreVar { value, .. } => *value = f(*value),
            Self::Call {
                dest,
                callee,
                this_val,
                args,
                ..
            } => {
                if let Some(dest) = dest {
                    *dest = f(*dest);
                }
                *callee = f(*callee);
                *this_val = f(*this_val);
                for arg in args {
                    *arg = f(*arg);
                }
            }
            Self::SuperCall {
                dest,
                callee,
                this_val,
                args,
                ..
            } => {
                if let Some(dest) = dest {
                    *dest = f(*dest);
                }
                *callee = f(*callee);
                *this_val = f(*this_val);
                for arg in args {
                    *arg = f(*arg);
                }
            }
            Self::ConstructCall {
                dest,
                callee,
                this_val,
                args,
                ..
            } => {
                if let Some(dest) = dest {
                    *dest = f(*dest);
                }
                *callee = f(*callee);
                *this_val = f(*this_val);
                for arg in args {
                    *arg = f(*arg);
                }
            }
            Self::NewObject { dest, .. }
            | Self::NewArray { dest, .. }
            | Self::CloneArrayTemplate { dest, .. } => *dest = f(*dest),
            Self::InitObjectLiteral { dest, values, .. } => {
                *dest = f(*dest);
                for value in values {
                    *value = f(*value);
                }
            }
            Self::GetProp { dest, object, key } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
            }
            Self::SetProp {
                dest,
                object,
                key,
                value,
                ..
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
                *value = f(*value);
            }
            Self::CreateDataProperty {
                dest,
                object,
                key,
                value,
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
                *value = f(*value);
            }
            Self::DeleteProp { dest, object, key } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
            }
            Self::SetProto { object, value } => {
                *object = f(*object);
                *value = f(*value);
            }
            Self::GetElem {
                dest,
                object,
                index,
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *index = f(*index);
            }
            Self::SetElem {
                dest,
                object,
                index,
                value,
                ..
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *index = f(*index);
                *value = f(*value);
            }
            Self::OptionalGetProp { dest, object, key } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
            }
            Self::OptionalGetElem { dest, object, key } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
            }
            Self::ElemShapeGuard { dest, array, .. } => {
                *dest = f(*dest);
                *array = f(*array);
            }
            Self::GetElemGuarded {
                dest,
                object,
                index,
                guard,
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *index = f(*index);
                *guard = f(*guard);
            }
            Self::GetPropGuarded {
                dest,
                object,
                key,
                guard,
                ..
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *key = f(*key);
                *guard = f(*guard);
            }
            Self::OptionalCall {
                dest,
                callee,
                this_val,
                args,
            } => {
                *dest = f(*dest);
                *callee = f(*callee);
                *this_val = f(*this_val);
                for arg in args {
                    *arg = f(*arg);
                }
            }
            Self::ObjectSpread {
                dest,
                object,
                source,
            } => {
                *dest = f(*dest);
                *object = f(*object);
                *source = f(*source);
            }
            Self::GetSuperBase { dest } | Self::GetSuperConstructor { dest } => {
                *dest = f(*dest);
            }
            Self::NewPromise { dest } => *dest = f(*dest),
            Self::PromiseResolve { promise, value } => {
                *promise = f(*promise);
                *value = f(*value);
            }
            Self::PromiseReject { promise, reason } => {
                *promise = f(*promise);
                *reason = f(*reason);
            }
            Self::Suspend { promise, .. } => *promise = f(*promise),
            Self::GeneratorSuspend { result, .. } => *result = f(*result),
            Self::CollectRestArgs { dest, .. } => *dest = f(*dest),
            Self::IsException { dest, value } => {
                *dest = f(*dest);
                *value = f(*value);
            }
            Self::GuardSameFunction { dest, callee, .. } => {
                *dest = f(*dest);
                *callee = f(*callee);
            }
            Self::EncodeException { dest, value } => {
                *dest = f(*dest);
                *value = f(*value);
            }
            Self::ExceptionToObject { dest, value } => {
                *dest = f(*dest);
                *value = f(*value);
            }
            Self::DebugCheck { .. } => {}
        }
    }

    /// 重映射指令内全部 BasicBlockId 引用（Phi source 前驱块）。
    pub fn remap_blocks(&mut self, f: &mut impl FnMut(BasicBlockId) -> BasicBlockId) {
        if let Self::Phi { sources, .. } = self {
            for source in sources {
                source.predecessor = f(source.predecessor);
            }
        }
    }
}

impl Terminator {
    /// 重映射终止器内全部 ValueId 操作数（Return/Branch/Switch/Throw 值）。
    pub fn remap_values(&mut self, f: &mut impl FnMut(ValueId) -> ValueId) {
        match self {
            Self::Return { value } => {
                if let Some(value) = value {
                    *value = f(*value);
                }
            }
            Self::Branch { condition, .. } => *condition = f(*condition),
            Self::Switch { value, .. } => *value = f(*value),
            Self::Throw { value } => *value = f(*value),
            Self::Jump { .. } | Self::Unreachable => {}
        }
    }

    /// 重映射终止器内全部 BasicBlockId 目标。
    pub fn remap_blocks(&mut self, f: &mut impl FnMut(BasicBlockId) -> BasicBlockId) {
        match self {
            Self::Jump { target } => *target = f(*target),
            Self::Branch {
                true_block,
                false_block,
                ..
            } => {
                *true_block = f(*true_block);
                *false_block = f(*false_block);
            }
            Self::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                for case in cases {
                    case.target = f(case.target);
                }
                *default_block = f(*default_block);
                *exit_block = f(*exit_block);
            }
            Self::Return { .. } | Self::Throw { .. } | Self::Unreachable => {}
        }
    }
}

// BinaryOp, UnaryOp, CompareOp → types.rs

// Terminator, SwitchCaseTarget, PhiSource → types.rs

// ConstantId, FunctionId, BasicBlockId, ValueId, ModuleId → types.rs

/// 合成模块顶层入口的 IR 函数名（与用户声明的 `main` 区分，避免入口约定冲突）。
pub const MODULE_ENTRY_IR_NAME: &str = "$module_main";

/// 合成 builtin 段入口的 IR 函数名（由 `build_builtin_segment` 把 `$module_main` 改名而来）。
pub const BUILTIN_ENTRY_IR_NAME: &str = "$builtin_main";

/// 是否为编译器合成的 builtin 段入口函数。
pub fn is_builtin_entry_ir_function(name: &str) -> bool {
    name == BUILTIN_ENTRY_IR_NAME
}

fn remap_user_function_ref(constant: Constant, split: u32, user_count: u32) -> Constant {
    match constant {
        Constant::FunctionRef(FunctionId(id)) if id >= split => {
            Constant::FunctionRef(FunctionId(id - split))
        }
        Constant::FunctionRef(FunctionId(id)) => Constant::FunctionRef(FunctionId(user_count + id)),
        other => other,
    }
}

fn max_constant_id_in(functions: &[Function]) -> Option<usize> {
    let mut max = None;
    for function in functions {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Const { constant, .. } = instruction {
                    let index = usize::try_from(constant.0).ok()?;
                    max = Some(max.map_or(index, |current: usize| current.max(index)));
                }
            }
            if let Terminator::Switch { cases, .. } = block.terminator() {
                for case in cases {
                    let index = usize::try_from(case.constant.0).ok()?;
                    max = Some(max.map_or(index, |current: usize| current.max(index)));
                }
            }
        }
    }
    max
}

/// 是否为编译器合成的模块入口函数（非用户 `function main()`）。
pub fn is_module_entry_ir_function(name: &str) -> bool {
    name == MODULE_ENTRY_IR_NAME
}

/// Import 绑定信息（用于模块系统）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReExportBinding {
    /// 被重导出的源模块 ID
    pub source_module: ModuleId,
    /// `export { local as exported } from` 的 local；`export *` 时为 None
    pub local_name: Option<String>,
    /// 当前模块对外导出名；`export *` 时为 None（表示复制源模块全部导出）
    pub exported_name: Option<String>,
}

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
/// 0x06 = string（字符串对象，payload 为原始字节或子引用句柄）
pub const HEAP_TYPE_STRING: u8 = 0x06;
pub const HEAP_TYPE_MODULE_NAMESPACE: u8 = 0x08;
