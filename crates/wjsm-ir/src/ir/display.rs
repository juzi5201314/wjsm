use super::types::*;
use std::fmt::{self, Write};

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
            Self::CollectRestArgs { dest, skip } => {
                write!(formatter, "{dest} = collect_rest_args skip={skip}")
            }
        }
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Mod => "mod",
            Self::Exp => "exp",
            Self::BitAnd => "bitand",
            Self::BitOr => "bitor",
            Self::BitXor => "bitxor",
            Self::Shl => "shl",
            Self::Shr => "shr",
            Self::UShr => "ushr",
        })
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Not => "not",
            Self::Neg => "neg",
            Self::Pos => "pos",
            Self::BitNot => "bitnot",
            Self::Void => "void",
            Self::IsNullish => "is_nullish",
            Self::Delete => "delete",
        })
    }
}

impl fmt::Display for CompareOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StrictEq => "stricteq",
            Self::StrictNotEq => "strictneq",
        })
    }
}

impl fmt::Display for Builtin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConsoleLog => "console.log",
            Self::ConsoleError => "console.error",
            Self::ConsoleWarn => "console.warn",
            Self::ConsoleInfo => "console.info",
            Self::ConsoleDebug => "console.debug",
            Self::ConsoleTrace => "console.trace",
            Self::Debugger => "debugger",
            Self::Throw => "throw",
            Self::AbortShadowStackOverflow => "abort_shadow_stack_overflow",
            Self::F64Mod => "f64.mod",
            Self::F64Exp => "f64.exp",
            Self::IteratorFrom => "iterator.from",
            Self::IteratorNext => "iterator.next",
            Self::IteratorClose => "iterator.close",
            Self::IteratorValue => "iterator.value",
            Self::IteratorDone => "iterator.done",
            Self::EnumeratorFrom => "enumerator.from",
            Self::EnumeratorNext => "enumerator.next",
            Self::EnumeratorKey => "enumerator.key",
            Self::EnumeratorDone => "enumerator.done",
            Self::TypeOf => "typeof",
            Self::In => "op_in",
            Self::InstanceOf => "op_instanceof",
            Self::AbstractEq => "abstract_eq",
            Self::AbstractCompare => "abstract_compare",
            Self::DefineProperty => "define_property",
            Self::GetOwnPropDesc => "get_own_prop_desc",
            Self::SetTimeout => "setTimeout",
            Self::ClearTimeout => "clearTimeout",
            Self::SetInterval => "setInterval",
            Self::ClearInterval => "clearInterval",
            Self::Fetch => "fetch",
            Self::Eval => "eval",
            Self::EvalIndirect => "eval.indirect",
            Self::EvalResult => "eval.result",
            Self::JsonStringify => "JSON.stringify",
            Self::JsonParse => "JSON.parse",
            Self::CreateClosure => "create_closure",
            Self::ArrayPush => "array.push",
            Self::ArrayPop => "array.pop",
            Self::ArrayIncludes => "array.includes",
            Self::ArrayIndexOf => "array.index_of",
            Self::ArrayJoin => "array.join",
            Self::ArrayConcat => "array.concat",
            Self::ArraySlice => "array.slice",
            Self::ArrayFill => "array.fill",
            Self::ArrayReverse => "array.reverse",
            Self::ArrayFlat => "array.flat",
            Self::ArrayInitLength => "array.init_length",
            Self::ArrayGetLength => "array.get_length",
            Self::ArrayShift => "array.shift",
            Self::ArrayUnshiftVa => "array.unshift",
            Self::ArraySort => "array.sort",
            Self::ArrayAt => "array.at",
            Self::ArrayCopyWithin => "array.copy_within",
            Self::ArrayForEach => "array.for_each",
            Self::ArrayMap => "array.map",
            Self::ArrayFilter => "array.filter",
            Self::ArrayReduce => "array.reduce",
            Self::ArrayReduceRight => "array.reduce_right",
            Self::ArrayFind => "array.find",
            Self::ArrayFindIndex => "array.find_index",
            Self::ArraySome => "array.some",
            Self::ArrayEvery => "array.every",
            Self::ArrayFlatMap => "array.flat_map",
            Self::ArrayIsArray => "array.is_array",
            Self::ArraySpliceVa => "array.splice_va",
            Self::ArrayConcatVa => "array.concat_va",
            Self::FuncCall => "func_call",
            Self::FuncApply => "func_apply",
            Self::FuncBind => "func_bind",
            Self::ObjectRest => "object_rest",
            Self::GetPrototypeFromConstructor => "get_prototype_from_constructor",
            Self::HasOwnProperty => "has_own_property",
            Self::PrivateGet => "private_get",
            Self::PrivateSet => "private_set",
            Self::PrivateHas => "private_has",
            Self::ObjectProtoToString => "object_proto_to_string",
            Self::ObjectProtoValueOf => "object_proto_value_of",
            Self::ObjectKeys => "object.keys",
            Self::ObjectValues => "object.values",
            Self::ObjectEntries => "object.entries",
            Self::ObjectAssign => "object.assign",
            Self::ObjectCreate => "object.create",
            Self::ObjectGetPrototypeOf => "object.get_prototype_of",
            Self::ObjectSetPrototypeOf => "object.set_prototype_of",
            Self::ObjectGetOwnPropertyNames => "object.get_own_property_names",
            Self::ObjectIs => "object.is",
            Self::BigIntFromLiteral => "bigint.from_literal",
            Self::BigIntAdd => "bigint.add",
            Self::BigIntSub => "bigint.sub",
            Self::BigIntMul => "bigint.mul",
            Self::BigIntDiv => "bigint.div",
            Self::BigIntMod => "bigint.mod",
            Self::BigIntPow => "bigint.pow",
            Self::BigIntNeg => "bigint.neg",
            Self::BigIntEq => "bigint.eq",
            Self::BigIntCmp => "bigint.cmp",
            Self::SymbolCreate => "symbol.create",
            Self::SymbolFor => "symbol.for",
            Self::SymbolKeyFor => "symbol.key_for",
            Self::SymbolWellKnown => "symbol.well_known",
            Self::RegExpCreate => "regexp.create",
            Self::RegExpTest => "regexp.test",
            Self::RegExpExec => "regexp.exec",
            Self::StringMatch => "string.match",
            Self::StringReplace => "string.replace",
            Self::StringSearch => "string.search",
            Self::StringSplit => "string.split",
            Self::PromiseCreate => "promise.create",
            Self::PromiseInstanceResolve => "promise.instance_resolve",
            Self::PromiseInstanceReject => "promise.instance_reject",
            Self::PromiseCreateResolveFunction => "promise.create_resolve_function",
            Self::PromiseCreateRejectFunction => "promise.create_reject_function",
            Self::PromiseThen => "promise.then",
            Self::PromiseCatch => "promise.catch",
            Self::PromiseFinally => "promise.finally",
            Self::PromiseAll => "promise.all",
            Self::PromiseRace => "promise.race",
            Self::PromiseAllSettled => "promise.all_settled",
            Self::PromiseAny => "promise.any",
            Self::PromiseResolveStatic => "promise.resolve_static",
            Self::PromiseRejectStatic => "promise.reject_static",
            Self::IsPromise => "is_promise",
            Self::QueueMicrotask => "queue_microtask",
            Self::DrainMicrotasks => "drain_microtasks",
            Self::AsyncFunctionStart => "async_function.start",
            Self::AsyncFunctionResume => "async_function.resume",
            Self::AsyncFunctionSuspend => "async_function.suspend",
            Self::ContinuationCreate => "continuation.create",
            Self::ContinuationSaveVar => "continuation.save_var",
            Self::ContinuationLoadVar => "continuation.load_var",
            Self::AsyncGeneratorStart => "async_generator.start",
            Self::AsyncGeneratorNext => "async_generator.next",
            Self::AsyncGeneratorReturn => "async_generator.return",
            Self::PromiseWithResolvers => "promise.with_resolvers",
            Self::IsCallable => "is_callable",
            Self::AsyncGeneratorThrow => "async_generator.throw",
            Self::DynamicImport => "dynamic_import",
            Self::RegisterModuleNamespace => "register_module_namespace",
            Self::JsxCreateElement => "jsx.create_element",
            Self::ProxyCreate => "proxy.create",
            Self::ProxyRevocable => "proxy.revocable",
            Self::ReflectGet => "reflect.get",
            Self::ReflectSet => "reflect.set",
            Self::ReflectHas => "reflect.has",
            Self::ReflectDeleteProperty => "reflect.delete_property",
            Self::ReflectApply => "reflect.apply",
            Self::ReflectConstruct => "reflect.construct",
            Self::ReflectGetPrototypeOf => "reflect.get_prototype_of",
            Self::ReflectSetPrototypeOf => "reflect.set_prototype_of",
            Self::ReflectIsExtensible => "reflect.is_extensible",
            Self::ReflectPreventExtensions => "reflect.prevent_extensions",
            Self::ReflectGetOwnPropertyDescriptor => "reflect.get_own_property_descriptor",
            Self::ReflectDefineProperty => "reflect.define_property",
            Self::ReflectOwnKeys => "reflect.own_keys",
            Self::StringAt => "string.at",
            Self::StringCharAt => "string.char_at",
            Self::StringCharCodeAt => "string.char_code_at",
            Self::StringCodePointAt => "string.code_point_at",
            Self::StringConcatVa => "string.concat_va",
            Self::StringEndsWith => "string.ends_with",
            Self::StringIncludes => "string.includes",
            Self::StringIndexOf => "string.index_of",
            Self::StringLastIndexOf => "string.last_index_of",
            Self::StringMatchAll => "string.match_all",
            Self::StringPadEnd => "string.pad_end",
            Self::StringPadStart => "string.pad_start",
            Self::StringRepeat => "string.repeat",
            Self::StringReplaceAll => "string.replace_all",
            Self::StringSlice => "string.slice",
            Self::StringStartsWith => "string.starts_with",
            Self::StringSubstring => "string.substring",
            Self::StringToLowerCase => "string.to_lower_case",
            Self::StringToUpperCase => "string.to_upper_case",
            Self::StringTrim => "string.trim",
            Self::StringTrimEnd => "string.trim_end",
            Self::StringTrimStart => "string.trim_start",
            Self::StringToString => "string.to_string",
            Self::StringValueOf => "string.value_of",
            Self::StringIterator => "string.iterator",
            Self::StringFromCharCode => "string.from_char_code",
            Self::StringFromCodePoint => "string.from_code_point",
            Self::MathAbs => "Math.abs",
            Self::MathAcos => "Math.acos",
            Self::MathAcosh => "Math.acosh",
            Self::MathAsin => "Math.asin",
            Self::MathAsinh => "Math.asinh",
            Self::MathAtan => "Math.atan",
            Self::MathAtanh => "Math.atanh",
            Self::MathAtan2 => "Math.atan2",
            Self::MathCbrt => "Math.cbrt",
            Self::MathCeil => "Math.ceil",
            Self::MathClz32 => "Math.clz32",
            Self::MathCos => "Math.cos",
            Self::MathCosh => "Math.cosh",
            Self::MathExp => "Math.exp",
            Self::MathExpm1 => "Math.expm1",
            Self::MathFloor => "Math.floor",
            Self::MathFround => "Math.fround",
            Self::MathHypot => "Math.hypot",
            Self::MathImul => "Math.imul",
            Self::MathLog => "Math.log",
            Self::MathLog1p => "Math.log1p",
            Self::MathLog10 => "Math.log10",
            Self::MathLog2 => "Math.log2",
            Self::MathMax => "Math.max",
            Self::MathMin => "Math.min",
            Self::MathPow => "Math.pow",
            Self::MathRandom => "Math.random",
            Self::MathRound => "Math.round",
            Self::MathSign => "Math.sign",
            Self::MathSin => "Math.sin",
            Self::MathSinh => "Math.sinh",
            Self::MathSqrt => "Math.sqrt",
            Self::MathTan => "Math.tan",
            Self::MathTanh => "Math.tanh",
            Self::MathTrunc => "Math.trunc",
            Self::NumberConstructor => "Number",
            Self::NumberIsNaN => "Number.isNaN",
            Self::NumberIsFinite => "Number.isFinite",
            Self::NumberIsInteger => "Number.isInteger",
            Self::NumberIsSafeInteger => "Number.isSafeInteger",
            Self::NumberParseInt => "Number.parseInt",
            Self::NumberParseFloat => "Number.parseFloat",
            Self::NumberProtoToString => "Number.prototype.toString",
            Self::NumberProtoValueOf => "Number.prototype.valueOf",
            Self::NumberProtoToFixed => "Number.prototype.toFixed",
            Self::NumberProtoToExponential => "Number.prototype.toExponential",
            Self::NumberProtoToPrecision => "Number.prototype.toPrecision",
            Self::BooleanConstructor => "Boolean",
            Self::BooleanProtoToString => "Boolean.prototype.toString",
            Self::BooleanProtoValueOf => "Boolean.prototype.valueOf",
            Self::ErrorConstructor => "Error",
            Self::TypeErrorConstructor => "TypeError",
            Self::RangeErrorConstructor => "RangeError",
            Self::SyntaxErrorConstructor => "SyntaxError",
            Self::ReferenceErrorConstructor => "ReferenceError",
            Self::URIErrorConstructor => "URIError",
            Self::EvalErrorConstructor => "EvalError",
            Self::ErrorProtoToString => "Error.prototype.toString",
            Self::MapConstructor => "Map",
            Self::MapProtoSet => "Map.prototype.set",
            Self::MapProtoGet => "Map.prototype.get",
            Self::SetConstructor => "Set",
            Self::SetProtoAdd => "Set.prototype.add",
            Self::MapSetHas => "MapSet.has",
            Self::MapSetDelete => "MapSet.delete",
            Self::MapSetClear => "MapSet.clear",
            Self::MapSetGetSize => "MapSet.size",
            Self::MapSetForEach => "MapSet.forEach",
            Self::MapSetKeys => "MapSet.keys",
            Self::MapSetValues => "MapSet.values",
            Self::MapSetEntries => "MapSet.entries",
            Self::DateConstructor => "Date",
            Self::DateNow => "Date.now",
            Self::DateParse => "Date.parse",
            Self::DateUTC => "Date.UTC",
            Self::WeakMapConstructor => "WeakMap",
            Self::WeakMapProtoSet => "WeakMap.prototype.set",
            Self::WeakMapProtoGet => "WeakMap.prototype.get",
            Self::WeakMapProtoHas => "WeakMap.prototype.has",
            Self::WeakMapProtoDelete => "WeakMap.prototype.delete",
            Self::WeakSetConstructor => "WeakSet",
            Self::WeakSetProtoAdd => "WeakSet.prototype.add",
            Self::WeakSetProtoHas => "WeakSet.prototype.has",
            Self::WeakSetProtoDelete => "WeakSet.prototype.delete",
            Self::ArrayBufferConstructor => "ArrayBuffer",
            Self::ArrayBufferProtoByteLength => "ArrayBuffer.prototype.byteLength",
            Self::ArrayBufferProtoSlice => "ArrayBuffer.prototype.slice",
            Self::DataViewConstructor => "DataView",
            Self::DataViewProtoGetFloat64 => "DataView.prototype.getFloat64",
            Self::DataViewProtoGetFloat32 => "DataView.prototype.getFloat32",
            Self::DataViewProtoGetInt32 => "DataView.prototype.getInt32",
            Self::DataViewProtoGetUint32 => "DataView.prototype.getUint32",
            Self::DataViewProtoGetInt16 => "DataView.prototype.getInt16",
            Self::DataViewProtoGetUint16 => "DataView.prototype.getUint16",
            Self::DataViewProtoGetInt8 => "DataView.prototype.getInt8",
            Self::DataViewProtoGetUint8 => "DataView.prototype.getUint8",
            Self::DataViewProtoSetFloat64 => "DataView.prototype.setFloat64",
            Self::DataViewProtoSetFloat32 => "DataView.prototype.setFloat32",
            Self::DataViewProtoSetInt32 => "DataView.prototype.setInt32",
            Self::DataViewProtoSetUint32 => "DataView.prototype.setUint32",
            Self::DataViewProtoSetInt16 => "DataView.prototype.setInt16",
            Self::DataViewProtoSetUint16 => "DataView.prototype.setUint16",
            Self::DataViewProtoSetInt8 => "DataView.prototype.setInt8",
            Self::DataViewProtoSetUint8 => "DataView.prototype.setUint8",
            Self::Int8ArrayConstructor => "Int8Array",
            Self::Uint8ArrayConstructor => "Uint8Array",
            Self::Uint8ClampedArrayConstructor => "Uint8ClampedArray",
            Self::Int16ArrayConstructor => "Int16Array",
            Self::Uint16ArrayConstructor => "Uint16Array",
            Self::Int32ArrayConstructor => "Int32Array",
            Self::Uint32ArrayConstructor => "Uint32Array",
            Self::Float32ArrayConstructor => "Float32Array",
            Self::Float64ArrayConstructor => "Float64Array",
            Self::TypedArrayProtoLength => "TypedArray.prototype.length",
            Self::TypedArrayProtoByteLength => "TypedArray.prototype.byteLength",
            Self::TypedArrayProtoByteOffset => "TypedArray.prototype.byteOffset",
            Self::TypedArrayProtoSet => "TypedArray.prototype.set",
            Self::TypedArrayProtoSlice => "TypedArray.prototype.slice",
            Self::TypedArrayProtoSubarray => "TypedArray.prototype.subarray",
            Self::GetBuiltinGlobal => "get_builtin_global",
        })
    }
}

