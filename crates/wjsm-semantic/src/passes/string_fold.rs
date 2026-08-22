//! string_fold pass：把常量 receiver 的字符串运算在编译期求值。
//!
//! AOT 的结构性优势：V8 必须在每次执行时重新做的切片、查找与拼接，只要 receiver
//! 与实参都是字面量就能在构建期一次算完，运行时退化为一次常量读取——既没有宿主
//! 往返，也没有中间字符串分配。
//!
//! 全程在 UTF-16 码元上计算，与 `RuntimeString` 的内部表示一致，避免经 UTF-8
//! 往返时孤立代理项被替换。只折叠语义无歧义的操作：大小写映射与 trim 依赖宿主侧
//! 的 Unicode 表，在此重实现会有随版本漂移的风险，一律留给运行时。

use std::collections::HashMap;

use wjsm_ir::{Builtin, Constant, ConstantId, Instruction, Module, ValueId};

#[derive(Debug)]
enum FoldedResult {
    Constant(Constant),
    ArrayTemplate(Vec<Constant>),
}

pub(crate) fn run(module: &mut Module) {
    let constants = module.constants().to_vec();
    let mut folded: Vec<Constant> = Vec::new();

    for index in 0..module.functions().len() {
        let function_id =
            wjsm_ir::FunctionId(u32::try_from(index).expect("function index fits u32"));
        let Some(function) = module.function_mut(function_id) else {
            continue;
        };
        // 值 → 常量下标；只收本函数内由 Const 指令定义的值。
        let mut defined: HashMap<ValueId, ConstantId> = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Const { dest, constant } = instruction {
                    defined.insert(*dest, *constant);
                }
            }
        }

        let mut rewrites: Vec<(usize, usize, FoldedResult)> = Vec::new();
        for (block_index, block) in function.blocks().iter().enumerate() {
            for (instruction_index, instruction) in block.instructions().iter().enumerate() {
                let Instruction::CallBuiltin {
                    dest: Some(_),
                    builtin,
                    args,
                } = instruction
                else {
                    continue;
                };
                let Some(result) = fold_call(*builtin, args, &defined, &constants, &folded) else {
                    continue;
                };
                rewrites.push((block_index, instruction_index, result));
            }
        }
        if rewrites.is_empty() {
            continue;
        }
        for (block_index, instruction_index, result) in rewrites {
            let constant = ConstantId(
                u32::try_from(constants.len() + folded.len()).expect("constant index fits u32"),
            );
            let replacement = match result {
                FoldedResult::Constant(value) => {
                    folded.push(value);
                    Instruction::Const {
                        dest: ValueId(0),
                        constant,
                    }
                }
                FoldedResult::ArrayTemplate(elements) => {
                    let ids = elements
                        .into_iter()
                        .map(|value| {
                            let id = ConstantId(
                                u32::try_from(constants.len() + folded.len())
                                    .expect("constant index fits u32"),
                            );
                            folded.push(value);
                            id
                        })
                        .collect();
                    folded.push(Constant::ArrayTemplate(ids));
                    Instruction::CloneArrayTemplate {
                        dest: ValueId(0),
                        template: ConstantId(
                            u32::try_from(constants.len() + folded.len() - 1)
                                .expect("constant index fits u32"),
                        ),
                    }
                }
            };
            let blocks = function.blocks_mut();
            let Instruction::CallBuiltin {
                dest: Some(dest), ..
            } = blocks[block_index].instructions()[instruction_index]
            else {
                continue;
            };
            let replacement = match replacement {
                Instruction::Const { constant, .. } => Instruction::Const { dest, constant },
                Instruction::CloneArrayTemplate { template, .. } => {
                    Instruction::CloneArrayTemplate { dest, template }
                }
                _ => unreachable!(),
            };
            blocks[block_index].instructions_mut()[instruction_index] = replacement;
        }
    }

    for constant in folded {
        module.add_constant(constant);
    }
}

/// 常量池视图：原有常量在前，本轮新折叠出的常量接在后面。
fn constant_at<'a>(
    id: ConstantId,
    constants: &'a [Constant],
    folded: &'a [Constant],
) -> Option<&'a Constant> {
    let index = id.0 as usize;
    constants
        .get(index)
        .or_else(|| folded.get(index - constants.len().min(index)))
}

fn string_arg(
    value: ValueId,
    defined: &HashMap<ValueId, ConstantId>,
    constants: &[Constant],
    folded: &[Constant],
) -> Option<Vec<u16>> {
    match constant_at(*defined.get(&value)?, constants, folded)? {
        Constant::String(text) => Some(text.encode_utf16().collect()),
        _ => None,
    }
}

