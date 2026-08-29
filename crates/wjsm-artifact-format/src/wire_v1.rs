use std::collections::BTreeSet;

use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, DeoptFrame,
    Function, FunctionId, HomeObject, Instruction, ModuleId, PhiSource, Program, SourceSpan,
    SwitchCaseTarget, Terminator, UnaryOp, ValueId,
};

use crate::{ArtifactFormatError, ArtifactLimits, ManifestModule, ModuleKind, ModuleManifest};

pub(crate) fn encode_manifest(manifest: &ModuleManifest) -> Result<Vec<u8>, ArtifactFormatError> {
    let mut encoder = Encoder::default();
    encoder.u32(manifest.entry.0);

    let mut modules: Vec<_> = manifest.modules.iter().collect();
    modules.sort_by_key(|module| module.id.0);
    encoder.len(modules.len())?;
    for module in modules {
        encode_manifest_module(&mut encoder, module)?;
    }

    let mut conditions = manifest.resolution_conditions.clone();
    conditions.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    conditions.dedup();
    encoder.strings(&conditions)?;
    Ok(encoder.finish())
}

fn encode_manifest_module(
    encoder: &mut Encoder,
    module: &ManifestModule,
) -> Result<(), ArtifactFormatError> {
    encoder.u32(module.id.0);
    encoder.string(&module.logical_url)?;
    encoder.u16(module.kind as u16);

    let mut static_dependencies = module.static_dependencies.clone();
    static_dependencies.sort_by_key(|id| id.0);
    static_dependencies.dedup();
    encoder.len(static_dependencies.len())?;
    for dependency in static_dependencies {
        encoder.u32(dependency.0);
    }

    let mut dynamic_dependencies = module.dynamic_dependencies.clone();
    dynamic_dependencies.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then_with(|| left.1.0.cmp(&right.1.0))
    });
    dynamic_dependencies.dedup();
    encoder.len(dynamic_dependencies.len())?;
    for (specifier, dependency) in dynamic_dependencies {
        encoder.string(&specifier)?;
        encoder.u32(dependency.0);
    }
    Ok(())
}

pub(crate) fn decode_manifest(
    bytes: &[u8],
    limits: &ArtifactLimits,
) -> Result<ModuleManifest, ArtifactFormatError> {
    let mut decoder = Decoder::new(bytes, limits);
    let entry = ModuleId(decoder.u32()?);
    let module_count = decoder.count(limits.max_modules)?;
    let mut modules = Vec::with_capacity(module_count);
    for _ in 0..module_count {
        let id = ModuleId(decoder.u32()?);
        let logical_url = decoder.string()?;
        let kind_tag = decoder.u16()?;
        let kind = ModuleKind::from_wire(kind_tag).ok_or(ArtifactFormatError::UnknownTag(
            "module kind",
            kind_tag.into(),
        ))?;
        let static_count = decoder.count(limits.max_module_edges)?;
        let mut static_dependencies = Vec::with_capacity(static_count);
        for _ in 0..static_count {
            static_dependencies.push(ModuleId(decoder.u32()?));
        }
        let dynamic_count = decoder.count(limits.max_module_edges)?;
        let mut dynamic_dependencies = Vec::with_capacity(dynamic_count);
        for _ in 0..dynamic_count {
            dynamic_dependencies.push((decoder.string()?, ModuleId(decoder.u32()?)));
        }
        modules.push(ManifestModule {
            id,
            logical_url,
            kind,
            static_dependencies,
            dynamic_dependencies,
        });
    }
    let resolution_conditions = decoder.strings(limits.max_strings)?;
    decoder.finish()?;
    Ok(ModuleManifest {
        entry,
        modules,
        resolution_conditions,
    })
}

pub(crate) fn encode_program(program: &Program) -> Result<Vec<u8>, ArtifactFormatError> {
    let mut encoder = Encoder::default();
    encoder.bool(program.script_mode());
    encoder.optional_string(program.source_file())?;
    encoder.len(program.constants().len())?;
    for (index, constant) in program.constants().iter().enumerate() {
        let id = ConstantId(u32::try_from(index).map_err(|_| ArtifactFormatError::LengthOverflow)?);
        let meta = program.string_constant_meta(id);
        encode_constant(&mut encoder, constant, meta)?;
    }
    encoder.len(program.functions().len())?;
    for function in program.functions() {
        encode_function(&mut encoder, function)?;
    }
    Ok(encoder.finish())
}

pub(crate) fn decode_program(
    bytes: &[u8],
    limits: &ArtifactLimits,
) -> Result<Program, ArtifactFormatError> {
    let mut decoder = Decoder::new(bytes, limits);
    let script_mode = decoder.bool()?;
    let source_file = decoder.optional_string()?;
    let constant_count = decoder.count(limits.max_constants)?;
    let mut program = Program::new();
    program.set_script_mode(script_mode);
    if let Some(source_file) = source_file {
        program.set_source_file(source_file);
    }
    for _ in 0..constant_count {
        let (constant, meta) = decode_constant(&mut decoder, limits)?;
        // wire 已携带烘焙元数据，直接填充：解码侧零哈希、零表示转换。
        program.add_constant_with_meta(constant, meta);
    }
    let function_count = decoder.count(limits.max_functions)?;
    for _ in 0..function_count {
        program.push_function(decode_function(&mut decoder, limits)?);
    }
    decoder.finish()?;
    Ok(program)
}