impl fmt::Display for Terminator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Return { value: Some(value) } => write!(formatter, "return {value}"),
            Self::Return { value: None } => formatter.write_str("return"),
            Self::Jump { target } => write!(formatter, "jump {target}"),
            Self::Branch {
                condition,
                true_block,
                false_block,
            } => {
                write!(formatter, "branch {condition}, {true_block}, {false_block}")
            }
            Self::Switch {
                value,
                cases,
                default_block,
                exit_block,
            } => {
                write!(formatter, "switch {value} [")?;
                for (i, case) in cases.iter().enumerate() {
                    if i > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "case {case}")?;
                }
                write!(formatter, "], default {default_block}, exit {exit_block}")
            }
            Self::Throw { value } => write!(formatter, "throw {value}"),
            Self::Unreachable => formatter.write_str("unreachable"),
        }
    }
}

impl fmt::Display for SwitchCaseTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "c{} -> {}", self.constant.0, self.target)
    }
}

impl fmt::Display for PhiSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}, {})", self.predecessor, self.value)
    }
}

impl fmt::Display for ConstantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "c{}", self.0)
    }
}

impl fmt::Display for FunctionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl fmt::Display for BasicBlockId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "bb{}", self.0)
    }
}

impl fmt::Display for ValueId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "%{}", self.0)
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mod{}", self.0)
    }
}