fn number_arg(
    value: ValueId,
    defined: &HashMap<ValueId, ConstantId>,
    constants: &[Constant],
    folded: &[Constant],
) -> Option<f64> {
    match constant_at(*defined.get(&value)?, constants, folded)? {
        Constant::Number(number) => Some(*number),
        _ => None,
    }
}

/// 折叠结果的码元上限：常量会被烘焙进制品，无节制展开会把 `repeat` 变成代码膨胀。
const MAX_FOLDED_UNITS: usize = 4096;

fn from_units(units: &[u16]) -> Option<Constant> {
    if units.len() > MAX_FOLDED_UNITS {
        return None;
    }
    // 含孤立代理项的串无法无损地经 `String` 承载，交回运行时处理。
    Some(Constant::String(String::from_utf16(units).ok()?))
}

fn fold_call(
    builtin: Builtin,
    args: &[ValueId],
    defined: &HashMap<ValueId, ConstantId>,
    constants: &[Constant],
    folded: &[Constant],
) -> Option<FoldedResult> {
    // 变长拼接单独处理：它没有固定 receiver 形态。
    if builtin == Builtin::StringConcatVa {
        let mut out: Vec<u16> = Vec::new();
        for part in args {
            out.extend(string_arg(*part, defined, constants, folded)?);
            if out.len() > MAX_FOLDED_UNITS {
                return None;
            }
        }
        return from_units(&out).map(FoldedResult::Constant);
    }

    let receiver = string_arg(*args.first()?, defined, constants, folded)?;
    let length = receiver.len();
    if builtin == Builtin::StringSplit {
        let separator = string_arg(*args.get(1)?, defined, constants, folded)?;
        let limit = split_limit(args, defined, constants, folded)?;
        let parts = split_units(&receiver, &separator, limit)?;
        return Some(FoldedResult::ArrayTemplate(
            parts
                .into_iter()
                .map(|units| String::from_utf16(&units).ok().map(Constant::String))
                .collect::<Option<Vec<_>>>()?,
        ));
    }
    let result = match builtin {
        Builtin::StringSlice => {
            let start =
                relative_index(integer_arg(args, 1, defined, constants, folded, 0)?, length);
            let end = relative_index(
                integer_arg(args, 2, defined, constants, folded, length as i64)?,
                length,
            );
            from_units(if end > start {
                &receiver[start..end]
            } else {
                &[]
            })
        }
        Builtin::StringSubstring => {
            let mut start =
                clamp_index(integer_arg(args, 1, defined, constants, folded, 0)?, length);
            let mut end = clamp_index(
                integer_arg(args, 2, defined, constants, folded, length as i64)?,
                length,
            );
            if start > end {
                std::mem::swap(&mut start, &mut end);
            }
            from_units(&receiver[start..end])
        }
        Builtin::StringCharAt => {
            let index = integer_arg(args, 1, defined, constants, folded, 0)?;
            match usize::try_from(index).ok().and_then(|i| receiver.get(i)) {
                Some(unit) => from_units(&[*unit]),
                None => Some(Constant::String(String::new())),
            }
        }
        Builtin::StringCharCodeAt => {
            let index = integer_arg(args, 1, defined, constants, folded, 0)?;
            let unit = usize::try_from(index).ok().and_then(|i| receiver.get(i));
            Some(Constant::Number(
                unit.map_or(f64::NAN, |unit| f64::from(*unit)),
            ))
        }
        Builtin::StringIndexOf | Builtin::StringLastIndexOf => {
            let needle = string_arg(*args.get(1)?, defined, constants, folded)?;
            let found = if builtin == Builtin::StringIndexOf {
                let from =
                    clamp_index(integer_arg(args, 2, defined, constants, folded, 0)?, length);
                find_units(&receiver, &needle, from)
            } else {
                let end = clamp_index(
                    integer_arg(args, 2, defined, constants, folded, length as i64)?,
                    length,
                );
                rfind_units(
                    &receiver,
                    &needle,
                    end.saturating_add(needle.len()).min(length),
                )
            };
            Some(Constant::Number(found.map_or(-1.0, |index| index as f64)))
        }
        Builtin::StringIncludes => {
            let needle = string_arg(*args.get(1)?, defined, constants, folded)?;
            let from = clamp_index(integer_arg(args, 2, defined, constants, folded, 0)?, length);
            Some(Constant::Bool(
                find_units(&receiver, &needle, from).is_some(),
            ))
        }
        Builtin::StringStartsWith => {
            let needle = string_arg(*args.get(1)?, defined, constants, folded)?;
            let from = clamp_index(integer_arg(args, 2, defined, constants, folded, 0)?, length);
            Some(Constant::Bool(
                from + needle.len() <= length && receiver[from..from + needle.len()] == needle[..],
            ))
        }
        Builtin::StringEndsWith => {
            let needle = string_arg(*args.get(1)?, defined, constants, folded)?;
            let end = clamp_index(
                integer_arg(args, 2, defined, constants, folded, length as i64)?,
                length,
            );
            Some(Constant::Bool(match end.checked_sub(needle.len()) {
                Some(start) => receiver[start..end] == needle[..],
                None => false,
            }))
        }
        Builtin::StringRepeat => {
            let count = integer_arg(args, 1, defined, constants, folded, 0)?;
            let count = usize::try_from(count).ok()?;
            let total = length.checked_mul(count)?;
            if total > MAX_FOLDED_UNITS {
                return None;
            }
            let mut out = Vec::with_capacity(total);
            for _ in 0..count {
                out.extend_from_slice(&receiver);
            }
            from_units(&out)
        }
        _ => None,
    };
    result.map(FoldedResult::Constant)
}