fn encode_constant(
    encoder: &mut Encoder,
    constant: &Constant,
    meta: Option<&wjsm_ir::StringConstantMeta>,
) -> Result<(), ArtifactFormatError> {
    match constant {
        Constant::Number(value) => {
            encoder.u16(0);
            encoder.u64(value.to_bits());
        }
        Constant::String(value) => {
            encoder.u16(1);
            // 元数据缺失（serde 兼容回退路径）时就地重算，保证编码形状一致。
            let owned;
            let meta = match meta {
                Some(meta) => meta,
                None => {
                    owned = wjsm_ir::StringConstantMeta::from_text(value);
                    &owned
                }
            };
            encode_string_meta(encoder, meta)?;
        }
        Constant::Bool(value) => {
            encoder.u16(2);
            encoder.bool(*value);
        }
        Constant::Null => encoder.u16(3),
        Constant::Undefined => encoder.u16(4),
        Constant::FunctionRef(id) => {
            encoder.u16(5);
            encoder.u32(id.0);
        }
        Constant::NativeCallableEval => encoder.u16(6),
        Constant::BigInt(value) => {
            encoder.u16(7);
            encoder.string(value)?;
        }
        Constant::RegExp { pattern, flags } => {
            encoder.u16(8);
            encoder.string(pattern)?;
            encoder.string(flags)?;
        }
        Constant::ModuleId(id) => {
            encoder.u16(9);
            encoder.u32(id.0);
        }
        Constant::ArrayTemplate(elements) => {
            encoder.u16(10);
            encoder.len(elements.len())?;
            for element in elements {
                encoder.u32(element.0);
            }
        }
        Constant::ObjectTemplate { keys } => {
            encoder.u16(11);
            encoder.len(keys.len())?;
            for key in keys {
                encoder.u64(*key);
            }
        }
        Constant::Uninitialized => encoder.u16(12),
        Constant::Utf16String(units) => {
            encoder.u16(13);
            let owned;
            let meta = match meta {
                Some(meta) => meta,
                None => {
                    owned = wjsm_ir::StringConstantMeta::from_units(units);
                    &owned
                }
            };
            encode_string_meta(encoder, meta)?;
        }
    }
    Ok(())
}

/// 字符串常量的烘焙元数据编码（tag 1 / tag 13 共用形状）：
/// repr(1=Latin-1, 2=UTF-16LE) + hash + 码元长度 + 载荷。
fn encode_string_meta(
    encoder: &mut Encoder,
    meta: &wjsm_ir::StringConstantMeta,
) -> Result<(), ArtifactFormatError> {
    encoder.u8(if meta.latin1 { 1 } else { 2 });
    encoder.u32(meta.hash);
    encoder.len(meta.unit_len() as usize)?;
    encoder.len(meta.payload.len())?;
    encoder.bytes(&meta.payload);
    Ok(())
}

