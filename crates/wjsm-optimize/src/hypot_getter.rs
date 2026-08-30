//! 识别 `get norm() { return Math.hypot(this.x, this.y); }` 形态的类 getter。
//!
//! 后端两条快路径共用本模块的形态判定：
//! - ACCESSOR IC 命中后 CLIF 比较 getter 身份，再双槽直读 + typed thunk；
//! - elem-guard 闩锁 `GetProp`：pre-header 已证明原型 accessor 仍是 hypot
//!   getter 时，跳过 IC，按模板槽直读双操作数并调用同一 thunk。

use std::collections::HashMap;

use wjsm_ir::{
    Builtin, Constant, ConstantId, Function, FunctionId, Instruction, Program, Terminator, ValueId,
};

use crate::ir_walk::instruction_dest;

/// 可被 CLIF 内联的双槽 `Math.hypot(this.a, this.b)` getter。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HypotGetter {
    pub function: FunctionId,
    /// 访问器属性名（`Point.get_norm` → `"norm"`）。
    pub property: String,
    /// `Math.hypot` 左操作数对应的自有数据键。
    pub lhs_key: String,
    /// `Math.hypot` 右操作数对应的自有数据键。
    pub rhs_key: String,
}

fn is_this_name(name: &str) -> bool {
    name == "$this" || name.ends_with(".$this")
}

/// IR 名 `Class.get_prop` → 属性名 `prop`。
fn getter_property_name(function_name: &str) -> Option<&str> {
    function_name
        .rsplit_once(".get_")
        .map(|(_, property)| property)
        .filter(|property| !property.is_empty())
}

fn const_string<'a>(
    function: &Function,
    constants: &'a [Constant],
    value: ValueId,
) -> Option<&'a str> {
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction
                && *dest == value
            {
                return match constants.get(constant.0 as usize) {
                    Some(Constant::String(text)) => Some(text.as_str()),
                    _ => None,
                };
            }
        }
    }
    None
}

fn defs_in_function(function: &Function) -> HashMap<ValueId, &Instruction> {
    let mut defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, instruction);
            }
        }
    }
    defs
}

fn this_own_key(
    function: &Function,
    constants: &[Constant],
    defs: &HashMap<ValueId, &Instruction>,
    value: ValueId,
) -> Option<String> {
    let Instruction::GetProp { object, key, .. } = defs.get(&value)? else {
        return None;
    };
    let Instruction::LoadVar { name, .. } = defs.get(object)? else {
        return None;
    };
    if !is_this_name(name) {
        return None;
    }
    const_string(function, constants, *key).map(str::to_owned)
}

/// 函数体是否只含 hypot getter 允许的指令（intrinsic 钻石 + 双槽读取）。
fn body_is_hypot_only(function: &Function) -> bool {
    let mut dynamic_calls = 0_u32;
    for block in function.blocks() {
        for instruction in block.instructions() {
            let allowed = match instruction {
                Instruction::Const { .. }
                | Instruction::Phi { .. }
                | Instruction::IsException { .. }
                | Instruction::DebugCheck { .. }
                | Instruction::GetProp { .. }
                | Instruction::LoadVar { .. } => true,
                Instruction::Call { .. } => {
                    dynamic_calls += 1;
                    dynamic_calls <= 1
                }
                Instruction::CallBuiltin { builtin, .. } => matches!(
                    builtin,
                    Builtin::MathHypot
                        | Builtin::IntrinsicPristine
                        | Builtin::IntrinsicResolve
                        | Builtin::ToBoolean
                        | Builtin::ExceptionValue
                ),
                _ => false,
            };
            if !allowed {
                return false;
            }
        }
        if !matches!(
            block.terminator(),
            Terminator::Return { .. }
                | Terminator::Branch { .. }
                | Terminator::Jump { .. }
                | Terminator::Throw { .. }
                | Terminator::Unreachable
        ) {
            return false;
        }
    }
    true
}

fn hypot_this_keys(function: &Function, constants: &[Constant]) -> Option<(String, String)> {
    if !body_is_hypot_only(function) {
        return None;
    }
    let defs = defs_in_function(function);
    let mut pair = None;
    for block in function.blocks() {
        for instruction in block.instructions() {
            let Instruction::CallBuiltin {
                builtin: Builtin::MathHypot,
                args,
                ..
            } = instruction
            else {
                continue;
            };
            if args.len() != 2 {
                return None;
            }
            let lhs = this_own_key(function, constants, &defs, args[0])?;
            let rhs = this_own_key(function, constants, &defs, args[1])?;
            if lhs == rhs {
                return None;
            }
            match &pair {
                None => pair = Some((lhs, rhs)),
                Some(existing) if existing == &(lhs, rhs) => {}
                Some(_) => return None,
            }
        }
    }
    pair
}