fn split_limit(
    args: &[ValueId],
    defined: &HashMap<ValueId, ConstantId>,
    constants: &[Constant],
    folded: &[Constant],
) -> Option<usize> {
    let Some(value) = args.get(2) else {
        return Some(usize::MAX);
    };
    let number = number_arg(*value, defined, constants, folded)?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return None;
    }
    Some((number as u64).min(usize::MAX as u64) as usize)
}

fn split_units(input: &[u16], separator: &[u16], limit: usize) -> Option<Vec<Vec<u16>>> {
    if limit == 0 {
        return Some(Vec::new());
    }
    let mut parts = Vec::new();
    if separator.is_empty() {
        for unit in input.iter().take(limit) {
            parts.push(vec![*unit]);
        }
    } else {
        let mut start = 0;
        while parts.len() + 1 < limit {
            let Some(pos) = find_units(input, separator, start) else {
                break;
            };
            parts.push(input[start..pos].to_vec());
            start = pos + separator.len();
        }
        parts.push(input[start..].to_vec());
    }
    if parts.len() > 256 || parts.iter().map(Vec::len).sum::<usize>() > MAX_FOLDED_UNITS {
        return None;
    }
    Some(parts)
}
fn integer_arg(
    args: &[ValueId],
    position: usize,
    defined: &HashMap<ValueId, ConstantId>,
    constants: &[Constant],
    folded: &[Constant],
    default: i64,
) -> Option<i64> {
    let Some(value) = args.get(position) else {
        return Some(default);
    };
    // 实参存在但不是数字常量时不可折叠：undefined 也要走运行时的 ToNumber。
    let number = number_arg(*value, defined, constants, folded)?;
    if number.is_nan() {
        return Some(0);
    }
    if number >= i64::MAX as f64 {
        return Some(i64::MAX);
    }
    if number <= i64::MIN as f64 {
        return Some(i64::MIN);
    }
    Some(number.trunc() as i64)
}

fn relative_index(index: i64, length: usize) -> usize {
    if index < 0 {
        (length as i64 + index).max(0) as usize
    } else {
        (index as usize).min(length)
    }
}

fn clamp_index(index: i64, length: usize) -> usize {
    index.clamp(0, length as i64) as usize
}