/// 解码字符串烘焙元数据并展开为 UTF-16 码元序列（tag 1 / tag 13 共用）。
fn decode_string_meta(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<(Vec<u16>, wjsm_ir::StringConstantMeta), ArtifactFormatError> {
    let repr = decoder.u8()?;
    let hash = decoder.u32()?;
    let unit_len = decoder.count(limits.max_string_bytes)?;
    let payload_len = decoder.count(limits.max_string_bytes.saturating_mul(2))?;
    let payload = decoder.take(payload_len)?.to_vec();
    let (latin1, expected_len) = match repr {
        1 => (true, unit_len),
        2 => (
            false,
            unit_len
                .checked_mul(2)
                .ok_or(ArtifactFormatError::LengthOverflow)?,
        ),
        _ => return Err(ArtifactFormatError::InvalidStringPayload("repr")),
    };
    if payload.len() != expected_len {
        return Err(ArtifactFormatError::InvalidStringPayload("length"));
    }
    let units: Vec<u16> = if latin1 {
        payload.iter().map(|byte| u16::from(*byte)).collect()
    } else {
        payload
            .chunks(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect()
    };
    Ok((
        units,
        wjsm_ir::StringConstantMeta {
            hash,
            latin1,
            payload,
        },
    ))
}

/// 解码常量；String 变体同时返回烘焙元数据（hash 与载荷直接来自 wire）。
fn decode_constant(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<(Constant, Option<wjsm_ir::StringConstantMeta>), ArtifactFormatError> {
    let tag = decoder.u16()?;
    match tag {
        0 => Ok((Constant::Number(f64::from_bits(decoder.u64()?)), None)),
        1 => {
            let (units, meta) = decode_string_meta(decoder, limits)?;
            // tag 1 只承载良构 UTF-16；孤立代理项属于 tag 13（Utf16String）。
            let text = String::from_utf16(&units)
                .map_err(|_| ArtifactFormatError::InvalidStringPayload("utf16"))?;
            Ok((Constant::String(text), Some(meta)))
        }
        2 => Ok((Constant::Bool(decoder.bool()?), None)),
        3 => Ok((Constant::Null, None)),
        4 => Ok((Constant::Undefined, None)),
        5 => Ok((Constant::FunctionRef(FunctionId(decoder.u32()?)), None)),
        6 => Ok((Constant::NativeCallableEval, None)),
        7 => Ok((Constant::BigInt(decoder.string()?), None)),
        8 => Ok((
            Constant::RegExp {
                pattern: decoder.string()?,
                flags: decoder.string()?,
            },
            None,
        )),
        9 => Ok((Constant::ModuleId(ModuleId(decoder.u32()?)), None)),
        10 => {
            let count = decoder.count(limits.max_constants)?;
            let mut elements = Vec::with_capacity(count);
            for _ in 0..count {
                elements.push(ConstantId(decoder.u32()?));
            }
            Ok((Constant::ArrayTemplate(elements), None))
        }
        11 => {
            let count = decoder.count(limits.max_constants)?;
            let mut keys = Vec::with_capacity(count);
            for _ in 0..count {
                keys.push(decoder.u64()?);
            }
            Ok((Constant::ObjectTemplate { keys }, None))
        }
        12 => Ok((Constant::Uninitialized, None)),
        13 => {
            let (units, meta) = decode_string_meta(decoder, limits)?;
            Ok((Constant::Utf16String(units), Some(meta)))
        }
        _ => Err(ArtifactFormatError::UnknownTag("constant", tag.into())),
    }
}

fn encode_function(encoder: &mut Encoder, function: &Function) -> Result<(), ArtifactFormatError> {
    encoder.string(function.name())?;
    encoder.strings(function.params())?;
    encoder.u32(function.entry().0);
    encoder.bool(function.has_eval());
    encoder.strings(function.captured_names())?;
    encoder.strings(function.env_layout_keys())?;

    let mut known_callees: Vec<_> = function.known_callee_vars().iter().collect();
    known_callees.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    encoder.len(known_callees.len())?;
    for (name, id) in known_callees {
        encoder.string(name)?;
        encoder.u32(id.0);
    }

    match function.home_object {
        None => encoder.u16(0),
        Some(HomeObject::Prototype(id)) => {
            encoder.u16(1);
            encoder.u32(id.0);
        }
        Some(HomeObject::Constructor(id)) => {
            encoder.u16(2);
            encoder.u32(id.0);
        }
    }
    encoder.bool(function.needs_prototype());
    match function.source_span() {
        None => encoder.bool(false),
        Some(span) => {
            encoder.bool(true);
            encoder.u32(span.line);
            encoder.u32(span.col);
        }
    }
    encoder.bool(function.direct_callable());
    match function.class_ctor_name() {
        None => encoder.bool(false),
        Some(name) => {
            encoder.bool(true);
            encoder.string(name)?;
        }
    }
    match function.js_name() {
        None => encoder.bool(false),
        Some(name) => {
            encoder.bool(true);
            encoder.string(name)?;
        }
    }
    match function.js_length() {
        None => encoder.bool(false),
        Some(length) => {
            encoder.bool(true);
            encoder.u32(length);
        }
    }
    match function.source_text() {
        None => encoder.bool(false),
        Some(text) => {
            encoder.bool(true);
            encoder.string(text)?;
        }
    }
    encoder.len(function.blocks().len())?;
    for block in function.blocks() {
        encode_block(encoder, block)?;
    }
    Ok(())
}

fn decode_function(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<Function, ArtifactFormatError> {
    let name = decoder.string()?;
    let params = decoder.strings(limits.max_strings)?;
    let entry = BasicBlockId(decoder.u32()?);
    let mut function = Function::new(name, entry);
    function.set_params(params);
    function.set_has_eval(decoder.bool()?);
    function.set_captured_names(decoder.strings(limits.max_strings)?);
    function.set_env_layout_keys(decoder.strings(limits.max_strings)?);

    let known_callee_count = decoder.count(limits.max_strings)?;
    for _ in 0..known_callee_count {
        function.record_known_callee(decoder.string()?, FunctionId(decoder.u32()?));
    }
    function.home_object = match decoder.u16()? {
        0 => None,
        1 => Some(HomeObject::Prototype(FunctionId(decoder.u32()?))),
        2 => Some(HomeObject::Constructor(FunctionId(decoder.u32()?))),
        tag => return Err(ArtifactFormatError::UnknownTag("home object", tag.into())),
    };
    function.set_needs_prototype(decoder.bool()?);
    if decoder.bool()? {
        function.set_source_span(SourceSpan::new(decoder.u32()?, decoder.u32()?));
    }
    function.set_direct_callable(decoder.bool()?);
    if decoder.bool()? {
        function.set_class_ctor_name(decoder.string()?);
    }
    if decoder.bool()? {
        function.set_js_name(decoder.string()?);
    }
    if decoder.bool()? {
        function.set_js_length(decoder.u32()?);
    }
    if decoder.bool()? {
        function.set_source_text(decoder.string()?);
    }
    let block_count = decoder.count(limits.max_blocks_per_function)?;
    for _ in 0..block_count {
        function.push_block(decode_block(decoder, limits)?);
    }
    Ok(function)
}

fn encode_block(encoder: &mut Encoder, block: &BasicBlock) -> Result<(), ArtifactFormatError> {
    encoder.u32(block.id().0);
    encoder.len(block.instructions().len())?;
    for instruction in block.instructions() {
        encode_instruction(encoder, instruction)?;
    }
    encode_terminator(encoder, block.terminator())
}

fn decode_block(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<BasicBlock, ArtifactFormatError> {
    let id = BasicBlockId(decoder.u32()?);
    let instruction_count = decoder.count(limits.max_instructions_per_block)?;
    let mut block = BasicBlock::new(id);
    for _ in 0..instruction_count {
        block.push_instruction(decode_instruction(decoder, limits)?);
    }
    block.set_terminator(decode_terminator(decoder, limits)?);
    Ok(block)
}

fn encode_instruction(
    encoder: &mut Encoder,
    instruction: &Instruction,
) -> Result<(), ArtifactFormatError> {
    match instruction {
        Instruction::Const { dest, constant } => {
            encoder.u16(0);
            value_id(encoder, *dest);
            encoder.u32(constant.0);
        }
        Instruction::Binary { dest, op, lhs, rhs } => {
            encoder.u16(1);
            value_id(encoder, *dest);
            encoder.u16(binary_tag(*op));
            value_id(encoder, *lhs);
            value_id(encoder, *rhs);
        }
        Instruction::Unary { dest, op, value } => {
            encoder.u16(2);
            value_id(encoder, *dest);
            encoder.u16(unary_tag(*op));
            value_id(encoder, *value);
        }
        Instruction::Compare { dest, op, lhs, rhs } => {
            encoder.u16(3);
            value_id(encoder, *dest);
            encoder.u16(compare_tag(*op));
            value_id(encoder, *lhs);
            value_id(encoder, *rhs);
        }
        Instruction::Phi { dest, sources } => {
            encoder.u16(4);
            value_id(encoder, *dest);
            encoder.len(sources.len())?;
            for source in sources {
                encoder.u32(source.predecessor.0);
                value_id(encoder, source.value);
            }
        }
        Instruction::CallBuiltin {
            dest,
            builtin,
            args,
        } => {
            encoder.u16(5);
            optional_value_id(encoder, *dest);
            encoder.u16(builtin.wire_id());
            value_ids(encoder, args)?;
        }
        Instruction::StringConcatVa { dest, parts } => {
            encoder.u16(6);
            value_id(encoder, *dest);
            value_ids(encoder, parts)?;
        }
        Instruction::LoadVar { dest, name } => {
            encoder.u16(7);
            value_id(encoder, *dest);
            encoder.string(name)?;
        }
        Instruction::StoreVar { name, value } => {
            encoder.u16(8);
            encoder.string(name)?;
            value_id(encoder, *value);
        }
        Instruction::Call {
            dest,
            callee,
            this_val,
            args,
            callsite,
        } => {
            encoder.u16(9);
            encode_call(encoder, *dest, *callee, *this_val, args)?;
            encoder.optional_string(callsite.as_deref())?;
        }
        Instruction::SuperCall {
            dest,
            callee,
            this_val,
            args,
            forward_args,
        } => {
            encoder.u16(10);
            encode_call(encoder, *dest, *callee, *this_val, args)?;
            encoder.bool(*forward_args);
        }
        Instruction::ConstructCall {
            dest,
            callee,
            this_val,
            args,
            callsite,
        } => {
            encoder.u16(11);
            encode_call(encoder, *dest, *callee, *this_val, args)?;
            encoder.optional_string(callsite.as_deref())?;
        }
        Instruction::NewObject { dest, capacity } => {
            encoder.u16(12);
            value_id(encoder, *dest);
            encoder.u32(*capacity);
        }
        Instruction::GetProp {
            dest,
            object,
            key,
            latch,
            latch_template,
        } => {
            encoder.u16(13);
            three_values(encoder, *dest, *object, *key);
            optional_value_id(encoder, *latch);
            encoder.u32(latch_template.map(|id| id.0).unwrap_or(u32::MAX));
        }
        Instruction::SetProp {
            dest,
            object,
            key,
            value,
            strict,
        } => {
            encoder.u16(14);
            value_id(encoder, *dest);
            value_id(encoder, *object);
            value_id(encoder, *key);
            value_id(encoder, *value);
            encoder.u16(u16::from(*strict));
        }
        Instruction::DeleteProp {
            dest,
            object,
            key,
            strict,
        } => {
            encoder.u16(15);
            three_values(encoder, *dest, *object, *key);
            encoder.u16(u16::from(*strict));
        }
        Instruction::SetProto { object, value } => {
            encoder.u16(16);
            two_values(encoder, *object, *value);
        }
        Instruction::NewArray { dest, capacity } => {
            encoder.u16(17);
            value_id(encoder, *dest);
            encoder.u32(*capacity);
        }
        Instruction::CloneArrayTemplate { dest, template } => {
            encoder.u16(38);
            value_id(encoder, *dest);
            encoder.u32(template.0);
        }
        Instruction::GetElem {
            dest,
            object,
            index,
            latch,
        } => {
            encoder.u16(18);
            three_values(encoder, *dest, *object, *index);
            optional_value_id(encoder, *latch);
        }
        Instruction::SetElem {
            dest,
            object,
            index,
            value,
            strict,
        } => {
            encoder.u16(19);
            value_id(encoder, *dest);
            value_id(encoder, *object);
            value_id(encoder, *index);
            value_id(encoder, *value);
            encoder.u16(u16::from(*strict));
        }
        Instruction::ObjectSpread {
            dest,
            object,
            source,
        } => {
            encoder.u16(23);
            three_values(encoder, *dest, *object, *source);
        }
        Instruction::GetSuperBase { dest } => {
            encoder.u16(24);
            value_id(encoder, *dest);
        }
        Instruction::GetSuperConstructor { dest } => {
            encoder.u16(25);
            value_id(encoder, *dest);
        }
        Instruction::NewPromise { dest } => {
            encoder.u16(26);
            value_id(encoder, *dest);
        }
        Instruction::PromiseResolve { promise, value } => {
            encoder.u16(27);
            two_values(encoder, *promise, *value);
        }
        Instruction::PromiseReject { promise, reason } => {
            encoder.u16(28);
            two_values(encoder, *promise, *reason);
        }
        Instruction::Suspend { promise, state } => {
            encoder.u16(29);
            value_id(encoder, *promise);
            encoder.u32(*state);
        }
        Instruction::GeneratorSuspend { result, state } => {
            encoder.u16(30);
            value_id(encoder, *result);
            encoder.u32(*state);
        }
        Instruction::CollectRestArgs { dest, skip } => {
            encoder.u16(31);
            value_id(encoder, *dest);
            encoder.u32(*skip);
        }
        Instruction::IsException { dest, value } => {
            encoder.u16(32);
            two_values(encoder, *dest, *value);
        }
        Instruction::GuardSameFunction {
            dest,
            callee,
            function,
        } => {
            encoder.u16(33);
            two_values(encoder, *dest, *callee);
            encoder.u32(function.0);
        }
        Instruction::EncodeException { dest, value } => {
            encoder.u16(34);
            two_values(encoder, *dest, *value);
        }
        Instruction::ExceptionToObject { dest, value } => {
            encoder.u16(35);
            two_values(encoder, *dest, *value);
        }
        Instruction::DebugCheck { line, col } => {
            encoder.u16(36);
            encoder.u32(*line);
            encoder.u32(*col);
        }
        Instruction::CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => {
            encoder.u16(37);
            value_id(encoder, *dest);
            value_id(encoder, *object);
            value_id(encoder, *key);
            value_id(encoder, *value);
        }
        Instruction::InitObjectLiteral {
            dest,
            template,
            values,
        } => {
            encoder.u16(39);
            value_id(encoder, *dest);
            encoder.u32(template.0);
            value_ids(encoder, values)?;
        }
        Instruction::GuardTag { dest, value, tag } => {
            encoder.u16(43);
            two_values(encoder, *dest, *value);
            encoder.u8(*tag);
        }
        Instruction::GuardShape {
            dest,
            object,
            shape_id,
        } => {
            encoder.u16(44);
            two_values(encoder, *dest, *object);
            encoder.u32(*shape_id);
        }
        Instruction::GuardElementsKind {
            dest,
            array,
            kind,
            template,
        } => {
            encoder.u16(45);
            two_values(encoder, *dest, *array);
            encoder.u32(*kind);
            encoder.u32(template.map(|id| id.0).unwrap_or(u32::MAX));
        }
        Instruction::GuardCallTarget {
            dest,
            callee,
            function,
        } => {
            encoder.u16(46);
            two_values(encoder, *dest, *callee);
            encoder.u32(function.0);
        }
        Instruction::LoadSlot {
            dest,
            object,
            index,
        } => {
            encoder.u16(47);
            two_values(encoder, *dest, *object);
            encoder.u32(*index);
        }
        Instruction::StoreSlot {
            dest,
            object,
            index,
            value,
            transition_shape,
        } => {
            encoder.u16(48);
            optional_value_id(encoder, *dest);
            two_values(encoder, *object, *value);
            encoder.u32(*index);
            encoder.u32(transition_shape.unwrap_or(u32::MAX));
        }
        Instruction::LoadEnvSlot { dest, env, slot, key } => {
            encoder.u16(49);
            two_values(encoder, *dest, *env);
            encoder.u32(*slot);
            value_id(encoder, *key);
        }
        Instruction::StoreEnvSlot {
            dest,
            env,
            slot,
            value,
            key,
            strict,
        } => {
            encoder.u16(50);
            optional_value_id(encoder, *dest);
            two_values(encoder, *env, *value);
            encoder.u32(*slot);
            value_id(encoder, *key);
            encoder.bool(*strict);
        }
    }
    Ok(())
}

fn decode_instruction(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<Instruction, ArtifactFormatError> {
    let tag = decoder.u16()?;
    match tag {
        0 => Ok(Instruction::Const {
            dest: next_value(decoder)?,
            constant: ConstantId(decoder.u32()?),
        }),
        1 => Ok(Instruction::Binary {
            dest: next_value(decoder)?,
            op: decode_binary(decoder.u16()?)?,
            lhs: next_value(decoder)?,
            rhs: next_value(decoder)?,
        }),
        2 => Ok(Instruction::Unary {
            dest: next_value(decoder)?,
            op: decode_unary(decoder.u16()?)?,
            value: next_value(decoder)?,
        }),
        3 => Ok(Instruction::Compare {
            dest: next_value(decoder)?,
            op: decode_compare(decoder.u16()?)?,
            lhs: next_value(decoder)?,
            rhs: next_value(decoder)?,
        }),
        4 => {
            let dest = next_value(decoder)?;
            let count = decoder.count(limits.max_phi_sources)?;
            let mut sources = Vec::with_capacity(count);
            for _ in 0..count {
                sources.push(PhiSource {
                    predecessor: BasicBlockId(decoder.u32()?),
                    value: next_value(decoder)?,
                });
            }
            Ok(Instruction::Phi { dest, sources })
        }
        5 => {
            let dest = decode_optional_value(decoder)?;
            let builtin_id = decoder.u16()?;
            let builtin = Builtin::from_wire_id(builtin_id).ok_or(
                ArtifactFormatError::UnknownTag("builtin", builtin_id.into()),
            )?;
            Ok(Instruction::CallBuiltin {
                dest,
                builtin,
                args: decode_value_ids(decoder, limits)?,
            })
        }
        6 => Ok(Instruction::StringConcatVa {
            dest: next_value(decoder)?,
            parts: decode_value_ids(decoder, limits)?,
        }),
        7 => Ok(Instruction::LoadVar {
            dest: next_value(decoder)?,
            name: decoder.string()?,
        }),
        8 => Ok(Instruction::StoreVar {
            name: decoder.string()?,
            value: next_value(decoder)?,
        }),
        9 => {
            let (dest, callee, this_val, args) = decode_call(decoder, limits)?;
            Ok(Instruction::Call {
                dest,
                callee,
                this_val,
                args,
                callsite: decoder.optional_string()?.map(String::into_boxed_str),
            })
        }
        10 => {
            let (dest, callee, this_val, args) = decode_call(decoder, limits)?;
            Ok(Instruction::SuperCall {
                dest,
                callee,
                this_val,
                args,
                forward_args: decoder.bool()?,
            })
        }
        11 => {
            let (dest, callee, this_val, args) = decode_call(decoder, limits)?;
            Ok(Instruction::ConstructCall {
                dest,
                callee,
                this_val,
                args,
                callsite: decoder.optional_string()?.map(String::into_boxed_str),
            })
        }
        12 => Ok(Instruction::NewObject {
            dest: next_value(decoder)?,
            capacity: decoder.u32()?,
        }),
        13 => {
            let (dest, object, key) = decode_three(decoder)?;
            let latch = decode_optional_value(decoder)?;
            let template = decoder.u32()?;
            Ok(Instruction::GetProp {
                dest,
                object,
                key,
                latch,
                latch_template: (template != u32::MAX).then_some(ConstantId(template)),
            })
        }
        14 => Ok(Instruction::SetProp {
            dest: next_value(decoder)?,
            object: next_value(decoder)?,
            key: next_value(decoder)?,
            value: next_value(decoder)?,
            strict: decoder.u16()? != 0,
        }),
        15 => {
            let (dest, object, key) = decode_three(decoder)?;
            Ok(Instruction::DeleteProp {
                dest,
                object,
                key,
                strict: decoder.u16()? != 0,
            })
        }
        16 => {
            let (object, value) = decode_two(decoder)?;
            Ok(Instruction::SetProto { object, value })
        }
        17 => Ok(Instruction::NewArray {
            dest: next_value(decoder)?,
            capacity: decoder.u32()?,
        }),
        18 => {
            let (dest, object, index) = decode_three(decoder)?;
            let latch = decode_optional_value(decoder)?;
            Ok(Instruction::GetElem {
                dest,
                object,
                index,
                latch,
            })
        }
        19 => Ok(Instruction::SetElem {
            dest: next_value(decoder)?,
            object: next_value(decoder)?,
            index: next_value(decoder)?,
            value: next_value(decoder)?,
            strict: decoder.u16()? != 0,
        }),
        // 20/21/22 曾是 OptionalGetProp / OptionalGetElem / OptionalCall：
        // 可选链改为链级短路分叉后由普通 GetProp / GetElem / Call 承载，
        // 编号保留空洞（FORMAT_VERSION 已提升，旧制品按版本拒收）。
        23 => {
            let (dest, object, source) = decode_three(decoder)?;
            Ok(Instruction::ObjectSpread {
                dest,
                object,
                source,
            })
        }
        24 => Ok(Instruction::GetSuperBase {
            dest: next_value(decoder)?,
        }),
        25 => Ok(Instruction::GetSuperConstructor {
            dest: next_value(decoder)?,
        }),
        26 => Ok(Instruction::NewPromise {
            dest: next_value(decoder)?,
        }),
        27 => {
            let (promise, value) = decode_two(decoder)?;
            Ok(Instruction::PromiseResolve { promise, value })
        }
        28 => {
            let (promise, reason) = decode_two(decoder)?;
            Ok(Instruction::PromiseReject { promise, reason })
        }
        29 => Ok(Instruction::Suspend {
            promise: next_value(decoder)?,
            state: decoder.u32()?,
        }),
        30 => Ok(Instruction::GeneratorSuspend {
            result: next_value(decoder)?,
            state: decoder.u32()?,
        }),
        31 => Ok(Instruction::CollectRestArgs {
            dest: next_value(decoder)?,
            skip: decoder.u32()?,
        }),
        32 => {
            let (dest, value) = decode_two(decoder)?;
            Ok(Instruction::IsException { dest, value })
        }
        33 => {
            let (dest, callee) = decode_two(decoder)?;
            Ok(Instruction::GuardSameFunction {
                dest,
                callee,
                function: FunctionId(decoder.u32()?),
            })
        }
        34 => {
            let (dest, value) = decode_two(decoder)?;
            Ok(Instruction::EncodeException { dest, value })
        }
        35 => {
            let (dest, value) = decode_two(decoder)?;
            Ok(Instruction::ExceptionToObject { dest, value })
        }
        36 => Ok(Instruction::DebugCheck {
            line: decoder.u32()?,
            col: decoder.u32()?,
        }),
        37 => {
            let dest = next_value(decoder)?;
            let object = next_value(decoder)?;
            let key = next_value(decoder)?;
            let value = next_value(decoder)?;
            Ok(Instruction::CreateDataProperty {
                dest,
                object,
                key,
                value,
            })
        }
        38 => Ok(Instruction::CloneArrayTemplate {
            dest: next_value(decoder)?,
            template: ConstantId(decoder.u32()?),
        }),
        39 => {
            let dest = next_value(decoder)?;
            let template = ConstantId(decoder.u32()?);
            let count = decoder.count(wjsm_ir::constants::OBJECT_TEMPLATE_MAX_PROPS)?;
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(next_value(decoder)?);
            }
            Ok(Instruction::InitObjectLiteral {
                dest,
                template,
                values,
            })
        }
        40 | 41 | 42 => Err(ArtifactFormatError::UnknownTag("instruction", tag.into())),
        43 => {
            let (dest, value) = decode_two(decoder)?;
            Ok(Instruction::GuardTag {
                dest,
                value,
                tag: decoder.u8()?,
            })
        }
        44 => {
            let (dest, object) = decode_two(decoder)?;
            Ok(Instruction::GuardShape {
                dest,
                object,
                shape_id: decoder.u32()?,
            })
        }
        45 => {
            let (dest, array) = decode_two(decoder)?;
            let kind = decoder.u32()?;
            let template = decoder.u32()?;
            Ok(Instruction::GuardElementsKind {
                dest,
                array,
                kind,
                template: (template != u32::MAX).then_some(ConstantId(template)),
            })
        }
        46 => {
            let (dest, callee) = decode_two(decoder)?;
            Ok(Instruction::GuardCallTarget {
                dest,
                callee,
                function: FunctionId(decoder.u32()?),
            })
        }
        47 => {
            let (dest, object) = decode_two(decoder)?;
            Ok(Instruction::LoadSlot {
                dest,
                object,
                index: decoder.u32()?,
            })
        }
        48 => {
            let dest = decode_optional_value(decoder)?;
            let (object, value) = decode_two(decoder)?;
            let index = decoder.u32()?;
            let transition = decoder.u32()?;
            Ok(Instruction::StoreSlot {
                dest,
                object,
                index,
                value,
                transition_shape: (transition != u32::MAX).then_some(transition),
            })
        }
        49 => {
            let (dest, env) = decode_two(decoder)?;
            let slot = decoder.u32()?;
            let key = next_value(decoder)?;
            Ok(Instruction::LoadEnvSlot {
                dest,
                env,
                slot,
                key,
            })
        }
        50 => {
            let dest = decode_optional_value(decoder)?;
            let (env, value) = decode_two(decoder)?;
            let slot = decoder.u32()?;
            let key = next_value(decoder)?;
            let strict = decoder.bool()?;
            Ok(Instruction::StoreEnvSlot {
                dest,
                env,
                slot,
                value,
                key,
                strict,
            })
        }
        _ => Err(ArtifactFormatError::UnknownTag("instruction", tag.into())),
    }
}

fn encode_terminator(
    encoder: &mut Encoder,
    terminator: &Terminator,
) -> Result<(), ArtifactFormatError> {
    match terminator {
        Terminator::Return { value } => {
            encoder.u16(0);
            optional_value_id(encoder, *value);
        }
        Terminator::Jump { target } => {
            encoder.u16(1);
            encoder.u32(target.0);
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            encoder.u16(2);
            value_id(encoder, *condition);
            encoder.u32(true_block.0);
            encoder.u32(false_block.0);
        }
        Terminator::Switch {
            value,
            cases,
            default_block,
            exit_block,
        } => {
            encoder.u16(3);
            value_id(encoder, *value);
            encoder.len(cases.len())?;
            for case in cases {
                encoder.u32(case.constant.0);
                encoder.u32(case.target.0);
            }
            encoder.u32(default_block.0);
            encoder.u32(exit_block.0);
        }
        Terminator::Throw { value } => {
            encoder.u16(4);
            value_id(encoder, *value);
        }
        Terminator::Unreachable => encoder.u16(5),
        Terminator::Deopt { frames } => {
            encoder.u16(6);
            encoder.len(frames.len())?;
            for frame in frames {
                encoder.u32(frame.function.0);
                encoder.u32(frame.block.0);
                encoder.u32(frame.instruction_index);
                encoder.len(frame.lives.len())?;
                for live in &frame.lives {
                    value_id(encoder, *live);
                }
            }
        }
    }
    Ok(())
}

fn decode_terminator(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<Terminator, ArtifactFormatError> {
    let tag = decoder.u16()?;
    match tag {
        0 => Ok(Terminator::Return {
            value: decode_optional_value(decoder)?,
        }),
        1 => Ok(Terminator::Jump {
            target: BasicBlockId(decoder.u32()?),
        }),
        2 => Ok(Terminator::Branch {
            condition: next_value(decoder)?,
            true_block: BasicBlockId(decoder.u32()?),
            false_block: BasicBlockId(decoder.u32()?),
        }),
        3 => {
            let value = next_value(decoder)?;
            let count = decoder.count(limits.max_switch_cases)?;
            let mut cases = Vec::with_capacity(count);
            for _ in 0..count {
                cases.push(SwitchCaseTarget {
                    constant: ConstantId(decoder.u32()?),
                    target: BasicBlockId(decoder.u32()?),
                });
            }
            Ok(Terminator::Switch {
                value,
                cases,
                default_block: BasicBlockId(decoder.u32()?),
                exit_block: BasicBlockId(decoder.u32()?),
            })
        }
        4 => Ok(Terminator::Throw {
            value: next_value(decoder)?,
        }),
        5 => Ok(Terminator::Unreachable),
        6 => {
            let frame_count = decoder.count(limits.max_blocks_per_function)?;
            let mut frames = Vec::with_capacity(frame_count);
            for _ in 0..frame_count {
                let function = FunctionId(decoder.u32()?);
                let block = BasicBlockId(decoder.u32()?);
                let instruction_index = decoder.u32()?;
                let live_count = decoder.count(limits.max_values_per_list)?;
                let mut lives = Vec::with_capacity(live_count);
                for _ in 0..live_count {
                    lives.push(next_value(decoder)?);
                }
                frames.push(DeoptFrame {
                    function,
                    block,
                    instruction_index,
                    lives,
                });
            }
            Ok(Terminator::Deopt { frames })
        }
        _ => Err(ArtifactFormatError::UnknownTag("terminator", tag.into())),
    }
}

pub(crate) fn encode_required_builtins(program: &Program) -> Vec<u8> {
    let mut ids = BTreeSet::new();
    for function in program.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::CallBuiltin { builtin, .. } = instruction {
                    ids.insert(builtin.wire_id());
                }
            }
        }
    }
    let mut encoder = Encoder::default();
    encoder.u32(u32::try_from(ids.len()).expect("builtin count fits u32"));
    for id in ids {
        encoder.u16(id);
    }
    encoder.finish()
}

pub(crate) fn decode_required_builtins(
    bytes: &[u8],
    limits: &ArtifactLimits,
) -> Result<Vec<u16>, ArtifactFormatError> {
    let mut decoder = Decoder::new(bytes, limits);
    let count = decoder.count(limits.max_required_builtins)?;
    let mut ids = Vec::with_capacity(count);
    let mut previous = None;
    for _ in 0..count {
        let id = decoder.u16()?;
        if Builtin::from_wire_id(id).is_none() {
            return Err(ArtifactFormatError::UnknownTag("builtin", id.into()));
        }
        if previous.is_some_and(|previous| previous >= id) {
            return Err(ArtifactFormatError::NonCanonical(
                "required builtin IDs are not strictly increasing".into(),
            ));
        }
        previous = Some(id);
        ids.push(id);
    }
    decoder.finish()?;
    Ok(ids)
}

fn binary_tag(op: BinaryOp) -> u16 {
    match op {
        BinaryOp::Add => 0,
        BinaryOp::Sub => 1,
        BinaryOp::Mul => 2,
        BinaryOp::Div => 3,
        BinaryOp::Mod => 4,
        BinaryOp::Exp => 5,
        BinaryOp::BitAnd => 6,
        BinaryOp::BitOr => 7,
        BinaryOp::BitXor => 8,
        BinaryOp::Shl => 9,
        BinaryOp::Shr => 10,
        BinaryOp::UShr => 11,
    }
}
fn decode_binary(tag: u16) -> Result<BinaryOp, ArtifactFormatError> {
    match tag {
        0 => Ok(BinaryOp::Add),
        1 => Ok(BinaryOp::Sub),
        2 => Ok(BinaryOp::Mul),
        3 => Ok(BinaryOp::Div),
        4 => Ok(BinaryOp::Mod),
        5 => Ok(BinaryOp::Exp),
        6 => Ok(BinaryOp::BitAnd),
        7 => Ok(BinaryOp::BitOr),
        8 => Ok(BinaryOp::BitXor),
        9 => Ok(BinaryOp::Shl),
        10 => Ok(BinaryOp::Shr),
        11 => Ok(BinaryOp::UShr),
        _ => Err(ArtifactFormatError::UnknownTag("binary op", tag.into())),
    }
}
fn unary_tag(op: UnaryOp) -> u16 {
    match op {
        UnaryOp::Not => 0,
        UnaryOp::Neg => 1,
        UnaryOp::Pos => 2,
        UnaryOp::BitNot => 3,
        UnaryOp::Void => 4,
        UnaryOp::IsNullish => 5,
        UnaryOp::Delete => 6,
    }
}
fn decode_unary(tag: u16) -> Result<UnaryOp, ArtifactFormatError> {
    match tag {
        0 => Ok(UnaryOp::Not),
        1 => Ok(UnaryOp::Neg),
        2 => Ok(UnaryOp::Pos),
        3 => Ok(UnaryOp::BitNot),
        4 => Ok(UnaryOp::Void),
        5 => Ok(UnaryOp::IsNullish),
        6 => Ok(UnaryOp::Delete),
        _ => Err(ArtifactFormatError::UnknownTag("unary op", tag.into())),
    }
}
fn compare_tag(op: CompareOp) -> u16 {
    match op {
        CompareOp::StrictEq => 0,
        CompareOp::StrictNotEq => 1,
        CompareOp::Lt => 2,
        CompareOp::Gt => 3,
        CompareOp::LtEq => 4,
        CompareOp::GtEq => 5,
    }
}
fn decode_compare(tag: u16) -> Result<CompareOp, ArtifactFormatError> {
    match tag {
        0 => Ok(CompareOp::StrictEq),
        1 => Ok(CompareOp::StrictNotEq),
        2 => Ok(CompareOp::Lt),
        3 => Ok(CompareOp::Gt),
        4 => Ok(CompareOp::LtEq),
        5 => Ok(CompareOp::GtEq),
        _ => Err(ArtifactFormatError::UnknownTag("compare op", tag.into())),
    }
}

fn encode_call(
    encoder: &mut Encoder,
    dest: Option<ValueId>,
    callee: ValueId,
    this_val: ValueId,
    args: &[ValueId],
) -> Result<(), ArtifactFormatError> {
    optional_value_id(encoder, dest);
    value_id(encoder, callee);
    value_id(encoder, this_val);
    value_ids(encoder, args)
}
fn decode_call(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<(Option<ValueId>, ValueId, ValueId, Vec<ValueId>), ArtifactFormatError> {
    Ok((
        decode_optional_value(decoder)?,
        next_value(decoder)?,
        next_value(decoder)?,
        decode_value_ids(decoder, limits)?,
    ))
}
fn value_id(encoder: &mut Encoder, value: ValueId) {
    encoder.u32(value.0);
}
fn optional_value_id(encoder: &mut Encoder, value: Option<ValueId>) {
    match value {
        Some(value) => {
            encoder.bool(true);
            value_id(encoder, value);
        }
        None => encoder.bool(false),
    }
}
fn decode_optional_value(
    decoder: &mut Decoder<'_>,
) -> Result<Option<ValueId>, ArtifactFormatError> {
    if decoder.bool()? {
        Ok(Some(next_value(decoder)?))
    } else {
        Ok(None)
    }
}
fn next_value(decoder: &mut Decoder<'_>) -> Result<ValueId, ArtifactFormatError> {
    Ok(ValueId(decoder.u32()?))
}
fn value_ids(encoder: &mut Encoder, values: &[ValueId]) -> Result<(), ArtifactFormatError> {
    encoder.len(values.len())?;
    for value in values {
        value_id(encoder, *value);
    }
    Ok(())
}
fn decode_value_ids(
    decoder: &mut Decoder<'_>,
    limits: &ArtifactLimits,
) -> Result<Vec<ValueId>, ArtifactFormatError> {
    let count = decoder.count(limits.max_values_per_list)?;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(next_value(decoder)?);
    }
    Ok(values)
}
fn two_values(encoder: &mut Encoder, first: ValueId, second: ValueId) {
    value_id(encoder, first);
    value_id(encoder, second);
}
fn three_values(encoder: &mut Encoder, first: ValueId, second: ValueId, third: ValueId) {
    value_id(encoder, first);
    value_id(encoder, second);
    value_id(encoder, third);
}
fn decode_two(decoder: &mut Decoder<'_>) -> Result<(ValueId, ValueId), ArtifactFormatError> {
    Ok((next_value(decoder)?, next_value(decoder)?))
}
fn decode_three(
    decoder: &mut Decoder<'_>,
) -> Result<(ValueId, ValueId, ValueId), ArtifactFormatError> {
    Ok((
        next_value(decoder)?,
        next_value(decoder)?,
        next_value(decoder)?,
    ))
}

#[derive(Default)]
struct Encoder {
    bytes: Vec<u8>,
}
impl Encoder {
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }
    fn bool(&mut self, value: bool) {
        self.bytes.push(u8::from(value));
    }
    fn len(&mut self, value: usize) -> Result<(), ArtifactFormatError> {
        self.u32(u32::try_from(value).map_err(|_| ArtifactFormatError::LengthOverflow)?);
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<(), ArtifactFormatError> {
        self.len(value.len())?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }
    fn optional_string(&mut self, value: Option<&str>) -> Result<(), ArtifactFormatError> {
        match value {
            Some(value) => {
                self.bool(true);
                self.string(value)?;
            }
            None => self.bool(false),
        }
        Ok(())
    }
    fn strings(&mut self, values: &[String]) -> Result<(), ArtifactFormatError> {
        self.len(values.len())?;
        for value in values {
            self.string(value)?;
        }
        Ok(())
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
    limits: &'a ArtifactLimits,
}
impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8], limits: &'a ArtifactLimits) -> Self {
        Self {
            bytes,
            cursor: 0,
            limits,
        }
    }
    fn finish(self) -> Result<(), ArtifactFormatError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(ArtifactFormatError::TrailingBytes(
                self.bytes.len() - self.cursor,
            ))
        }
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], ArtifactFormatError> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or(ArtifactFormatError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.cursor..end)
            .ok_or(ArtifactFormatError::Truncated)?;
        self.cursor = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, ArtifactFormatError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ArtifactFormatError> {
        let bytes: [u8; 2] = self.take(2)?.try_into().expect("fixed-width slice");
        Ok(u16::from_le_bytes(bytes))
    }
    fn u32(&mut self) -> Result<u32, ArtifactFormatError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("fixed-width slice");
        Ok(u32::from_le_bytes(bytes))
    }
    fn u64(&mut self) -> Result<u64, ArtifactFormatError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("fixed-width slice");
        Ok(u64::from_le_bytes(bytes))
    }
    fn bool(&mut self) -> Result<bool, ArtifactFormatError> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(ArtifactFormatError::InvalidBoolean(value)),
        }
    }
    fn count(&mut self, maximum: u32) -> Result<usize, ArtifactFormatError> {
        let value = self.u32()?;
        if value > maximum {
            return Err(ArtifactFormatError::LimitExceeded {
                kind: "count",
                actual: u64::from(value),
                maximum: u64::from(maximum),
            });
        }
        usize::try_from(value).map_err(|_| ArtifactFormatError::LengthOverflow)
    }
    fn string(&mut self) -> Result<String, ArtifactFormatError> {
        let len = self.count(self.limits.max_string_bytes)?;
        let bytes = self.take(len)?;
        let value = std::str::from_utf8(bytes).map_err(|_| ArtifactFormatError::InvalidUtf8)?;
        Ok(value.to_owned())
    }
    fn optional_string(&mut self) -> Result<Option<String>, ArtifactFormatError> {
        if self.bool()? {
            Ok(Some(self.string()?))
        } else {
            Ok(None)
        }
    }
    fn strings(&mut self, maximum: u32) -> Result<Vec<String>, ArtifactFormatError> {
        let count = self.count(maximum)?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.string()?);
        }
        Ok(values)
    }
}