/// 收集全部合格 hypot getter；含 `eval` 的程序全部放弃（开放世界）。
pub fn collect_hypot_getters(program: &Program) -> Vec<HypotGetter> {
    if program.functions().iter().any(Function::has_eval) {
        return Vec::new();
    }
    let constants = program.constants();
    let mut getters = Vec::new();
    for (index, function) in program.functions().iter().enumerate() {
        let Some(property) = getter_property_name(function.name()) else {
            continue;
        };
        let Some((lhs_key, rhs_key)) = hypot_this_keys(function, constants) else {
            continue;
        };
        getters.push(HypotGetter {
            function: FunctionId(index as u32),
            property: property.to_owned(),
            lhs_key,
            rhs_key,
        });
    }
    getters.sort_by(|left, right| left.function.0.cmp(&right.function.0));
    getters
}

/// 函数下标 → `(lhs_key, rhs_key)`，供宿主 ACCESSOR IC 回填槽下标。
pub fn hypot_getter_slots_by_function(program: &Program) -> HashMap<u32, (String, String)> {
    collect_hypot_getters(program)
        .into_iter()
        .map(|getter| (getter.function.0, (getter.lhs_key, getter.rhs_key)))
        .collect()
}

/// 模板自有键能覆盖该 hypot getter 的双操作数时，返回 `(lhs_key, rhs_key)`。
/// 同一属性名对应多组不同槽键则放弃（闩锁快路径不能猜槽）。
pub fn hypot_own_slots_for_property<'a>(
    getters: &'a [HypotGetter],
    property: &str,
    template_keys: &[String],
) -> Option<(&'a str, &'a str)> {
    let mut found = None;
    for getter in getters {
        if getter.property != property {
            continue;
        }
        if !template_keys.iter().any(|key| key == &getter.lhs_key) {
            continue;
        }
        if !template_keys.iter().any(|key| key == &getter.rhs_key) {
            continue;
        }
        let pair = (getter.lhs_key.as_str(), getter.rhs_key.as_str());
        match found {
            None => found = Some(pair),
            Some(existing) if existing == pair => {}
            Some(_) => return None,
        }
    }
    found
}

/// 按模板常量解析 hypot getter 双槽键名。
pub fn hypot_own_slots_for_template_key(
    program: &Program,
    template: ConstantId,
    property: &str,
) -> Option<(String, String)> {
    let keys = crate::escape_scalar_record::template_key_names(program.constants(), template)?;
    let getters = collect_hypot_getters(program);
    let (lhs, rhs) = hypot_own_slots_for_property(&getters, property, &keys)?;
    Some((lhs.to_owned(), rhs.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_parser::parse_module;
    use wjsm_semantic::lower_module;

    const POINT: &str = r#"
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  get norm() {
    return Math.hypot(this.x, this.y);
  }
  scale(factor) {
    return new Point(this.x * factor, this.y * factor);
  }
}
"#;

    #[test]
    fn recognizes_point_norm_hypot_getter() {
        let module = parse_module(POINT).expect("解析");
        let program = lower_module(module, false).expect("lowering");
        let getters = collect_hypot_getters(&program);
        assert_eq!(getters.len(), 1, "{getters:?}");
        assert_eq!(getters[0].property, "norm");
        assert_eq!(getters[0].lhs_key, "x");
        assert_eq!(getters[0].rhs_key, "y");
        let getter = program
            .functions()
            .get(getters[0].function.0 as usize)
            .expect("getter 函数");
        assert!(
            getter.name().ends_with(".get_norm"),
            "getter 名 {}",
            getter.name()
        );
    }

    #[test]
    fn ignores_area_mul_getter() {
        let source = r#"
class Rectangle {
  constructor(w, h) { this.w = w; this.h = h; }
  get area() { return this.w * this.h; }
}
"#;
        let module = parse_module(source).expect("解析");
        let program = lower_module(module, false).expect("lowering");
        assert!(collect_hypot_getters(&program).is_empty());
    }

    #[test]
    fn collects_two_hypot_getters() {
        let source = r#"
class Point {
  constructor(x, y) { this.x = x; this.y = y; }
  get norm() { return Math.hypot(this.x, this.y); }
}
class Vec {
  constructor(dx, dy) { this.dx = dx; this.dy = dy; }
  get length() { return Math.hypot(this.dx, this.dy); }
}
"#;
        let module = parse_module(source).expect("解析");
        let program = lower_module(module, false).expect("lowering");
        let getters = collect_hypot_getters(&program);
        assert_eq!(getters.len(), 2, "{getters:?}");
        let names: Vec<_> = getters.iter().map(|g| g.property.as_str()).collect();
        assert!(names.contains(&"norm"), "{names:?}");
        assert!(names.contains(&"length"), "{names:?}");
    }

    #[test]
    fn hypot_own_slots_match_template_keys() {
        let module = parse_module(POINT).expect("解析");
        let program = lower_module(module, false).expect("lowering");
        let getters = collect_hypot_getters(&program);
        let keys = vec!["x".to_owned(), "y".to_owned()];
        let pair = hypot_own_slots_for_property(&getters, "norm", &keys).expect("norm 槽");
        assert_eq!(pair, ("x", "y"));
        assert!(hypot_own_slots_for_property(&getters, "norm", &["dx".into()]).is_none());
        assert!(hypot_own_slots_for_property(&getters, "area", &keys).is_none());
    }
}