impl Module {
    pub fn dump_text(&self) -> String {
        let mut out = String::from("module {\n");

        if self.constants().is_empty() {
            out.push_str("  constants: []\n");
        } else {
            out.push_str("  constants:\n");
            for (index, constant) in self.constants().iter().enumerate() {
                let _ = writeln!(out, "    c{index} = {constant}");
            }
        }

        out.push('\n');

        for (index, function) in self.functions().iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            function.dump_into(&mut out);
        }

        out.push_str("}\n");
        out
    }
}

impl Function {
    fn dump_into(&self, out: &mut String) {
        let _ = write!(out, "  fn @{}", self.name());
        if let Some(home) = self.home_object {
            let _ = write!(out, " [home_object=@{}]", home.0);
        }
        if self.has_eval() {
            let _ = write!(out, " [has_eval]");
        }
        if !self.captured_names().is_empty() {
            let _ = write!(out, " [captures: ");
            for (i, name) in self.captured_names().iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                let _ = write!(out, "{name}");
            }
            let _ = write!(out, "]");
        }
        if self.params().is_empty() {
            let _ = writeln!(out, " [entry={}]:", self.entry());
        } else {
            let _ = write!(out, " [params: ");
            for (i, param) in self.params().iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                let _ = write!(out, "{param}");
            }
            let _ = writeln!(out, "] [entry={}]:", self.entry());
        }

        for block in self.blocks() {
            let _ = writeln!(out, "    {}:", block.id());

            for instruction in block.instructions() {
                let _ = writeln!(out, "      {instruction}");
            }

            let _ = writeln!(out, "      {}", block.terminator());
        }
    }
}