fn find_units(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    if from > haystack.len() || needle.len() > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

fn rfind_units(haystack: &[u16], needle: &[u16], end: usize) -> Option<usize> {
    let end = end.min(haystack.len());
    if needle.is_empty() {
        return Some(end);
    }
    if needle.len() > end {
        return None;
    }
    haystack[..end]
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Function, Terminator};

    fn module_with(builtin: Builtin, constants: Vec<Constant>, args: Vec<ValueId>) -> Module {
        let mut module = Module::new();
        for constant in constants {
            module.add_constant(constant);
        }
        let mut function = Function::new("fold", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        for (index, value) in args.iter().enumerate() {
            block.push_instruction(Instruction::Const {
                dest: *value,
                constant: ConstantId(u32::try_from(index).expect("fits")),
            });
        }
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(100)),
            builtin,
            args,
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(100)),
        });
        function.push_block(block);
        module.push_function(function);
        module
    }

    fn folded_result(module: &Module) -> Option<Constant> {
        let instruction = module.functions()[0].blocks()[0]
            .instructions()
            .last()
            .cloned()?;
        let Instruction::Const { constant, .. } = instruction else {
            return None;
        };
        module.constants().get(constant.0 as usize).cloned()
    }

    #[test]
    fn folds_constant_slice() {
        let mut module = module_with(
            Builtin::StringSlice,
            vec![
                Constant::String("the quick brown fox".into()),
                Constant::Number(4.0),
                Constant::Number(9.0),
            ],
            vec![ValueId(0), ValueId(1), ValueId(2)],
        );
        run(&mut module);
        assert_eq!(
            folded_result(&module),
            Some(Constant::String("quick".into()))
        );
    }

    #[test]
    fn folds_negative_slice_index() {
        let mut module = module_with(
            Builtin::StringSlice,
            vec![
                Constant::String("abcdef".into()),
                Constant::Number(-2.0),
                Constant::Number(6.0),
            ],
            vec![ValueId(0), ValueId(1), ValueId(2)],
        );
        run(&mut module);
        assert_eq!(folded_result(&module), Some(Constant::String("ef".into())));
    }

    #[test]
    fn folds_index_of_and_includes() {
        let mut module = module_with(
            Builtin::StringIndexOf,
            vec![
                Constant::String("alpha,beta".into()),
                Constant::String("beta".into()),
            ],
            vec![ValueId(0), ValueId(1)],
        );
        run(&mut module);
        assert_eq!(folded_result(&module), Some(Constant::Number(6.0)));

        let mut module = module_with(
            Builtin::StringIncludes,
            vec![
                Constant::String("alpha,beta".into()),
                Constant::String("gamma".into()),
            ],
            vec![ValueId(0), ValueId(1)],
        );
        run(&mut module);
        assert_eq!(folded_result(&module), Some(Constant::Bool(false)));
    }

    #[test]
    fn folds_concat_chain() {
        let mut module = module_with(
            Builtin::StringConcatVa,
            vec![
                Constant::String("a".into()),
                Constant::String("b".into()),
                Constant::String("c".into()),
            ],
            vec![ValueId(0), ValueId(1), ValueId(2)],
        );
        run(&mut module);
        assert_eq!(folded_result(&module), Some(Constant::String("abc".into())));
    }

    #[test]
    fn folds_split_to_array_template() {
        let mut module = module_with(
            Builtin::StringSplit,
            vec![
                Constant::String("a,b,c".into()),
                Constant::String(",".into()),
                Constant::Number(2.0),
            ],
            vec![ValueId(0), ValueId(1), ValueId(2)],
        );
        run(&mut module);
        let instruction = module.functions()[0].blocks()[0].instructions().last();
        let Some(Instruction::CloneArrayTemplate { template, .. }) = instruction else {
            panic!("split should lower to a fresh array clone");
        };
        let Constant::ArrayTemplate(elements) = &module.constants()[template.0 as usize] else {
            panic!("missing split template");
        };
        assert_eq!(elements.len(), 2);
    }

    #[test]
    fn split_zero_limit_is_empty_template() {
        let mut module = module_with(
            Builtin::StringSplit,
            vec![
                Constant::String("a,b".into()),
                Constant::String(",".into()),
                Constant::Number(0.0),
            ],
            vec![ValueId(0), ValueId(1), ValueId(2)],
        );
        run(&mut module);
        let instruction = module.functions()[0].blocks()[0].instructions().last();
        let Some(Instruction::CloneArrayTemplate { template, .. }) = instruction else {
            panic!("split should lower to a fresh array clone");
        };
        assert_eq!(
            module.constants()[template.0 as usize],
            Constant::ArrayTemplate(Vec::new())
        );
    }

    #[test]
    fn keeps_non_constant_receiver() {
        let mut module = Module::new();
        module.add_constant(Constant::Number(1.0));
        let mut function = Function::new("keep", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$1.s".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: ConstantId(0),
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(2)),
            builtin: Builtin::StringSlice,
            args: vec![ValueId(0), ValueId(1)],
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        function.push_block(block);
        module.push_function(function);
        run(&mut module);
        assert!(matches!(
            module.functions()[0].blocks()[0].instructions()[2],
            Instruction::CallBuiltin { .. }
        ));
    }

    #[test]
    fn refuses_oversized_repeat() {
        let mut module = module_with(
            Builtin::StringRepeat,
            vec![Constant::String("x".repeat(64)), Constant::Number(1024.0)],
            vec![ValueId(0), ValueId(1)],
        );
        run(&mut module);
        assert!(matches!(
            module.functions()[0].blocks()[0].instructions()[2],
            Instruction::CallBuiltin { .. }
        ));
    }
}
